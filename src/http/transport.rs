use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
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
    Absent,
    Json(Value),
    Utf8(String),
    Bytes(Vec<u8>),
}

/// Transport request shape used by pluggable endpoint HTTP clients.
#[derive(Clone, Debug)]
pub struct EndpointHttpRequest {
    pub method: HttpMethod,
    pub url: reqwest::Url,
    pub headers: HeaderMap,
    pub timeout_ms: Option<u64>,
    pub response_max_bytes: Option<usize>,
    pub body: EndpointHttpRequestBody,
}

/// Transport response shape used by pluggable endpoint HTTP clients.
#[derive(Debug)]
pub struct EndpointHttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

/// Endpoint HTTP client abstraction configured at pool level.
///
/// Runtime contract:
/// - `execute` futures are polled on the pool worker's internal Tokio runtime.
/// - The built-in retry path in [`crate::http::HttpEndpoint::execute`] also uses Tokio timers.
/// - Implementations may therefore rely on Tokio async primitives.
pub trait EndpointHttpClient: Send + Sync + std::fmt::Debug {
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
            let mut req = client
                .request(request.method.as_reqwest_method(), request.url)
                .headers(request.headers);

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
            let headers = res.headers().clone();
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
