use http::HeaderMap;
use mechanics_config::HttpMethod;
use mechanics_http_client::{Method as HttpMethodKind, Response as HttpResponse};
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

pub(crate) trait HttpMethodHttpExt {
    fn as_http_method(&self) -> HttpMethodKind;
}

impl HttpMethodHttpExt for HttpMethod {
    fn as_http_method(&self) -> HttpMethodKind {
        match self {
            HttpMethod::Get => HttpMethodKind::GET,
            HttpMethod::Post => HttpMethodKind::POST,
            HttpMethod::Put => HttpMethodKind::PUT,
            HttpMethod::Patch => HttpMethodKind::PATCH,
            HttpMethod::Delete => HttpMethodKind::DELETE,
            HttpMethod::Head => HttpMethodKind::HEAD,
            HttpMethod::Options => HttpMethodKind::OPTIONS,
        }
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

    pub(crate) fn from_http_map(headers: &HeaderMap) -> Self {
        let mut out = Self::new();
        let mut seen_multi: HashSet<http::HeaderName> = HashSet::new();
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

/// Default endpoint HTTP client backed by [`mechanics_http_client::Client`]
/// (hyper-rustls + webpki-roots + aws-lc-rs).
#[derive(Clone, Debug)]
pub struct DefaultEndpointHttpClient {
    client: mechanics_http_client::Client,
}

impl DefaultEndpointHttpClient {
    /// Wraps a configured [`mechanics_http_client::Client`] as an endpoint transport.
    pub fn new(client: mechanics_http_client::Client) -> Self {
        Self { client }
    }
}

impl EndpointHttpClient for DefaultEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<EndpointHttpResponse>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            let mut req = client.request(request.method.as_http_method(), &request.url);
            for (name, value) in request.headers.iter() {
                req = req.header(name, value);
            }

            if let Some(timeout_ms) = request.timeout_ms {
                req = req.timeout(Duration::from_millis(timeout_ms));
            }

            match request.body {
                EndpointHttpRequestBody::Absent => {}
                EndpointHttpRequestBody::Json(v) => {
                    req = req.json(&v);
                }
                EndpointHttpRequestBody::Utf8(s) => {
                    req = req.body(s.into_bytes());
                }
                EndpointHttpRequestBody::Bytes(bytes) => {
                    req = req.body(bytes);
                }
            }

            let res: HttpResponse = req.send().await.map_err(|err| {
                if err.is_timeout() {
                    Error::new(ErrorKind::TimedOut, err)
                } else {
                    into_io_error(err)
                }
            })?;
            let status = res.status().as_u16();
            let content_length = res.content_length();
            let headers = EndpointHttpHeaders::from_http_map(res.headers());

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

            let body = match request.response_max_bytes {
                Some(max) => match res.bytes_with_cap(max).await {
                    Ok(bytes) => bytes.to_vec(),
                    Err(mechanics_http_client::Error::BodyTooLarge { limit, .. }) => {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            format!("response body exceeds configured max bytes ({limit})"),
                        ));
                    }
                    Err(err) => return Err(into_io_error(err)),
                },
                None => res.bytes().await.map_err(into_io_error)?.to_vec(),
            };

            Ok(EndpointHttpResponse {
                status,
                headers,
                content_length,
                body,
            })
        })
    }
}
