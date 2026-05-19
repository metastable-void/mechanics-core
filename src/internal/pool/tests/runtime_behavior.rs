use super::*;
use crate::endpoint::http_client::{
    EndpointHttpClient, EndpointHttpHeaders, EndpointHttpRequest, EndpointHttpRequestBody,
    EndpointHttpResponse, EndpointTransportResult,
};
use crate::internal::runtime::RuntimeInternal;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

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
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        json!({"n": 7}),
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["got"]["n"], json!(7));
}

#[test]
fn global_mutations_do_not_persist_across_jobs() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let set_global = make_job(
        r#"
            export default function main(_arg) {
                globalThis.__mechanics_cross_job_leak_test__ = "leak";
                return null;
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    pool.run(set_global).expect("run first module");

    let read_global = make_job(
        r#"
            export default function main(_arg) {
                return Object.prototype.hasOwnProperty.call(
                    globalThis,
                    "__mechanics_cross_job_leak_test__"
                );
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(read_global).expect("run second module");
    assert_eq!(value, Value::Bool(false));
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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
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
fn unhandled_async_error_does_not_override_fulfilled_main() {
    // mechanics-core 0.4.0 changed semantics: when `main` returns a fulfilled
    // value, we trust the script's own outcome rather than overriding it with
    // an "Unhandled promise rejection" engine error. This prevents a Boa
    // tracker false-positive (`NativeFunction::from_async_fn` rejection
    // tracking does not reliably balance with await-chain handler attachment)
    // from breaking workflows whose `await ... catch` correctly handled the
    // failure.
    //
    // The trade-off: a script that legitimately leaves a promise rejection
    // unhandled (canonical example below) no longer fails the step. It still
    // produces a (silently) misbehaving result, but a misbehaving result is
    // strictly preferable to a hard step failure for the much more common
    // case of a correctly-caught endpoint error tripping the same tracker.
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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let result = pool
        .run(job)
        .expect("fulfilled main succeeds despite an unhandled inner rejection");
    assert_eq!(result, Value::from(1));
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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
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

#[derive(Debug)]
struct ImmediateEndpointHttpClient {
    call_count: Arc<AtomicUsize>,
}

impl EndpointHttpClient for ImmediateEndpointHttpClient {
    fn execute(
        &self,
        _request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        let call_count = Arc::clone(&self.call_count);
        Box::pin(async move {
            call_count.fetch_add(1, Ordering::Relaxed);
            Ok(EndpointHttpResponse {
                status: 200,
                headers: EndpointHttpHeaders::new(),
                content_length: Some(11),
                body: br#"{"ok":true}"#.to_vec(),
            })
        })
    }
}

#[derive(Debug)]
struct BlockingEndpointHttpClient {
    call_count: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl BlockingEndpointHttpClient {
    fn release(&self) {
        let (lock, condvar) = &*self.gate;
        let mut released = lock.lock().expect("lock endpoint gate");
        *released = true;
        condvar.notify_all();
    }
}

impl EndpointHttpClient for BlockingEndpointHttpClient {
    fn execute(
        &self,
        _request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        let call_count = Arc::clone(&self.call_count);
        let gate = Arc::clone(&self.gate);
        Box::pin(async move {
            call_count.fetch_add(1, Ordering::Relaxed);
            let (lock, condvar) = &*gate;
            let mut released = lock.lock().expect("lock endpoint gate");
            while !*released {
                released = condvar.wait(released).expect("wait endpoint gate");
            }
            Ok(EndpointHttpResponse {
                status: 200,
                headers: EndpointHttpHeaders::new(),
                content_length: Some(11),
                body: br#"{"ok":true}"#.to_vec(),
            })
        })
    }
}

fn mock_endpoint_config() -> MechanicsConfig {
    endpoint_config(
        "mock",
        HttpEndpoint::new(HttpMethod::Get, "https://mock.local/ping", HashMap::new()),
    )
}

#[derive(Debug)]
struct HangingEndpointHttpClient;

impl EndpointHttpClient for HangingEndpointHttpClient {
    fn execute(
        &self,
        _request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        Box::pin(std::future::pending())
    }
}

fn slow_endpoint_config() -> MechanicsConfig {
    endpoint_config(
        "slow",
        HttpEndpoint::new(HttpMethod::Get, "https://slow.local/ping", HashMap::new()),
    )
}

#[derive(Debug)]
struct TimedOutEndpointHttpClient;

impl EndpointHttpClient for TimedOutEndpointHttpClient {
    fn execute(
        &self,
        _request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        Box::pin(async move { Err(crate::endpoint::http_client::EndpointTransportError::Timeout) })
    }
}

fn wait_for_call_count(counter: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if counter.load(Ordering::Relaxed) >= expected {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(counter.load(Ordering::Relaxed), expected);
}

fn assert_no_tail_abort_for(job_id: &str) {
    assert!(
        !RuntimeInternal::tail_poll_abort_records_for_test()
            .iter()
            .any(|record| record.job_id == job_id),
        "unexpected tail-poll abort record for {job_id}"
    );
}

#[test]
fn d17_sync_return_no_pending_work_preserves_value() {
    let job_id = "d17-sync-return";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(ImmediateEndpointHttpClient {
            call_count: Arc::new(AtomicUsize::new(0)),
        }))
        .expect("create runtime");
    let job = make_job(
        r#"
            export default function main(_arg) {
                return { ok: 1 };
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );

    let started = Instant::now();
    let value = runtime.run_source(job).expect("run sync return");

    assert_eq!(value, json!({"ok": 1}));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "sync job should return promptly"
    );
    assert_no_tail_abort_for(job_id);
}

#[test]
fn d17_async_return_all_awaited_preserves_value() {
    let job_id = "d17-awaited-endpoint";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(ImmediateEndpointHttpClient {
            call_count: Arc::clone(&calls),
        }))
        .expect("create runtime");
    let job = make_job(
        r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                const res = await endpoint("mock", {});
                return res.body;
            }
        "#,
        mock_endpoint_config(),
        Value::Null,
    );

    let value = runtime.run_source(job).expect("run awaited endpoint");

    assert_eq!(value, json!({"ok": true}));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_no_tail_abort_for(job_id);
}

#[test]
fn d17_fire_and_forget_endpoint_replies_before_tail_completes() {
    let job_id = "d17-fire-and-forget-endpoint";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(BlockingEndpointHttpClient {
        call_count: Arc::clone(&calls),
        gate: Arc::new((Mutex::new(false), Condvar::new())),
    });
    let release_client = Arc::clone(&client);
    let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
    let (tail_done_tx, tail_done_rx) = crossbeam_channel::bounded(1);

    let handle = thread::spawn(move || {
        let mut runtime =
            RuntimeInternal::new_with_endpoint_http_client(client).expect("create runtime");
        let job = make_job(
            r#"
                import endpoint from "mechanics:endpoint";
                export default function main(_arg) {
                    endpoint("mock", {});
                    return { ok: 2 };
                }
            "#,
            mock_endpoint_config(),
            Value::Null,
        );
        runtime
            .run_source_with_early_reply(job, job_id, |result| {
                reply_tx.send(result).expect("send early reply");
            })
            .expect("tail poll should finish after release");
        tail_done_tx.send(()).expect("send tail completion");
    });

    let response = reply_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("early reply should arrive before endpoint future is released")
        .expect("main response should succeed");
    assert_eq!(response, json!({"ok": 2}));
    wait_for_call_count(&calls, 1);
    assert!(
        tail_done_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "tail poll must keep the in-flight endpoint future alive until it completes"
    );
    release_client.release();
    tail_done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("tail poll should finish after endpoint release");
    handle.join().expect("runtime thread should join");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_no_tail_abort_for(job_id);
}

#[test]
fn d17_unawaited_promise_runs_during_tail_poll() {
    let job_id = "d17-unawaited-promise-tail";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(ImmediateEndpointHttpClient {
            call_count: Arc::clone(&calls),
        }))
        .expect("create runtime");
    let job = make_job(
        r#"
            import endpoint from "mechanics:endpoint";
            export default function main(_arg) {
                Promise.resolve().then(() => { endpoint("mock", {}); });
                return { ok: 4 };
            }
        "#,
        mock_endpoint_config(),
        Value::Null,
    );

    let value = runtime.run_source(job).expect("run promise tail job");

    assert_eq!(value, json!({"ok": 4}));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_no_tail_abort_for(job_id);
}

#[test]
fn d17_deadline_mid_tail_poll_replies_then_warns_once() {
    let job_id = "d17-deadline-tail";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
    let handle = thread::spawn(move || {
        let mut runtime =
            RuntimeInternal::new_with_endpoint_http_client(Arc::new(HangingEndpointHttpClient))
                .expect("create runtime");
        runtime.set_execution_limits(MechanicsExecutionLimits {
            max_execution_time: Duration::from_millis(200),
            ..Default::default()
        });
        // The endpoint promise from `endpoint("slow", {})` never
        // resolves (HangingEndpointHttpClient returns
        // `std::future::pending()`), so the runtime's in-flight
        // async-job counter stays positive past main's reply and
        // tail-poll runs until the per-job deadline aborts it.
        let job = make_job(
            r#"
                import endpoint from "mechanics:endpoint";
                export default function main(_arg) {
                    endpoint("slow", {}).then(() => { globalThis.__late = true; });
                    return { ok: 5 };
                }
            "#,
            slow_endpoint_config(),
            Value::Null,
        );
        let started = Instant::now();
        runtime
            .run_source_with_early_reply(job, job_id, |result| {
                reply_tx
                    .send((started.elapsed(), result))
                    .expect("send early reply");
            })
            .expect("tail deadline abort is handled after early reply");
        started.elapsed()
    });

    let (reply_elapsed, response) = reply_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("main response should arrive before tail deadline");
    assert!(
        reply_elapsed < Duration::from_millis(100),
        "main response should be prompt"
    );
    assert_eq!(
        response.expect("main response should succeed"),
        json!({"ok": 5})
    );
    let total_elapsed = handle.join().expect("runtime thread should join");
    assert!(total_elapsed >= Duration::from_millis(180));
    let records = RuntimeInternal::tail_poll_abort_records_for_test();
    let record = records
        .iter()
        .find(|record| record.job_id == job_id)
        .expect("tail-poll abort should be recorded");
    assert!(
        record.snapshot.in_flight_async_jobs >= 1,
        "expected pending endpoint future, got snapshot {:?}",
        record.snapshot
    );
    assert_eq!(record.snapshot.timeout_jobs, 0);
}

#[test]
fn d17_unhandled_rejection_during_tail_poll_does_not_fail_response() {
    let job_id = "d17-tail-rejection";
    RuntimeInternal::clear_tail_poll_abort_records_for_test();
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(ImmediateEndpointHttpClient {
            call_count: Arc::new(AtomicUsize::new(0)),
        }))
        .expect("create runtime");
    let job = make_job(
        r#"
            export default function main(_arg) {
                Promise.reject(new Error("boom"));
                return { ok: 3 };
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );

    let mut response = None;
    runtime
        .run_source_with_early_reply(job, job_id, |result| {
            response = Some(result);
        })
        .expect("tail rejection should not fail run_source after main reply");

    assert_eq!(
        response
            .expect("main response should be captured")
            .expect("main response should succeed"),
        json!({"ok": 3})
    );
    assert!(
        RuntimeInternal::recorded_unhandled_rejection_count_for_test(job_id)
            .expect("unhandled rejection count should be recorded")
            > 0
    );
    assert_no_tail_abort_for(job_id);
}

#[test]
fn d17_main_never_settles_preserves_default_export_timeout_error() {
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(ImmediateEndpointHttpClient {
            call_count: Arc::new(AtomicUsize::new(0)),
        }))
        .expect("create runtime");
    runtime.set_execution_limits(MechanicsExecutionLimits {
        max_execution_time: Duration::from_millis(100),
        ..Default::default()
    });
    let job = make_job(
        r#"
            export default function main(_arg) {
                return new Promise(() => {});
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );

    let err = runtime
        .run_source(job)
        .expect_err("pending main promise should fail");

    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("Default export promise did not settle"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_transport_errors_include_endpoint_name() {
    let mut runtime =
        RuntimeInternal::new_with_endpoint_http_client(Arc::new(TimedOutEndpointHttpClient))
            .expect("create runtime");
    let job = make_job(
        r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                try {
                    await endpoint("llm", {});
                } catch (e) {
                    return String(e);
                }
            }
        "#,
        endpoint_config(
            "llm",
            HttpEndpoint::new(HttpMethod::Post, "https://mock.local/llm", HashMap::new()),
        ),
        Value::Null,
    );

    let value = runtime.run_source(job).expect("run endpoint error job");
    assert_eq!(
        value,
        json!("Error: endpoint `llm` request failed: request timed out")
    );
}

#[derive(Debug)]
struct MockEndpointHttpClient {
    call_count: Arc<AtomicUsize>,
}

impl EndpointHttpClient for MockEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            use crate::endpoint::http_client::EndpointTransportError;
            if request.method.as_str() != "GET" {
                return Err(EndpointTransportError::InvalidRequest(
                    "expected GET method in mock client".to_owned(),
                ));
            }
            if request.url != "https://mock.local/ping" {
                return Err(EndpointTransportError::InvalidRequest(
                    "unexpected URL in mock client".to_owned(),
                ));
            }
            if !matches!(request.body, EndpointHttpRequestBody::Absent) {
                return Err(EndpointTransportError::InvalidRequest(
                    "mock client expected no request body".to_owned(),
                ));
            }
            let mut headers = EndpointHttpHeaders::new();
            headers.insert("x-trace-id", "trace-123");
            Ok(EndpointHttpResponse {
                status: 200,
                headers,
                content_length: Some(30),
                body: br#"{"ok":true,"source":"mock"}"#.to_vec(),
            })
        })
    }
}

