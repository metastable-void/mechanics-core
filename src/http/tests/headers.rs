use super::super::*;

#[test]
fn build_headers_rejects_invalid_name() {
    let mut headers = HashMap::new();
    headers.insert("bad header".to_owned(), "ok".to_owned());
    let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", headers);
    let err = endpoint
        .build_headers(Some("application/json"))
        .expect_err("invalid header name must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("invalid header name"));
}
