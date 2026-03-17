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
