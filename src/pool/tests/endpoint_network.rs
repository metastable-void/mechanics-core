use super::*;

#[test]
#[ignore = "requires local socket bind permission in the execution environment"]
fn execution_timeout_stops_slow_async_job() {
    let (url, server) = spawn_json_server(Duration::from_millis(350), r#"{"ok":true}"#);
    let endpoint =
        HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new()).with_timeout_ms(Some(2_000));
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
                return await endpoint("slow", { body: arg });
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
fn endpoint_uses_pool_default_timeout() {
    let (url, server) = spawn_json_server(Duration::from_millis(180), r#"{"ok":true}"#);
    let endpoint = HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new());
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
                return await endpoint("slow", { body: arg });
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
    let endpoint =
        HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new()).with_timeout_ms(Some(400));
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
                return await endpoint("slow", { body: arg });
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
fn endpoint_uses_pool_default_response_max_bytes() {
    let large_json = format!(r#"{{"blob":"{}"}}"#, "x".repeat(128));
    let (url, server) = spawn_json_server_owned(Duration::from_millis(0), large_json);
    let endpoint = HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new());
    let config = endpoint_config("slow", endpoint);

    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        default_http_response_max_bytes: Some(64),
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
                return await endpoint("slow", { body: arg });
            }
        "#;
    let job = make_job(source, config, json!({"hello":"world"}));
    let err = pool
        .run(job)
        .expect_err("request should fail because response body is too large");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("exceeds configured max bytes"));
        }
        other => panic!("unexpected error kind: {other}"),
    }

    let _ = server.join();
}

#[test]
#[ignore = "requires local socket bind permission in the execution environment"]
fn endpoint_response_max_bytes_overrides_pool_default() {
    let large_json = format!(r#"{{"blob":"{}"}}"#, "x".repeat(128));
    let (url, server) = spawn_json_server_owned(Duration::from_millis(0), large_json);
    let endpoint = HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new())
        .with_response_max_bytes(Some(512));
    let config = endpoint_config("slow", endpoint);

    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        default_http_response_max_bytes: Some(64),
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
                return await endpoint("slow", { body: arg });
            }
        "#;
    let job = make_job(source, config, json!({"hello":"world"}));
    let value = pool
        .run(job)
        .expect("endpoint-level response max bytes should allow success");
    assert_eq!(value["blob"].as_str().map(str::len), Some(128));

    let _ = server.join();
}

#[test]
#[ignore = "requires local socket bind permission in the execution environment"]
fn endpoint_non_success_status_is_error_by_default() {
    let (url, server) =
        spawn_json_server_with_status(Duration::from_millis(0), 500, r#"{"ok":false}"#);
    let endpoint = HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new());
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
                return await endpoint("failing", { body: arg });
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
    let endpoint = HttpEndpoint::new(HttpMethod::Post, &url, HashMap::new())
        .with_allow_non_success_status(true);
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
                return await endpoint("failing", { body: arg });
            }
        "#;
    let job = make_job(source, config, json!({"hello":"status"}));
    let value = pool
        .run(job)
        .expect("opt-in should allow JSON parse on non-success status");
    assert_eq!(value["ok"], json!(false));

    let _ = server.join();
}
