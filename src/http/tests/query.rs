use super::super::*;

#[test]
fn optional_allow_empty_query_mode_emits_empty_values() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
        .with_query_specs(vec![QuerySpec::Slotted {
            key: "q".to_owned(),
            slot: "q".to_owned(),
            mode: SlottedQueryMode::OptionalAllowEmpty,
            default: None,
            min_bytes: None,
            max_bytes: None,
        }]);

    let mut options = EndpointCallOptions::default();
    options.queries.insert("q".to_owned(), "".to_owned());
    let url = endpoint
        .build_url(&options)
        .expect("optional_allow_empty should emit empty value");
    assert_eq!(url.query(), Some("q="));
}

#[test]
fn build_url_rejects_unknown_queries_key() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
        .with_query_specs(vec![QuerySpec::Slotted {
            key: "q".to_owned(),
            slot: "q".to_owned(),
            mode: SlottedQueryMode::Optional,
            default: None,
            min_bytes: None,
            max_bytes: None,
        }]);
    let mut options = EndpointCallOptions::default();
    options
        .queries
        .insert("unexpected".to_owned(), "x".to_owned());
    let err = endpoint
        .build_url(&options)
        .expect_err("unknown query key should be rejected");
    assert!(err.to_string().contains("unknown queries key"));
}

#[test]
fn required_query_rejects_empty_value() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
        .with_query_specs(vec![QuerySpec::Slotted {
            key: "q".to_owned(),
            slot: "q".to_owned(),
            mode: SlottedQueryMode::Required,
            default: None,
            min_bytes: None,
            max_bytes: None,
        }]);
    let mut options = EndpointCallOptions::default();
    options.queries.insert("q".to_owned(), "".to_owned());
    let err = endpoint
        .build_url(&options)
        .expect_err("required mode should reject empty value");
    assert!(err.to_string().contains("missing or empty"));
}

#[test]
fn optional_query_omits_empty_value() {
    let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
        .with_query_specs(vec![QuerySpec::Slotted {
            key: "q".to_owned(),
            slot: "q".to_owned(),
            mode: SlottedQueryMode::Optional,
            default: None,
            min_bytes: None,
            max_bytes: None,
        }]);
    let mut options = EndpointCallOptions::default();
    options.queries.insert("q".to_owned(), "".to_owned());
    let url = endpoint
        .build_url(&options)
        .expect("optional mode should omit empty value");
    assert!(url.query().is_none());
}
