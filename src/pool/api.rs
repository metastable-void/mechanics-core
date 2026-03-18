use crate::{
    error::MechanicsError,
    http::{ReqwestEndpointHttpClient, into_io_error},
    job::MechanicsJob,
};
use crossbeam_channel::{
    RecvTimeoutError, SendTimeoutError, Sender, TrySendError, bounded, select, tick, unbounded,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    config::MechanicsPoolConfig,
    restart_guard::RestartGuard,
    shared::MechanicsPoolShared,
    worker::{PoolJob, PoolMessage, WorkerExit},
};

/// Thread pool of script runtimes for executing [`MechanicsJob`] workloads.
///
/// The pool is designed for stateless execution across interchangeable workers.
/// Any data required for one execution should be carried by the submitted job.
pub struct MechanicsPool {
    pub(crate) shared: Arc<MechanicsPoolShared>,
    pub(crate) enqueue_timeout: Duration,
    pub(crate) run_timeout: Duration,
    pub(crate) supervisor: Option<thread::JoinHandle<()>>,
    pub(crate) supervisor_shutdown_tx: Option<Sender<()>>,
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
            workers: parking_lot::RwLock::new(HashMap::new()),
            next_worker_id: std::sync::atomic::AtomicUsize::new(0),
            desired_worker_count: config.worker_count,
            closed: std::sync::atomic::AtomicBool::new(false),
            restart_blocked: std::sync::atomic::AtomicBool::new(false),
            restart_guard: parking_lot::Mutex::new(RestartGuard::new(
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
        let (supervisor_shutdown_tx, supervisor_shutdown_rx) = bounded::<()>(1);
        let reconcile_tick = tick(Self::reconcile_interval(config.restart_window));
        let supervisor = thread::Builder::new()
            .name("mechanics-supervisor".to_owned())
            .spawn(move || {
                loop {
                    select! {
                        recv(supervisor_shutdown_rx) -> _ => {
                            break;
                        }
                        recv(supervisor_shared.exit_rx) -> event => {
                            match event {
                                Ok(event) => {
                                    let maybe_old = {
                                        let mut workers = supervisor_shared.workers_write();
                                        workers.remove(&event.worker_id)
                                    };
                                    if let Some(handle) = maybe_old {
                                        let _ = handle.join.join();
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        recv(reconcile_tick) -> _ => {}
                    }

                    if supervisor_shared.closed.load(Ordering::Acquire) {
                        break;
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
            supervisor_shutdown_tx: Some(supervisor_shutdown_tx),
        })
    }

    pub(crate) fn reconcile_interval(restart_window: Duration) -> Duration {
        let quarter = restart_window.div_f64(4.0);
        quarter
            .max(Duration::from_millis(50))
            .min(Duration::from_millis(500))
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