#[test]
fn pool_uses_injected_endpoint_http_client() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        endpoint_http_client: Some(Arc::new(MockEndpointHttpClient {
            call_count: Arc::clone(&calls),
        })),
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://mock.local/ping", HashMap::new())
        .with_exposed_response_headers(vec!["x-trace-id".to_owned()]);
    let job = make_job(
        r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("mock", {});
            }
        "#,
        endpoint_config("mock", endpoint),
        Value::Null,
    );

    let value = pool.run(job).expect("run endpoint with injected client");
    assert_eq!(value["status"], json!(200));
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["body"]["ok"], json!(true));
    assert_eq!(value["body"]["source"], json!("mock"));
    assert_eq!(value["headers"]["x-trace-id"], json!("trace-123"));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[derive(Debug)]
struct RecordingEndpointHttpClient {
    seen_urls: Arc<Mutex<Vec<String>>>,
}

impl EndpointHttpClient for RecordingEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        let seen_urls = Arc::clone(&self.seen_urls);
        Box::pin(async move {
            seen_urls
                .lock()
                .expect("lock seen urls")
                .push(request.url.clone());
            let body = serde_json::to_vec(&json!({
                "url": request.url,
                "max": request.response_max_bytes
            }))
            .expect("serialize mock body");
            Ok(EndpointHttpResponse {
                status: 200,
                headers: EndpointHttpHeaders::new(),
                content_length: Some(u64::try_from(body.len()).expect("body length fits u64")),
                body,
            })
        })
    }
}

