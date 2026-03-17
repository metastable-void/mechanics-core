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
