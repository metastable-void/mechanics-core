use super::*;
use crate::http::{
    EndpointCallOptions, into_io_error,
    query::{
        resolve_slotted_query_value, validate_min_max_bounds, validate_query_key,
        validate_slot_name,
    },
    template::percent_encode_component,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::io::{Error, ErrorKind};

impl HttpEndpoint {
    pub(super) fn effective_request_body_type(&self) -> EndpointBodyType {
        self.request_body_type
            .clone()
            .unwrap_or(EndpointBodyType::Json)
    }

    #[cfg(test)]
    pub(super) fn build_headers(
        &self,
        default_content_type: Option<&str>,
        options: &EndpointCallOptions,
    ) -> std::io::Result<HeaderMap> {
        let prepared = self.prepare_runtime()?;
        self.build_headers_prepared(default_content_type, options, &prepared)
    }

    pub(super) fn build_headers_prepared(
        &self,
        default_content_type: Option<&str>,
        options: &EndpointCallOptions,
        prepared: &PreparedHttpEndpoint,
    ) -> std::io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        // Header precedence is explicit:
        // 1) auto defaults, 2) endpoint configured headers, 3) JS allowlisted overrides.
        let user_agent = HeaderValue::try_from(Self::USER_AGENT).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid default User-Agent header: {e}"),
            )
        })?;
        headers.insert(USER_AGENT, user_agent);

        if let Some(default_content_type) = default_content_type {
            let content_type = HeaderValue::try_from(default_content_type).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid default Content-Type header: {e}"),
                )
            })?;
            headers.insert(CONTENT_TYPE, content_type);
        }

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

        for (k, v) in &options.headers {
            let name = HeaderName::try_from(k.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid override header name `{k}`: {e}"),
                )
            })?;
            if !prepared.allowed_overrides.contains(&name) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "override header `{k}` is not allowlisted in overridable_request_headers"
                    ),
                ));
            }
            let value = HeaderValue::try_from(v.as_str()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid override header value for `{k}`: {e}"),
                )
            })?;
            headers.insert(name, value);
        }

        Ok(headers)
    }

    #[cfg(test)]
    pub(super) fn build_url(&self, options: &EndpointCallOptions) -> std::io::Result<reqwest::Url> {
        let prepared = self.prepare_runtime()?;
        self.build_url_prepared(options, &prepared)
    }

    pub(super) fn build_url_prepared(
        &self,
        options: &EndpointCallOptions,
        prepared: &PreparedHttpEndpoint,
    ) -> std::io::Result<reqwest::Url> {
        for provided in options.url_params.keys() {
            if !prepared.url_slot_set.contains(provided) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "unknown urlParams key `{provided}` for endpoint template `{}`",
                        self.url_template
                    ),
                ));
            }
        }

        for slot in &prepared.url_slot_names {
            if !self.url_param_specs.contains_key(slot) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("missing url_param_specs entry for slot `{slot}`"),
                ));
            }
        }

        for configured in self.url_param_specs.keys() {
            if !prepared.url_slot_set.contains(configured) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "url_param_specs entry `{configured}` has no placeholder in url_template"
                    ),
                ));
            }
        }

        let mut resolved_url = String::with_capacity(self.url_template.len().saturating_add(16));
        for chunk in &prepared.parsed_url_chunks {
            match chunk {
                UrlTemplateChunk::Literal(s) => resolved_url.push_str(s),
                UrlTemplateChunk::Slot(slot) => {
                    let spec = self.url_param_specs.get(slot.as_str()).ok_or(Error::new(
                        ErrorKind::InvalidInput,
                        format!("missing url_param_specs entry for slot `{slot}`"),
                    ))?;
                    let provided = options.url_params.get(slot.as_str()).map(String::as_str);
                    let value = spec.resolve_value(slot, provided)?;
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

        for provided in options.queries.keys() {
            if !prepared.allowed_query_slots.contains(provided) {
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
}
