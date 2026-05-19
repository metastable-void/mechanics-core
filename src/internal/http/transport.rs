use http::HeaderMap;
use mechanics_config::{EndpointRetryPolicy, HttpMethod};
use mechanics_http_client::{Method as HttpMethodKind, Response as HttpResponse};
use serde_json::Value;
use std::{
    collections::HashSet,
    future::Future,
    io::{Error, ErrorKind},
    pin::Pin,
    time::{Duration, Instant},
};

/// Normalizes arbitrary error types into `std::io::Error` for shared propagation paths.
pub(crate) fn into_io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}

/// Typed transport error surfaced by [`EndpointHttpClient::execute`].
///
/// The variant **is** the retryability class — `Network` and
/// `Timeout` are network-state errors that retries can plausibly
/// recover from, the others are deterministic local conditions
/// (request shape wrong, response too large, decode failure) where
/// retrying the same request rebuilt against the same upstream
/// would fail the same way and just burn budget.
///
/// Use [`EndpointTransportError::is_retryable_per`] to consult the
/// caller-configured [`EndpointRetryPolicy`] rather than discriminating
/// on `std::io::ErrorKind` (too coarse — `InvalidData` covers both
/// "TCP-level corruption" and "operator cap violation").
#[derive(Debug)]
pub enum EndpointTransportError {
    /// Retryable: TCP refused/reset/aborted, DNS resolution failure,
    /// mid-flight network drop, hyper connect error, etc. Carries
    /// the original `io::Error` for diagnostics.
    Network(std::io::Error),
    /// Retryable per [`EndpointRetryPolicy::retry_on_timeout`]: the
    /// request deadline fired before headers (or a body frame)
    /// arrived.
    Timeout,
    /// Terminal: response body exceeded the configured cap. Retries
    /// would rebuild the same request against the same upstream and
    /// fail the same way.
    BodyTooLarge {
        /// Operator-configured cap, in bytes.
        limit: usize,
        /// Reported / observed body size; `None` if streaming and the
        /// limit was hit mid-stream without a content-length.
        seen: Option<u64>,
    },
    /// Terminal: caller-supplied request shape is invalid (malformed
    /// URL, illegal header name/value, body type mismatch, etc.).
    InvalidRequest(String),
    /// Terminal: response could not be decoded (body bytes-to-text,
    /// JSON parse, unsupported content-encoding).
    Decode(String),
    /// Conservative-not-retryable catch-all for transport failures
    /// that don't fit the typed variants. Treated as terminal by the
    /// default retry policy so a never-seen-before error class does
    /// not silently burn retry budget.
    Other(String),
}

impl std::fmt::Display for EndpointTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "{e}"),
            Self::Timeout => f.write_str("request timed out"),
            Self::BodyTooLarge { limit, seen } => match seen {
                Some(n) => write!(
                    f,
                    "response body exceeds configured max bytes ({limit}): content-length is {n}"
                ),
                None => write!(f, "response body exceeds configured max bytes ({limit})"),
            },
            Self::InvalidRequest(m) => write!(f, "{m}"),
            Self::Decode(m) => write!(f, "{m}"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for EndpointTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(e) => Some(e),
            _ => None,
        }
    }
}

impl EndpointTransportError {
    /// True iff the configured retry policy says this class is
    /// retryable. Encodes the structural contract: only `Network`
    /// and `Timeout` are *ever* retryable, and each is gated on its
    /// own policy switch. Other classes are terminal.
    pub fn is_retryable_per(&self, policy: &EndpointRetryPolicy) -> bool {
        match self {
            Self::Network(_) => policy.retry_on_io_errors,
            Self::Timeout => policy.retry_on_timeout,
            Self::BodyTooLarge { .. }
            | Self::InvalidRequest(_)
            | Self::Decode(_)
            | Self::Other(_) => false,
        }
    }

    /// Convert back to `std::io::Error` at the function boundary. The
    /// `ErrorKind` encodes the class so call sites that still read
    /// `error.kind()` (e.g. the boa native-function error formatter
    /// in `runtime/builtins/endpoint.rs`) see the same surface as
    /// before this refactor.
    pub fn into_io_error(self) -> std::io::Error {
        match self {
            Self::Network(e) => e,
            Self::Timeout => Error::new(ErrorKind::TimedOut, "request timed out"),
            err @ Self::BodyTooLarge { .. } => Error::new(ErrorKind::InvalidData, err.to_string()),
            err @ Self::InvalidRequest(_) => Error::new(ErrorKind::InvalidInput, err.to_string()),
            err @ Self::Decode(_) => Error::other(err.to_string()),
            err @ Self::Other(_) => Error::other(err.to_string()),
        }
    }
}

