use crate::{
    error::MechanicsError,
    http::{EndpointHttpClient, ReqwestEndpointHttpClient, into_io_error},
    job::{MechanicsExecutionLimits, MechanicsJob},
    runtime::RuntimeInternal,
};
use crossbeam_channel::{
    Receiver, RecvTimeoutError, SendTimeoutError, Sender, TryRecvError, TrySendError, bounded,
    unbounded,
};
use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

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
    force_worker_runtime_init_failure: bool,
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

#[derive(Debug)]
struct RestartGuard {
    window: Duration,
    max_restarts: usize,
    restarts: VecDeque<Instant>,
}

impl RestartGuard {
    fn new(window: Duration, max_restarts: usize) -> Self {
        Self {
            window,
            max_restarts,
            restarts: VecDeque::new(),
        }
    }

    fn allow_restart(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.restarts.front() {
            if now.duration_since(*oldest) > self.window {
                self.restarts.pop_front();
            } else {
                break;
            }
        }

        if self.restarts.len() >= self.max_restarts {
            return false;
        }
        self.restarts.push_back(now);
        true
    }
}

#[derive(Debug)]
struct PoolJob {
    job: MechanicsJob,
    reply: Sender<Result<Value, MechanicsError>>,
    canceled: Arc<AtomicBool>,
}

#[derive(Debug)]
enum PoolMessage {
    Run(PoolJob),
}

#[derive(Debug)]
struct WorkerExit {
    worker_id: usize,
}

#[derive(Debug)]
struct MechanicsPoolShared {
    tx: Sender<PoolMessage>,
    rx: Receiver<PoolMessage>,
    exit_tx: Sender<WorkerExit>,
    exit_rx: Receiver<WorkerExit>,
    workers: RwLock<HashMap<usize, thread::JoinHandle<()>>>,
    next_worker_id: AtomicUsize,
    desired_worker_count: usize,
    closed: AtomicBool,
    restart_blocked: AtomicBool,
    restart_guard: Mutex<RestartGuard>,
    execution_limits: MechanicsExecutionLimits,
    default_http_timeout_ms: Option<u64>,
    default_http_response_max_bytes: Option<usize>,
    endpoint_http_client: Arc<dyn EndpointHttpClient>,
    #[cfg(test)]
    force_worker_runtime_init_failure: bool,
}

