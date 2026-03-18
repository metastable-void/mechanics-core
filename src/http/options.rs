#[cfg(test)]
use super::into_io_error;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
#[cfg(test)]
use std::io::{Error, ErrorKind};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct EndpointCallOptions {
    pub(crate) url_params: HashMap<String, String>,
    pub(crate) queries: HashMap<String, String>,
    pub(crate) headers: HashMap<String, String>,
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
                None => EndpointCallBody::Absent,
                Some(Value::Null) => EndpointCallBody::Json(Value::Null),
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

#[derive(Debug, Clone)]
pub(crate) struct EndpointResponse {
    pub(crate) body: EndpointResponseBody,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) status: u16,
    pub(crate) ok: bool,
}
