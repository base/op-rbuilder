//! Base-specific builder context.

/// Base-specific context for payload building.
/// Add this as a single field to OpPayloadBuilderCtx to minimize diff.
#[derive(Debug, Default, Clone)]
pub struct BaseBuilderCtx {
    /// Block execution time limit in microseconds
    pub block_execution_time_limit_us: u128,
}

impl BaseBuilderCtx {
    /// Create a new BaseBuilderCtx with the given execution time limit.
    pub fn new(block_execution_time_limit_us: u128) -> Self {
        Self {
            block_execution_time_limit_us,
        }
    }
}