impl MechanicsPoolShared {
    fn workers_read(&self) -> RwLockReadGuard<'_, HashMap<usize, thread::JoinHandle<()>>> {
        self.workers.read()
    }

    fn workers_write(&self) -> RwLockWriteGuard<'_, HashMap<usize, thread::JoinHandle<()>>> {
        self.workers.write()
    }

    fn restart_guard_guard(&self) -> MutexGuard<'_, RestartGuard> {
        self.restart_guard.lock()
    }

    fn remove_worker_handle(&self, worker_id: usize) -> Option<thread::JoinHandle<()>> {
        let mut workers = self.workers_write();
        workers.remove(&worker_id)
    }

    fn reap_finished_workers(&self) {
        let finished_ids: Vec<usize> = {
            let workers = self.workers_read();
            workers
                .iter()
                .filter_map(|(id, handle)| handle.is_finished().then_some(*id))
                .collect()
        };
        if finished_ids.is_empty() {
            return;
        }

        let mut finished_handles = Vec::with_capacity(finished_ids.len());
        {
            let mut workers = self.workers_write();
            for id in finished_ids {
                if let Some(handle) = workers.remove(&id) {
                    finished_handles.push(handle);
                }
            }
        }
        for handle in finished_handles {
            let _ = handle.join();
        }
    }

    fn spawn_worker(shared: &Arc<Self>) -> Result<usize, MechanicsError> {
        let worker_id = shared.next_worker_id.fetch_add(1, Ordering::Relaxed);

        let rx = shared.rx.clone();
        let exit_tx = shared.exit_tx.clone();
        let shared_for_worker = Arc::clone(shared);
        let (ready_tx, ready_rx) = bounded::<Result<(), MechanicsError>>(1);
        let endpoint_http_client = Arc::clone(&shared.endpoint_http_client);
        let execution_limits = shared.execution_limits;
        let default_http_timeout_ms = shared.default_http_timeout_ms;
        let default_http_response_max_bytes = shared.default_http_response_max_bytes;
        #[cfg(test)]
        let force_runtime_init_failure = shared.force_worker_runtime_init_failure;

        let handle = thread::Builder::new()
            .name(format!("mechanics-worker-{worker_id}"))
            .spawn(move || {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if force_runtime_init_failure {
                        let _ = ready_tx.send(Err(MechanicsError::runtime_pool(
                            "forced runtime initialization failure for tests",
                        )));
                        return;
                    }

                    let mut runtime = match RuntimeInternal::new_with_endpoint_http_client(
                        endpoint_http_client,
                    ) {
                        Ok(runtime) => {
                            let _ = ready_tx.send(Ok(()));
                            runtime
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            return;
                        }
                    };
                    runtime.set_execution_limits(execution_limits);
                    runtime.set_default_endpoint_timeout_ms(default_http_timeout_ms);
                    runtime
                        .set_default_endpoint_response_max_bytes(default_http_response_max_bytes);

                    loop {
                        match rx.recv_timeout(Duration::from_millis(100)) {
                            Ok(PoolMessage::Run(pool_job)) => {
                                if pool_job.canceled.load(Ordering::Acquire) {
                                    let _ = pool_job.reply.send(Err(MechanicsError::canceled(
                                        "job timed out before execution",
                                    )));
                                    continue;
                                }
                                let result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        runtime.run_source(pool_job.job)
                                    }));
                                match result {
                                    Ok(result) => {
                                        let _ = pool_job.reply.send(result);
                                    }
                                    Err(_) => {
                                        let _ = pool_job.reply.send(Err(MechanicsError::panic(
                                            "worker panicked while running job",
                                        )));
                                        break;
                                    }
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => {
                                if shared_for_worker.closed.load(Ordering::Acquire) {
                                    break;
                                }
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }));

                if run.is_err() {
                    // If the worker panicked outside task execution (runtime setup/loop),
                    // notify a synthetic panic event via restart path.
                    let _ =
                        ready_tx.send(Err(MechanicsError::panic("worker panicked during startup")));
                    let _ = exit_tx.send(WorkerExit { worker_id });
                    return;
                }

                let _ = exit_tx.send(WorkerExit { worker_id });
            })
            .map_err(|e| {
                MechanicsError::runtime_pool(format!("failed to spawn worker thread: {e}"))
            })?;

        {
            let mut workers = shared.workers_write();
            workers.insert(worker_id, handle);
        }

        match ready_rx.recv() {
            Ok(Ok(())) => {
                shared.restart_blocked.store(false, Ordering::Release);
                Ok(worker_id)
            }
            Ok(Err(err)) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    let _ = handle.join();
                }
                Err(err)
            }
            Err(_) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    let _ = handle.join();
                }
                Err(MechanicsError::runtime_pool(
                    "worker exited before startup completed",
                ))
            }
        }
    }

    fn live_workers(&self) -> usize {
        self.reap_finished_workers();
        self.workers_read().len()
    }

    fn reconcile_workers(shared: &Arc<Self>) {
        if shared.closed.load(Ordering::Acquire) {
            return;
        }

        let live = shared.live_workers();
        let missing = shared.desired_worker_count.saturating_sub(live);
        if missing == 0 {
            shared.restart_blocked.store(false, Ordering::Release);
            return;
        }

        for _ in 0..missing {
            let now = Instant::now();
            let can_restart = {
                let mut guard = shared.restart_guard_guard();
                guard.allow_restart(now)
            };
            if !can_restart {
                shared.restart_blocked.store(true, Ordering::Release);
                return;
            }

            if MechanicsPoolShared::spawn_worker(shared).is_err() {
                shared.restart_blocked.store(true, Ordering::Release);
                return;
            }
        }
    }
}

