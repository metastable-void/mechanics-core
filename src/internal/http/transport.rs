use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    future::Future,
    io::{Error, ErrorKind},
    pin::Pin,
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
    /// HTTP `PATCH`.
    Patch,
    /// HTTP `DELETE`.
    Delete,
    /// HTTP `HEAD`.
    Head,
    /// HTTP `OPTIONS`.
    Options,
}

impl HttpMethod {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    pub(crate) fn as_reqwest_method(&self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
            Self::Head => reqwest::Method::HEAD,
            Self::Options => reqwest::Method::OPTIONS,
        }
    }

    pub(crate) fn supports_request_body(&self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }
}

/// Request payload used by pluggable endpoint HTTP clients.
#[derive(Clone, Debug)]
pub enum EndpointHttpRequestBody {
    /// No request body is sent.
    Absent,
    /// JSON request body payload.
    Json(Value),
    /// UTF-8 text request body payload.
    Utf8(String),
    /// Raw binary request body payload.
    Bytes(Vec<u8>),
}

/// Transport-neutral header collection used by endpoint HTTP client abstractions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointHttpHeaders {
    entries: Vec<(String, String)>,
}

impl EndpointHttpHeaders {
    /// Creates an empty header collection.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Appends one header entry.
    ///
    /// Header name/value validation is deferred until transport execution.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.entries.push((name.into(), value.into()));
        self
    }

    /// Iterates over all header entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Iterates over values for a case-insensitive header name match.
    pub fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.entries
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn from_reqwest(headers: &reqwest::header::HeaderMap) -> Self {
        let mut out = Self::new();
        let mut seen_multi: HashSet<reqwest::header::HeaderName> = HashSet::new();
        for name in headers.keys() {
            let name = name.clone();
            if seen_multi.insert(name.clone()) {
                for entry in headers.get_all(&name) {
                    let text = entry
                        .to_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|_| String::from_utf8_lossy(entry.as_bytes()).into_owned());
                    out.insert(name.as_str().to_owned(), text);
                }
            }
        }
        out
    }

    pub(crate) fn to_reqwest(&self) -> std::io::Result<reqwest::header::HeaderMap> {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in &self.entries {
            let header_name =
                reqwest::header::HeaderName::try_from(name.as_str()).map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidInput,
                        format!("invalid transport request header name `{name}`: {e}"),
                    )
                })?;
            let header_value =
                reqwest::header::HeaderValue::try_from(value.as_str()).map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidInput,
                        format!("invalid transport request header value for `{name}`: {e}"),
                    )
                })?;
            map.append(header_name, header_value);
        }
        Ok(map)
    }
}

/// Transport request shape used by pluggable endpoint HTTP clients.
#[derive(Clone, Debug)]
pub struct EndpointHttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Absolute URL.
    pub url: String,
    /// Request headers.
    pub headers: EndpointHttpHeaders,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Optional response-body limit in bytes.
    pub response_max_bytes: Option<usize>,
    /// Request body payload.
    pub body: EndpointHttpRequestBody,
}

/// Transport response shape used by pluggable endpoint HTTP clients.
#[derive(Debug)]
pub struct EndpointHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: EndpointHttpHeaders,
    /// Content-Length value when known by transport.
    pub content_length: Option<u64>,
    /// Full response body bytes.
    pub body: Vec<u8>,
}

/// Endpoint HTTP client abstraction configured at pool level.
///
/// Runtime contract:
/// - `execute` futures are polled on the pool worker's internal Tokio runtime.
/// - The built-in endpoint retry path also uses Tokio timers.
/// - Implementations may therefore rely on Tokio async primitives.
pub trait EndpointHttpClient: Send + Sync + std::fmt::Debug {
    /// Executes one transport request and returns a transport response.
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<EndpointHttpResponse>> + Send>>;
}

/// Default endpoint HTTP client backed by `reqwest`.
#[derive(Clone, Debug)]
pub struct ReqwestEndpointHttpClient {
    client: reqwest::Client,
}

impl ReqwestEndpointHttpClient {
    /// Wraps a configured reqwest client as an endpoint transport.
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl EndpointHttpClient for ReqwestEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<EndpointHttpResponse>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            let url = reqwest::Url::parse(&request.url).map_err(into_io_error)?;
            let headers = request.headers.to_reqwest()?;
            let mut req = client
                .request(request.method.as_reqwest_method(), url)
                .headers(headers);

            if let Some(timeout_ms) = request.timeout_ms {
                req = req.timeout(Duration::from_millis(timeout_ms));
            }

            match request.body {
                EndpointHttpRequestBody::Absent => {}
                EndpointHttpRequestBody::Json(v) => {
                    req = req.json(&v);
                }
                EndpointHttpRequestBody::Utf8(s) => {
                    req = req.body(s);
                }
                EndpointHttpRequestBody::Bytes(bytes) => {
                    req = req.body(bytes);
                }
            }

            let res = req.send().await.map_err(|err| {
                if err.is_timeout() {
                    Error::new(ErrorKind::TimedOut, err)
                } else {
                    into_io_error(err)
                }
            })?;
            let status = res.status().as_u16();
            let content_length = res.content_length();
            let headers = EndpointHttpHeaders::from_reqwest(res.headers());
            if let (Some(max), Some(len)) = (request.response_max_bytes, content_length)
                && len > max as u64
            {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "response body exceeds configured max bytes ({max}): content-length is {len}"
                    ),
                ));
            }

            let mut body = Vec::new();
            let mut res = res;
            while let Some(chunk) = res.chunk().await.map_err(into_io_error)? {
                if let Some(max) = request.response_max_bytes {
                    let next_len = body.len().checked_add(chunk.len()).ok_or(Error::new(
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
                body.extend_from_slice(&chunk);
            }
            Ok(EndpointHttpResponse {
                status,
                headers,
                content_length,
                body,
            })
        })
    }
}
