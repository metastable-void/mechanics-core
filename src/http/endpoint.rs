#[cfg(test)]
use super::MechanicsConfig;
#[cfg(test)]
use super::parse_endpoint_call_options;
use super::{
    EndpointCallBody, EndpointCallOptions, EndpointHttpClient, EndpointHttpRequest,
    EndpointHttpRequestBody, EndpointResponse, EndpointResponseBody, HttpMethod, into_io_error,
};
use super::{
    query::{
        resolve_slotted_query_value, validate_byte_len, validate_min_max_bounds,
        validate_query_key, validate_slot_name,
    },
    template::{UrlTemplateChunk, parse_url_template, percent_encode_component},
};
use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{Error, ErrorKind},
    sync::Arc,
    time::Duration,
};

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

/// Endpoint-level resilience policy for retries, backoff, and rate-limit handling.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct EndpointRetryPolicy {
    /// Maximum total attempts (initial request + retries).
    pub max_attempts: usize,
    /// Base backoff delay in milliseconds for retry calculation.
    pub base_backoff_ms: u64,
    /// Maximum exponential backoff delay in milliseconds.
    pub max_backoff_ms: u64,
    /// Maximum delay applied from any retry rule in milliseconds.
    pub max_retry_delay_ms: u64,
    /// Fallback delay in milliseconds for rate-limited responses when `Retry-After` is absent.
    pub rate_limit_backoff_ms: u64,
    /// Whether transport I/O failures should be retried.
    pub retry_on_io_errors: bool,
    /// Whether timeout failures should be retried.
    pub retry_on_timeout: bool,
    /// Whether to honor `Retry-After` on status `429`.
    pub respect_retry_after: bool,
    /// HTTP statuses eligible for retries.
    pub retry_on_status: Vec<u16>,
}

impl Default for EndpointRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            base_backoff_ms: 100,
            max_backoff_ms: 5_000,
            max_retry_delay_ms: 30_000,
            rate_limit_backoff_ms: 1_000,
            retry_on_io_errors: true,
            retry_on_timeout: true,
            respect_retry_after: true,
            retry_on_status: vec![429, 500, 502, 503, 504],
        }
    }
}

impl EndpointRetryPolicy {
    fn validate(&self) -> std::io::Result<()> {
        if self.max_attempts == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_attempts must be > 0",
            ));
        }
        if self.max_backoff_ms < self.base_backoff_ms {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_backoff_ms must be >= base_backoff_ms",
            ));
        }
        if self.max_retry_delay_ms == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_retry_delay_ms must be > 0",
            ));
        }
        for status in &self.retry_on_status {
            if !(100..=599).contains(status) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("retry_policy.retry_on_status contains invalid status code `{status}`"),
                ));
            }
        }
        Ok(())
    }

    fn should_retry_status(&self, status: u16) -> bool {
        self.retry_on_status.contains(&status)
    }

    fn should_retry_transport_error(&self, err: &std::io::Error) -> bool {
        if err.kind() == ErrorKind::TimedOut {
            return self.retry_on_timeout;
        }
        self.retry_on_io_errors
    }

    fn retry_delay_for_transport(&self, attempt: usize) -> Duration {
        Duration::from_millis(self.backoff_delay_ms(attempt))
    }

    fn retry_delay_for_status(&self, status: u16, headers: &HeaderMap, attempt: usize) -> Duration {
        let delay_ms = if status == 429 {
            self.rate_limit_delay_ms(headers, attempt)
        } else {
            self.backoff_delay_ms(attempt)
        };
        Duration::from_millis(delay_ms)
    }

    fn rate_limit_delay_ms(&self, headers: &HeaderMap, attempt: usize) -> u64 {
        let retry_after_ms = if self.respect_retry_after {
            headers
                .get(RETRY_AFTER)
                .and_then(Self::parse_retry_after_ms)
                .map(|v| v.min(self.max_retry_delay_ms))
        } else {
            None
        };
        retry_after_ms.unwrap_or_else(|| {
            self.rate_limit_backoff_ms
                .max(self.backoff_delay_ms(attempt))
                .min(self.max_retry_delay_ms)
        })
    }

    fn parse_retry_after_ms(value: &HeaderValue) -> Option<u64> {
        let seconds = value.to_str().ok()?.trim().parse::<u64>().ok()?;
        Some(seconds.saturating_mul(1_000))
    }

    fn backoff_delay_ms(&self, attempt: usize) -> u64 {
        let exp = (attempt.saturating_sub(1)).min(20);
        let exp_u32 = u32::try_from(exp).unwrap_or(20);
        let factor = 2u64.saturating_pow(exp_u32);
        self.base_backoff_ms
            .saturating_mul(factor)
            .min(self.max_backoff_ms)
            .min(self.max_retry_delay_ms)
    }
}

