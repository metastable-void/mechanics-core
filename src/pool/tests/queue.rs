use super::*;

fn test_shared_with_channels(
    tx: crossbeam_channel::Sender<PoolMessage>,
    rx: crossbeam_channel::Receiver<PoolMessage>,
    exit_tx: crossbeam_channel::Sender<WorkerExit>,
    exit_rx: crossbeam_channel::Receiver<WorkerExit>,
    worker_count: usize,
    queue_capacity: usize,
) -> Arc<MechanicsPoolShared> {
    let config = MechanicsPoolConfig::new()
        .with_worker_count(worker_count)
        .with_queue_capacity(queue_capacity)
        .with_restart_window(Duration::from_secs(1))
        .with_max_restarts_in_window(1)
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
fn run_maps_reply_timeout_to_run_timeout() {
    let limits = MechanicsExecutionLimits {
        max_execution_time: Duration::from_millis(5),
        ..Default::default()
    };
    let pool = synthetic_pool(8, limits);

    {
        let mut workers = pool.shared.workers_write();
        workers.insert(0, WorkerHandle::from_join_for_test(thread::spawn(|| {})));
    }

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
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
    let shared = test_shared_with_channels(tx, rx, exit_tx, exit_rx, 1, 1);

    let pool = MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_secs(1),
        run_timeout: Duration::from_millis(5),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let (reply_tx, _reply_rx) = bounded(1);
    let queued = make_job(
        r#"export default function main() { return 0; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    pool.shared
        .job_sender()
        .send(PoolMessage::Run(PoolJob::new(
            queued,
            reply_tx,
            Arc::new(AtomicBool::new(false)),
        )))
        .expect("fill queue");

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("run_timeout should fire while waiting for enqueue");
    assert!(matches!(err, MechanicsError::RunTimeout(_)));
}

#[test]
fn run_try_enqueue_reports_queue_full_without_network_dependencies() {
    let pool = synthetic_pool(1, MechanicsExecutionLimits::default());
    let (reply_tx, _reply_rx) = bounded(1);

    let queued = make_job(
        r#"export default function main() { return 0; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    pool.shared
        .job_sender()
        .send(PoolMessage::Run(PoolJob::new(
            queued,
            reply_tx,
            Arc::new(AtomicBool::new(false)),
        )))
        .expect("fill queue");

    let contender = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run_try_enqueue(contender)
        .expect_err("full queue must reject immediate enqueue");
    assert!(matches!(err, MechanicsError::QueueFull(_)));
}

#[test]
fn run_reports_enqueue_timeout_without_network_dependencies() {
    let (tx, rx) = bounded(1);
    let (exit_tx, exit_rx) = bounded(8);
    let shared = test_shared_with_channels(tx, rx, exit_tx, exit_rx, 1, 1);

    let pool = MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(200),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let (reply_tx, _reply_rx) = bounded(1);
    let queued = make_job(
        r#"export default function main() { return 0; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    pool.shared
        .job_sender()
        .send(PoolMessage::Run(PoolJob::new(
            queued,
            reply_tx,
            Arc::new(AtomicBool::new(false)),
        )))
        .expect("fill queue");

    let contender = make_job(
        r#"export default function main() { return 2; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(contender)
        .expect_err("run should report enqueue timeout when queue remains full");
    assert!(matches!(err, MechanicsError::QueueTimeout(_)));
}

#[test]
fn run_and_run_try_enqueue_report_worker_unavailable_when_job_queue_is_disconnected() {
    let (tx_disconnected, rx_disconnected) = bounded(1);
    drop(rx_disconnected);
    let (_tx_alive, rx_alive) = bounded(1);
    let (exit_tx, exit_rx) = bounded(8);
    let shared = test_shared_with_channels(tx_disconnected, rx_alive, exit_tx, exit_rx, 1, 1);
    let pool = MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(50),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job.clone())
        .expect_err("disconnected queue should surface worker unavailable");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));

    let err = pool
        .run_try_enqueue(job)
        .expect_err("disconnected queue should surface worker unavailable");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));
}

#[test]
fn run_and_run_try_enqueue_report_worker_unavailable_when_worker_drops_reply_channel() {
    let (tx, rx) = bounded(8);
    let (exit_tx, exit_rx) = bounded(8);
    let shared = test_shared_with_channels(tx, rx.clone(), exit_tx, exit_rx, 1, 8);
    let pool = MechanicsPool {
        shared: Arc::clone(&shared),
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(200),
        supervisor: None,
        supervisor_shutdown_tx: None,
    };

    let consumer = thread::spawn(move || {
        for _ in 0..2 {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(PoolMessage::Run(pool_job)) => {
                    drop(pool_job);
                }
                other => panic!("unexpected queue event: {other:?}"),
            }
        }
    });

    let job = make_job(
        r#"export default function main() { return 1; }"#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );

    let err = pool
        .run(job.clone())
        .expect_err("dropped reply channel should surface worker unavailable");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));

    let err = pool
        .run_try_enqueue(job)
        .expect_err("dropped reply channel should surface worker unavailable");
    assert!(matches!(err, MechanicsError::WorkerUnavailable(_)));

    consumer.join().expect("join consumer");
}

#[test]
#[ignore = "requires local socket bind permission in the execution environment"]
fn run_try_enqueue_reports_queue_full() {
    let (url, server) = spawn_json_server(Duration::from_millis(900), r#"{"ok":true}"#);
    let blocking_endpoint =
        HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new()).with_timeout_ms(Some(3_000));
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
                    return await endpoint("slow", { body: arg });
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
                MechanicsConfig::new(HashMap::new()).expect("create config"),
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
        HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new()).with_timeout_ms(Some(3_000));
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
                    return await endpoint("slow", { body: arg });
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
                MechanicsConfig::new(HashMap::new()).expect("create config"),
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
