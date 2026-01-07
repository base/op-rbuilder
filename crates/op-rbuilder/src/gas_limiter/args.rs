use clap::Args;

/// Configuration arguments for address-based gas rate limiting.
///
/// The gas limiter uses a token bucket algorithm to prevent any single address
/// from consuming excessive gas during block building. This protects against
/// DoS attacks from malicious searchers who might submit many expensive
/// transactions that always revert.
///
/// # Token Bucket Algorithm
///
/// Each address is assigned a "bucket" with a maximum capacity of gas units.
/// When a transaction is processed:
/// 1. The gas used is deducted from the address's available gas
/// 2. If the address doesn't have enough gas, the transaction is rejected
/// 3. Gas is refilled at a configurable rate per block
///
/// # Example Configuration
///
/// For a typical setup protecting against gas-griefing attacks:
/// - `max_gas_per_address`: 10,000,000 (allows ~500 simple transactions)
/// - `refill_rate_per_block`: 1,000,000 (refills 10% per block)
/// - `cleanup_interval`: 100 (cleans up stale buckets every 100 blocks)
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct GasLimiterArgs {
    /// Enable address-based gas rate limiting.
    ///
    /// When enabled, each address is subject to gas limits based on a token
    /// bucket algorithm. This helps prevent DoS attacks from addresses that
    /// submit many expensive reverting transactions.
    #[arg(long = "gas-limiter.enabled", env)]
    pub gas_limiter_enabled: bool,

    /// Maximum gas per address in the token bucket.
    ///
    /// This is the maximum amount of gas that an address can consume before
    /// being rate limited. Once exhausted, transactions from this address
    /// will be rejected until gas is refilled. Defaults to 10 million gas.
    #[arg(
        long = "gas-limiter.max-gas-per-address",
        env,
        default_value = "10000000"
    )]
    pub max_gas_per_address: u64,

    /// Gas refill rate per block.
    ///
    /// After each block, this amount of gas is added back to each address's
    /// bucket (up to the maximum capacity). Higher values allow addresses
    /// to recover faster but provide less protection. Defaults to 1 million
    /// gas per block.
    #[arg(
        long = "gas-limiter.refill-rate-per-block",
        env,
        default_value = "1000000"
    )]
    pub refill_rate_per_block: u64,

    /// Number of blocks between cleanup cycles for stale address buckets.
    ///
    /// Periodically, address buckets that have been refilled to full capacity
    /// are removed to free memory. Lower values reduce memory usage but
    /// increase CPU overhead. Defaults to 100 blocks.
    #[arg(long = "gas-limiter.cleanup-interval", env, default_value = "100")]
    pub cleanup_interval: u64,
}
