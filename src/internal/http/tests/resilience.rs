use super::super::*;
use crate::internal::http::EndpointHttpHeaders;
use serde_json::json;

#[test]
fn endpoint_deserializes_retry_policy_from_json() {
    let endpoint: HttpEndpoint = serde_json::from_value(json!({
        "method": "get",
        "url_template": "https://example.com/{id}",
        "url_param_specs": { "id": {} },
        "retry_policy": {
            "max_attempts": 4,
            "base_backoff_ms": 25,
            "max_backoff_ms": 200,
            "max_retry_delay_ms": 500,
            "rate_limit_backoff_ms": 75,
            "retry_on_io_errors": true,
            "retry_on_timeout": true,
            "respect_retry_after": true,
            "retry_on_status": [429, 503]
        }
    }))
    .expect("endpoint with retry policy should deserialize");

    endpoint
        .validate_config()
        .expect("retry policy values should validate");
}

#[test]
fn endpoint_rejects_invalid_retry_policy() {
    let err = serde_json::from_value::<MechanicsConfig>(json!({
        "endpoints": {
            "bad": {
                "method": "get",
                "url_template": "https://example.com/{id}",
                "url_param_specs": { "id": {} },
                "retry_policy": {
                    "max_attempts": 0
                }
            }
        }
    }))
    .expect_err("invalid retry policy should fail config parsing");

    assert!(
        err.to_string()
            .contains("retry_policy.max_attempts must be > 0")
    );
}

#[test]
fn endpoint_rejects_unknown_retry_policy_fields() {
    let err = serde_json::from_value::<MechanicsConfig>(json!({
        "endpoints": {
            "bad": {
                "method": "get",
                "url_template": "https://example.com/{id}",
                "url_param_specs": { "id": {} },
                "retry_policy": {
                    "max_attempts": 2,
                    "unknown": true
                }
            }
        }
    }))
    .expect_err("unknown retry_policy fields should fail config parsing");

    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn endpoint_rejects_zero_max_retry_delay() {
    let err = serde_json::from_value::<MechanicsConfig>(json!({
        "endpoints": {
            "bad": {
                "method": "get",
                "url_template": "https://example.com/{id}",
                "url_param_specs": { "id": {} },
                "retry_policy": {
                    "max_attempts": 2,
                    "max_retry_delay_ms": 0
                }
            }
        }
    }))
    .expect_err("zero max_retry_delay_ms should fail config parsing");

    assert!(
        err.to_string()
            .contains("retry_policy.max_retry_delay_ms must be > 0")
    );
}

#[test]
fn retry_policy_uses_retry_after_for_rate_limit() {
    let policy = EndpointRetryPolicy {
        max_attempts: 3,
        base_backoff_ms: 10,
        max_backoff_ms: 100,
        max_retry_delay_ms: 5_000,
        rate_limit_backoff_ms: 250,
        retry_on_io_errors: true,
        retry_on_timeout: true,
        respect_retry_after: true,
        retry_on_status: vec![429],
    };

    let mut headers = EndpointHttpHeaders::new();
    headers.insert("retry-after", "2");
    let delay = policy.retry_delay_for_status(429, &headers, 1);
    assert_eq!(delay, std::time::Duration::from_secs(2));
}

#[test]
fn retry_policy_falls_back_to_rate_limit_backoff_without_retry_after() {
    let policy = EndpointRetryPolicy {
        max_attempts: 3,
        base_backoff_ms: 10,
        max_backoff_ms: 100,
        max_retry_delay_ms: 5_000,
        rate_limit_backoff_ms: 250,
        retry_on_io_errors: true,
        retry_on_timeout: true,
        respect_retry_after: true,
        retry_on_status: vec![429],
    };

    let delay = policy.retry_delay_for_status(429, &EndpointHttpHeaders::new(), 1);
    assert_eq!(delay, std::time::Duration::from_millis(250));
}

#[test]
fn retry_policy_backoff_caps_to_max_delay() {
    let policy = EndpointRetryPolicy {
        max_attempts: 5,
        base_backoff_ms: 200,
        max_backoff_ms: 10_000,
        max_retry_delay_ms: 500,
        rate_limit_backoff_ms: 100,
        retry_on_io_errors: true,
        retry_on_timeout: true,
        respect_retry_after: true,
        retry_on_status: vec![500],
    };

    let delay = policy.retry_delay_for_status(500, &EndpointHttpHeaders::new(), 4);
    assert_eq!(delay, std::time::Duration::from_millis(500));
}
