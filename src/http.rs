use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{Error, ErrorKind},
    time::Duration,
};

/// Normalizes arbitrary error types into `std::io::Error` for shared propagation paths.
pub(crate) fn into_io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}

/// Supported HTTP methods for runtime-managed endpoint calls.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    /// HTTP `GET`.
    Get,
    /// HTTP `POST`.
    Post,
    /// HTTP `PUT`.
    Put,
    /// HTTP `DELETE`.
    Delete,
}

impl HttpMethod {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }

    fn as_reqwest_method(&self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Delete => reqwest::Method::DELETE,
        }
    }

    fn supports_request_body(&self) -> bool {
        matches!(self, Self::Post | Self::Put)
    }
}

/// Endpoint body encoding/decoding mode.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EndpointBodyType {
    /// JSON payload (`application/json`).
    Json,
    /// UTF-8 string payload (`text/plain; charset=utf-8`).
    Utf8,
    /// Raw bytes payload (`application/octet-stream`).
    Bytes,
}

impl Default for EndpointBodyType {
    fn default() -> Self {
        Self::Json
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
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SlottedQueryMode {
    /// Slot must resolve and must be non-empty.
    Required,
    /// Slot must resolve and may be empty.
    RequiredAllowEmpty,
    /// Missing/empty is treated as omitted.
    Optional,
    /// Missing is omitted; if provided, empty is emitted.
    OptionalAllowEmpty,
}

impl Default for SlottedQueryMode {
    fn default() -> Self {
        Self::Required
    }
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
    request_body_type: Option<EndpointBodyType>,
    #[serde(default)]
    response_body_type: EndpointBodyType,
    timeout_ms: Option<u64>,
    #[serde(default)]
    allow_non_success_status: bool,
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
            request_body_type: None,
            response_body_type: EndpointBodyType::Json,
            timeout_ms: None,
            allow_non_success_status: false,
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

    /// Sets response body decoding mode.
    ///
    /// Defaults to JSON.
    pub fn with_response_body_type(mut self, body_type: EndpointBodyType) -> Self {
        self.response_body_type = body_type;
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

    fn effective_request_body_type(&self) -> EndpointBodyType {
        self.request_body_type
            .clone()
            .unwrap_or(EndpointBodyType::Json)
    }

    fn build_headers(&self, default_content_type: Option<&str>) -> std::io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
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

        if !headers.contains_key(USER_AGENT) {
            let user_agent = HeaderValue::try_from(Self::USER_AGENT).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid default User-Agent header: {e}"),
                )
            })?;
            headers.insert(USER_AGENT, user_agent);
        }

