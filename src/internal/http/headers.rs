use super::EndpointHttpHeaders;
use http::HeaderMap;
use std::{
    collections::{HashMap, HashSet},
    io::Error,
};

pub(super) fn extract_exposed_response_headers_prepared(
    headers: &EndpointHttpHeaders,
    allowlisted: &HashSet<String>,
) -> std::io::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for name in allowlisted {
        let parts = headers.values(name).map(str::to_owned).collect::<Vec<_>>();
        if !parts.is_empty() {
            out.insert(name.to_ascii_lowercase(), parts.join(", "));
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(super) fn extract_exposed_response_headers(
    headers: &HeaderMap,
    allowlisted: &[String],
) -> std::io::Result<HashMap<String, String>> {
    let allowlisted = allowlisted
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let headers = EndpointHttpHeaders::from_http_map(headers);
    extract_exposed_response_headers_prepared(&headers, &allowlisted)
}

pub(super) fn header_from_pairs(pairs: Vec<(String, String)>) -> std::io::Result<HeaderMap> {
    use http::{HeaderName, HeaderValue};

    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        let header_name = HeaderName::try_from(name.as_str())
            .map_err(|e| Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let header_value = HeaderValue::try_from(value.as_str())
            .map_err(|e| Error::new(std::io::ErrorKind::InvalidInput, e))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}