#[test]
fn prepared_endpoint_cache_is_isolated_per_job_config() {
    let seen_urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        endpoint_http_client: Some(Arc::new(RecordingEndpointHttpClient {
            seen_urls: Arc::clone(&seen_urls),
        })),
        ..Default::default()
    })
    .expect("create pool");

    let endpoint_v1 = HttpEndpoint::new(HttpMethod::Get, "https://mock.local/one", HashMap::new());
    let endpoint_v2 = HttpEndpoint::new(HttpMethod::Get, "https://mock.local/two", HashMap::new());
    let js = r#"
        import endpoint from "mechanics:endpoint";
        export default async function main(_arg) {
            const res = await endpoint("ep", {});
            return res.body.url;
        }
    "#;

    let first = pool
        .run(make_job(
            js,
            endpoint_config("ep", endpoint_v1),
            Value::Null,
        ))
        .expect("run first job");
    let second = pool
        .run(make_job(
            js,
            endpoint_config("ep", endpoint_v2),
            Value::Null,
        ))
        .expect("run second job");

    assert_eq!(first, json!("https://mock.local/one"));
    assert_eq!(second, json!("https://mock.local/two"));
    assert_eq!(
        *seen_urls.lock().expect("lock seen urls"),
        vec![
            "https://mock.local/one".to_owned(),
            "https://mock.local/two".to_owned()
        ]
    );
}