        if let Some(default_content_type) = default_content_type
            && !headers.contains_key(CONTENT_TYPE)
        {
            let content_type = HeaderValue::try_from(default_content_type).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid default Content-Type header: {e}"),
                )
            })?;
            headers.insert(CONTENT_TYPE, content_type);
        }

        Ok(headers)
    }

    fn build_url(&self, options: &EndpointCallOptions) -> std::io::Result<reqwest::Url> {
        let (chunks, slot_names) = parse_url_template(&self.url_template)?;
        let slot_set: HashSet<&str> = slot_names.iter().map(String::as_str).collect();

        for provided in options.url_params.keys() {
            if !slot_set.contains(provided.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "unknown urlParams key `{provided}` for endpoint template `{}`",
                        self.url_template
                    ),
                ));
            }
        }

        for slot in &slot_names {
            if !self.url_param_specs.contains_key(slot) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("missing url_param_specs entry for slot `{slot}`"),
                ));
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

        let mut resolved_url = String::with_capacity(self.url_template.len() + 16);
        for chunk in chunks {
            match chunk {
                UrlTemplateChunk::Literal(s) => resolved_url.push_str(&s),
                UrlTemplateChunk::Slot(slot) => {
                    let spec = self.url_param_specs.get(&slot).ok_or(Error::new(
                        ErrorKind::InvalidInput,
                        format!("missing url_param_specs entry for slot `{slot}`"),
                    ))?;
                    let provided = options.url_params.get(&slot).map(String::as_str);
                    let value = spec.resolve_value(&slot, provided)?;
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

        let allowed_query_slots = self.allowed_query_slots();
        for provided in options.queries.keys() {
            if !allowed_query_slots.contains(provided.as_str()) {
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

    fn allowed_query_slots(&self) -> HashSet<&str> {
        let mut slots = HashSet::new();
        for spec in &self.query_specs {
            if let QuerySpec::Slotted { slot, .. } = spec {
                slots.insert(slot.as_str());
            }
        }
        slots
    }

    /// Sends the configured HTTP request and decodes response according to endpoint body policy.
    pub(crate) async fn execute(
        &self,
        client: reqwest::Client,
        default_timeout_ms: Option<u64>,
        options: &EndpointCallOptions,
    ) -> std::io::Result<EndpointResponseBody> {
        let url = self.build_url(options)?;
        let timeout_ms = self.timeout_ms.or(default_timeout_ms);
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

        let headers = self.build_headers(default_content_type)?;
        let mut req = client
            .request(self.method.as_reqwest_method(), url)
            .headers(headers);

        if let Some(timeout_ms) = timeout_ms {
            req = req.timeout(Duration::from_millis(timeout_ms));
        }

        if supports_body {
            match (&request_body_type, &options.body) {
                (_, EndpointCallBody::Absent) => {}
                (EndpointBodyType::Json, EndpointCallBody::Json(v)) => {
                    req = req.json(v);
                }
                (EndpointBodyType::Json, EndpointCallBody::Utf8(s)) => {
                    req = req.json(s);
                }
                (EndpointBodyType::Json, EndpointCallBody::Bytes(_)) => {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "request_body_type `json` requires a JSON-compatible value",
                    ));
                }
                (EndpointBodyType::Utf8, EndpointCallBody::Utf8(s)) => {
                    req = req.body(s.clone());
                }
                (EndpointBodyType::Utf8, _) => {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "request_body_type `utf8` requires `options.body` to be a string",
                    ));
                }
                (EndpointBodyType::Bytes, EndpointCallBody::Bytes(bytes)) => {
                    req = req.body(bytes.clone());
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
        }

        let res = req.send().await.map_err(into_io_error)?;
        let res = if self.allow_non_success_status {
            res
        } else {
            res.error_for_status().map_err(into_io_error)?
        };

        let bytes = res.bytes().await.map_err(into_io_error)?;
        if bytes.is_empty() {
            return Ok(EndpointResponseBody::Empty);
        }

        match self.response_body_type.clone() {
            EndpointBodyType::Json => {
                let data = serde_json::from_slice::<Value>(&bytes).map_err(into_io_error)?;
                Ok(EndpointResponseBody::Json(data))
            }
            EndpointBodyType::Utf8 => {
                let data = std::str::from_utf8(&bytes)
                    .map_err(into_io_error)?
                    .to_owned();
                Ok(EndpointResponseBody::Utf8(data))
            }
            EndpointBodyType::Bytes => Ok(EndpointResponseBody::Bytes(bytes.to_vec())),
        }
    }
}

#[derive(Debug)]
enum UrlTemplateChunk {
    Literal(String),
    Slot(String),
}

fn parse_url_template(template: &str) -> std::io::Result<(Vec<UrlTemplateChunk>, Vec<String>)> {
    let mut chunks = Vec::new();
    let mut slots = Vec::new();
    let mut seen_slots = HashSet::new();

    let mut cursor = 0usize;
    loop {
        let Some(open_rel) = template[cursor..].find('{') else {
            break;
        };

        let open = cursor + open_rel;
        if open > cursor {
            chunks.push(UrlTemplateChunk::Literal(template[cursor..open].to_owned()));
        }

        let Some(close_rel) = template[open + 1..].find('}') else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains unmatched `{`",
            ));
        };

        let close = open + 1 + close_rel;
        let slot = &template[open + 1..close];
        if slot.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains empty `{}` placeholder",
            ));
        }
        if slot.contains('{') {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains nested `{` in placeholder",
            ));
        }
        validate_slot_name(slot)?;

        if !seen_slots.insert(slot.to_owned()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("url_template contains duplicate slot `{slot}`"),
            ));
        }

        let slot_owned = slot.to_owned();
        slots.push(slot_owned.clone());
        chunks.push(UrlTemplateChunk::Slot(slot_owned));
        cursor = close + 1;
    }

    if let Some(stray) = template[cursor..].find('}') {
        let idx = cursor + stray;
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("url_template contains unmatched `}}` at byte index {idx}"),
        ));
    }

    if cursor < template.len() {
        chunks.push(UrlTemplateChunk::Literal(template[cursor..].to_owned()));
    }

    Ok((chunks, slots))
}

fn resolve_slotted_query_value(
    slot: &str,
    mode: SlottedQueryMode,
    default: Option<&str>,
    provided: Option<&str>,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<Option<String>> {
    let value = match mode {
        SlottedQueryMode::Required => {
            let candidate = match provided {
                Some(v) if !v.is_empty() => Some(v),
                Some(_) | None => default.filter(|v| !v.is_empty()),
            };
            let Some(candidate) = candidate else {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("required query slot `{slot}` is missing or empty"),
                ));
            };
            Some(candidate.to_owned())
        }
        SlottedQueryMode::RequiredAllowEmpty => {
            let candidate = match provided {
                Some(v) => Some(v),
                None => default,
            };
            let Some(candidate) = candidate else {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("required query slot `{slot}` is missing"),
                ));
            };
            Some(candidate.to_owned())
        }
        SlottedQueryMode::Optional => match provided {
            Some(v) if !v.is_empty() => Some(v.to_owned()),
            Some(_) | None => default.filter(|v| !v.is_empty()).map(ToOwned::to_owned),
        },
        SlottedQueryMode::OptionalAllowEmpty => match provided {
            Some(v) => Some(v.to_owned()),
            None => default.map(ToOwned::to_owned),
        },
    };

    if let Some(ref value) = value {
        validate_byte_len(slot, value, min_bytes, max_bytes)?;
    }

    Ok(value)
}

