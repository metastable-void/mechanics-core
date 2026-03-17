use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    time::Duration,
};

/// Normalizes arbitrary error types into `std::io::Error` for shared propagation paths.
pub(crate) fn into_io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}

/// HTTP endpoint configuration used by the runtime-provided JS helper.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
pub struct HttpEndpoint {
    url: String,
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
    pub fn new(url: &str, headers: HashMap<String, String>) -> Self {
        Self {
            url: url.to_owned(),
            headers,
            timeout_ms: None,
            allow_non_success_status: false,
        }
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

    fn build_headers(&self) -> std::io::Result<HeaderMap> {
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
        let user_agent = HeaderValue::try_from(Self::USER_AGENT).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid default User-Agent header: {e}"),
            )
        })?;
        headers.insert(USER_AGENT, user_agent);
        let content_type = HeaderValue::try_from("application/json").map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid default Content-Type header: {e}"),
            )
        })?;
        headers.insert(CONTENT_TYPE, content_type);
        Ok(headers)
    }

    /// Sends a JSON POST request and deserializes the JSON response into `Res`.
    pub(crate) async fn post<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        client: reqwest::Client,
        default_timeout_ms: Option<u64>,
        req_data: &Req,
    ) -> std::io::Result<Res> {
        let json = serde_json::to_string(req_data).map_err(into_io_error)?;
        let url = reqwest::Url::parse(&self.url).map_err(into_io_error)?;
        let headers = self.build_headers()?;
        let timeout_ms = self.timeout_ms.or(default_timeout_ms);
        let mut req = client.post(url).headers(headers).body(json);
        if let Some(timeout_ms) = timeout_ms {
            req = req.timeout(Duration::from_millis(timeout_ms));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_headers_rejects_invalid_name() {
        let mut headers = HashMap::new();
        headers.insert("bad header".to_owned(), "ok".to_owned());
        let endpoint = HttpEndpoint::new("https://example.com", headers);
        let err = endpoint
            .build_headers()
            .expect_err("invalid header name must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("invalid header name"));
    }

    #[test]
    fn build_headers_rejects_invalid_value() {
        let mut headers = HashMap::new();
        headers.insert("x-test".to_owned(), "bad\r\nvalue".to_owned());
        let endpoint = HttpEndpoint::new("https://example.com", headers);
        let err = endpoint
            .build_headers()
            .expect_err("invalid header value must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("invalid header value"));
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
