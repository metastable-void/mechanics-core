use super::*;

#[test]
fn invalid_endpoint_header_is_reported_as_execution_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let mut headers = HashMap::new();
    headers.insert("bad header".to_owned(), "value".to_owned());
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com/anything", headers);
    let config = endpoint_config("bad", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(arg) {
                return await endpoint("bad", { body: arg });
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
fn endpoint_requires_object_options() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com/anything",
        HashMap::new(),
    );
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", 1);
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("non-object endpoint options must fail");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("endpoint options must be an object"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_get_rejects_non_null_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://example.com/anything",
        HashMap::new(),
    );
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: { x: 1 } });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("GET endpoint should reject non-null body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("does not accept a request body"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_get_rejects_explicit_null_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://example.com/anything",
        HashMap::new(),
    );
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: null });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("GET endpoint should reject explicit null body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("does not accept a request body"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_head_rejects_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Head,
        "https://example.com/anything",
        HashMap::new(),
    );
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: { x: 1 } });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("HEAD endpoint should reject request body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("does not accept a request body"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_options_rejects_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Options,
        "https://example.com/anything",
        HashMap::new(),
    );
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: { x: 1 } });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("OPTIONS endpoint should reject request body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("does not accept a request body"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_bytes_request_type_rejects_non_buffer_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com/anything",
        HashMap::new(),
    )
    .with_request_body_type(EndpointBodyType::Bytes);
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: "not-bytes" });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("bytes request type should reject string body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("request_body_type `bytes`"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_utf8_request_type_rejects_non_string_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com/anything",
        HashMap::new(),
    )
    .with_request_body_type(EndpointBodyType::Utf8);
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: { x: 1 } });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("utf8 request type should reject non-string body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("request_body_type `utf8`"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_json_request_type_rejects_bytes_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com/anything",
        HashMap::new(),
    )
    .with_request_body_type(EndpointBodyType::Json);
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", { body: new Uint8Array([1, 2, 3]) });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("json request type should reject bytes body");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("request_body_type `json`"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn endpoint_rejects_non_allowlisted_header_override() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com/anything",
        HashMap::new(),
    )
    .with_overridable_request_headers(vec!["x-allowed".to_owned()]);
    let config = endpoint_config("ep", endpoint);

    let source = r#"
            import endpoint from "mechanics:endpoint";
            export default async function main(_arg) {
                return await endpoint("ep", {
                    headers: { "x-not-allowed": "blocked" },
                    body: { x: 1 }
                });
            }
        "#;
    let job = make_job(source, config, Value::Null);
    let err = pool
        .run(job)
        .expect_err("non-allowlisted override header should fail");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("not allowlisted"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}
