//! Base-specific metrics.

use reth_metrics::{
    Metrics,
    metrics::{Counter, Histogram},
};

/// Base-specific metrics for resource metering.
#[derive(Metrics, Clone)]
#[metrics(scope = "op_rbuilder_base")]
pub struct BaseMetrics {
    /// Count of transactions excluded due to execution time limit
    pub execution_time_limit_exceeded: Counter,
    /// Histogram of tx execution time (us) that caused the limit to be exceeded
    pub execution_time_limit_tx_us: Histogram,
    /// Histogram of remaining execution time (us) when a tx was excluded
    pub execution_time_limit_remaining_us: Histogram,
    /// Histogram of how much the tx exceeded the remaining time (us)
    pub execution_time_limit_exceeded_by_us: Histogram,
    /// Histogram of tx gas limit when excluded due to execution time limit
    pub execution_time_limit_tx_gas: Histogram,
    /// Histogram of remaining gas when excluded due to execution time limit
    pub execution_time_limit_remaining_gas: Histogram,
}
