//! Base-specific flashblocks context.

use super::context::BaseBuilderCtx;
use crate::builders::flashblocks::FlashblocksConfig;

/// Base-specific flashblocks context for per-batch execution time tracking.
/// Add this as a single field to FlashblocksExtraCtx to minimize diff.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseFlashblocksCtx {
    /// Total execution time (us) limit for the current flashblock batch
    pub target_execution_time_us: u128,
    /// Execution time (us) limit per flashblock batch
    pub execution_time_per_batch_us: u128,
}

impl BaseFlashblocksCtx {
    /// Create a new BaseFlashblocksCtx from flashblocks config.
    pub fn new(config: &FlashblocksConfig) -> Self {
        let execution_time_per_batch_us = config.interval.as_micros();
        Self {
            target_execution_time_us: execution_time_per_batch_us,
            execution_time_per_batch_us,
        }
    }

    /// Advance to the next batch, updating the target execution time.
    /// 
    /// Unlike gas and DA, execution time does not carry over to the next batch.
    pub fn next(self, cumulative_execution_time_us: u128) -> Self {
        Self {
            target_execution_time_us: cumulative_execution_time_us + self.execution_time_per_batch_us,
            ..self
        }
    }
}

impl From<&BaseFlashblocksCtx> for BaseBuilderCtx {
    fn from(ctx: &BaseFlashblocksCtx) -> Self {
        BaseBuilderCtx::new(ctx.target_execution_time_us)
    }
}