/// Validation and default policy for one URL template slot.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, Default)]
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
#[serde(tag = "type", rename_all = "snake_case")]
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
    const USER_AGENT: &str = concat!(
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

    pub(crate) fn validate_config(&self) -> std::io::Result<()> {
        self.retry_policy.validate()?;
        validate_header_name_list(
            &self.overridable_request_headers,
            "overridable_request_headers",
        )?;
        validate_header_name_list(&self.exposed_response_headers, "exposed_response_headers")?;

        let (chunks, slot_names) = parse_url_template(&self.url_template)?;
        let slot_set: HashSet<&str> = slot_names.iter().map(String::as_str).collect();

        for slot in &slot_names {
            let spec = self.url_param_specs.get(slot).ok_or(Error::new(
                ErrorKind::InvalidInput,
                format!("missing url_param_specs entry for slot `{slot}`"),
            ))?;
            validate_min_max_bounds(slot, spec.min_bytes, spec.max_bytes)?;
            if let Some(default_value) = spec.default.as_deref() {
                validate_byte_len(slot, default_value, spec.min_bytes, spec.max_bytes)?;
            }
        }

        for configured in self.url_param_specs.keys() {
            if !slot_set.contains(configured.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "url_param_specs entry `{configured}` has no placeholder in url_template"
                    ),
                ));
            }
        }

        let mut template_probe = String::with_capacity(self.url_template.len().saturating_add(16));
        for chunk in chunks {
            match chunk {
                UrlTemplateChunk::Literal(s) => template_probe.push_str(&s),
                UrlTemplateChunk::Slot(_) => template_probe.push('x'),
            }
        }
        let url = reqwest::Url::parse(&template_probe).map_err(into_io_error)?;
        if url.fragment().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template must not include URL fragments",
            ));
        }
        if url.query().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template must not include query parameters; use query_specs instead",
            ));
        }

        for spec in &self.query_specs {
            match spec {
                QuerySpec::Const { key, .. } => validate_query_key(key)?,
                QuerySpec::Slotted {
                    key,
                    slot,
                    mode,
                    default,
                    min_bytes,
                    max_bytes,
                    ..
                } => {
                    validate_query_key(key)?;
                    validate_slot_name(slot)?;
                    validate_min_max_bounds(slot, *min_bytes, *max_bytes)?;
                    if let Some(default_value) = default {
                        let should_validate_default = match mode {
                            SlottedQueryMode::Required | SlottedQueryMode::Optional => {
                                !default_value.is_empty()
                            }
                            SlottedQueryMode::RequiredAllowEmpty
                            | SlottedQueryMode::OptionalAllowEmpty => true,
                        };
                        if should_validate_default {
                            validate_byte_len(slot, default_value, *min_bytes, *max_bytes)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn prepare_runtime(&self) -> std::io::Result<PreparedHttpEndpoint> {
        let (parsed_url_chunks, url_slot_names) = parse_url_template(&self.url_template)?;
        let url_slot_set = url_slot_names.iter().cloned().collect::<HashSet<_>>();
        let allowed_query_slots = self
            .query_specs
            .iter()
            .filter_map(|spec| match spec {
                QuerySpec::Slotted { slot, .. } => Some(slot.clone()),
                QuerySpec::Const { .. } => None,
            })
            .collect::<HashSet<_>>();
        let allowed_overrides = allowlisted_header_names(
            &self.overridable_request_headers,
            "overridable_request_headers",
        )?;
        let exposed_response_allowlist =
            allowlisted_header_names(&self.exposed_response_headers, "exposed_response_headers")?;

        Ok(PreparedHttpEndpoint {
            parsed_url_chunks,
            url_slot_names,
            url_slot_set,
            allowed_query_slots,
            allowed_overrides,
            exposed_response_allowlist,
        })
    }

    fn effective_request_body_type(&self) -> EndpointBodyType {
        self.request_body_type
            .clone()
            .unwrap_or(EndpointBodyType::Json)
    }

    #[cfg(test)]
    fn build_headers(
        &self,
        default_content_type: Option<&str>,
        options: &EndpointCallOptions,
    ) -> std::io::Result<HeaderMap> {
        let prepared = self.prepare_runtime()?;
        self.build_headers_prepared(default_content_type, options, &prepared)
    }

    fn build_headers_prepared(
        &self,
        default_content_type: Option<&str>,
        options: &EndpointCallOptions,
        prepared: &PreparedHttpEndpoint,
    ) -> std::io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        // Header precedence is explicit:
        // 1) auto defaults, 2) endpoint configured headers, 3) JS allowlisted overrides.
        let user_agent = HeaderValue::try_from(Self::USER_AGENT).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid default User-Agent header: {e}"),
            )
        })?;
        headers.insert(USER_AGENT, user_agent);

        if let Some(default_content_type) = default_content_type {
            let content_type = HeaderValue::try_from(default_content_type).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid default Content-Type header: {e}"),
                )
            })?;
            headers.insert(CONTENT_TYPE, content_type);
        }

        for (k, v) in &self.headers {
            let name = HeaderName::try_from(k.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid header name `{k}`: {e}"),
                )
            })?;
            let value = HeaderValue::try_from(v.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid header value for `{k}`: {e}"),
                )
            })?;
            headers.insert(name, value);
        }

        for (k, v) in &options.headers {
            let name = HeaderName::try_from(k.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid override header name `{k}`: {e}"),
                )
            })?;
            if !prepared.allowed_overrides.contains(&name) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "override header `{k}` is not allowlisted in overridable_request_headers"
                    ),
                ));
            }
            let value = HeaderValue::try_from(v.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid override header value for `{k}`: {e}"),
                )
            })?;
            headers.insert(name, value);
        }

        Ok(headers)
    }

    #[cfg(test)]
    fn build_url(&self, options: &EndpointCallOptions) -> std::io::Result<reqwest::Url> {
        let prepared = self.prepare_runtime()?;
        self.build_url_prepared(options, &prepared)
    }

    fn build_url_prepared(
        &self,
        options: &EndpointCallOptions,
        prepared: &PreparedHttpEndpoint,
    ) -> std::io::Result<reqwest::Url> {
        for provided in options.url_params.keys() {
            if !prepared.url_slot_set.contains(provided) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "unknown urlParams key `{provided}` for endpoint template `{}`",
                        self.url_template
                    ),
                ));
            }
        }

        for slot in &prepared.url_slot_names {
            if !self.url_param_specs.contains_key(slot) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("missing url_param_specs entry for slot `{slot}`"),
                ));
            }
        }

        for configured in self.url_param_specs.keys() {
            if !prepared.url_slot_set.contains(configured) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "url_param_specs entry `{configured}` has no placeholder in url_template"
                    ),
                ));
            }
        }

        let mut resolved_url = String::with_capacity(self.url_template.len().saturating_add(16));
        for chunk in &prepared.parsed_url_chunks {
            match chunk {
                UrlTemplateChunk::Literal(s) => resolved_url.push_str(s),
                UrlTemplateChunk::Slot(slot) => {
                    let spec = self.url_param_specs.get(slot.as_str()).ok_or(Error::new(
                        ErrorKind::InvalidInput,
                        format!("missing url_param_specs entry for slot `{slot}`"),
                    ))?;
                    let provided = options.url_params.get(slot.as_str()).map(String::as_str);
                    let value = spec.resolve_value(slot, provided)?;
                    resolved_url.push_str(&percent_encode_component(&value));
                }
            }
        }

        let mut url = reqwest::Url::parse(&resolved_url).map_err(into_io_error)?;
        if url.fragment().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template must not include URL fragments",
            ));
        }
        if url.query().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template must not include query parameters; use query_specs instead",
            ));
        }

        for provided in options.queries.keys() {
            if !prepared.allowed_query_slots.contains(provided) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("unknown queries key `{provided}` for endpoint"),
                ));
            }
        }

        let mut emitted_pairs = Vec::<(String, String)>::new();
        for spec in &self.query_specs {
            match spec {
                QuerySpec::Const { key, value } => {
                    validate_query_key(key)?;
                    emitted_pairs.push((key.clone(), value.clone()));
                }
                QuerySpec::Slotted {
                    key,
                    slot,
                    mode,
                    default,
                    min_bytes,
                    max_bytes,
                } => {
                    validate_query_key(key)?;
                    validate_slot_name(slot)?;
                    validate_min_max_bounds(slot, *min_bytes, *max_bytes)?;

                    let provided = options.queries.get(slot).map(String::as_str);
                    if let Some(value) = resolve_slotted_query_value(
                        slot,
                        mode.clone(),
                        default.as_deref(),
                        provided,
                        *min_bytes,
                        *max_bytes,
                    )? {
                        emitted_pairs.push((key.clone(), value));
                    }
                }
            }
        }

        if !emitted_pairs.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in emitted_pairs {
                pairs.append_pair(&key, &value);
            }
        }

        Ok(url)
    }

    /// Sends the configured HTTP request and decodes response according to endpoint body policy.
    pub(crate) async fn execute(
        &self,
        client: Arc<dyn EndpointHttpClient>,
        prepared: &PreparedHttpEndpoint,
        default_timeout_ms: Option<u64>,
        default_response_max_bytes: Option<usize>,
        options: &EndpointCallOptions,
    ) -> std::io::Result<EndpointResponse> {
        let url = self.build_url_prepared(options, prepared)?;
        let timeout_ms = self.timeout_ms.or(default_timeout_ms);
        let response_max_bytes = self.response_max_bytes.or(default_response_max_bytes);
        let supports_body = self.method.supports_request_body();
        let request_body_type = self.effective_request_body_type();

        let default_content_type =
            if supports_body && !matches!(options.body, EndpointCallBody::Absent) {
                Some(match request_body_type {
                    EndpointBodyType::Json => "application/json",
                    EndpointBodyType::Utf8 => "text/plain; charset=utf-8",
                    EndpointBodyType::Bytes => "application/octet-stream",
                })
            } else {
                None
            };

        let headers = self.build_headers_prepared(default_content_type, options, prepared)?;
        let build_request = || -> std::io::Result<EndpointHttpRequest> {
            let body = if supports_body {
                match (&request_body_type, &options.body) {
                    (_, EndpointCallBody::Absent) => EndpointHttpRequestBody::Absent,
                    (EndpointBodyType::Json, EndpointCallBody::Json(v)) => {
                        EndpointHttpRequestBody::Json(v.clone())
                    }
                    (EndpointBodyType::Json, EndpointCallBody::Utf8(s)) => {
                        EndpointHttpRequestBody::Json(Value::String(s.clone()))
                    }
                    (EndpointBodyType::Json, EndpointCallBody::Bytes(_)) => {
                        return Err(Error::new(
                            ErrorKind::InvalidInput,
                            "request_body_type `json` requires a JSON-compatible value",
                        ));
                    }
                    (EndpointBodyType::Utf8, EndpointCallBody::Utf8(s)) => {
                        EndpointHttpRequestBody::Utf8(s.clone())
                    }
                    (EndpointBodyType::Utf8, _) => {
                        return Err(Error::new(
                            ErrorKind::InvalidInput,
                            "request_body_type `utf8` requires `options.body` to be a string",
                        ));
                    }
                    (EndpointBodyType::Bytes, EndpointCallBody::Bytes(bytes)) => {
                        EndpointHttpRequestBody::Bytes(bytes.clone())
                    }
                    (EndpointBodyType::Bytes, _) => {
                        return Err(Error::new(
                            ErrorKind::InvalidInput,
                            "request_body_type `bytes` requires `options.body` to be a typed array, ArrayBuffer, or DataView",
                        ));
                    }
                }
            } else if !matches!(options.body, EndpointCallBody::Absent) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "HTTP {} endpoint does not accept a request body",
                        self.method.as_str()
                    ),
                ));
            } else {
                EndpointHttpRequestBody::Absent
            };

            Ok(EndpointHttpRequest {
                method: self.method.clone(),
                url: url.clone(),
                headers: headers.clone(),
                timeout_ms,
                response_max_bytes,
                body,
            })
        };

        let max_attempts = self.retry_policy.max_attempts;
        let mut final_response = None;
        for attempt in 1..=max_attempts {
            let req = build_request()?;
            match client.execute(req).await {
                Ok(res) => {
                    let status_code = res.status;
                    let should_retry_status = attempt < max_attempts
                        && self.retry_policy.should_retry_status(status_code);
                    if should_retry_status {
                        let delay = self.retry_policy.retry_delay_for_status(
                            status_code,
                            &res.headers,
                            attempt,
                        );
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    final_response = Some(res);
                    break;
                }
                Err(err) => {
                    if attempt < max_attempts
                        && self.retry_policy.should_retry_transport_error(&err)
                    {
                        let delay = self.retry_policy.retry_delay_for_transport(attempt);
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    return Err(err);
                }
            }
        }

        let Some(res) = final_response else {
            return Err(Error::other(
                "endpoint request attempts exhausted without terminal response",
            ));
        };

        let status_code = res.status;
        let ok = (200..=299).contains(&status_code);
        if !self.allow_non_success_status && !ok {
            return Err(Error::other(format!("HTTP status {status_code}")));
        }

        let response_headers = extract_exposed_response_headers_prepared(
            &res.headers,
            &prepared.exposed_response_allowlist,
        )?;

        if let (Some(max), Some(content_len)) = (response_max_bytes, res.content_length)
            && content_len > max as u64
        {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "response body exceeds configured max bytes ({max}): content-length is {content_len}"
                ),
            ));
        }

        let mut bytes = Vec::new();
        extend_body_with_limit(&mut bytes, &res.body, response_max_bytes)?;
        if bytes.is_empty() {
            return Ok(EndpointResponse {
                body: EndpointResponseBody::Empty,
                headers: response_headers,
                status: status_code,
                ok,
            });
        }

        let body = match self.response_body_type.clone() {
            EndpointBodyType::Json => {
                let data = serde_json::from_slice::<Value>(&bytes).map_err(into_io_error)?;
                EndpointResponseBody::Json(data)
            }
            EndpointBodyType::Utf8 => {
                let data = std::str::from_utf8(&bytes)
                    .map_err(into_io_error)?
                    .to_owned();
                EndpointResponseBody::Utf8(data)
            }
            EndpointBodyType::Bytes => EndpointResponseBody::Bytes(bytes),
        };

        Ok(EndpointResponse {
            body,
            headers: response_headers,
            status: status_code,
            ok,
        })
    }
}

