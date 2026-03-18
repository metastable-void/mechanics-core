use super::*;

fn test_shared(worker_count: usize, queue_capacity: usize) -> Arc<MechanicsPoolShared> {
    let (tx, rx) = bounded(queue_capacity);
    let (exit_tx, exit_rx) = bounded(8);
    let config = MechanicsPoolConfig::new()
        .with_worker_count(worker_count)
        .with_queue_capacity(queue_capacity)
        .with_restart_window(Duration::from_secs(1))
        .with_max_restarts_in_window(4)
        .with_execution_limits(MechanicsExecutionLimits::default());
    Arc::new(MechanicsPoolShared::new(
        &config,
        Arc::new(ReqwestEndpointHttpClient::new(reqwest::Client::new())),
        tx,
        rx,
        exit_tx,
        exit_rx,
    ))
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

    let err = match MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        queue_capacity: 1,
        run_timeout: Duration::MAX,
        ..Default::default()
    }) {
        Err(err) => err,
        Ok(_) => panic!("oversized run_timeout must fail"),
    };
    assert!(matches!(err, MechanicsError::RuntimePool(_)));
    assert!(err.msg().contains("too large"));

    let err = match MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        queue_capacity: 1,
        default_http_timeout_ms: Some(0),
        ..Default::default()
    }) {
        Err(err) => err,
        Ok(_) => panic!("default_http_timeout_ms=0 must fail"),
    };
    assert!(matches!(err, MechanicsError::RuntimePool(_)));
    assert!(err.msg().contains("default_http_timeout_ms"));

    let err = match MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        queue_capacity: 1,
        default_http_response_max_bytes: Some(0),
        ..Default::default()
    }) {
        Err(err) => err,
        Ok(_) => panic!("default_http_response_max_bytes=0 must fail"),
    };
    assert!(matches!(err, MechanicsError::RuntimePool(_)));
    assert!(err.msg().contains("default_http_response_max_bytes"));
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
fn run_and_run_nonblocking_enqueue_fail_when_pool_closed() {
    let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
    pool.shared.mark_closed();

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job.clone())
        .expect_err("closed pool must reject run");
    assert!(matches!(err, MechanicsError::PoolClosed(_)));

    let err = pool
        .run_nonblocking_enqueue(job)
        .expect_err("closed pool must reject run_nonblocking_enqueue");
    assert!(matches!(err, MechanicsError::PoolClosed(_)));
}

#[test]
fn run_and_run_nonblocking_enqueue_fail_when_workers_unavailable_and_restart_blocked() {
    let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
    pool.shared.set_restart_blocked(true);

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job.clone())
        .expect_err("must fail when no workers and restart blocked");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));

    let err = pool
        .run_nonblocking_enqueue(job)
        .expect_err("must fail when no workers and restart blocked");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));
}

#[test]
fn drop_cancels_queued_jobs() {
    let pool = synthetic_pool(8, MechanicsExecutionLimits::default());
    let (reply_tx, reply_rx) = bounded(1);

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    pool.shared
        .job_sender()
        .send(PoolMessage::Run(PoolJob::new(
            job,
            reply_tx,
            Arc::new(AtomicBool::new(false)),
        )))
        .expect("enqueue queued job");

    drop(pool);
    let canceled = reply_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("drop should send canceled error");
    assert!(matches!(canceled, Err(MechanicsError::Canceled(_))));
}

