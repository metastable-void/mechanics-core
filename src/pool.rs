use crate::{
    error::MechanicsError,
    http::into_io_error,
    job::{MechanicsExecutionLimits, MechanicsJob},
    runtime::RuntimeInternal,
};
use crossbeam_channel::{
    Receiver, RecvTimeoutError, SendTimeoutError, Sender, TryRecvError, TrySendError, bounded,
};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Configuration for constructing a [`MechanicsPool`].
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
    /// Sliding window duration used by worker restart rate limiting.
    pub restart_window: Duration,
    /// Maximum automatic worker restarts allowed within `restart_window`.
    pub max_restarts_in_window: usize,
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
            restart_window: Duration::from_secs(10),
            max_restarts_in_window: 16,
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
    Shutdown,
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
    workers: Mutex<HashMap<usize, thread::JoinHandle<()>>>,
    next_worker_id: AtomicUsize,
    closed: AtomicBool,
    restart_blocked: AtomicBool,
    restart_guard: Mutex<RestartGuard>,
    execution_limits: MechanicsExecutionLimits,
    default_http_timeout_ms: Option<u64>,
    reqwest_client: reqwest::Client,
}

impl MechanicsPoolShared {
    fn spawn_worker(shared: &Arc<Self>) -> usize {
        let worker_id = shared.next_worker_id.fetch_add(1, Ordering::Relaxed);

        let rx = shared.rx.clone();
        let exit_tx = shared.exit_tx.clone();
        let (start_tx, start_rx) = bounded::<()>(0);
        let reqwest_client = shared.reqwest_client.clone();
        let execution_limits = shared.execution_limits;
        let default_http_timeout_ms = shared.default_http_timeout_ms;

        let handle = thread::spawn(move || {
            if start_rx.recv().is_err() {
                let _ = exit_tx.send(WorkerExit { worker_id });
                return;
            }

            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut runtime = RuntimeInternal::new_with_client(reqwest_client);
                runtime.set_execution_limits(execution_limits);
                runtime.set_default_endpoint_timeout_ms(default_http_timeout_ms);

                loop {
                    match rx.recv() {
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
                        Ok(PoolMessage::Shutdown) => break,
                        Err(_) => break,
                    }
                }
            }));

            if run.is_err() {
                // If the worker panicked outside task execution (runtime setup/loop),
                // notify a synthetic panic event via restart path.
                let _ = exit_tx.send(WorkerExit { worker_id });
                return;
            }

            let _ = exit_tx.send(WorkerExit { worker_id });
        });

        let mut workers = shared.workers.lock().expect("workers mutex poisoned");
        workers.insert(worker_id, handle);
        let _ = start_tx.send(());
        shared.restart_blocked.store(false, Ordering::Release);
        worker_id
    }

    fn live_workers(&self) -> usize {
        self.workers.lock().expect("workers mutex poisoned").len()
    }
}

/// Thread pool of script runtimes for executing [`MechanicsJob`] workloads.
pub struct MechanicsPool {
    shared: Arc<MechanicsPoolShared>,
    enqueue_timeout: Duration,
    run_timeout: Duration,
    supervisor: Option<thread::JoinHandle<()>>,
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

    /// Creates a new mechanics runtime pool.
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

        let reqwest_client = reqwest::Client::builder()
            .build()
            .map_err(into_io_error)
            .map_err(|e| MechanicsError::runtime_pool(e.to_string()))?;

        let (tx, rx) = bounded(config.queue_capacity);
        let (exit_tx, exit_rx) =
            bounded::<WorkerExit>(config.worker_count.saturating_mul(4).max(8));

        let shared = Arc::new(MechanicsPoolShared {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: Mutex::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: Mutex::new(RestartGuard::new(
                config.restart_window,
                config.max_restarts_in_window,
            )),
            execution_limits: config.execution_limits,
            default_http_timeout_ms: config.default_http_timeout_ms,
            reqwest_client,
        });

        for _ in 0..config.worker_count {
            MechanicsPoolShared::spawn_worker(&shared);
        }

