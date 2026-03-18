#[cfg(test)]
use super::MechanicsConfig;
#[cfg(test)]
use super::headers::extract_exposed_response_headers;
#[cfg(test)]
use super::parse_endpoint_call_options;
#[cfg(test)]
use super::{EndpointCallBody, EndpointCallOptions};
use super::{
    HttpMethod,
    query::{validate_byte_len, validate_min_max_bounds},
    retry::EndpointRetryPolicy,
    template::UrlTemplateChunk,
};
use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::io::ErrorKind;

#[cfg(test)]
use self::execute::extend_body_with_limit;

mod execute;
mod request;
mod validate;

#[derive(
    JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum EndpointBodyType {
    /// JSON payload (`application/json`).
    #[default]
    Json,
    /// UTF-8 string payload (`text/plain; charset=utf-8`).
    Utf8,
    /// Raw bytes payload (`application/octet-stream`).
    Bytes,
}

/// Validation and default policy for one URL template slot.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct UrlParamSpec {
    /// Optional fallback value used when the JS-provided value is missing or empty.
    #[serde(default)]
    pub default: Option<String>,
    /// Optional minimum UTF-8 byte length accepted for the resolved value.
    #[serde(default)]
    pub min_bytes: Option<usize>,
    /// Optional maximum UTF-8 byte length accepted for the resolved value.
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

impl UrlParamSpec {
    fn resolve_value(&self, slot: &str, provided: Option<&str>) -> std::io::Result<String> {
        validate_min_max_bounds(slot, self.min_bytes, self.max_bytes)?;
        let value = match provided {
            Some(v) if !v.is_empty() => v,
            Some(_) | None => self.default.as_deref().unwrap_or(""),
        };
        validate_byte_len(slot, value, self.min_bytes, self.max_bytes)?;
        Ok(value.to_owned())
    }
}

/// Emission mode for a slotted query parameter.
#[derive(
    JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum SlottedQueryMode {
    /// Slot must resolve and must be non-empty.
    #[default]
    Required,
    /// Slot must resolve and may be empty.
    RequiredAllowEmpty,
    /// Missing/empty is treated as omitted.
    Optional,
    /// Missing is omitted; if provided, empty is emitted.
    OptionalAllowEmpty,
}

/// One query emission rule.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuerySpec {
    /// Emits a constant key/value pair.
    Const {
        /// Query key to emit.
        key: String,
        /// Constant value to emit.
        value: String,
    },
    /// Emits a query pair from a JS slot (`queries[slot]`) under configured policy.
    Slotted {
        /// Query key to emit.
        key: String,
        /// JS `queries` slot name to read.
        slot: String,
        /// Resolution and omission policy.
        #[serde(default)]
        mode: SlottedQueryMode,
        /// Optional fallback value used when slot input is missing.
        #[serde(default)]
        default: Option<String>,
        /// Optional minimum UTF-8 byte length for emitted value.
        #[serde(default)]
        min_bytes: Option<usize>,
        /// Optional maximum UTF-8 byte length for emitted value.
        #[serde(default)]
        max_bytes: Option<usize>,
    },
}

/// HTTP endpoint configuration used by the runtime-provided JS helper.
///
/// Endpoint definitions are pure configuration inputs and should be treated as stateless.
/// Any caching behavior should be implemented outside this crate.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct HttpEndpoint {
    method: HttpMethod,
    url_template: String,
    #[serde(default)]
    url_param_specs: HashMap<String, UrlParamSpec>,
    #[serde(default)]
    query_specs: Vec<QuerySpec>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    overridable_request_headers: Vec<String>,
    #[serde(default)]
    exposed_response_headers: Vec<String>,
    #[serde(default)]
    request_body_type: Option<EndpointBodyType>,
    #[serde(default)]
    response_body_type: EndpointBodyType,
    #[serde(default)]
    response_max_bytes: Option<usize>,
    timeout_ms: Option<u64>,
    #[serde(default)]
    allow_non_success_status: bool,
    // SAFETY: `EndpointRetryPolicy` stores plain Rust data and does not hold GC-managed values.
    #[unsafe_ignore_trace]
    #[serde(default)]
    retry_policy: EndpointRetryPolicy,
}

