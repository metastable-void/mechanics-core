use super::super::*;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

#[test]
fn build_headers_rejects_invalid_name() {
    let mut headers = HashMap::new();
    headers.insert("bad header".to_owned(), "ok".to_owned());
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", headers);
    let err = endpoint
        .build_headers(Some("application/json"), &EndpointCallOptions::default())
        .expect_err("invalid header name must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("invalid header name"));
}

#[test]
fn build_headers_allows_case_insensitive_allowlisted_overrides() {
    let mut static_headers = HashMap::new();
    static_headers.insert("X-Fixed".to_owned(), "fixed".to_owned());
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", static_headers)
        .with_overridable_request_headers(vec!["x-fixed".to_owned(), "content-type".to_owned()]);

    let mut options = EndpointCallOptions::default();
    options
        .headers
        .insert("X-FiXeD".to_owned(), "overridden".to_owned());
    options.headers.insert(
        "Content-Type".to_owned(),
        "application/custom+json".to_owned(),
    );

    let headers = endpoint
        .build_headers(Some("application/json"), &options)
        .expect("allowlisted overrides should succeed");
    assert_eq!(headers["x-fixed"], "overridden");
    assert_eq!(headers["content-type"], "application/custom+json");
}

#[test]
fn build_headers_rejects_non_allowlisted_override() {
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", HashMap::new())
        .with_overridable_request_headers(vec!["x-allowed".to_owned()]);
    let mut options = EndpointCallOptions::default();
    options
        .headers
        .insert("x-not-allowed".to_owned(), "value".to_owned());

    let err = endpoint
        .build_headers(None, &options)
        .expect_err("non-allowlisted override should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("not allowlisted"));
}

#[test]
fn build_headers_rejects_case_insensitive_duplicate_overrides() {
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", HashMap::new())
        .with_overridable_request_headers(vec!["x-dup".to_owned()]);
    let mut options = EndpointCallOptions::default();
    options.headers.insert("x-dup".to_owned(), "one".to_owned());
    options.headers.insert("X-Dup".to_owned(), "two".to_owned());

    let err = endpoint
        .build_headers(None, &options)
        .expect_err("case-insensitive duplicate overrides in the same layer should fail");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("duplicate override header"));
}

#[test]
fn build_headers_applies_intended_precedence_order() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Post,
        "https://example.com",
        HashMap::from([
            ("content-type".to_owned(), "configured/type".to_owned()),
            ("user-agent".to_owned(), "configured-agent".to_owned()),
        ]),
    )
    .with_overridable_request_headers(vec!["content-type".to_owned()]);

    let mut options = EndpointCallOptions::default();
    options
        .headers
        .insert("content-type".to_owned(), "override/type".to_owned());

    let headers = endpoint
        .build_headers(Some("application/json"), &options)
        .expect("header layering should succeed");

    // JS override beats configured and auto default.
    assert_eq!(headers["content-type"], "override/type");
    // Configured beats auto default.
    assert_eq!(headers["user-agent"], "configured-agent");
}

#[test]
fn extract_exposed_response_headers_is_case_insensitive() {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-response-id"),
        HeaderValue::from_static("id-1"),
    );
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );

    let extracted = extract_exposed_response_headers(
        &headers,
        &["X-Response-Id".to_owned(), "CONTENT-TYPE".to_owned()],
    )
    .expect("header extraction should succeed");

    assert_eq!(extracted.get("x-response-id"), Some(&"id-1".to_owned()));
    assert_eq!(
        extracted.get("content-type"),
        Some(&"application/json".to_owned())
    );
}
