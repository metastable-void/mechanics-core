use super::super::*;

#[test]
fn extend_body_with_limit_accepts_exact_boundary() {
    let mut body = Vec::new();
    extend_body_with_limit(&mut body, b"abc", Some(3)).expect("exact boundary should be allowed");
    assert_eq!(body, b"abc");
}

#[test]
fn extend_body_with_limit_rejects_oversize() {
    let mut body = vec![1, 2, 3];
    let err = extend_body_with_limit(&mut body, b"45", Some(4))
        .expect_err("exceeding max bytes should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(err.to_string().contains("exceeds configured max bytes"));
}

#[test]
fn endpoint_deserializes_response_max_bytes_from_snake_case() {
    let endpoint: HttpEndpoint = serde_json::from_value(serde_json::json!({
        "method": "get",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "response_max_bytes": 1024
    }))
    .expect("endpoint config should deserialize response_max_bytes");

    assert_eq!(endpoint.response_max_bytes, Some(1024));
}