/// Convenience: `Result<T, EndpointTransportError>`.
pub type EndpointTransportResult<T> = std::result::Result<T, EndpointTransportError>;

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

#[derive(Clone, Copy, Debug)]
struct EndpointRequestDeadline {
    expires_at: Instant,
}

impl EndpointRequestDeadline {
    fn new(timeout_ms: Option<u64>) -> std::io::Result<Option<Self>> {
        let Some(timeout_ms) = timeout_ms else {
            return Ok(None);
        };
        let timeout = Duration::from_millis(timeout_ms);
        let expires_at = Instant::now().checked_add(timeout).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "request timeout is too large for the current platform clock",
            )
        })?;
        Ok(Some(Self { expires_at }))
    }

    fn remaining(self) -> std::io::Result<Duration> {
        self.expires_at
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| Error::new(ErrorKind::TimedOut, "request timed out"))
    }
}

/// Endpoint HTTP client abstraction configured at pool level.
///
/// Runtime contract:
/// - `execute` futures are polled on the pool worker's internal Tokio runtime.
/// - The built-in endpoint retry path also uses Tokio timers.
/// - Implementations may therefore rely on Tokio async primitives.
///
/// Error contract: returns [`EndpointTransportError`] rather than
/// `std::io::Error`. The variant **is** the retryability class —
/// `Network` and `Timeout` are retryable per the caller's
/// [`EndpointRetryPolicy`]; the other variants are terminal. This
/// avoids the previous coarse-`ErrorKind` heuristic that retried
/// deterministic local conditions (e.g. body-cap violations) as if
/// they were transient network errors.
pub trait EndpointHttpClient: Send + Sync + std::fmt::Debug {
    /// Executes one transport request and returns a transport response.
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>>;
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

/// Classify an mhc-side error as an `EndpointTransportError`.
///
/// `mechanics_http_client::Error` is `#[non_exhaustive]`, so this
/// match requires a wildcard arm. The named arms cover the
/// currently-defined variants; the wildcard maps to `Other`
/// (conservative — not retryable) so a future variant added in mhc
/// does not silently flip a class of error into the retryable
/// `Network` bucket without an explicit decision here.
fn classify_mhc_error(err: mechanics_http_client::Error) -> EndpointTransportError {
    use mechanics_http_client::Error as MhcError;
    let message = err.to_string();
    match err {
        MhcError::Timeout => EndpointTransportError::Timeout,
        // Network-class: TCP / DNS / TLS / QUIC handshake / mid-flight cancel.
        MhcError::Unreachable(_)
        | MhcError::Tls(_)
        | MhcError::Cancelled(_)
        | MhcError::Dns(_)
        | MhcError::QuicHandshake(_) => EndpointTransportError::Network(Error::other(message)),
        // Deterministic local conditions — retrying would rebuild the
        // same request and fail the same way.
        MhcError::BodyTooLarge { limit, seen } => EndpointTransportError::BodyTooLarge {
            limit,
            seen: Some(seen as u64),
        },
        MhcError::InvalidUrl(m) | MhcError::InvalidHeader(m) | MhcError::SerializeJson(m) => {
            EndpointTransportError::InvalidRequest(m)
        }
        MhcError::Decode(m) => EndpointTransportError::Decode(m),
        MhcError::Internal(m) => EndpointTransportError::Other(m),
        _ => EndpointTransportError::Other(message),
    }
}

/// Lift `EndpointRequestDeadline` errors into the typed enum.
/// `InvalidInput` (timeout too large) is an invalid-request shape;
/// `TimedOut` (already-expired deadline) is `Timeout`.
fn classify_deadline_error(err: std::io::Error) -> EndpointTransportError {
    match err.kind() {
        ErrorKind::TimedOut => EndpointTransportError::Timeout,
        ErrorKind::InvalidInput => EndpointTransportError::InvalidRequest(err.to_string()),
        _ => EndpointTransportError::Other(err.to_string()),
    }
}

impl EndpointHttpClient for DefaultEndpointHttpClient {
    fn execute(
        &self,
        request: EndpointHttpRequest,
    ) -> Pin<Box<dyn Future<Output = EndpointTransportResult<EndpointHttpResponse>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            // Fresh transport build is local TLS-config plumbing; failure
            // here is not a network condition — retrying rebuilds the
            // same config and fails the same way.
            let client = client
                .fresh_transport()
                .map_err(|e| EndpointTransportError::Other(e.to_string()))?;
            let deadline = EndpointRequestDeadline::new(request.timeout_ms)
                .map_err(classify_deadline_error)?;
            let mut req = client.request(request.method.as_http_method(), &request.url);
            for (name, value) in request.headers.iter() {
                req = req.header(name, value);
            }