#[test]
fn drop_does_not_block_when_workers_map_contains_finished_threads() {
    let shared = test_shared(2, 1);

    {
        let mut workers = shared.workers_write();
        workers.insert(0, WorkerHandle::from_join_for_test(thread::spawn(|| {})));
        workers.insert(1, WorkerHandle::from_join_for_test(thread::spawn(|| {})));
    }
    loop {
        let all_finished = {
            let workers = shared.workers_read();
            workers.values().all(WorkerHandle::is_finished)
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
        supervisor_shutdown_tx: None,
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
fn stats_is_non_blocking_with_finished_worker_handles() {
    let shared = test_shared(2, 1);
    shared.set_restart_blocked(true);

    {
        let mut workers = shared.workers_write();
        workers.insert(0, WorkerHandle::from_join_for_test(thread::spawn(|| {})));
        workers.insert(
            1,
            WorkerHandle::from_join_for_test(thread::spawn(|| {
                thread::sleep(Duration::from_millis(50))
            })),
        );
    }
    loop {
        let finished = {
            let workers = shared.workers_read();
            workers.get(&0).map(WorkerHandle::is_finished)
        };
        if finished == Some(true) {
            break;
        }
        thread::yield_now();
    }

    assert!(shared.record_restart_attempt(Instant::now()));

    let pool = MechanicsPool {
        shared: Arc::clone(&shared),
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(50),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let (done_tx, done_rx) = bounded(1);
    thread::spawn(move || {
        let stats = pool.stats();
        let _ = done_tx.send(stats);
    });
    let stats = done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("stats should be non-blocking");

    assert_eq!(stats.desired_workers, 2);
    assert_eq!(stats.known_workers, 2);
    assert_eq!(stats.finished_workers_pending_reap, 1);
    assert_eq!(stats.live_workers, 1);
    assert!(stats.restart_blocked);
    assert_eq!(stats.restart_attempts_in_window, 1);
    assert_eq!(stats.max_restarts_in_window, 4);
    assert_eq!(stats.queue_capacity, Some(1));
    assert_eq!(stats.queue_depth, 0);
}

#[test]
fn drop_does_not_block_when_queue_is_full_and_worker_is_not_receiving() {
    let shared = test_shared(1, 1);

    let blocker = thread::spawn(|| thread::sleep(Duration::from_millis(200)));
    {
        let mut workers = shared.workers_write();
        workers.insert(0, WorkerHandle::from_join_for_test(blocker));
    }

    let (reply_tx, _reply_rx) = bounded(1);
    let queued_job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    shared
        .job_sender()
        .send(PoolMessage::Run(PoolJob::new(
            queued_job,
            reply_tx,
            Arc::new(AtomicBool::new(false)),
        )))
        .expect("fill queue");

    let pool = MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(50),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let (done_tx, done_rx) = bounded::<()>(1);
    thread::spawn(move || {
        drop(pool);
        let _ = done_tx.send(());
    });

    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("drop should not block on a full queue");
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
fn reconcile_workers_recovers_after_restart_window_without_new_exit_events() {
    let (tx, rx) = bounded(1);
    let (exit_tx, exit_rx) = bounded(8);
    let config = MechanicsPoolConfig::new()
        .with_worker_count(1)
        .with_queue_capacity(1)
        .with_restart_window(Duration::from_millis(20))
        .with_max_restarts_in_window(1)
        .with_execution_limits(MechanicsExecutionLimits::default());
    let shared = Arc::new(MechanicsPoolShared::new(
        &config,
        Arc::new(ReqwestEndpointHttpClient::new(reqwest::Client::new())),
        tx,
        rx,
        exit_tx,
        exit_rx,
    ));
    shared.set_restart_blocked(true);

    assert!(shared.record_restart_attempt(Instant::now()));

    MechanicsPoolShared::reconcile_workers(&shared);
    assert_eq!(shared.live_workers(), 0);
    assert!(shared.is_restart_blocked());

    thread::sleep(Duration::from_millis(30));
    MechanicsPoolShared::reconcile_workers(&shared);
    assert_eq!(shared.live_workers(), 1);
    assert!(!shared.is_restart_blocked());

    shared.mark_closed();
    {
        let workers = shared.workers_read();
        for handle in workers.values() {
            handle.request_shutdown();
        }
    }
    let mut workers = shared.workers_write();
    for (_, handle) in workers.drain() {
        handle.join();
    }
}
