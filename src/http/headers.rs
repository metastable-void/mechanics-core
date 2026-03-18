use super::EndpointHttpHeaders;
#[cfg(test)]
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use std::{
    collections::{HashMap, HashSet},
    io::{Error, ErrorKind},
};

pub(super) fn validate_header_name_list(
    headers: &[String],
    field_name: &str,
) -> std::io::Result<()> {
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

pub(super) fn allowlisted_header_names(
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
pub(super) fn extract_exposed_response_headers(
    headers: &HeaderMap,
    allowlist: &[String],
) -> std::io::Result<HashMap<String, String>> {
    let allowlisted = allowlisted_header_names(allowlist, "exposed_response_headers")?;
    extract_exposed_response_headers_prepared(
        &EndpointHttpHeaders::from_reqwest(headers),
        &allowlisted,
    )
}

pub(super) fn extract_exposed_response_headers_prepared(
    headers: &EndpointHttpHeaders,
    allowlisted: &HashSet<HeaderName>,
) -> std::io::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for name in allowlisted {
        let parts = headers
            .values(name.as_str())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            out.insert(name.as_str().to_ascii_lowercase(), parts.join(", "));
        }
    }
    Ok(out)
}