/// Thread pool of script runtimes for executing [`MechanicsJob`] workloads.
///
/// The pool is designed for stateless execution across interchangeable workers.
/// Any data required for one execution should be carried by the submitted job.
pub struct MechanicsPool {
    shared: Arc<MechanicsPoolShared>,
    enqueue_timeout: Duration,
    run_timeout: Duration,
    supervisor: Option<thread::JoinHandle<()>>,
}

/// Non-blocking snapshot of observable pool state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MechanicsPoolStats {
    /// Whether the pool has been marked closed.
    pub is_closed: bool,
    /// Whether worker restarts are currently blocked by the restart guard.
    pub restart_blocked: bool,
    /// Desired target number of workers.
    pub desired_workers: usize,
    /// Number of worker handles currently tracked (including finished-but-not-yet-reaped workers).
    pub known_workers: usize,
    /// Number of workers that are still running (`known_workers - finished_workers_pending_reap`).
    pub live_workers: usize,
    /// Number of finished worker handles still present in the workers map.
    pub finished_workers_pending_reap: usize,
    /// Current number of queued jobs waiting for workers.
    pub queue_depth: usize,
    /// Queue capacity when bounded.
    pub queue_capacity: Option<usize>,
    /// Number of restart attempts currently remembered inside the active restart window.
    pub restart_attempts_in_window: usize,
    /// Maximum restarts allowed within the restart window.
    pub max_restarts_in_window: usize,
}

impl MechanicsPool {
    fn deadline_from_timeout(timeout: Duration) -> Result<Instant, MechanicsError> {
        Instant::now().checked_add(timeout).ok_or_else(|| {
            MechanicsError::runtime_pool("run_timeout is too large for the current platform clock")
        })
    }

    fn remaining_to_deadline(deadline: Instant) -> Option<Duration> {
        let now = Instant::now();
        if now >= deadline {
            None
        } else {
            Some(deadline.duration_since(now))
        }
    }

    /// Returns a synchronous, non-blocking snapshot of pool state.
    ///
    /// This method intentionally avoids worker reaping and thread joins.
    pub fn stats(&self) -> MechanicsPoolStats {
        let (known_workers, finished_workers_pending_reap) = {
            let workers = self.shared.workers_read();
            let known = workers.len();
            let finished = workers.values().filter(|h| h.is_finished()).count();
            (known, finished)
        };
        let (restart_attempts_in_window, max_restarts_in_window) = {
            let guard = self.shared.restart_guard_guard();
            (guard.restarts.len(), guard.max_restarts)
        };

        MechanicsPoolStats {
            is_closed: self.shared.closed.load(Ordering::Acquire),
            restart_blocked: self.shared.restart_blocked.load(Ordering::Acquire),
            desired_workers: self.shared.desired_worker_count,
            known_workers,
            live_workers: known_workers.saturating_sub(finished_workers_pending_reap),
            finished_workers_pending_reap,
            queue_depth: self.shared.rx.len(),
            queue_capacity: self.shared.rx.capacity(),
            restart_attempts_in_window,
            max_restarts_in_window,
        }
    }

