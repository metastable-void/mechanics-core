use super::*;

#[test]
fn run_maps_reply_timeout_to_run_timeout() {
    let limits = MechanicsExecutionLimits {
        max_execution_time: Duration::from_millis(5),
        ..Default::default()
    };
    let pool = synthetic_pool(8, limits);

    {
        let mut workers = pool.shared.workers.write();
        workers.insert(0, thread::spawn(|| {}));
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
    let shared = Arc::new(MechanicsPoolShared {
        tx,
        rx,
        exit_tx,
        exit_rx,
        workers: RwLock::new(HashMap::new()),
        next_worker_id: AtomicUsize::new(0),
        desired_worker_count: 1,
        closed: AtomicBool::new(false),
        restart_blocked: AtomicBool::new(false),
        restart_guard: Mutex::new(RestartGuard::new(Duration::from_secs(1), 1)),
        execution_limits: MechanicsExecutionLimits::default(),
        default_http_timeout_ms: None,
        default_http_response_max_bytes: None,
        reqwest_client: reqwest::Client::new(),
        #[cfg(test)]
        force_worker_runtime_init_failure: false,
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
        MechanicsConfig::new(HashMap::new()).expect("create config"),
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
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("run_timeout should fire while waiting for enqueue");
    assert!(matches!(err, MechanicsError::RunTimeout(_)));
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
