use crate::{http::EndpointHttpClient, job::MechanicsExecutionLimits};
use std::{sync::Arc, time::Duration};

/// Configuration for constructing a [`MechanicsPool`].
///
/// This configuration is intended for stateless workers that can be replicated horizontally.
/// Avoid correctness assumptions that depend on in-process caches or sticky worker routing.
#[derive(Debug, Clone)]
pub struct MechanicsPoolConfig {
    /// Number of worker threads in the pool.
    pub worker_count: usize,
    /// Maximum number of enqueued jobs waiting to run.
    pub queue_capacity: usize,
    /// Maximum time to wait while enqueueing in [`MechanicsPool::run`].
    pub enqueue_timeout: Duration,
    /// Maximum total wall-clock time that a `run`/`run_try_enqueue` call may block.
    pub run_timeout: Duration,
    /// Script execution limits applied to every job.
    pub execution_limits: MechanicsExecutionLimits,
    /// Default timeout in milliseconds for endpoint HTTP calls.
    ///
    /// Per-endpoint timeout set via [`HttpEndpoint::with_timeout_ms`] overrides this value.
    pub default_http_timeout_ms: Option<u64>,
    /// Default maximum HTTP response-body size in bytes for endpoint calls.
    ///
    /// Per-endpoint limit set via [`HttpEndpoint::with_response_max_bytes`] overrides this value.
    /// `None` means no global response-body size cap.
    pub default_http_response_max_bytes: Option<usize>,
    /// Sliding window duration used by worker restart rate limiting.
    pub restart_window: Duration,
    /// Maximum automatic worker restarts allowed within `restart_window`.
    pub max_restarts_in_window: usize,
    /// Pool-level endpoint transport used by `mechanics:endpoint` executions.
    ///
    /// If `None`, the pool constructs a default reqwest-backed client.
    /// This is Rust-side runtime wiring and is intentionally not part of JSON job config.
    pub endpoint_http_client: Option<Arc<dyn EndpointHttpClient>>,
    /// Test-only hook to force worker runtime init failures during pool creation.
    #[cfg(test)]
    pub(crate) force_worker_runtime_init_failure: bool,
}

impl Default for MechanicsPoolConfig {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(1);
        Self {
            worker_count: workers.max(1),
            queue_capacity: workers.saturating_mul(64).max(64),
            enqueue_timeout: Duration::from_millis(500),
            run_timeout: Duration::from_secs(30),
            execution_limits: MechanicsExecutionLimits::default(),
            default_http_timeout_ms: Some(120_000),
            default_http_response_max_bytes: Some(8 * 1024 * 1024),
            restart_window: Duration::from_secs(10),
            max_restarts_in_window: 16,
            endpoint_http_client: None,
            #[cfg(test)]
            force_worker_runtime_init_failure: false,
        }
    }
}