    /// Creates a new mechanics runtime pool.
    ///
    /// Construction is fail-fast:
    /// - invalid config values return [`MechanicsError::RuntimePool`],
    /// - each worker must initialize its runtime successfully,
    /// - supervisor thread startup must succeed.
    ///
    /// If any of those steps fail, no usable pool is returned.
    pub fn new(config: MechanicsPoolConfig) -> Result<Self, MechanicsError> {
        if config.worker_count == 0 {
            return Err(MechanicsError::runtime_pool("worker_count must be > 0"));
        }
        if config.queue_capacity == 0 {
            return Err(MechanicsError::runtime_pool("queue_capacity must be > 0"));
        }
        if config.max_restarts_in_window == 0 {
            return Err(MechanicsError::runtime_pool(
                "max_restarts_in_window must be > 0",
            ));
        }
        if config.run_timeout.is_zero() {
            return Err(MechanicsError::runtime_pool("run_timeout must be > 0"));
        }
        if Instant::now().checked_add(config.run_timeout).is_none() {
            return Err(MechanicsError::runtime_pool(
                "run_timeout is too large for the current platform clock",
            ));
        }

        let endpoint_http_client = if let Some(client) = config.endpoint_http_client.clone() {
            client
        } else {
            let reqwest_client = reqwest::Client::builder()
                .build()
                .map_err(into_io_error)
                .map_err(|e| MechanicsError::runtime_pool(e.to_string()))?;
            Arc::new(ReqwestEndpointHttpClient::new(reqwest_client))
        };

        let (tx, rx) = bounded(config.queue_capacity);
        let (exit_tx, exit_rx) = unbounded::<WorkerExit>();

        let shared = Arc::new(MechanicsPoolShared {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: RwLock::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            desired_worker_count: config.worker_count,
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: Mutex::new(RestartGuard::new(
                config.restart_window,
                config.max_restarts_in_window,
            )),
            execution_limits: config.execution_limits,
            default_http_timeout_ms: config.default_http_timeout_ms,
            default_http_response_max_bytes: config.default_http_response_max_bytes,
            endpoint_http_client,
            #[cfg(test)]
            force_worker_runtime_init_failure: config.force_worker_runtime_init_failure,
        });

        for _ in 0..config.worker_count {
            MechanicsPoolShared::spawn_worker(&shared)?;
        }

        let supervisor_shared = Arc::clone(&shared);
        let supervisor = thread::Builder::new()
            .name("mechanics-supervisor".to_owned())
            .spawn(move || {
                loop {
                    if supervisor_shared.closed.load(Ordering::Acquire) {
                        break;
                    }

                    match supervisor_shared
                        .exit_rx
                        .recv_timeout(Duration::from_millis(100))
                    {
                        Ok(event) => {
                            let maybe_old = {
                                let mut workers = supervisor_shared.workers_write();
                                workers.remove(&event.worker_id)
                            };
                            if let Some(handle) = maybe_old {
                                let _ = handle.join();
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }

                    MechanicsPoolShared::reconcile_workers(&supervisor_shared);
                }
            })
            .map_err(|e| {
                MechanicsError::runtime_pool(format!("failed to spawn supervisor thread: {e}"))
            })?;

        Ok(Self {
            shared,
            enqueue_timeout: config.enqueue_timeout,
            run_timeout: config.run_timeout,
            supervisor: Some(supervisor),
        })
    }

    /// Enqueues a job and blocks until the script finishes or fails.
    ///
    /// Timeout behavior:
    /// 1. Waits up to [`MechanicsPoolConfig::enqueue_timeout`] for queue space.
    /// 2. Entire call is additionally bounded by [`MechanicsPoolConfig::run_timeout`].
    ///
    /// This keeps `run` from blocking indefinitely under load.
    /// If the wait times out, the job is marked canceled before execution (best effort).
    /// Jobs that already started continue until runtime limits terminate them.
    pub fn run(&self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(MechanicsError::pool_closed("runtime pool is closed"));
        }
        if self.shared.restart_blocked.load(Ordering::Acquire) && self.shared.live_workers() == 0 {
            return Err(MechanicsError::worker_unavailable(
                "all workers are unavailable and restart guard is active",
            ));
        }

        let deadline = Self::deadline_from_timeout(self.run_timeout)?;
        let (reply_tx, reply_rx) = bounded(1);
        let canceled = Arc::new(AtomicBool::new(false));
        let message = PoolMessage::Run(PoolJob {
            job,
            reply: reply_tx,
            canceled: Arc::clone(&canceled),
        });

        let Some(remaining_for_enqueue) = Self::remaining_to_deadline(deadline) else {
            canceled.store(true, Ordering::Release);
            return Err(MechanicsError::run_timeout(
                "run timeout elapsed before enqueue",
            ));
        };
        let enqueue_wait = self.enqueue_timeout.min(remaining_for_enqueue);
        let limited_by_run_timeout = enqueue_wait == remaining_for_enqueue;
        match self.shared.tx.send_timeout(message, enqueue_wait) {
            Ok(()) => {}
            Err(SendTimeoutError::Timeout(PoolMessage::Run(pool_job))) => {
                if limited_by_run_timeout {
                    pool_job.canceled.store(true, Ordering::Release);
                    let _ = pool_job.reply.send(Err(MechanicsError::run_timeout(
                        "run timeout elapsed while waiting to enqueue",
                    )));
                    return Err(MechanicsError::run_timeout(
                        "run timeout elapsed while waiting to enqueue",
                    ));
                }
                let _ = pool_job.reply.send(Err(MechanicsError::queue_timeout(
                    "enqueue timed out because queue is full",
                )));
                return Err(MechanicsError::queue_timeout(
                    "enqueue timed out because queue is full",
                ));
            }
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err(MechanicsError::worker_unavailable(
                    "job queue disconnected from workers",
                ));
            }
        }

        let Some(remaining_for_reply) = Self::remaining_to_deadline(deadline) else {
            canceled.store(true, Ordering::Release);
            return Err(MechanicsError::run_timeout(
                "run timeout elapsed while waiting for worker reply",
            ));
        };
        match reply_rx.recv_timeout(remaining_for_reply) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                canceled.store(true, Ordering::Release);
                Err(MechanicsError::run_timeout(
                    "run timeout elapsed while waiting for worker reply",
                ))
            }
            Err(_) => Err(MechanicsError::worker_unavailable(
                "worker dropped reply channel",
            )),
        }
    }

    /// Attempts to enqueue a job without waiting for queue space.
    ///
    /// After successful enqueue, total call duration is bounded by
    /// [`MechanicsPoolConfig::run_timeout`], like [`Self::run`].
    ///
    /// Returns [`MechanicsError::QueueFull`] immediately if the queue is currently full.
    pub fn run_try_enqueue(&self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(MechanicsError::pool_closed("runtime pool is closed"));
        }
        if self.shared.restart_blocked.load(Ordering::Acquire) && self.shared.live_workers() == 0 {
            return Err(MechanicsError::worker_unavailable(
                "all workers are unavailable and restart guard is active",
            ));
        }

        let deadline = Self::deadline_from_timeout(self.run_timeout)?;
        let (reply_tx, reply_rx) = bounded(1);
        let canceled = Arc::new(AtomicBool::new(false));
        let message = PoolMessage::Run(PoolJob {
            job,
            reply: reply_tx,
            canceled: Arc::clone(&canceled),
        });

        match self.shared.tx.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(PoolMessage::Run(_))) => {
                return Err(MechanicsError::queue_full("queue is full"));
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(MechanicsError::worker_unavailable(
                    "job queue disconnected from workers",
                ));
            }
        }

        let Some(remaining_for_reply) = Self::remaining_to_deadline(deadline) else {
            canceled.store(true, Ordering::Release);
            return Err(MechanicsError::run_timeout(
                "run timeout elapsed while waiting for worker reply",
            ));
        };
        match reply_rx.recv_timeout(remaining_for_reply) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                canceled.store(true, Ordering::Release);
                Err(MechanicsError::run_timeout(
                    "run timeout elapsed while waiting for worker reply",
                ))
            }
            Err(_) => Err(MechanicsError::worker_unavailable(
                "worker dropped reply channel",
            )),
        }
    }
}

impl Drop for MechanicsPool {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);

        loop {
            match self.shared.rx.try_recv() {
                Ok(PoolMessage::Run(job)) => {
                    let _ = job.reply.send(Err(MechanicsError::canceled(
                        "pool dropped before job execution",
                    )));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }

        let mut workers = self.shared.workers_write();
        for (_, handle) in workers.drain() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests;
