//! Heavily influenced by [reth](https://github.com/paradigmxyz/reth/blob/1e965caf5fa176f244a31c0d2662ba1b590938db/crates/optimism/payload/src/builder.rs#L570)
use alloy_primitives::{Address, U256};
use core::fmt::Debug;
use derive_more::Display;
use op_revm::OpTransactionError;
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};

#[derive(Debug, Display)]
pub enum TxnExecutionResult {
    TransactionDALimitExceeded,
    #[display("BlockDALimitExceeded: total_da_used={_0} tx_da_size={_1} block_da_limit={_2}")]
    BlockDALimitExceeded(u64, u64, u64),
    #[display("TransactionGasLimitExceeded: total_gas_used={_0} tx_gas_limit={_1}")]
    TransactionGasLimitExceeded(u64, u64, u64),
    #[display(
        "BlockExecutionTimeLimitExceeded: total_time_us={_0} tx_time_us={_1} block_time_limit_us={_2}"
    )]
    BlockExecutionTimeLimitExceeded(u128, u128, u128),
    SequencerTransaction,
    NonceTooLow,
    InteropFailed,
    #[display("InternalError({_0})")]
    InternalError(OpTransactionError),
    EvmError,
    Success,
    Reverted,
    RevertedAndExcluded,
    MaxGasUsageExceeded,
}

#[derive(Default, Debug)]
pub struct ExecutionInfo<Extra: Debug + Default = ()> {
    /// All executed transactions (unrecovered).
    pub executed_transactions: Vec<OpTransactionSigned>,
    /// The recovered senders for the executed transactions.
    pub executed_senders: Vec<Address>,
    /// The transaction receipts
    pub receipts: Vec<OpReceipt>,
    /// All gas used so far
    pub cumulative_gas_used: u64,
    /// Estimated DA size
    pub cumulative_da_bytes_used: u64,
    /// Tracks fees from executed mempool transactions
    pub total_fees: U256,
    /// Extra execution information that can be attached by individual builders.
    pub extra: Extra,
    /// DA Footprint Scalar for Jovian
    pub da_footprint_scalar: Option<u16>,
    /// Cumulative execution time in microseconds
    pub cumulative_execution_time_us: u128,
}

/// Block-wide resource ceilings.
#[derive(Debug, Clone, Copy)]
pub struct BlockLimits {
    pub gas: u64,
    pub data: Option<u64>,
    pub da_footprint: Option<u64>,
    pub execution_time_us: u128,
}

/// Transaction-specific ceilings (per-tx limits imposed by protocol rules).
#[derive(Debug, Clone, Copy)]
pub struct TxLimits {
    pub data: Option<u64>,
}

/// Additional limit modifiers derived from the chain state.
#[derive(Debug, Clone, Copy)]
pub struct LimitContext {
    pub block: BlockLimits,
    pub tx: TxLimits,
    pub da_footprint_gas_scalar: Option<u16>,
}

/// Measured resource usage for a candidate transaction.
#[derive(Debug, Clone, Copy)]
pub struct TxUsage {
    pub data_size: u64,
    pub gas_limit: u64,
    pub execution_time_us: u128,
}

impl<T: Debug + Default> ExecutionInfo<T> {
    /// Create a new instance with allocated slots.
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
            cumulative_execution_time_us: 0,
        }
    }

    /// Returns true if the transaction would exceed the block limits:
    /// - block gas limit: ensures the transaction still fits into the block.
    /// - tx DA limit: if configured, ensures the tx does not exceed the maximum allowed DA limit
    ///   per tx.
    /// - block DA limit: if configured, ensures the transaction's DA size does not exceed the
    ///   maximum allowed DA limit per block.
    /// - block execution time limit: if configured, ensures the transaction's execution time does
    ///   not exceed the maximum allowed execution time per block.
    pub fn is_tx_over_limits(
        &self,
        usage: &TxUsage,
        limits: &LimitContext,
    ) -> Result<(), TxnExecutionResult> {
        if limits
            .tx
            .data
            .is_some_and(|da_limit| usage.data_size > da_limit)
        {
            return Err(TxnExecutionResult::TransactionDALimitExceeded);
        }
        let total_da_bytes_used = self
            .cumulative_da_bytes_used
            .saturating_add(usage.data_size);

        if limits
            .block
            .data
            .is_some_and(|da_limit| total_da_bytes_used > da_limit)
        {
            return Err(TxnExecutionResult::BlockDALimitExceeded(
                self.cumulative_da_bytes_used,
                usage.data_size,
                limits.block.data.unwrap_or_default(),
            ));
        }

        // Post Jovian: the tx DA footprint must be less than the block gas limit
        if let Some(da_footprint_gas_scalar) = limits.da_footprint_gas_scalar {
            let tx_da_footprint =
                total_da_bytes_used.saturating_mul(da_footprint_gas_scalar as u64);
            if tx_da_footprint > limits.block.da_footprint.unwrap_or(limits.block.gas) {
                return Err(TxnExecutionResult::BlockDALimitExceeded(
                    total_da_bytes_used,
                    usage.data_size,
                    tx_da_footprint,
                ));
            }
        }

        if self.cumulative_gas_used + usage.gas_limit > limits.block.gas {
            return Err(TxnExecutionResult::TransactionGasLimitExceeded(
                self.cumulative_gas_used,
                usage.gas_limit,
                limits.block.gas,
            ));
        }

        // Check block execution time limit
        let total_execution_time_us = self
            .cumulative_execution_time_us
            .saturating_add(usage.execution_time_us);
        if total_execution_time_us > limits.block.execution_time_us {
            return Err(TxnExecutionResult::BlockExecutionTimeLimitExceeded(
                self.cumulative_execution_time_us,
                usage.execution_time_us,
                limits.block.execution_time_us,
            ));
        }

        Ok(())
    }
}
