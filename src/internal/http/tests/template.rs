use super::super::*;

#[test]
fn duplicate_template_slot_is_rejected() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://example.com/{id}/{id}",
        HashMap::new(),
    )
    .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));

    let err = endpoint
        .build_url(&EndpointCallOptions::default())
        .expect_err("duplicate slot should be invalid");
    assert!(err.to_string().contains("duplicate slot"));
}

#[test]
fn url_template_rejects_built_in_query_string() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com?a=1", HashMap::new());
    let err = endpoint
        .build_url(&EndpointCallOptions::default())
        .expect_err("query in url template should fail");
    assert!(
        err.to_string()
            .contains("url_template must not include query parameters")
    );
}

#[test]
fn url_param_missing_without_default_resolves_to_empty_string() {
    let endpoint = HttpEndpoint::new(
        HttpMethod::Get,
        "https://example.com/items/{id}",
        HashMap::new(),
    )
    .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));

    let url = endpoint
        .build_url(&EndpointCallOptions::default())
        .expect("missing url param without default should resolve as empty");
    assert_eq!(url.as_str(), "https://example.com/items/");
}