        let supervisor_shared = Arc::clone(&shared);
        let supervisor = thread::spawn(move || {
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
                            let mut workers = supervisor_shared
                                .workers
                                .lock()
                                .expect("workers mutex poisoned");
                            workers.remove(&event.worker_id)
                        };
                        if let Some(handle) = maybe_old {
                            let _ = handle.join();
                        }

                        if supervisor_shared.closed.load(Ordering::Acquire) {
                            continue;
                        }

                        let now = Instant::now();
                        let can_restart = {
                            let mut guard = supervisor_shared
                                .restart_guard
                                .lock()
                                .expect("restart guard mutex poisoned");
                            guard.allow_restart(now)
                        };

                        if can_restart {
                            MechanicsPoolShared::spawn_worker(&supervisor_shared);
                        } else {
                            supervisor_shared
                                .restart_blocked
                                .store(true, Ordering::Release);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

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
            Err(SendTimeoutError::Timeout(PoolMessage::Shutdown)) => {
                return Err(MechanicsError::runtime_pool(
                    "unexpected shutdown message timeout",
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
            Err(TrySendError::Full(PoolMessage::Shutdown)) => {
                return Err(MechanicsError::runtime_pool(
                    "unexpected shutdown queue state",
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
                Ok(PoolMessage::Shutdown) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        let worker_count = self.shared.live_workers();
        for _ in 0..worker_count {
            let _ = self.shared.tx.send(PoolMessage::Shutdown);
        }

        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }

        let mut workers = self.shared.workers.lock().expect("workers mutex poisoned");
        for (_, handle) in workers.drain() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpEndpoint, MechanicsConfig};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Barrier;

    fn make_job(source: &str, config: MechanicsConfig, arg: Value) -> MechanicsJob {
        MechanicsJob {
            mod_source: Arc::<str>::from(source),
            arg: Arc::new(arg),
            config: Arc::new(config),
        }
    }

    fn http_status_reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Status",
        }
    }

    fn spawn_json_server_with_status(
        delay: Duration,
        status: u16,
        response_json: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("read local addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");

            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            if !delay.is_zero() {
                thread::sleep(delay);
            }

            let body = response_json.as_bytes();
            let response = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                http_status_reason(status),
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write headers");
            stream.write_all(body).expect("write body");
            let _ = stream.flush();
        });

        (format!("http://{addr}"), handle)
    }

    fn spawn_json_server(
        delay: Duration,
        response_json: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        spawn_json_server_with_status(delay, 200, response_json)
    }

    fn endpoint_config(name: &str, endpoint: HttpEndpoint) -> MechanicsConfig {
        let mut endpoints = HashMap::new();
        endpoints.insert(name.to_owned(), endpoint);
        MechanicsConfig::new(endpoints)
    }

    fn synthetic_pool(
        queue_capacity: usize,
        execution_limits: MechanicsExecutionLimits,
    ) -> MechanicsPool {
        let (tx, rx) = bounded(queue_capacity);
        let (exit_tx, exit_rx) = bounded(8);
        let shared = Arc::new(MechanicsPoolShared {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: Mutex::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: Mutex::new(RestartGuard::new(Duration::from_secs(1), 1)),
            execution_limits,
            default_http_timeout_ms: None,
            reqwest_client: reqwest::Client::new(),
        });

        MechanicsPool {
            shared,
            enqueue_timeout: Duration::from_millis(10),
            run_timeout: Duration::from_millis(50),
            supervisor: None,
        }
    }

    fn is_transient_internet_transport_error(msg: &str) -> bool {
        let msg = msg.to_ascii_lowercase();
        msg.contains("error sending request")
            || msg.contains("dns error")
            || msg.contains("failed to lookup address")
            || msg.contains("connection refused")
            || msg.contains("connection reset")
            || msg.contains("network is unreachable")
            || msg.contains("tls")
            || msg.contains("certificate")
    }

    fn run_internet_job_with_retry(
        pool: &MechanicsPool,
        job: &MechanicsJob,
        test_name: &str,
    ) -> Option<Result<Value, MechanicsError>> {
        const ATTEMPTS: usize = 3;
        for attempt in 1..=ATTEMPTS {
            let result = pool.run(job.clone());
            match &result {
                Err(MechanicsError::Execution(msg))
                    if is_transient_internet_transport_error(msg) =>
                {
                    if attempt < ATTEMPTS {
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    eprintln!(
                        "skipping {test_name}: transient internet transport error after {ATTEMPTS} attempts: {msg}"
                    );
                    return None;
                }
                _ => return Some(result),
            }
        }
        None
    }

    #[test]
    fn pool_new_rejects_invalid_config() {
        let err = match MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 0,
            ..Default::default()
        }) {
            Err(err) => err,
            Ok(_) => panic!("worker_count=0 must fail"),
        };
        assert!(matches!(err, MechanicsError::RuntimePool(_)));

        let err = match MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            queue_capacity: 0,
            ..Default::default()
        }) {
            Err(err) => err,
            Ok(_) => panic!("queue_capacity=0 must fail"),
        };
        assert!(matches!(err, MechanicsError::RuntimePool(_)));

