use super::*;
use crate::endpoint::http_client::{
    EndpointHttpClient, EndpointHttpHeaders, EndpointHttpRequest, EndpointHttpRequestBody,
    EndpointHttpResponse,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
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
struct MockEndpointHttpClient {
    call_count: Arc<AtomicUsize>,
}

impl EndpointHttpClient for MockEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<EndpointHttpResponse>> + Send>> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            if request.method != HttpMethod::Get {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "expected GET method in mock client",
                ));
            }
            if request.url != "https://mock.local/ping" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unexpected URL in mock client",
                ));
            }
            if !matches!(request.body, EndpointHttpRequestBody::Absent) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "mock client expected no request body",
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
    ) -> Pin<Box<dyn Future<Output = std::io::Result<EndpointHttpResponse>> + Send>> {
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
            max_execution_time: Duration::from_millis(25),
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
