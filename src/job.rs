use crate::MechanicsConfig;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// One script execution request submitted to [`MechanicsPool`].
///
/// Jobs are expected to be self-contained for stateless runtime execution.
/// Do not assume any cross-job cache residency in workers.
#[derive(Debug, Clone)]
pub struct MechanicsJob {
    /// ECMAScript module source containing a `default` export callable.
    pub mod_source: Arc<str>,
    /// JSON argument passed to the script's default export.
    pub arg: Arc<Value>,
    /// Runtime configuration used for resolving `mechanics:endpoint` calls.
    pub config: Arc<MechanicsConfig>,
}

/// Per-job execution limits enforced by runtime workers.
#[derive(Debug, Clone, Copy)]
pub struct MechanicsExecutionLimits {
    /// Maximum wall-clock time allowed for one script execution.
    pub max_execution_time: Duration,
    /// Maximum loop iterations before the VM throws a runtime limit error.
    pub max_loop_iterations: u64,
    /// Maximum JS recursion depth before the VM throws a runtime limit error.
    pub max_recursion_depth: usize,
    /// Maximum VM stack size before the VM throws a runtime limit error.
    pub max_stack_size: usize,
}

impl Default for MechanicsExecutionLimits {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_secs(10),
            max_loop_iterations: 1_000_000,
            max_recursion_depth: 512,
            max_stack_size: 10 * 1024,
        }
    }
}