fn validate_header_name_list(headers: &[String], field_name: &str) -> std::io::Result<()> {
    for header in headers {
        HeaderName::try_from(header.as_str()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid header name `{header}` in `{field_name}`: {e}"),
            )
        })?;
    }
    Ok(())
}

fn allowlisted_header_names(
    headers: &[String],
    field_name: &str,
) -> std::io::Result<HashSet<HeaderName>> {
    headers
        .iter()
        .map(|header| {
            HeaderName::try_from(header.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid header name `{header}` in `{field_name}`: {e}"),
                )
            })
        })
        .collect()
}

#[cfg(test)]
fn extract_exposed_response_headers(
    headers: &HeaderMap,
    allowlist: &[String],
) -> std::io::Result<HashMap<String, String>> {
    let allowlisted = allowlisted_header_names(allowlist, "exposed_response_headers")?;
    extract_exposed_response_headers_prepared(headers, &allowlisted)
}

fn extract_exposed_response_headers_prepared(
    headers: &HeaderMap,
    allowlisted: &HashSet<HeaderName>,
) -> std::io::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for name in allowlisted {
        let values = headers.get_all(name);
        let mut parts = Vec::new();
        for value in values {
            let text = value
                .to_str()
                .map(str::to_owned)
                .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).into_owned());
            parts.push(text);
        }
        if !parts.is_empty() {
            out.insert(name.as_str().to_ascii_lowercase(), parts.join(", "));
        }
    }
    Ok(out)
}

fn extend_body_with_limit(
    target: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: Option<usize>,
) -> std::io::Result<()> {
    if let Some(max) = max_bytes {
        let next_len = target.len().checked_add(chunk.len()).ok_or(Error::new(
            ErrorKind::InvalidData,
            "response body size overflow while enforcing max bytes limit",
        ))?;
        if next_len > max {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("response body exceeds configured max bytes ({max})"),
            ));
        }
    }
    target.extend_from_slice(chunk);
    Ok(())
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
#[path = "tests/mod.rs"]
mod tests;
