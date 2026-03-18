use crate::{error::MechanicsError, http::EndpointHttpClient, job::MechanicsExecutionLimits};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Configuration for constructing a [`MechanicsPool`].
///
/// This configuration is intended for stateless workers that can be replicated horizontally.
/// Avoid correctness assumptions that depend on in-process caches or sticky worker routing.
#[derive(Debug, Clone)]
pub struct MechanicsPoolConfig {
    /// Number of worker threads in the pool.
    pub(crate) worker_count: usize,
    /// Maximum number of enqueued jobs waiting to run.
    pub(crate) queue_capacity: usize,
    /// Maximum time to wait while enqueueing in [`MechanicsPool::run`].
    pub(crate) enqueue_timeout: Duration,
    /// Maximum total wall-clock time that a `run`/`run_try_enqueue` call may block.
    pub(crate) run_timeout: Duration,
    /// Script execution limits applied to every job.
    pub(crate) execution_limits: MechanicsExecutionLimits,
    /// Default timeout in milliseconds for endpoint HTTP calls.
    ///
    /// Per-endpoint timeout set via [`HttpEndpoint::with_timeout_ms`] overrides this value.
    pub(crate) default_http_timeout_ms: Option<u64>,
    /// Default maximum HTTP response-body size in bytes for endpoint calls.
    ///
    /// Per-endpoint limit set via [`HttpEndpoint::with_response_max_bytes`] overrides this value.
    /// `None` means no global response-body size cap.
    pub(crate) default_http_response_max_bytes: Option<usize>,
    /// Sliding window duration used by worker restart rate limiting.
    pub(crate) restart_window: Duration,
    /// Maximum automatic worker restarts allowed within `restart_window`.
    pub(crate) max_restarts_in_window: usize,
    /// Pool-level endpoint transport used by `mechanics:endpoint` executions.
    ///
    /// If `None`, the pool constructs a default reqwest-backed client.
    /// This is Rust-side runtime wiring and is intentionally not part of JSON job config.
    pub(crate) endpoint_http_client: Option<Arc<dyn EndpointHttpClient>>,
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

impl MechanicsPoolConfig {
    /// Constructs a default pool config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates construction-time pool invariants.
    pub(crate) fn validate(&self) -> Result<(), MechanicsError> {
        if self.worker_count == 0 {
            return Err(MechanicsError::runtime_pool("worker_count must be > 0"));
        }
        if self.queue_capacity == 0 {
            return Err(MechanicsError::runtime_pool("queue_capacity must be > 0"));
        }
        if self.max_restarts_in_window == 0 {
            return Err(MechanicsError::runtime_pool(
                "max_restarts_in_window must be > 0",
            ));
        }
        if self.run_timeout.is_zero() {
            return Err(MechanicsError::runtime_pool("run_timeout must be > 0"));
        }
        if Instant::now().checked_add(self.run_timeout).is_none() {
            return Err(MechanicsError::runtime_pool(
                "run_timeout is too large for the current platform clock",
            ));
        }
        self.execution_limits.validate()?;
        Ok(())
    }

    pub fn with_worker_count(mut self, worker_count: usize) -> Self {
        self.worker_count = worker_count;
        self
    }

    pub fn with_queue_capacity(mut self, queue_capacity: usize) -> Self {
        self.queue_capacity = queue_capacity;
        self
    }

    pub fn with_enqueue_timeout(mut self, enqueue_timeout: Duration) -> Self {
        self.enqueue_timeout = enqueue_timeout;
        self
    }

    pub fn with_run_timeout(mut self, run_timeout: Duration) -> Self {
        self.run_timeout = run_timeout;
        self
    }

    pub fn with_execution_limits(mut self, execution_limits: MechanicsExecutionLimits) -> Self {
        self.execution_limits = execution_limits;
        self
    }

    pub fn with_default_http_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.default_http_timeout_ms = timeout_ms;
        self
    }

    pub fn with_default_http_response_max_bytes(mut self, max_bytes: Option<usize>) -> Self {
        self.default_http_response_max_bytes = max_bytes;
        self
    }

    pub fn with_restart_window(mut self, restart_window: Duration) -> Self {
        self.restart_window = restart_window;
        self
    }

    pub fn with_max_restarts_in_window(mut self, max_restarts_in_window: usize) -> Self {
        self.max_restarts_in_window = max_restarts_in_window;
        self
    }

    pub fn with_endpoint_http_client(
        mut self,
        endpoint_http_client: Arc<dyn EndpointHttpClient>,
    ) -> Self {
        self.endpoint_http_client = Some(endpoint_http_client);
        self
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    pub fn enqueue_timeout(&self) -> Duration {
        self.enqueue_timeout
    }

    pub fn run_timeout(&self) -> Duration {
        self.run_timeout
    }

    pub fn execution_limits(&self) -> MechanicsExecutionLimits {
        self.execution_limits
    }

    pub fn default_http_timeout_ms(&self) -> Option<u64> {
        self.default_http_timeout_ms
    }

    pub fn default_http_response_max_bytes(&self) -> Option<usize> {
        self.default_http_response_max_bytes
    }

    pub fn restart_window(&self) -> Duration {
        self.restart_window
    }

    pub fn max_restarts_in_window(&self) -> usize {
        self.max_restarts_in_window
    }

    pub fn endpoint_http_client(&self) -> Option<Arc<dyn EndpointHttpClient>> {
        self.endpoint_http_client.clone()
    }
}
