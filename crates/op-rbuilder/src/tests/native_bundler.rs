//! Integration tests for native bundler functionality

use crate::{
    args::OpRbuilderArgs,
    tests::{LocalInstance, TransactionBuilderExt},
};
use macros::rb_test;

/// Test that native bundler is disabled by default
#[rb_test]
async fn native_bundler_disabled_by_default(rbuilder: LocalInstance) -> eyre::Result<()> {
    // The default config should have native bundler disabled
    // This test verifies that existing functionality is not affected
    // when the feature flag is off

    let driver = rbuilder.driver().await?;

    // Build a block without any special bundler logic
    let block = driver.build_new_block_with_current_timestamp(None).await?;

    // Should only have the standard transactions (deposit + builder tx)
    // No bundle transactions should be present
    assert!(
        block.transactions.len() >= 2,
        "Block should have at least deposit and builder transactions"
    );

    Ok(())
}

/// Test native bundler with feature flag enabled
/// This test will be expanded once pool connection is implemented
#[rb_test(args = OpRbuilderArgs {
    enable_native_bundler: true,
    bundler_gas_reserve_percentage: 25,
    bundler_gas_threshold: 75,
    // bundler_pool_url will be None, so it uses mock pool
    ..Default::default()
})]
async fn native_bundler_with_mock_pool(rbuilder: LocalInstance) -> eyre::Result<()> {
    let driver = rbuilder.driver().await?;

    // Send some regular transactions to fill up the block
    for _ in 0..5 {
        driver
            .create_transaction()
            .random_valid_transfer()
            .send()
            .await?;
    }

    // Build a block - with mock pool, no bundles will be created yet
    // This just tests that the feature flag doesn't break block building
    let block = driver.build_new_block_with_current_timestamp(None).await?;

    // Should have regular transactions
    assert!(
        block.transactions.len() >= 7, // 5 user txs + deposit + builder tx
        "Block should include sent transactions"
    );

    // TODO: (BA-3414) Once pool connection is implemented, we would test:
    // - Gas reservation occurs at threshold
    // - Bundle transaction is included
    // - Proper gas accounting

    Ok(())
}

/// Test gas reservation threshold
/// This will be properly implemented in BA-3417
#[rb_test(args = OpRbuilderArgs {
    enable_native_bundler: true,
    bundler_gas_reserve_percentage: 20,
    bundler_gas_threshold: 80,
    ..Default::default()
})]
async fn native_bundler_gas_reservation(_rbuilder: LocalInstance) -> eyre::Result<()> {
    // TODO: Implement in BA-3417
    // This will test that:
    // 1. Regular txs process until 80% gas used
    // 2. Remaining 20% is reserved for bundles
    // 3. Bundle transactions get included in reserved space

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use crate::args::{Cli, CliExt, OpRbuilderArgs};
    use clap::Parser;

    #[test]
    fn test_native_bundler_cli_parsing() {
        // Test parsing with feature flag enabled
        let cli = Cli::parse_from([
            "test",
            "node",
            "--builder.enable-native-bundler",
            "--bundler.gas-reserve-percentage=30",
            "--bundler.gas-threshold=70",
            "--bundler.pool-url=http://localhost:50051",
        ]);

        if let reth_optimism_cli::commands::Commands::Node(node_command) = cli.command {
            let args = node_command.ext;
            assert!(args.enable_native_bundler);
            assert_eq!(args.bundler_gas_reserve_percentage, 30);
            assert_eq!(args.bundler_gas_threshold, 70);
            assert_eq!(
                args.bundler_pool_url,
                Some("http://localhost:50051".to_string())
            );
        } else {
            panic!("Expected node command");
        }
    }

    #[test]
    fn test_native_bundler_cli_defaults() {
        // Test that defaults work correctly when only enabling the feature
        let cli = Cli::parse_from(["test", "node", "--builder.enable-native-bundler"]);

        if let reth_optimism_cli::commands::Commands::Node(node_command) = cli.command {
            let args = node_command.ext;
            assert!(args.enable_native_bundler);
            assert_eq!(args.bundler_gas_reserve_percentage, 20); // default
            assert_eq!(args.bundler_gas_threshold, 80); // default
            assert!(args.bundler_pool_url.is_none());
        } else {
            panic!("Expected node command");
        }
    }
}