fn validate_slot_name(slot: &str) -> std::io::Result<()> {
    if slot.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "slot name must not be empty",
        ));
    }

    if !slot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "slot name `{slot}` is invalid: only ASCII letters, digits, and `_` are allowed"
            ),
        ));
    }

    Ok(())
}

fn validate_query_key(key: &str) -> std::io::Result<()> {
    if key.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "query key must not be empty",
        ));
    }
    Ok(())
}

fn validate_min_max_bounds(
    slot: &str,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<()> {
    if let (Some(min), Some(max)) = (min_bytes, max_bytes)
        && min > max
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` has invalid byte bounds: min_bytes ({min}) > max_bytes ({max})"),
        ));
    }
    Ok(())
}

fn validate_byte_len(
    slot: &str,
    value: &str,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<()> {
    let len = value.len();
    if let Some(min) = min_bytes
        && len < min
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` is too short: {len} bytes < min_bytes ({min})"),
        ));
    }
    if let Some(max) = max_bytes
        && len > max
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` is too long: {len} bytes > max_bytes ({max})"),
        ));
    }
    Ok(())
}

fn percent_encode_component(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        let is_unreserved = matches!(
            b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        );
        if is_unreserved {
            out.push(char::from(b));
            continue;
        }
        out.push('%');
        out.push(char::from(HEX[(b >> 4) as usize]));
        out.push(char::from(HEX[(b & 0x0F) as usize]));
    }
    out
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct EndpointCallOptions {
    pub(crate) url_params: HashMap<String, String>,
    pub(crate) queries: HashMap<String, String>,
    #[serde(skip)]
    pub(crate) body: EndpointCallBody,
}

#[cfg(test)]
pub(crate) fn parse_endpoint_call_options(
    value: Option<Value>,
) -> std::io::Result<EndpointCallOptions> {
    match value {
        None | Some(Value::Null) => Ok(EndpointCallOptions::default()),
        Some(Value::Object(mut map)) => {
            let body = match map.remove("body") {
                None | Some(Value::Null) => EndpointCallBody::Absent,
                Some(Value::String(s)) => EndpointCallBody::Utf8(s),
                Some(other) => EndpointCallBody::Json(other),
            };
            let mut parsed: EndpointCallOptions =
                serde_json::from_value(Value::Object(map)).map_err(into_io_error)?;
            parsed.body = body;
            Ok(parsed)
        }
        Some(_) => Err(Error::new(
            ErrorKind::InvalidInput,
            "endpoint options must be an object or null/undefined",
        )),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) enum EndpointCallBody {
    #[default]
    Absent,
    Json(Value),
    Utf8(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone)]
pub(crate) enum EndpointResponseBody {
    Json(Value),
    Utf8(String),
    Bytes(Vec<u8>),
    Empty,
}

/// Serializable runtime data injected into the JS context.
///
/// This is intended to be supplied per job so workers remain stateless and horizontally scalable.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
pub struct MechanicsConfig {
    pub(crate) endpoints: HashMap<String, HttpEndpoint>,
}

impl MechanicsConfig {
    /// Builds runtime state from endpoint definitions.
    ///
    /// Provide the complete endpoint map needed by a job; workers do not maintain shared endpoint
    /// cache state across jobs.
    pub fn new(endpoints: HashMap<String, HttpEndpoint>) -> Self {
        Self { endpoints }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn parse_endpoint_call_options_requires_object_or_nullish() {
        let err = parse_endpoint_call_options(Some(json!(1))).expect_err("number must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);

        let parsed = parse_endpoint_call_options(Some(json!({
            "urlParams": {"x": "1"},
            "queries": {"q": "v"},
            "body": {"ok": true}
        })))
        .expect("object should parse");

        assert_eq!(parsed.url_params.get("x"), Some(&"1".to_owned()));
        assert_eq!(parsed.queries.get("q"), Some(&"v".to_owned()));
        match parsed.body {
            EndpointCallBody::Json(v) => assert_eq!(v, json!({"ok": true})),
            other => panic!("unexpected body variant: {other:?}"),
        }
    }

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
    fn url_template_rejects_built_in_query_string() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com?a=1", HashMap::new());
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
}