        let err = match MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            queue_capacity: 1,
            max_restarts_in_window: 0,
            ..Default::default()
        }) {
            Err(err) => err,
            Ok(_) => panic!("max_restarts_in_window=0 must fail"),
        };
        assert!(matches!(err, MechanicsError::RuntimePool(_)));

        let err = match MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            queue_capacity: 1,
            run_timeout: Duration::ZERO,
            ..Default::default()
        }) {
            Err(err) => err,
            Ok(_) => panic!("run_timeout=0 must fail"),
        };
        assert!(matches!(err, MechanicsError::RuntimePool(_)));
    }

    #[test]
    fn run_and_run_try_enqueue_fail_when_pool_closed() {
        let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
        pool.shared.closed.store(true, Ordering::Release);

        let job = make_job(
            r#"export default function main() { return 1; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        let err = pool
            .run(job.clone())
            .expect_err("closed pool must reject run");
        assert!(matches!(err, MechanicsError::PoolClosed(_)));

        let err = pool
            .run_try_enqueue(job)
            .expect_err("closed pool must reject run_try_enqueue");
        assert!(matches!(err, MechanicsError::PoolClosed(_)));
    }

    #[test]
    fn run_and_run_try_enqueue_fail_when_workers_unavailable_and_restart_blocked() {
        let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
        pool.shared.restart_blocked.store(true, Ordering::Release);

        let job = make_job(
            r#"export default function main() { return 1; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        let err = pool
            .run(job.clone())
            .expect_err("must fail when no workers and restart blocked");
        assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));

        let err = pool
            .run_try_enqueue(job)
            .expect_err("must fail when no workers and restart blocked");
        assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));
    }

    #[test]
    fn run_maps_reply_timeout_to_run_timeout() {
        let limits = MechanicsExecutionLimits {
            max_execution_time: Duration::from_millis(5),
            ..Default::default()
        };
        let pool = synthetic_pool(8, limits);

        {
            let mut workers = pool.shared.workers.lock().expect("workers mutex poisoned");
            workers.insert(0, thread::spawn(|| {}));
        }

        let job = make_job(
            r#"export default function main() { return 1; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        let err = pool
            .run(job)
            .expect_err("no worker consumes queue; should hit reply timeout");
        assert!(matches!(err, MechanicsError::RunTimeout(_)));
    }

    #[test]
    fn run_timeout_can_expire_while_waiting_to_enqueue() {
        let (tx, rx) = bounded(1);
        let (exit_tx, exit_rx) = bounded(8);
        let shared = Arc::new(MechanicsPoolShared {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: Mutex::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: Mutex::new(RestartGuard::new(Duration::from_secs(1), 1)),
            execution_limits: MechanicsExecutionLimits::default(),
            default_http_timeout_ms: None,
            reqwest_client: reqwest::Client::new(),
        });

        let pool = MechanicsPool {
            shared,
            enqueue_timeout: Duration::from_secs(1),
            run_timeout: Duration::from_millis(5),
            supervisor: None,
        };

        let (reply_tx, _reply_rx) = bounded(1);
        let queued = make_job(
            r#"export default function main() { return 0; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        pool.shared
            .tx
            .send(PoolMessage::Run(PoolJob {
                job: queued,
                reply: reply_tx,
                canceled: Arc::new(AtomicBool::new(false)),
            }))
            .expect("fill queue");

        let job = make_job(
            r#"export default function main() { return 1; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        let err = pool
            .run(job)
            .expect_err("run_timeout should fire while waiting for enqueue");
        assert!(matches!(err, MechanicsError::RunTimeout(_)));
    }

    #[test]
    fn drop_cancels_queued_jobs() {
        let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
        let (reply_tx, reply_rx) = bounded(1);

        let job = make_job(
            r#"export default function main() { return 1; }"#,
            MechanicsConfig::new(HashMap::new()),
            Value::Null,
        );
        pool.shared
            .tx
            .send(PoolMessage::Run(PoolJob {
                job,
                reply: reply_tx,
                canceled: Arc::new(AtomicBool::new(false)),
            }))
            .expect("enqueue queued job");

        drop(pool);
        let canceled = reply_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("drop should send canceled error");
        assert!(matches!(canceled, Err(MechanicsError::Canceled(_))));
    }

    #[test]
    fn restart_guard_blocks_after_limit() {
        let mut guard = RestartGuard::new(Duration::from_secs(1), 2);
        let t0 = Instant::now();
        assert!(guard.allow_restart(t0));
        assert!(guard.allow_restart(t0 + Duration::from_millis(100)));
        assert!(!guard.allow_restart(t0 + Duration::from_millis(200)));
        assert!(guard.allow_restart(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn run_simple_module_returns_value() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(arg) {
                return { ok: true, got: arg };
            }
        "#;
        let job = make_job(
            source,
            MechanicsConfig::new(HashMap::new()),
            json!({"n": 7}),
        );
        let value = pool.run(job).expect("run module");
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["got"]["n"], json!(7));
    }

    #[test]
    fn loop_iteration_limit_stops_infinite_loop() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(5),
                max_loop_iterations: 1_000,
                max_recursion_depth: 512,
                max_stack_size: 10 * 1024,
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(_arg) {
                while (true) {}
            }
        "#;
        let job = make_job(source, MechanicsConfig::new(HashMap::new()), Value::Null);
        let err = pool.run(job).expect_err("must hit loop iteration limit");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("Maximum loop iteration limit"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    fn json_conversion_error_is_reported_as_execution_error() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(_arg) {
                return 1n;
            }
        "#;
        let job = make_job(source, MechanicsConfig::new(HashMap::new()), Value::Null);
        let err = pool
            .run(job)
            .expect_err("BigInt result should fail JSON conversion");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(
                    msg.contains("BigInt")
                        || msg.contains("JSON")
                        || msg.contains("serialize")
                        || msg.contains("convert")
                );
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    fn invalid_endpoint_header_is_reported_as_execution_error() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            ..Default::default()
        })
        .expect("create pool");

        let mut headers = HashMap::new();
        headers.insert("bad header".to_owned(), "value".to_owned());
        let endpoint = HttpEndpoint::new("https://example.com/anything", headers);
        let config = endpoint_config("bad", endpoint);

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("bad", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"headers"}));
        let err = pool
            .run(job)
            .expect_err("invalid configured header must fail");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("invalid header name"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    fn pending_default_promise_is_reported_as_execution_error() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(_arg) {
                return new Promise(() => {});
            }
        "#;
        let job = make_job(source, MechanicsConfig::new(HashMap::new()), Value::Null);
        let err = pool
            .run(job)
            .expect_err("pending promise should not be treated as success");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("did not settle") || msg.contains("pending"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    fn unhandled_async_error_is_reported_as_execution_error() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(_arg) {
                Promise.resolve().then(() => {
                    throw new Error("boom");
                });
                return 1;
            }
        "#;
        let job = make_job(source, MechanicsConfig::new(HashMap::new()), Value::Null);
        let err = pool
            .run(job)
            .expect_err("unhandled async error should fail current job");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("boom") || msg.contains("Error") || msg.contains("Unhandled"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    fn oversized_execution_timeout_is_reported_as_execution_error() {
        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::MAX,
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            export default function main(_arg) {
                return 1;
            }
        "#;
        let job = make_job(source, MechanicsConfig::new(HashMap::new()), Value::Null);
        let err = pool
            .run(job)
            .expect_err("oversized max_execution_time must not panic worker");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("max_execution_time") || msg.contains("too large"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn execution_timeout_stops_slow_async_job() {
        let (url, server) = spawn_json_server(Duration::from_millis(350), r#"{"ok":true}"#);
        let endpoint = HttpEndpoint::new(&url, HashMap::new()).with_timeout_ms(Some(2_000));
        let config = endpoint_config("slow", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_millis(120),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("slow", arg);
            }
        "#;
        let job = make_job(source, config, Value::Null);
        let err = pool.run(job).expect_err("must time out");
        let _ = server.join();
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("Maximum execution time exceeded"));
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn run_try_enqueue_reports_queue_full() {
        let (url, server) = spawn_json_server(Duration::from_millis(900), r#"{"ok":true}"#);
        let blocking_endpoint =
            HttpEndpoint::new(&url, HashMap::new()).with_timeout_ms(Some(3_000));
        let blocking_cfg = endpoint_config("slow", blocking_endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            queue_capacity: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(3),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let blocking = make_job(
            r#"
                import endpoint from "mechanics:endpoint";
                export default async function main(arg) {
                    return await endpoint("slow", arg);
                }
            "#,
            blocking_cfg,
            Value::Null,
        );

        let pool_ref = Arc::new(pool);
        let p = Arc::clone(&pool_ref);
        let t = thread::spawn(move || p.run(blocking));
        thread::sleep(Duration::from_millis(40));

        let contenders = 8usize;
        let gate = Arc::new(Barrier::new(contenders + 1));
        let mut handles = Vec::with_capacity(contenders);
        for _ in 0..contenders {
            let p = Arc::clone(&pool_ref);
            let g = Arc::clone(&gate);
            handles.push(thread::spawn(move || {
                g.wait();
                let over = make_job(
                    r#"export default function main() { return { over: true }; }"#,
                    MechanicsConfig::new(HashMap::new()),
                    Value::Null,
                );
                p.run_try_enqueue(over)
            }));
        }
        gate.wait();

        let mut saw_queue_full = false;
        for h in handles {
            match h.join().expect("join contender") {
                Err(MechanicsError::QueueFull(_)) => saw_queue_full = true,
                Ok(_) => {}
                Err(MechanicsError::Execution(_)) => {}
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        assert!(
            saw_queue_full,
            "expected to observe QueueFull while worker is blocked"
        );

        let _ = t.join();
        let _ = server.join();
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn run_reports_enqueue_timeout_when_queue_is_full() {
        let (url, server) = spawn_json_server(Duration::from_millis(900), r#"{"ok":true}"#);
        let blocking_endpoint =
            HttpEndpoint::new(&url, HashMap::new()).with_timeout_ms(Some(3_000));
        let blocking_cfg = endpoint_config("slow", blocking_endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            queue_capacity: 1,
            enqueue_timeout: Duration::from_millis(10),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(3),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let blocking = make_job(
            r#"
                import endpoint from "mechanics:endpoint";
                export default async function main(arg) {
                    return await endpoint("slow", arg);
                }
            "#,
            blocking_cfg,
            Value::Null,
        );

        let pool_ref = Arc::new(pool);
        let p = Arc::clone(&pool_ref);
        let t = thread::spawn(move || p.run(blocking));
        thread::sleep(Duration::from_millis(40));

        let contenders = 8usize;
        let gate = Arc::new(Barrier::new(contenders + 1));
        let mut handles = Vec::with_capacity(contenders);
        for _ in 0..contenders {
            let p = Arc::clone(&pool_ref);
            let g = Arc::clone(&gate);
            handles.push(thread::spawn(move || {
                g.wait();
                let timeout = make_job(
                    r#"export default function main() { return 2; }"#,
                    MechanicsConfig::new(HashMap::new()),
                    Value::Null,
                );
                p.run(timeout)
            }));
        }
        gate.wait();

        let mut saw_queue_timeout = false;
        for h in handles {
            match h.join().expect("join contender") {
                Err(MechanicsError::QueueTimeout(_)) => saw_queue_timeout = true,
                Ok(_) => {}
                Err(MechanicsError::Execution(_)) => {}
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        assert!(
            saw_queue_timeout,
            "expected to observe QueueTimeout while worker is blocked"
        );

        let _ = t.join();
        let _ = server.join();
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn endpoint_uses_pool_default_timeout() {
        let (url, server) = spawn_json_server(Duration::from_millis(180), r#"{"ok":true}"#);
        let endpoint = HttpEndpoint::new(&url, HashMap::new());
        let config = endpoint_config("slow", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(60),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(2),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("slow", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"world"}));
        let err = pool.run(job).expect_err("request should timeout");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(
                    msg.contains("timed out")
                        || msg.contains("timeout")
                        || msg.contains("deadline")
                        || msg.contains("request")
                        || msg.contains("Maximum execution time exceeded")
                );
            }
            other => panic!("unexpected error kind: {other}"),
        }

        let _ = server.join();
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn endpoint_specific_timeout_overrides_pool_default() {
        let (url, server) = spawn_json_server(Duration::from_millis(150), r#"{"ok":true}"#);
        let endpoint = HttpEndpoint::new(&url, HashMap::new()).with_timeout_ms(Some(400));
        let config = endpoint_config("slow", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(40),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(2),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("slow", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"world"}));
        let value = pool
            .run(job)
            .expect("endpoint-level timeout should allow success");
        assert_eq!(value["ok"], json!(true));

        let _ = server.join();
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn endpoint_non_success_status_is_error_by_default() {
        let (url, server) =
            spawn_json_server_with_status(Duration::from_millis(0), 500, r#"{"ok":false}"#);
        let endpoint = HttpEndpoint::new(&url, HashMap::new());
        let config = endpoint_config("failing", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(2),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("failing", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"status"}));
        let err = pool
            .run(job)
            .expect_err("non-success status must fail by default");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(msg.contains("500") || msg.contains("status"));
            }
            other => panic!("unexpected error kind: {other}"),
        }

        let _ = server.join();
    }

    #[test]
    #[ignore = "requires local socket bind permission in the execution environment"]
    fn endpoint_non_success_status_can_be_allowed() {
        let (url, server) =
            spawn_json_server_with_status(Duration::from_millis(0), 500, r#"{"ok":false}"#);
        let endpoint = HttpEndpoint::new(&url, HashMap::new()).with_allow_non_success_status(true);
        let config = endpoint_config("failing", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(2),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("failing", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"status"}));
        let value = pool
            .run(job)
            .expect("opt-in should allow JSON parse on non-success status");
        assert_eq!(value["ok"], json!(false));

        let _ = server.join();
    }

    #[test]
    #[ignore = "requires internet access to https://httpbin.org"]
    fn internet_endpoint_roundtrip_httpbin() {
        let endpoint = HttpEndpoint::new("https://httpbin.org/post", HashMap::new());
        let config = endpoint_config("internet", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(10_000),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(15),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("internet", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"internet"}));
        let Some(result) =
            run_internet_job_with_retry(&pool, &job, "internet_endpoint_roundtrip_httpbin")
        else {
            return;
        };
        let value = result.expect("internet endpoint call should succeed");

        assert_eq!(value["json"]["hello"], json!("internet"));
    }

    #[test]
    #[ignore = "requires internet access to https://httpbin.org"]
    fn internet_http_timeout_from_pool_default() {
        let endpoint = HttpEndpoint::new("https://httpbin.org/delay/3", HashMap::new());
        let config = endpoint_config("internet", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(400),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(10),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("internet", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"timeout"}));
        let Some(result) =
            run_internet_job_with_retry(&pool, &job, "internet_http_timeout_from_pool_default")
        else {
            return;
        };
        let err = result.expect_err("request should timeout");
        match err {
            MechanicsError::Execution(msg) => {
                assert!(
                    msg.contains("timed out")
                        || msg.contains("timeout")
                        || msg.contains("deadline")
                );
            }
            other => panic!("unexpected error kind: {other}"),
        }
    }

    #[test]
    #[ignore = "requires internet access to https://httpbin.org"]
    fn internet_endpoint_timeout_overrides_pool_default() {
        let endpoint = HttpEndpoint::new("https://httpbin.org/delay/1", HashMap::new())
            .with_timeout_ms(Some(4_000));
        let config = endpoint_config("internet", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(200),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(10),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("internet", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"override"}));
        let Some(result) = run_internet_job_with_retry(
            &pool,
            &job,
            "internet_endpoint_timeout_overrides_pool_default",
        ) else {
            return;
        };
        let value = result.expect("endpoint-level timeout should allow success");
        let echoed_json = value
            .get("json")
            .and_then(|v| v.get("hello"))
            .and_then(Value::as_str);
        let echoed_data = value.get("data").and_then(Value::as_str);
        let json_ok = echoed_json == Some("override");
        let data_ok = echoed_data
            .map(|s| s.contains("\"hello\":\"override\"") || s.contains("\"hello\": \"override\""))
            .unwrap_or(false);
        assert!(
            json_ok || data_ok,
            "httpbin did not echo request payload in expected fields: {value}"
        );
    }

    #[test]
    #[ignore = "requires internet access to https://httpbin.org"]
    fn internet_endpoint_sends_custom_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Mechanics-Test".to_owned(), "header-check".to_owned());
        let endpoint = HttpEndpoint::new("https://httpbin.org/anything", headers);
        let config = endpoint_config("internet", endpoint);

        let pool = MechanicsPool::new(MechanicsPoolConfig {
            worker_count: 1,
            default_http_timeout_ms: Some(10_000),
            execution_limits: MechanicsExecutionLimits {
                max_execution_time: Duration::from_secs(15),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("create pool");

        let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("internet", arg);
            }
        "#;
        let job = make_job(source, config, json!({"hello":"headers"}));
        let Some(result) =
            run_internet_job_with_retry(&pool, &job, "internet_endpoint_sends_custom_headers")
        else {
            return;
        };
        let value = result.expect("internet endpoint call should succeed");

        assert_eq!(value["json"]["hello"], json!("headers"));
        assert_eq!(value["headers"]["X-Mechanics-Test"], json!("header-check"));
    }
}
