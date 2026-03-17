use super::*;

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
fn pool_new_fails_when_worker_runtime_init_fails() {
    let result = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        force_worker_runtime_init_failure: true,
        ..Default::default()
    });

    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("worker runtime init failure must fail pool creation"),
    };
    assert!(matches!(err, MechanicsError::RuntimePool(_)));
}

#[test]
fn pool_new_fails_promptly_when_many_workers_fail_startup() {
    let result = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 64,
        force_worker_runtime_init_failure: true,
        ..Default::default()
    });
    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("startup failures must fail pool creation"),
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
fn drop_does_not_block_when_workers_map_contains_finished_threads() {
    let (tx, rx) = bounded(1);
    let (exit_tx, exit_rx) = bounded(8);
    let shared = Arc::new(MechanicsPoolShared {
        tx,
        rx,
        exit_tx,
        exit_rx,
        workers: RwLock::new(HashMap::new()),
        next_worker_id: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
        restart_blocked: AtomicBool::new(false),
        restart_guard: Mutex::new(RestartGuard::new(Duration::from_secs(1), 1)),
        execution_limits: MechanicsExecutionLimits::default(),
        default_http_timeout_ms: None,
        reqwest_client: reqwest::Client::new(),
        #[cfg(test)]
        force_worker_runtime_init_failure: false,
    });

    {
        let mut workers = shared.workers.write();
        workers.insert(0, thread::spawn(|| {}));
        workers.insert(1, thread::spawn(|| {}));
    }
    loop {
        let all_finished = {
            let workers = shared.workers.read();
            workers.values().all(thread::JoinHandle::is_finished)
        };
        if all_finished {
            break;
        }
        thread::yield_now();
    }

    let pool = MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(50),
        supervisor: None,
    };

    let (done_tx, done_rx) = bounded::<()>(1);
    thread::spawn(move || {
        drop(pool);
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("drop should not block with stale finished worker handles");
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
