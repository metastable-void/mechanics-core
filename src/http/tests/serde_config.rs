use super::super::*;
use serde_json::json;

#[test]
fn endpoint_deserializes_from_snake_case_body_types() {
    let endpoint: HttpEndpoint = serde_json::from_value(json!({
        "method": "post",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "request_body_type": "bytes",
        "response_body_type": "utf8"
    }))
    .expect("snake_case endpoint config should deserialize");

    let mut options = EndpointCallOptions::default();
    options.url_params.insert("id".to_owned(), "1".to_owned());
    let url = endpoint
        .build_url(&options)
        .expect("deserialized endpoint should build URL");
    assert_eq!(url.as_str(), "https://example.com/1");
}

#[test]
fn mechanics_config_new_rejects_invalid_endpoint_configuration() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
        .with_url_param_specs(HashMap::from([(
            "other".to_owned(),
            UrlParamSpec::default(),
        )]));

    let mut endpoints = HashMap::new();
    endpoints.insert("bad".to_owned(), endpoint);

    let err = MechanicsConfig::new(endpoints).expect_err("config should fail fast");
    assert!(matches!(err, crate::MechanicsError::RuntimePool(_)));
    assert!(
        err.msg()
            .contains("missing url_param_specs entry for slot `id`")
    );
}

#[test]
fn mechanics_config_deserialize_rejects_invalid_endpoint_configuration() {
    let err = serde_json::from_value::<MechanicsConfig>(json!({
        "endpoints": {
            "bad": {
                "method": "get",
                "url_template": "https://example.com/{id}",
                "url_param_specs": { "other": {} }
            }
        }
    }))
    .expect_err("deserialization should fail fast");

    assert!(
        err.to_string()
            .contains("missing url_param_specs entry for slot `id`")
    );
}

#[test]
fn mechanics_config_rejects_invalid_header_allowlist_name() {
    let endpoint: HttpEndpoint = serde_json::from_value(json!({
        "method": "post",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "overridable_request_headers": ["bad header"]
    }))
    .expect("endpoint itself should deserialize");

    let mut endpoints = HashMap::new();
    endpoints.insert("bad".to_owned(), endpoint);
    let err = MechanicsConfig::new(endpoints).expect_err("config should fail fast");
    assert!(err.msg().contains("invalid header name `bad header`"));
}

#[test]
fn mechanics_config_allows_empty_default_for_optional_query_with_min_bytes() {
    let endpoint: HttpEndpoint = serde_json::from_value(json!({
        "method": "get",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "query_specs": [{
            "type": "slotted",
            "key": "q",
            "slot": "q",
            "mode": "optional",
            "default": "",
            "min_bytes": 1
        }]
    }))
    .expect("endpoint should deserialize");

    let mut endpoints = HashMap::new();
    endpoints.insert("ok".to_owned(), endpoint);
    MechanicsConfig::new(endpoints).expect("empty optional default should be treated as omitted");
}

#[test]
fn mechanics_config_rejects_empty_default_for_optional_allow_empty_with_min_bytes() {
    let endpoint: HttpEndpoint = serde_json::from_value(json!({
        "method": "get",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "query_specs": [{
            "type": "slotted",
            "key": "q",
            "slot": "q",
            "mode": "optional_allow_empty",
            "default": "",
            "min_bytes": 1
        }]
    }))
    .expect("endpoint should deserialize");

    let mut endpoints = HashMap::new();
    endpoints.insert("bad".to_owned(), endpoint);
    let err = MechanicsConfig::new(endpoints)
        .expect_err("empty optional_allow_empty default should violate min_bytes");
    assert!(err.msg().contains("too short"));
}

#[test]
fn endpoint_deserializes_additional_http_methods() {
    for method in ["patch", "head", "options"] {
        let endpoint: HttpEndpoint = serde_json::from_value(json!({
            "method": method,
            "url_template": "https://example.com/{id}",
            "url_param_specs": { "id": {} }
        }))
        .expect("endpoint should deserialize additional method");

        let mut options = EndpointCallOptions::default();
        options.url_params.insert("id".to_owned(), "1".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("deserialized endpoint should build URL");
        assert_eq!(url.as_str(), "https://example.com/1");
    }
}

#[test]
fn http_method_body_support_matrix_matches_contract() {
    assert!(HttpMethod::Post.supports_request_body());
    assert!(HttpMethod::Put.supports_request_body());
    assert!(HttpMethod::Patch.supports_request_body());
    assert!(!HttpMethod::Get.supports_request_body());
    assert!(!HttpMethod::Delete.supports_request_body());
    assert!(!HttpMethod::Head.supports_request_body());
    assert!(!HttpMethod::Options.supports_request_body());
}

#[test]
fn mechanics_config_composition_helpers_apply_validation_and_overrides() {
    let base = MechanicsConfig::new(HashMap::from([(
        "base".to_owned(),
        HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
            .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())])),
    )]))
    .expect("base config should build");

    let over = HttpEndpoint::new(HttpMethod::Patch, "https://example.com/{id}", HashMap::new())
        .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));
    let cfg = base
        .clone()
        .with_endpoint("base", over.clone())
        .expect("single endpoint override should validate");
    assert_eq!(cfg.endpoints["base"].method, HttpMethod::Patch);

    let cfg = base
        .with_endpoint_overrides(HashMap::from([("extra".to_owned(), over)]))
        .expect("bulk overrides should validate");
    assert!(cfg.endpoints.contains_key("base"));
    assert!(cfg.endpoints.contains_key("extra"));

    let removed = cfg.without_endpoint("extra");
    assert!(!removed.endpoints.contains_key("extra"));
}

#[test]
fn mechanics_config_composition_helpers_reject_invalid_endpoint() {
    let base = MechanicsConfig::new(HashMap::new()).expect("base config should build");
    let invalid = HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
        .with_url_param_specs(HashMap::from([(
            "other".to_owned(),
            UrlParamSpec::default(),
        )]));

    let err = base
        .with_endpoint("bad", invalid)
        .expect_err("invalid endpoint must be rejected");
    assert!(err.msg().contains("missing url_param_specs entry for slot `id`"));
}