impl HttpEndpoint {
    pub(super) const USER_AGENT: &str = concat!(
        "Mozilla/5.0 (compatible; mechanics-rs/",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    /// Constructs an endpoint definition used by runtime-owned HTTP helpers.
    pub fn new(method: HttpMethod, url_template: &str, headers: HashMap<String, String>) -> Self {
        Self {
            method,
            url_template: url_template.to_owned(),
            url_param_specs: HashMap::new(),
            query_specs: Vec::new(),
            headers,
            overridable_request_headers: Vec::new(),
            exposed_response_headers: Vec::new(),
            request_body_type: None,
            response_body_type: EndpointBodyType::Json,
            response_max_bytes: None,
            timeout_ms: None,
            allow_non_success_status: false,
            retry_policy: EndpointRetryPolicy::default(),
        }
    }

    /// Replaces URL slot constraints used by `url_template` placeholders.
    pub fn with_url_param_specs(mut self, url_param_specs: HashMap<String, UrlParamSpec>) -> Self {
        self.url_param_specs = url_param_specs;
        self
    }

    /// Replaces query emission rules.
    pub fn with_query_specs(mut self, query_specs: Vec<QuerySpec>) -> Self {
        self.query_specs = query_specs;
        self
    }

    /// Sets request body decoding mode.
    ///
    /// If unset, request body mode defaults to JSON.
    pub fn with_request_body_type(mut self, body_type: EndpointBodyType) -> Self {
        self.request_body_type = Some(body_type);
        self
    }

    /// Sets request header names that JS may override via `endpoint(..., { headers })`.
    ///
    /// Matching is case-insensitive.
    pub fn with_overridable_request_headers(mut self, headers: Vec<String>) -> Self {
        self.overridable_request_headers = headers;
        self
    }

    /// Sets response header names that are exposed to JS in endpoint response objects.
    ///
    /// Matching is case-insensitive.
    pub fn with_exposed_response_headers(mut self, headers: Vec<String>) -> Self {
        self.exposed_response_headers = headers;
        self
    }

    /// Sets response body decoding mode.
    ///
    /// Defaults to JSON.
    pub fn with_response_body_type(mut self, body_type: EndpointBodyType) -> Self {
        self.response_body_type = body_type;
        self
    }

    /// Sets a per-endpoint maximum response-body size in bytes.
    ///
    /// If this is `Some`, it overrides the pool default response limit.
    /// If this is `None`, the pool default response limit is used.
    pub fn with_response_max_bytes(mut self, response_max_bytes: Option<usize>) -> Self {
        self.response_max_bytes = response_max_bytes;
        self
    }

    /// Sets a per-endpoint timeout in milliseconds.
    ///
    /// If this is `Some`, it overrides the pool default endpoint timeout.
    /// If this is `None`, the pool default timeout is used.
    pub fn with_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Allows non-success (non-2xx) HTTP status responses to proceed.
    ///
    /// Defaults to `false`, which treats non-success statuses as request errors.
    pub fn with_allow_non_success_status(mut self, allow: bool) -> Self {
        self.allow_non_success_status = allow;
        self
    }

    /// Sets endpoint retry/backoff/rate-limit policy.
    pub fn with_retry_policy(mut self, retry_policy: EndpointRetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedHttpEndpoint {
    parsed_url_chunks: Vec<UrlTemplateChunk>,
    url_slot_names: Vec<String>,
    url_slot_set: HashSet<String>,
    allowed_query_slots: HashSet<String>,
    allowed_overrides: HashSet<HeaderName>,
    exposed_response_allowlist: HashSet<HeaderName>,
}

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;
