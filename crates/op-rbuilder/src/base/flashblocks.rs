//! Base-specific flashblocks context.

use super::context::BaseBuilderCtx;

/// Base-specific flashblocks context for per-batch execution time tracking.
/// Add this as a single field to FlashblocksExtraCtx to minimize diff.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseFlashblocksCtx {
    /// Total execution time (us) limit for the current flashblock batch
    pub target_execution_time_us: u128,
    /// Execution time (us) limit per flashblock batch
    pub execution_time_per_batch_us: u128,
    /// Whether to enforce resource metering limits
    pub enforce_limits: bool,
}

impl BaseFlashblocksCtx {
    /// Create a new BaseFlashblocksCtx with the given execution time limit per batch.
    pub fn new(execution_time_per_batch_us: u128, enforce_limits: bool) -> Self {
        Self {
            target_execution_time_us: execution_time_per_batch_us,
            execution_time_per_batch_us,
            enforce_limits,
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
        BaseBuilderCtx::new(ctx.target_execution_time_us, ctx.enforce_limits)
    }
}
