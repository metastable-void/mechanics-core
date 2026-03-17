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

    fn supports_json_body(&self) -> bool {
        matches!(self, Self::Post | Self::Put)
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

    /// Sets a per-endpoint timeout in milliseconds.
    ///
    /// If this is `Some`, it overrides the pool default endpoint timeout.
    /// If this is `None`, the pool default timeout is used.
    pub fn with_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Allows non-success (non-2xx) HTTP status responses to proceed to JSON parsing.
    ///
    /// Defaults to `false`, which treats non-success statuses as request errors.
    pub fn with_allow_non_success_status(mut self, allow: bool) -> Self {
        self.allow_non_success_status = allow;
        self
    }

    fn build_headers(&self, include_json_content_type: bool) -> std::io::Result<HeaderMap> {
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

        if include_json_content_type && !headers.contains_key(CONTENT_TYPE) {
            let content_type = HeaderValue::try_from("application/json").map_err(|e| {
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

    /// Sends the configured HTTP request and deserializes the JSON response into `Res`.
    pub(crate) async fn execute<Res: serde::de::DeserializeOwned>(
        &self,
        client: reqwest::Client,
        default_timeout_ms: Option<u64>,
        options: &EndpointCallOptions,
    ) -> std::io::Result<Res> {
        let url = self.build_url(options)?;
        let timeout_ms = self.timeout_ms.or(default_timeout_ms);
        let supports_body = self.method.supports_json_body();

        let headers = self.build_headers(supports_body)?;
        let mut req = client
            .request(self.method.as_reqwest_method(), url)
            .headers(headers);

        if let Some(timeout_ms) = timeout_ms {
            req = req.timeout(Duration::from_millis(timeout_ms));
        }

        if supports_body {
            let body = options.body.clone().unwrap_or(Value::Null);
            req = req.json(&body);
        } else if options.body.as_ref().is_some_and(|v| !v.is_null()) {
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
        let res: Res = res.json().await.map_err(into_io_error)?;
        Ok(res)
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
    pub(crate) body: Option<Value>,
}

pub(crate) fn parse_endpoint_call_options(
    value: Option<Value>,
) -> std::io::Result<EndpointCallOptions> {
    match value {
        None | Some(Value::Null) => Ok(EndpointCallOptions::default()),
        Some(value @ Value::Object(_)) => serde_json::from_value(value).map_err(into_io_error),
        Some(_) => Err(Error::new(
            ErrorKind::InvalidInput,
            "endpoint options must be an object or null/undefined",
        )),
    }
}

/// Serializable runtime data injected into the JS context.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
pub struct MechanicsConfig {
    pub(crate) endpoints: HashMap<String, HttpEndpoint>,
}

impl MechanicsConfig {
    /// Builds runtime state from endpoint definitions.
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
            .build_headers(true)
            .expect_err("invalid header name must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("invalid header name"));
    }

    #[test]
    fn build_headers_rejects_invalid_value() {
        let mut headers = HashMap::new();
        headers.insert("x-test".to_owned(), "bad\r\nvalue".to_owned());
        let endpoint = HttpEndpoint::new(HttpMethod::Post, "https://example.com", headers);
        let err = endpoint
            .build_headers(true)
            .expect_err("invalid header value must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("invalid header value"));
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
    fn url_params_must_match_template_slots_exactly() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::new());

        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("missing slot spec should fail");
        assert!(err.to_string().contains("missing url_param_specs entry"));

        let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
            .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));

        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("unused slot spec should fail");
        assert!(
            err.to_string()
                .contains("url_param_specs entry `id` has no placeholder")
        );
    }

    #[test]
    fn url_param_default_fills_missing_or_empty_values() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([(
                    "id".to_owned(),
                    UrlParamSpec {
                        default: Some("fallback".to_owned()),
                        min_bytes: None,
                        max_bytes: None,
                    },
                )]));

        let url = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect("missing should use default");
        assert_eq!(url.as_str(), "https://example.com/fallback");

        let mut options = EndpointCallOptions::default();
        options.url_params.insert("id".to_owned(), "".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("empty should use default");
        assert_eq!(url.as_str(), "https://example.com/fallback");
    }

    #[test]
    fn unknown_query_slot_is_rejected() {
        let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
            .with_query_specs(vec![QuerySpec::Slotted {
                key: "page".to_owned(),
                slot: "page".to_owned(),
                mode: SlottedQueryMode::Optional,
                default: None,
                min_bytes: None,
                max_bytes: None,
            }]);

        let mut options = EndpointCallOptions::default();
        options.queries.insert("other".to_owned(), "1".to_owned());

        let err = endpoint
            .build_url(&options)
            .expect_err("unknown query slot should fail");
        assert!(err.to_string().contains("unknown queries key"));
    }

    #[test]
    fn optional_query_mode_omits_empty_values() {
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
            .expect("optional empty should be omitted");
        assert_eq!(url.as_str(), "https://example.com/");
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
    fn required_query_mode_rejects_missing_or_empty() {
        let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
            .with_query_specs(vec![QuerySpec::Slotted {
                key: "q".to_owned(),
                slot: "q".to_owned(),
                mode: SlottedQueryMode::Required,
                default: None,
                min_bytes: None,
                max_bytes: None,
            }]);

        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("required mode should reject missing");
        assert!(err.to_string().contains("required query slot"));

        let mut options = EndpointCallOptions::default();
        options.queries.insert("q".to_owned(), "".to_owned());

        let err = endpoint
            .build_url(&options)
            .expect_err("required mode should reject empty");
        assert!(err.to_string().contains("required query slot"));
    }

    #[test]
    fn required_allow_empty_query_mode_accepts_empty_but_not_missing() {
        let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
            .with_query_specs(vec![QuerySpec::Slotted {
                key: "q".to_owned(),
                slot: "q".to_owned(),
                mode: SlottedQueryMode::RequiredAllowEmpty,
                default: None,
                min_bytes: None,
                max_bytes: None,
            }]);

        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("required_allow_empty should reject missing");
        assert!(err.to_string().contains("required query slot"));

        let mut options = EndpointCallOptions::default();
        options.queries.insert("q".to_owned(), "".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("required_allow_empty should accept empty");
        assert_eq!(url.query(), Some("q="));
    }

    #[test]
    fn optional_query_mode_treats_empty_as_omitted_then_applies_default() {
        let endpoint = HttpEndpoint::new(HttpMethod::Get, "https://example.com", HashMap::new())
            .with_query_specs(vec![QuerySpec::Slotted {
                key: "q".to_owned(),
                slot: "q".to_owned(),
                mode: SlottedQueryMode::Optional,
                default: Some("fallback".to_owned()),
                min_bytes: None,
                max_bytes: None,
            }]);

        let mut options = EndpointCallOptions::default();
        options.queries.insert("q".to_owned(), "".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("optional empty should be treated as omitted/defaulted");
        assert_eq!(url.query(), Some("q=fallback"));
    }

    #[test]
    fn url_param_without_default_allows_empty_when_missing() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([(
                    "id".to_owned(),
                    UrlParamSpec {
                        default: None,
                        min_bytes: None,
                        max_bytes: None,
                    },
                )]));

        let url = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect("missing url param without default should resolve to empty");
        assert_eq!(url.as_str(), "https://example.com/");
    }

    #[test]
    fn unknown_url_param_key_is_rejected() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));

        let mut options = EndpointCallOptions::default();
        options
            .url_params
            .insert("other".to_owned(), "x".to_owned());
        let err = endpoint
            .build_url(&options)
            .expect_err("unknown url param key must fail");
        assert!(err.to_string().contains("unknown urlParams key"));
    }

    #[test]
    fn byte_length_validation_uses_utf8_bytes() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([(
                    "id".to_owned(),
                    UrlParamSpec {
                        default: None,
                        min_bytes: Some(5),
                        max_bytes: Some(5),
                    },
                )]));

        let mut options = EndpointCallOptions::default();
        options.url_params.insert("id".to_owned(), "あ".to_owned());
        let err = endpoint
            .build_url(&options)
            .expect_err("3-byte UTF-8 char should fail min=5");
        assert!(err.to_string().contains("min_bytes"));

        options.url_params.insert("id".to_owned(), "ééé".to_owned());
        let err = endpoint
            .build_url(&options)
            .expect_err("6-byte UTF-8 value should fail max=5");
        assert!(err.to_string().contains("max_bytes"));

        options
            .url_params
            .insert("id".to_owned(), "abあ".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("2 ASCII + 1 hiragana should be exactly 5 bytes");
        assert!(url.as_str().starts_with("https://example.com/"));
    }

    #[test]
    fn invalid_byte_bounds_are_rejected() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([(
                    "id".to_owned(),
                    UrlParamSpec {
                        default: None,
                        min_bytes: Some(10),
                        max_bytes: Some(1),
                    },
                )]));

        let mut options = EndpointCallOptions::default();
        options.url_params.insert("id".to_owned(), "abc".to_owned());
        let err = endpoint
            .build_url(&options)
            .expect_err("min > max should fail");
        assert!(err.to_string().contains("min_bytes"));
    }

    #[test]
    fn endpoint_deserializes_from_snake_case_json_shape() {
        let endpoint: HttpEndpoint = serde_json::from_value(json!({
            "method": "put",
            "url_template": "https://example.com/users/{user_id}",
            "url_param_specs": {
                "user_id": {
                    "default": "guest",
                    "min_bytes": 1,
                    "max_bytes": 64
                }
            },
            "query_specs": [
                { "type": "const", "key": "v", "value": "1" },
                {
                    "type": "slotted",
                    "key": "filter",
                    "slot": "filter",
                    "mode": "optional_allow_empty",
                    "default": "all",
                    "min_bytes": 0,
                    "max_bytes": 32
                }
            ],
            "headers": {
                "x-test": "ok"
            },
            "timeout_ms": 2500,
            "allow_non_success_status": true
        }))
        .expect("snake_case endpoint config should deserialize");

        let mut options = EndpointCallOptions::default();
        options.queries.insert("filter".to_owned(), "".to_owned());
        let url = endpoint
            .build_url(&options)
            .expect("deserialized endpoint should build URL");
        assert_eq!(url.as_str(), "https://example.com/users/guest?v=1&filter=");
    }

    #[test]
    fn url_template_rejects_unmatched_braces() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id", HashMap::new())
                .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));
        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("unmatched opening brace should fail");
        assert!(err.to_string().contains("unmatched `{`"));

        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/id}", HashMap::new());
        let err = endpoint
            .build_url(&EndpointCallOptions::default())
            .expect_err("unmatched closing brace should fail");
        assert!(err.to_string().contains("unmatched `}`"));
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
        assert_eq!(parsed.body, Some(json!({"ok": true})));
    }

    #[test]
    fn parse_endpoint_call_options_rejects_non_string_slot_values() {
        let err = parse_endpoint_call_options(Some(json!({
            "urlParams": {"x": 1}
        })))
        .expect_err("non-string urlParam value should fail");
        assert!(err.to_string().contains("invalid type"));

        let err = parse_endpoint_call_options(Some(json!({
            "queries": {"q": 1}
        })))
        .expect_err("non-string query value should fail");
        assert!(err.to_string().contains("invalid type"));
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
    fn percent_encoding_is_applied_to_url_slot_values() {
        let endpoint =
            HttpEndpoint::new(HttpMethod::Get, "https://example.com/{id}", HashMap::new())
                .with_url_param_specs(HashMap::from([("id".to_owned(), UrlParamSpec::default())]));

        let mut options = EndpointCallOptions::default();
        options
            .url_params
            .insert("id".to_owned(), "x y/あ".to_owned());

        let url = endpoint
            .build_url(&options)
            .expect("url should be resolved and encoded");
        assert_eq!(url.as_str(), "https://example.com/x%20y%2F%E3%81%82");
    }
}
