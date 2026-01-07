use alloy_primitives::Address;

/// Errors that can occur during gas limiting operations.
///
/// Gas limiting is used to prevent any single address from consuming
/// excessive gas within a block building cycle, which helps protect
/// against DoS attacks from malicious searchers.
#[derive(Debug, thiserror::Error)]
pub enum GasLimitError {
    /// An address has exceeded its allocated gas limit.
    ///
    /// This error occurs when a transaction from an address would
    /// consume more gas than the address has available in its token bucket.
    #[error(
        "Address {address} exceeded gas limit: {requested} gas units requested, {available} gas units available"
    )]
    AddressLimitExceeded {
        /// The address that exceeded its gas limit
        address: Address,
        /// The amount of gas requested by the transaction
        requested: u64,
        /// The amount of gas currently available for this address
        available: u64,
    },
}
