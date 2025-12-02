//! Base-specific execution time tracking and limit checking.

use super::metrics::BaseMetrics;
use crate::resource_metering::ResourceMetering;
use alloy_primitives::TxHash;
use tracing::warn;

/// Base-specific execution state bundled into one type.
/// Add this as a single field to ExecutionInfo to minimize diff.
#[derive(Debug, Default, Clone)]
pub struct BaseExecutionState {
    pub cumulative_execution_time_us: u128,
}

/// Base-specific transaction usage bundled into one type.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseTxUsage {
    pub execution_time_us: u128,
}

/// Base-specific block limits bundled into one type.
#[derive(Debug, Clone, Copy)]
pub struct BaseBlockLimits {
    pub execution_time_us: u128,
}

/// Result type for Base-specific limit checks.
#[derive(Debug)]
pub enum BaseLimitExceeded {
    ExecutionTime {
        tx_hash: TxHash,
        cumulative_us: u128,
        tx_us: u128,
        limit_us: u128,
        tx_gas: u64,
        remaining_gas: u64,
    },
}

impl BaseLimitExceeded {
    /// Returns the tx usage that caused the limit to be exceeded.
    pub fn usage(&self) -> BaseTxUsage {
        match self {
            Self::ExecutionTime { tx_us, .. } => BaseTxUsage {
                execution_time_us: *tx_us,
            },
        }
    }

    /// Log and record metrics for this limit exceeded event.
    ///
    /// Only logs/records if this is the first tx to exceed the limit
    /// (i.e., cumulative was within the limit before this tx).
    pub fn log_and_record(&self, metrics: &BaseMetrics) {
        match self {
            Self::ExecutionTime {
                tx_hash,
                cumulative_us,
                tx_us,
                limit_us,
                tx_gas,
                remaining_gas,
            } => {
                // Only log/record for the first tx that exceeds the limit
                if *cumulative_us > *limit_us {
                    return;
                }

                let remaining_us = limit_us.saturating_sub(*cumulative_us);
                let exceeded_by_us = tx_us.saturating_sub(remaining_us);
                warn!(
                    target: "payload_builder",
                    %tx_hash,
                    cumulative_us,
                    tx_us,
                    limit_us,
                    remaining_us,
                    exceeded_by_us,
                    tx_gas,
                    remaining_gas,
                    "Execution time limit exceeded"
                );
                metrics.execution_time_limit_exceeded.increment(1);
                metrics.execution_time_limit_tx_us.record(*tx_us as f64);
                metrics
                    .execution_time_limit_remaining_us
                    .record(remaining_us as f64);
                metrics
                    .execution_time_limit_exceeded_by_us
                    .record(exceeded_by_us as f64);
                metrics.execution_time_limit_tx_gas.record(*tx_gas as f64);
                metrics
                    .execution_time_limit_remaining_gas
                    .record(*remaining_gas as f64);
            }
        }
    }
}

impl BaseExecutionState {
    /// Check if adding a tx would exceed Base-specific limits.
    /// Call this AFTER the upstream is_tx_over_limits().
    /// Returns the usage for later recording via `record_tx`.
    pub fn check_tx(
        &self,
        metering: &ResourceMetering,
        tx_hash: &TxHash,
        execution_time_limit_us: u128,
        tx_gas: u64,
        cumulative_gas_used: u64,
        block_gas_limit: u64,
    ) -> Result<BaseTxUsage, BaseLimitExceeded> {
        let usage = BaseTxUsage::from_metering(metering, tx_hash);
        let total = self
            .cumulative_execution_time_us
            .saturating_add(usage.execution_time_us);

        if total > execution_time_limit_us {
            let remaining_gas = block_gas_limit.saturating_sub(cumulative_gas_used);
            return Err(BaseLimitExceeded::ExecutionTime {
                tx_hash: *tx_hash,
                cumulative_us: self.cumulative_execution_time_us,
                tx_us: usage.execution_time_us,
                limit_us: execution_time_limit_us,
                tx_gas,
                remaining_gas,
            });
        }
        Ok(usage)
    }

    /// Record that a transaction was included.
    pub fn record_tx(&mut self, usage: &BaseTxUsage) {
        self.cumulative_execution_time_us += usage.execution_time_us;
    }
}

impl BaseTxUsage {
    /// Get tx execution time from resource metering.
    pub fn from_metering(metering: &ResourceMetering, tx_hash: &TxHash) -> Self {
        let execution_time_us = metering
            .get(tx_hash)
            .map(|r| r.total_execution_time_us)
            .unwrap_or(0);
        Self { execution_time_us }
    }
}
