//! Execution primitives for block building.
//!
//! This module contains types used to track transaction execution results
//! and accumulate execution information during block building.
//!
//! Heavily influenced by [reth](https://github.com/paradigmxyz/reth/blob/1e965caf5fa176f244a31c0d2662ba1b590938db/crates/optimism/payload/src/builder.rs#L570)

use alloy_primitives::{Address, U256};
use core::fmt::Debug;
use derive_more::Display;
use op_revm::OpTransactionError;
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};

/// Result of attempting to execute a transaction during block building.
///
/// This enum captures all possible outcomes when a transaction is considered
/// for inclusion in a block, helping with logging, metrics, and debugging.
#[derive(Debug, Display)]
pub enum TxnExecutionResult {
    /// Transaction's data availability size exceeds the per-transaction limit.
    TransactionDALimitExceeded,

    /// Transaction would cause block to exceed its data availability limit.
    #[display("BlockDALimitExceeded: total_da_used={_0} tx_da_size={_1} block_da_limit={_2}")]
    BlockDALimitExceeded(u64, u64, u64),

    /// Transaction's gas limit would cause block to exceed its gas limit.
    #[display("TransactionGasLimitExceeded: total_gas_used={_0} tx_gas_limit={_1} block_gas_limit={_2}")]
    TransactionGasLimitExceeded(u64, u64, u64),

    /// Transaction is a sequencer transaction (blob or deposit) which shouldn't
    /// come from the pool.
    SequencerTransaction,

    /// Transaction nonce is lower than the account's current nonce.
    NonceTooLow,

    /// Transaction failed interop validation (cross-chain deadline expired).
    InteropFailed,

    /// An internal EVM error occurred during transaction validation.
    #[display("InternalError({_0})")]
    InternalError(OpTransactionError),

    /// A fatal EVM error occurred during transaction execution.
    EvmError,

    /// Transaction executed successfully.
    Success,

    /// Transaction reverted but was included (allowed to revert).
    Reverted,

    /// Transaction reverted and was excluded from the block.
    RevertedAndExcluded,

    /// Transaction exceeded the per-address gas limit (rate limiting).
    MaxGasUsageExceeded,
}

/// Accumulated information about executed transactions during block building.
///
/// This structure tracks all the transactions that have been executed,
/// their receipts, cumulative resource usage, and total fees collected.
/// It is used to build the final block and generate metrics.
#[derive(Default, Debug)]
pub struct ExecutionInfo<Extra: Debug + Default = ()> {
    /// All executed transactions (unrecovered).
    ///
    /// These are the raw signed transactions that have been successfully
    /// executed and will be included in the block.
    pub executed_transactions: Vec<OpTransactionSigned>,

    /// The recovered senders for the executed transactions.
    ///
    /// Each entry corresponds to the transaction at the same index in
    /// `executed_transactions`.
    pub executed_senders: Vec<Address>,

    /// The transaction receipts for executed transactions.
    ///
    /// Each receipt contains the execution result, logs, and cumulative
    /// gas used up to and including that transaction.
    pub receipts: Vec<OpReceipt>,

    /// Total gas consumed by all executed transactions.
    pub cumulative_gas_used: u64,

    /// Estimated total data availability bytes used.
    ///
    /// This is used to ensure blocks don't exceed DA limits, which is
    /// important for L2 chains where DA costs are significant.
    pub cumulative_da_bytes_used: u64,

    /// Total fees collected from executed mempool transactions.
    ///
    /// This represents the total priority fees (tips) that will be
    /// paid to the block builder.
    pub total_fees: U256,

    /// Extra execution information that can be attached by individual builders.
    ///
    /// This allows different builder implementations to track additional
    /// state without modifying this core structure.
    pub extra: Extra,

    /// Data Availability footprint scalar for Jovian hardfork.
    ///
    /// After the Jovian hardfork, this scalar is used to calculate
    /// the DA footprint gas consumption.
    pub da_footprint_scalar: Option<u16>,
}

impl<T: Debug + Default> ExecutionInfo<T> {
    /// Create a new instance with pre-allocated capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - The expected number of transactions to be executed
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            executed_transactions: Vec::with_capacity(capacity),
            executed_senders: Vec::with_capacity(capacity),
            receipts: Vec::with_capacity(capacity),
            cumulative_gas_used: 0,
            cumulative_da_bytes_used: 0,
            total_fees: U256::ZERO,
            extra: Default::default(),
            da_footprint_scalar: None,
        }
    }

    /// Check if a transaction would exceed block limits.
    ///
    /// This method validates that including a transaction won't violate any
    /// of the following limits:
    /// - Block gas limit: ensures the transaction still fits into the block
    /// - Per-transaction DA limit: ensures the transaction doesn't exceed max DA size
    /// - Block DA limit: ensures cumulative DA doesn't exceed the block limit
    /// - Post-Jovian DA footprint: ensures DA footprint gas stays within limits
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the transaction can be included
    /// - `Err(TxnExecutionResult)` describing why the transaction cannot be included
    #[allow(clippy::too_many_arguments)]
    pub fn is_tx_over_limits(
        &self,
        tx_da_size: u64,
        block_gas_limit: u64,
        tx_data_limit: Option<u64>,
        block_data_limit: Option<u64>,
        tx_gas_limit: u64,
        da_footprint_gas_scalar: Option<u16>,
        block_da_footprint_limit: Option<u64>,
    ) -> Result<(), TxnExecutionResult> {
        // Check per-transaction DA limit
        if tx_data_limit.is_some_and(|da_limit| tx_da_size > da_limit) {
            return Err(TxnExecutionResult::TransactionDALimitExceeded);
        }

        // Check block DA limit
        let total_da_bytes_used = self.cumulative_da_bytes_used.saturating_add(tx_da_size);
        if block_data_limit.is_some_and(|da_limit| total_da_bytes_used > da_limit) {
            return Err(TxnExecutionResult::BlockDALimitExceeded(
                self.cumulative_da_bytes_used,
                tx_da_size,
                block_data_limit.unwrap_or_default(),
            ));
        }

        // Post Jovian: the tx DA footprint must be less than the block gas limit
        if let Some(da_footprint_gas_scalar) = da_footprint_gas_scalar {
            let tx_da_footprint =
                total_da_bytes_used.saturating_mul(da_footprint_gas_scalar as u64);
            if tx_da_footprint > block_da_footprint_limit.unwrap_or(block_gas_limit) {
                return Err(TxnExecutionResult::BlockDALimitExceeded(
                    total_da_bytes_used,
                    tx_da_size,
                    tx_da_footprint,
                ));
            }
        }

        // Check block gas limit
        if self.cumulative_gas_used + tx_gas_limit > block_gas_limit {
            return Err(TxnExecutionResult::TransactionGasLimitExceeded(
                self.cumulative_gas_used,
                tx_gas_limit,
                block_gas_limit,
            ));
        }

        Ok(())
    }
}
