use crate::{
    args::OpRbuilderArgs,
    tests::{LocalInstance, TransactionBuilderExt, funded_signer},
};
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use macros::rb_test;

// L1FeeVault address on Optimism
const L1_FEE_VAULT: Address =
    alloy_primitives::address!("420000000000000000000000000000000000001A");

// Hand-crafted bytecode that reads the L1FeeVault balance and stores it in storage slot 0
//
// Bytecode breakdown (deployment/constructor code):
// 73 420000000000000000000000000000000000001a  - PUSH20: Push L1FeeVault address (0x420...001a) onto stack
// 31                                              - BALANCE: Get balance of address on stack, pushes balance onto stack
// 5f                                              - PUSH0: Push 0 onto stack (storage slot 0)
// 55                                              - SSTORE: Store balance (from stack) into slot 0 (from stack)
// 5f                                              - PUSH0: Push 0 (size of runtime code to return)
// 5f                                              - PUSH0: Push 0 (offset in memory)
// f3                                              - RETURN: Return 0 bytes of runtime code (creates contract with no code)
//
// This deployment bytecode:
// 1. Reads the balance of 0x420000000000000000000000000000000000001A using the BALANCE opcode
// 2. Stores that balance in storage slot 0
// 3. Returns no runtime code (contract will exist but have no code, which is fine for our test)
//
// The key test: Does the BALANCE opcode see the lazy balance increments from the deployment tx itself?
const FEE_VAULT_READER_BYTECODE: &str = "73420000000000000000000000000000000000001a31805f555f5ff3";

#[rb_test(args = OpRbuilderArgs {
    parallel_threads: Some(88),
    ..Default::default()
})]
async fn test_fee_vault_balance_read_during_parallel_execution(
    rbuilder: LocalInstance,
) -> eyre::Result<()> {
    let driver = rbuilder.driver().await?;

    // Add external validation node on Linux to verify parallel execution matches sequential
    #[cfg(target_os = "linux")]
    let driver = driver
        .with_validation_node(crate::tests::ExternalNode::reth().await?)
        .await?;

    let provider = rbuilder.provider().await?;

    // Get initial L1FeeVault balance before any transactions
    let initial_vault_balance = provider.get_balance(L1_FEE_VAULT).await?;
    tracing::info!("Initial L1FeeVault balance: {}", initial_vault_balance);

    // Send several random transfers BEFORE the deployment
    // These will execute in parallel and their fees should be visible to the deployment constructor
    let num_txs_before = 5;
    tracing::info!(
        "Sending {} random transfers before deployment",
        num_txs_before
    );
    for i in 0..num_txs_before {
        let tx = driver
            .create_transaction()
            .random_valid_transfer()
            .send()
            .await?;
        tracing::info!("Sent pre-deploy tx {}: {:?}", i, tx.tx_hash());
    }

    // Deploy the FeeVaultReader contract
    // The constructor will read the L1FeeVault balance using the BALANCE opcode
    let deploy_tx = driver
        .create_transaction()
        .with_create()
        .with_input(Bytes::from(hex::decode(FEE_VAULT_READER_BYTECODE)?))
        .send()
        .await?;

    tracing::info!(
        "Deployed FeeVaultReader contract with tx: {:?}",
        deploy_tx.tx_hash()
    );

    // Send several random transfers AFTER the deployment
    // These will also execute in parallel
    let num_txs_after = 5;
    tracing::info!(
        "Sending {} random transfers after deployment",
        num_txs_after
    );
    for i in 0..num_txs_after {
        let tx = driver
            .create_transaction()
            .random_valid_transfer()
            .send()
            .await?;
        tracing::info!("Sent post-deploy tx {}: {:?}", i, tx.tx_hash());
    }

    // Get the deployer address and nonce to compute contract address
    let deployer = funded_signer().address;
    // The nonce for the deploy transaction (we can get this from the transaction)
    let deploy_nonce = provider.get_transaction_count(deployer).await?;
    // Contract address is deterministically computed from sender and nonce
    // For this test, we'll just use a simple calculation: we know it's the first transaction
    // from this signer, so nonce is 0 (or whatever the current nonce is - 1)
    let contract_address = deployer.create(deploy_nonce);
    tracing::info!("Contract will be deployed at: {}", contract_address);

    // Build block with parallel execution (5 txs before + deployment + 5 txs after = 11 user txs + 1 deposit)
    let block = driver.build_new_block_with_current_timestamp(None).await?;
    tracing::info!(
        "Block built with {} transactions (parallel execution with Block-STM)",
        block.transactions.len()
    );

    // Get L1FeeVault balance after all transactions in the block
    let final_vault_balance = provider.get_balance(L1_FEE_VAULT).await?;
    tracing::info!("L1FeeVault balance after block: {}", final_vault_balance);
    assert!(
        final_vault_balance > initial_vault_balance,
        "L1FeeVault balance should have increased after all transactions"
    );

    // The stored balance was captured during deployment in the constructor
    // Now let's read storage slot 0 to get what the contract stored
    let stored_balance = provider
        .get_storage_at(contract_address, U256::ZERO)
        .await?;
    tracing::info!(
        "Balance stored by contract during deployment constructor: {}",
        stored_balance
    );

    // The critical test: during the deployment transaction's constructor execution,
    // the contract reads the L1FeeVault balance using the BALANCE opcode.
    //
    // In parallel execution with Block-STM:
    // - Multiple transactions execute concurrently
    // - The deployment constructor should see lazy balance increments from:
    //   1. Its own transaction's fees (critical!)
    //   2. Possibly fees from other transactions that executed before it
    //
    // The stored balance must be:
    // 1. >= initial_vault_balance (at minimum sees the starting balance)
    // 2. > initial_vault_balance (should see at least its own fees added)
    // 3. <= final_vault_balance (can't see more than the final balance after all txs)

    assert!(
        stored_balance > initial_vault_balance,
        "Contract should have read L1FeeVault balance that is greater than initial. \
         This means it saw lazy balance increments (at least from its own deployment tx). \
         Stored: {}, Initial: {}",
        stored_balance,
        initial_vault_balance
    );

    assert!(
        stored_balance <= final_vault_balance,
        "Contract cannot have read more than the final balance. \
         Stored: {}, Final: {}",
        stored_balance,
        final_vault_balance
    );

    let fees_seen = stored_balance - initial_vault_balance;
    let total_fees = final_vault_balance - initial_vault_balance;

    tracing::info!(
        "✓ Test passed: Contract correctly read L1FeeVault balance with lazy increments visible. \
         Initial: {}, Stored during constructor: {}, Final: {}. \
         Constructor saw {} of {} total fees",
        initial_vault_balance,
        stored_balance,
        final_vault_balance,
        fees_seen,
        total_fees
    );

    Ok(())
}