#[test]
fn endpoint_request_uses_effective_response_max_bytes_precedence() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        default_http_response_max_bytes: Some(111),
        endpoint_http_client: Some(Arc::new(RecordingEndpointHttpClient {
            seen_urls: Arc::new(Mutex::new(Vec::new())),
        })),
        ..Default::default()
    })
    .expect("create pool");

    let js = r#"
        import endpoint from "mechanics:endpoint";
        export default async function main(_arg) {
            const res = await endpoint("ep", {});
            return res.body.max;
        }
    "#;

    let default_max = pool
        .run(make_job(
            js,
            endpoint_config(
                "ep",
                HttpEndpoint::new(
                    HttpMethod::Get,
                    "https://mock.local/default",
                    HashMap::new(),
                ),
            ),
            Value::Null,
        ))
        .expect("run job with pool default");
    assert_eq!(default_max, json!(111));

    let endpoint_override = pool
        .run(make_job(
            js,
            endpoint_config(
                "ep",
                HttpEndpoint::new(
                    HttpMethod::Get,
                    "https://mock.local/override",
                    HashMap::new(),
                )
                .with_response_max_bytes(Some(222)),
            ),
            Value::Null,
        ))
        .expect("run job with endpoint override");
    assert_eq!(endpoint_override, json!(222));
}

