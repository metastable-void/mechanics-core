use super::*;
use crate::http::{
    EndpointCallBody, EndpointCallOptions, EndpointHttpClient, EndpointHttpHeaders,
    EndpointHttpRequest, EndpointHttpRequestBody, EndpointResponse, EndpointResponseBody,
    into_io_error,
};
use serde_json::Value;
use std::{
    io::{Error, ErrorKind},
    sync::Arc,
};

impl HttpEndpoint {
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
                url: url.as_str().to_owned(),
                headers: EndpointHttpHeaders::from_reqwest(&headers),
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

        let response_headers = super::super::headers::extract_exposed_response_headers_prepared(
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

pub(super) fn extend_body_with_limit(
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
