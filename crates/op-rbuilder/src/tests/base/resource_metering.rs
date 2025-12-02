use crate::{
    args::OpRbuilderArgs,
    tests::{BlockTransactionsExt, ChainDriver, FlashblocksListener, Ipc, LocalInstance},
};
use alloy_primitives::{B256, TxHash, U256};
use alloy_provider::{Provider, RootProvider};
use macros::rb_test;
use op_alloy_network::Optimism;
use tips_core::MeterBundleResponse;
use tokio::time::{Duration, sleep};

type TestDriver = ChainDriver<Ipc>;

const EXECUTION_LIMIT_MS: u64 = 200;

#[rb_test(args = {
    let mut args = OpRbuilderArgs::default();
    args.chain_block_time = EXECUTION_LIMIT_MS;
    args.enable_resource_metering = true;
    args.enforce_resource_metering = true;
    args.flashblocks.flashblocks_block_time = EXECUTION_LIMIT_MS;
    args
})]
async fn execution_time_limit_rejects_excessive_transactions(
    rbuilder: LocalInstance,
) -> eyre::Result<()> {
    let driver = rbuilder.driver().await?;
    enable_metering(driver.provider()).await?;

    let first = send_metered_tx(&driver, 120_000).await?;
    let second = send_metered_tx(&driver, 120_000).await?;

    let block = driver.build_new_block().await?;
    assert!(
        block.includes(&first),
        "first transaction should be included before budget is exhausted"
    );
    assert!(
        !block.includes(&second),
        "second transaction should be excluded once the execution budget is exceeded"
    );

    Ok(())
}

#[rb_test(args = {
    let mut args = OpRbuilderArgs::default();
    args.chain_block_time = EXECUTION_LIMIT_MS;
    args.enable_resource_metering = true;
    args.flashblocks.flashblocks_block_time = EXECUTION_LIMIT_MS;
    args
})]
async fn execution_time_budget_resets_each_block(rbuilder: LocalInstance) -> eyre::Result<()> {
    let driver = rbuilder.driver().await?;
    enable_metering(driver.provider()).await?;

    let first = send_metered_tx(&driver, 150_000).await?;
    let first_block = driver.build_new_block().await?;
    assert!(
        first_block.includes(&first),
        "transaction should be included while under the execution budget"
    );

    let second = send_metered_tx(&driver, 150_000).await?;
    let second_block = driver.build_new_block().await?;
    assert!(
        second_block.includes(&second),
        "execution budget should reset between blocks"
    );

    Ok(())
}

#[rb_test(args = {
    let mut args = OpRbuilderArgs::default();
    args.chain_block_time = EXECUTION_LIMIT_MS;
    args.enable_resource_metering = true;
    args.flashblocks.flashblocks_block_time = EXECUTION_LIMIT_MS;
    args
})]
async fn missing_metering_information_defaults_to_zero(
    rbuilder: LocalInstance,
) -> eyre::Result<()> {
    let driver = rbuilder.driver().await?;
    enable_metering(driver.provider()).await?;

    let unmetered = send_unmetered_tx(&driver).await?;
    let metered = send_metered_tx(&driver, 180_000).await?;

    let block = driver.build_new_block().await?;
    assert!(
        block.includes(&unmetered),
        "transactions without metering info should still be included"
    );
    assert!(
        block.includes(&metered),
        "execution budget should account only for metered time"
    );

    Ok(())
}

#[rb_test(
    flashblocks,
    args = {
        let mut args = OpRbuilderArgs::default();
        args.chain_block_time = EXECUTION_LIMIT_MS;
        args.enable_resource_metering = true;
        args.enforce_resource_metering = true;
        args.flashblocks.flashblocks_block_time = EXECUTION_LIMIT_MS / 2;
        args
    }
)]
async fn flashblock_execution_time_limit_enforced_per_batch(
    rbuilder: LocalInstance,
) -> eyre::Result<()> {
    let listener = rbuilder.spawn_flashblocks_listener();
    let driver = rbuilder.driver().await?;
    enable_metering(driver.provider()).await?;

    let first = send_metered_tx(&driver, 60_000).await?;
    let second = send_metered_tx(&driver, 60_000).await?;

    let block = driver.build_new_block().await?;
    assert!(
        block.includes(&first),
        "first transaction should be included before the per-batch budget is exhausted"
    );
    assert!(
        block.includes(&second),
        "second transaction should still be included once a new flashblock begins"
    );

    let first_fb = wait_for_flashblock(&listener, &first).await?;
    assert_eq!(first_fb, 1, "first tx should land in the first flashblock");
    assert!(
        wait_for_flashblock(&listener, &second).await? > first_fb,
        "second tx should spill over into a later flashblock once the first budget is exhausted"
    );

    listener.stop().await?;
    Ok(())
}

async fn send_metered_tx(driver: &TestDriver, execution_time_us: u128) -> eyre::Result<TxHash> {
    let pending = driver
        .create_transaction()
        .with_max_priority_fee_per_gas(100)
        .send()
        .await?;
    let tx_hash = *pending.tx_hash();
    set_metering_information(driver.provider(), tx_hash, execution_time_us).await?;
    Ok(tx_hash)
}

async fn send_unmetered_tx(driver: &TestDriver) -> eyre::Result<TxHash> {
    let pending = driver
        .create_transaction()
        .with_max_priority_fee_per_gas(100)
        .send()
        .await?;
    Ok(*pending.tx_hash())
}

async fn enable_metering(provider: &RootProvider<Optimism>) -> eyre::Result<()> {
    provider
        .raw_request::<(bool,), ()>("base_setMeteringEnabled".into(), (true,))
        .await?;
    Ok(())
}

async fn set_metering_information(
    provider: &RootProvider<Optimism>,
    tx_hash: TxHash,
    execution_time_us: u128,
) -> eyre::Result<()> {
    provider
        .raw_request::<(TxHash, MeterBundleResponse), ()>(
            "base_setMeteringInformation".into(),
            (tx_hash, metering_response(execution_time_us)),
        )
        .await?;
    Ok(())
}

fn metering_response(execution_time_us: u128) -> MeterBundleResponse {
    MeterBundleResponse {
        bundle_hash: B256::random(),
        bundle_gas_price: U256::from(1),
        coinbase_diff: U256::ZERO,
        eth_sent_to_coinbase: U256::ZERO,
        gas_fees: U256::ZERO,
        results: vec![],
        state_block_number: 0,
        state_flashblock_index: None,
        total_gas_used: 21_000,
        total_execution_time_us: execution_time_us,
    }
}

async fn wait_for_flashblock(
    listener: &FlashblocksListener,
    tx_hash: &TxHash,
) -> eyre::Result<u64> {
    for _ in 0..80 {
        if let Some(index) = listener.find_transaction_flashblock(tx_hash) {
            return Ok(index);
        }
        sleep(Duration::from_millis(50)).await;
    }
    eyre::bail!("transaction {tx_hash:?} was not observed in any flashblock");
}