#[test]
fn timed_out_job_does_not_leak_pending_timeout_tasks_into_next_job() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        execution_limits: MechanicsExecutionLimits {
            max_execution_time: Duration::from_millis(250),
            max_loop_iterations: 5_000_000,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("create pool");

    let timeout_job = make_job(
        r#"
            export default function main(_arg) {
                Promise.resolve().then(() => {
                    throw new Error("late microtask should not run in next job");
                });
                while (true) {}
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(timeout_job)
        .expect_err("job must terminate from execution limits");
    assert!(matches!(err, MechanicsError::Execution(_)));

    let clean_job = make_job(
        r#"
            export default function main(_arg) {
                return 7;
            }
        "#,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool
        .run(clean_job)
        .expect("next job should not execute leaked timer tasks");
    assert_eq!(value, json!(7));
}

#[test]
fn pool_run_inside_tokio_spawn_blocking_succeeds() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime");

    let task_result = runtime.block_on(async {
        tokio::task::spawn_blocking(|| {
            let pool = MechanicsPool::new(MechanicsPoolConfig {
                worker_count: 1,
                ..Default::default()
            })
            .expect("create pool");
            let job = make_job(
                r#"
                    export default function main(arg) {
                        return { ok: true, got: arg };
                    }
                "#,
                MechanicsConfig::new(HashMap::new()).expect("create config"),
                json!({"via":"spawn_blocking"}),
            );
            pool.run(job)
        })
        .await
    });

    let value = task_result
        .expect("spawn_blocking task should join successfully")
        .expect("run should succeed from spawn_blocking");
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["got"]["via"], json!("spawn_blocking"));
}
