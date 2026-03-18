use crate::internal::{
    error::MechanicsError, http::EndpointHttpClient, job::MechanicsExecutionLimits,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Configuration for constructing a [`crate::MechanicsPool`].
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
    /// Maximum total wall-clock time that a `run`/`run_nonblocking_enqueue` call may block.
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
        if self.default_http_timeout_ms == Some(0) {
            return Err(MechanicsError::runtime_pool(
                "default_http_timeout_ms must be >= 1 when provided",
            ));
        }
        if self.default_http_response_max_bytes == Some(0) {
            return Err(MechanicsError::runtime_pool(
                "default_http_response_max_bytes must be >= 1 when provided",
            ));
        }
        self.execution_limits.validate()?;
        Ok(())
    }

    /// Sets worker thread count (`>= 1`).
    pub fn with_worker_count(mut self, worker_count: usize) -> Self {
        self.worker_count = worker_count;
        self
    }

    /// Sets bounded queue capacity (`>= 1`).
    pub fn with_queue_capacity(mut self, queue_capacity: usize) -> Self {
        self.queue_capacity = queue_capacity;
        self
    }

    /// Sets maximum time to wait for queue space during [`crate::MechanicsPool::run`].
    pub fn with_enqueue_timeout(mut self, enqueue_timeout: Duration) -> Self {
        self.enqueue_timeout = enqueue_timeout;
        self
    }

    /// Sets maximum total blocking time for `run` and `run_nonblocking_enqueue`.
    pub fn with_run_timeout(mut self, run_timeout: Duration) -> Self {
        self.run_timeout = run_timeout;
        self
    }

    /// Sets per-job script execution limits.
    pub fn with_execution_limits(mut self, execution_limits: MechanicsExecutionLimits) -> Self {
        self.execution_limits = execution_limits;
        self
    }

    /// Sets default endpoint timeout in milliseconds (`Some(0)` is invalid).
    pub fn with_default_http_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.default_http_timeout_ms = timeout_ms;
        self
    }

    /// Sets default endpoint response-body cap in bytes (`Some(0)` is invalid).
    pub fn with_default_http_response_max_bytes(mut self, max_bytes: Option<usize>) -> Self {
        self.default_http_response_max_bytes = max_bytes;
        self
    }

    /// Sets sliding window duration for worker restart limiting.
    pub fn with_restart_window(mut self, restart_window: Duration) -> Self {
        self.restart_window = restart_window;
        self
    }

    /// Sets maximum allowed automatic restarts within the restart window (`>= 1`).
    pub fn with_max_restarts_in_window(mut self, max_restarts_in_window: usize) -> Self {
        self.max_restarts_in_window = max_restarts_in_window;
        self
    }

    /// Sets a pool-level endpoint transport implementation.
    pub fn with_endpoint_http_client(
        mut self,
        endpoint_http_client: Arc<dyn EndpointHttpClient>,
    ) -> Self {
        self.endpoint_http_client = Some(endpoint_http_client);
        self
    }

    /// Returns configured worker thread count.
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Returns configured bounded queue capacity.
    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    /// Returns configured enqueue wait timeout.
    pub fn enqueue_timeout(&self) -> Duration {
        self.enqueue_timeout
    }

    /// Returns configured total call timeout for `run`/`run_nonblocking_enqueue`.
    pub fn run_timeout(&self) -> Duration {
        self.run_timeout
    }

    /// Returns configured per-job execution limits.
    pub fn execution_limits(&self) -> MechanicsExecutionLimits {
        self.execution_limits
    }

    /// Returns default endpoint timeout in milliseconds, if configured.
    pub fn default_http_timeout_ms(&self) -> Option<u64> {
        self.default_http_timeout_ms
    }

    /// Returns default endpoint response-body cap in bytes, if configured.
    pub fn default_http_response_max_bytes(&self) -> Option<usize> {
        self.default_http_response_max_bytes
    }

    /// Returns restart limiting window duration.
    pub fn restart_window(&self) -> Duration {
        self.restart_window
    }

    /// Returns maximum automatic restarts allowed within the restart window.
    pub fn max_restarts_in_window(&self) -> usize {
        self.max_restarts_in_window
    }

    /// Returns the configured pool-level endpoint transport, if any.
    pub fn endpoint_http_client(&self) -> Option<Arc<dyn EndpointHttpClient>> {
        self.endpoint_http_client.clone()
    }
}
