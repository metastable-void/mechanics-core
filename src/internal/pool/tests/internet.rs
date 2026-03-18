use super::*;

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_roundtrip_httpbin() {
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://httpbin.org/post", HashMap::new());
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
                return await endpoint("internet", { body: arg });
            }
        "#;
    let job = make_job(source, config, json!({"hello":"internet"}));
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_roundtrip_httpbin")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");

    assert_eq!(value["body"]["json"]["hello"], json!("internet"));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_http_timeout_from_pool_default() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://httpbin.org/delay/3",
        HashMap::new(),
    );
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
                return await endpoint("internet", { body: arg });
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
                msg.contains("timed out") || msg.contains("timeout") || msg.contains("deadline")
            );
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_timeout_overrides_pool_default() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://httpbin.org/delay/1",
        HashMap::new(),
    )
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
                return await endpoint("internet", { body: arg });
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
    let body = &value["body"];
    let echoed_json = body
        .get("json")
        .and_then(|v| v.get("hello"))
        .and_then(Value::as_str);
    let echoed_data = body.get("data").and_then(Value::as_str);
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
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://httpbin.org/anything", headers)
        .with_exposed_response_headers(vec!["content-type".to_owned()]);
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
                return await endpoint("internet", { body: arg });
            }
        "#;
    let job = make_job(source, config, json!({"hello":"headers"}));
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_sends_custom_headers")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");

    assert_eq!(value["body"]["json"]["hello"], json!("headers"));
    assert_eq!(
        value["body"]["headers"]["X-Mechanics-Test"],
        json!("header-check")
    );
    assert!(
        value["headers"]["content-type"]
            .as_str()
            .unwrap_or_default()
            .contains("application/json")
    );
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_allows_allowlisted_request_header_override() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://httpbin.org/anything",
        HashMap::new(),
    )
    .with_overridable_request_headers(vec!["x-mechanics-override".to_owned()]);
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
            export default async function main(_arg) {
                return await endpoint("internet", {
                    headers: { "X-Mechanics-Override": "yes" }
                });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) = run_internet_job_with_retry(
        &pool,
        &job,
        "internet_endpoint_allows_allowlisted_request_header_override",
    ) else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    assert_eq!(
        value["body"]["headers"]["X-Mechanics-Override"],
        json!("yes")
    );
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_get_uses_url_and_query_slots() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://httpbin.org/anything/{resource}",
        HashMap::new(),
    )
    .with_url_param_specs(HashMap::from([(
        "resource".to_owned(),
        UrlParamSpec {
            default: None,
            min_bytes: Some(1),
            max_bytes: Some(64),
        },
    )]))
    .with_query_specs(vec![
        QuerySpec::Const {
            key: "v".to_owned(),
            value: "1".to_owned(),
        },
        QuerySpec::Slotted {
            key: "q".to_owned(),
            slot: "q".to_owned(),
            mode: SlottedQueryMode::Required,
            default: None,
            min_bytes: Some(1),
            max_bytes: Some(64),
        },
    ]);
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
            export default async function main(_arg) {
                return await endpoint("internet", {
                    urlParams: { resource: "abc_123" },
                    queries: { q: "slot-ok" }
                });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) = run_internet_job_with_retry(
        &pool,
        &job,
        "internet_endpoint_get_uses_url_and_query_slots",
    ) else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    let body = &value["body"];
    assert_eq!(body["method"], json!("GET"));
    assert_eq!(body["args"]["v"], json!("1"));
    assert_eq!(body["args"]["q"], json!("slot-ok"));
    let url = body["url"].as_str().unwrap_or_default();
    assert!(url.contains("/anything/abc_123"));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_put_utf8_request_roundtrip() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Put,
        "https://httpbin.org/anything",
        HashMap::new(),
    )
    .with_request_body_type(EndpointBodyType::Utf8);
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
            export default async function main(_arg) {
                return await endpoint("internet", { body: "hello-put" });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_put_utf8_request_roundtrip")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    assert_eq!(value["body"]["method"], json!("PUT"));
    let echoed = value["body"]["data"].as_str().unwrap_or_default();
    assert!(echoed.contains("hello-put"));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_delete_roundtrip() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Delete,
        "https://httpbin.org/anything",
        HashMap::new(),
    )
    .with_query_specs(vec![QuerySpec::Slotted {
        key: "tag".to_owned(),
        slot: "tag".to_owned(),
        mode: SlottedQueryMode::Required,
        default: None,
        min_bytes: Some(1),
        max_bytes: Some(16),
    }]);
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
            export default async function main(_arg) {
                return await endpoint("internet", { queries: { tag: "gone" } });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_delete_roundtrip")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    if value["body"].is_null() {
        eprintln!(
            "skipping internet_endpoint_delete_roundtrip: upstream returned empty response body"
        );
        return;
    }
    assert_eq!(value["body"]["method"], json!("DELETE"));
    assert_eq!(value["body"]["args"]["tag"], json!("gone"));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_utf8_response_body_mode() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://httpbin.org/base64/SFRUUEJJTiBpcyBhd2Vzb21l",
        HashMap::new(),
    )
    .with_response_body_type(EndpointBodyType::Utf8);
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
            export default async function main(_arg) {
                return (await endpoint("internet")).body;
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_utf8_response_body_mode")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    let text = value.as_str().unwrap_or_default();
    assert!(text.contains("HTTPBIN"));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_bytes_response_body_mode() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://httpbin.org/base64/SFRUUEJJTiBpcyBhd2Vzb21l",
        HashMap::new(),
    )
    .with_response_body_type(EndpointBodyType::Bytes);
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
            export default async function main(_arg) {
                const bytes = (await endpoint("internet")).body;
                return Array.from(bytes);
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_bytes_response_body_mode")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    let bytes = value
        .as_array()
        .expect("bytes should convert to JSON array");
    assert!(bytes.len() >= 7);
    assert_eq!(bytes[0], json!(72));
    assert_eq!(bytes[1], json!(84));
    assert_eq!(bytes[2], json!(84));
    assert_eq!(bytes[3], json!(80));
    assert_eq!(bytes[4], json!(66));
    assert_eq!(bytes[5], json!(73));
    assert_eq!(bytes[6], json!(78));
}

#[test]
#[ignore = "requires internet access to https://httpbin.org"]
fn internet_endpoint_empty_response_is_null() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://httpbin.org/status/204",
        HashMap::new(),
    )
    .with_response_body_type(EndpointBodyType::Json);
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
            export default async function main(_arg) {
                return (await endpoint("internet")).body;
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let Some(result) =
        run_internet_job_with_retry(&pool, &job, "internet_endpoint_empty_response_is_null")
    else {
        return;
    };
    let value = result.expect("internet endpoint call should succeed");
    assert_eq!(value, Value::Null);
}