            if let Some(deadline) = deadline {
                req = req.timeout(deadline.remaining().map_err(classify_deadline_error)?);
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

            let res: HttpResponse = req.send().await.map_err(classify_mhc_error)?;
            let status = res.status().as_u16();
            let content_length = res.content_length();
            let headers = EndpointHttpHeaders::from_http_map(res.headers());

            if let (Some(max), Some(len)) = (request.response_max_bytes, content_length)
                && len > max as u64
            {
                return Err(EndpointTransportError::BodyTooLarge {
                    limit: max,
                    seen: Some(len),
                });
            }

            let response_max_bytes = request.response_max_bytes;
            let read_body = async move {
                match response_max_bytes {
                    Some(max) => match res.bytes_with_cap(max).await {
                        Ok(bytes) => Ok(bytes.to_vec()),
                        Err(mechanics_http_client::Error::BodyTooLarge { limit, .. }) => {
                            Err(EndpointTransportError::BodyTooLarge { limit, seen: None })
                        }
                        Err(err) => Err(classify_mhc_error(err)),
                    },
                    None => res
                        .bytes()
                        .await
                        .map(|bytes| bytes.to_vec())
                        .map_err(classify_mhc_error),
                }
            };

            let body = if let Some(deadline) = deadline {
                let remaining = deadline.remaining().map_err(classify_deadline_error)?;
                match tokio::time::timeout(remaining, read_body).await {
                    Ok(result) => result?,
                    Err(_) => return Err(EndpointTransportError::Timeout),
                }
            } else {
                read_body.await?
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

#[cfg(test)]
mod retry_classification_tests {
    use super::*;

    fn default_policy() -> EndpointRetryPolicy {
        EndpointRetryPolicy {
            max_attempts: 3,
            retry_on_io_errors: true,
            retry_on_timeout: true,
            ..EndpointRetryPolicy::default()
        }
    }

    #[test]
    fn network_class_is_retryable_when_io_errors_enabled() {
        let policy = default_policy();
        let err = EndpointTransportError::Network(Error::other("tcp reset"));
        assert!(err.is_retryable_per(&policy));
    }

    #[test]
    fn network_class_is_not_retryable_when_io_errors_disabled() {
        let mut policy = default_policy();
        policy.retry_on_io_errors = false;
        let err = EndpointTransportError::Network(Error::other("tcp reset"));
        assert!(!err.is_retryable_per(&policy));
    }

    #[test]
    fn timeout_is_retryable_when_timeout_enabled() {
        let policy = default_policy();
        assert!(EndpointTransportError::Timeout.is_retryable_per(&policy));
    }

    #[test]
    fn timeout_is_not_retryable_when_timeout_disabled() {
        let mut policy = default_policy();
        policy.retry_on_timeout = false;
        assert!(!EndpointTransportError::Timeout.is_retryable_per(&policy));
    }

    #[test]
    fn body_too_large_is_never_retryable() {
        let policy = default_policy();
        let err = EndpointTransportError::BodyTooLarge {
            limit: 1024,
            seen: Some(2048),
        };
        assert!(!err.is_retryable_per(&policy));
    }

    #[test]
    fn invalid_request_is_never_retryable() {
        let policy = default_policy();
        let err = EndpointTransportError::InvalidRequest("bad url".to_owned());
        assert!(!err.is_retryable_per(&policy));
    }

    #[test]
    fn decode_is_never_retryable() {
        let policy = default_policy();
        let err = EndpointTransportError::Decode("not utf-8".to_owned());
        assert!(!err.is_retryable_per(&policy));
    }

    #[test]
    fn other_is_never_retryable() {
        let policy = default_policy();
        let err = EndpointTransportError::Other("unknown".to_owned());
        assert!(!err.is_retryable_per(&policy));
    }

    #[test]
    fn into_io_error_preserves_kind() {
        assert_eq!(
            EndpointTransportError::Timeout.into_io_error().kind(),
            ErrorKind::TimedOut,
        );
        assert_eq!(
            EndpointTransportError::BodyTooLarge {
                limit: 1,
                seen: Some(2)
            }
            .into_io_error()
            .kind(),
            ErrorKind::InvalidData,
        );
        assert_eq!(
            EndpointTransportError::InvalidRequest("x".into())
                .into_io_error()
                .kind(),
            ErrorKind::InvalidInput,
        );
    }
}
