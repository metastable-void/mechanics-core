use super::super::*;
use serde_json::json;

#[test]
fn parse_endpoint_call_options_requires_object_or_nullish() {
    let err = parse_endpoint_call_options(Some(json!(1))).expect_err("number must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    let parsed = parse_endpoint_call_options(Some(json!({
        "urlParams": {"x": "1"},
        "queries": {"q": "v"},
        "headers": {"x-test": "v1"},
        "body": {"ok": true}
    })))
    .expect("object should parse");

    assert_eq!(parsed.url_params.get("x"), Some(&"1".to_owned()));
    assert_eq!(parsed.queries.get("q"), Some(&"v".to_owned()));
    assert_eq!(parsed.headers.get("x-test"), Some(&"v1".to_owned()));
    match parsed.body {
        EndpointCallBody::Json(v) => assert_eq!(v, json!({"ok": true})),
        other => panic!("unexpected body variant: {other:?}"),
    }
}
