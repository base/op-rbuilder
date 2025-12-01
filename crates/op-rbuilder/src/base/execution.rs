//! Base-specific execution time tracking and limit checking.

use crate::resource_metering::ResourceMetering;
use alloy_primitives::TxHash;

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
        cumulative_us: u128,
        tx_us: u128,
        limit_us: u128,
    },
}

impl BaseExecutionState {
    /// Check if adding this tx would exceed Base-specific limits.
    /// Call this AFTER the upstream is_tx_over_limits().
    pub fn is_tx_over_base_limits(
        &self,
        usage: &BaseTxUsage,
        limits: &BaseBlockLimits,
    ) -> Result<(), BaseLimitExceeded> {
        let total = self
            .cumulative_execution_time_us
            .saturating_add(usage.execution_time_us);
        if total > limits.execution_time_us {
            return Err(BaseLimitExceeded::ExecutionTime {
                cumulative_us: self.cumulative_execution_time_us,
                tx_us: usage.execution_time_us,
                limit_us: limits.execution_time_us,
            });
        }
        Ok(())
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
