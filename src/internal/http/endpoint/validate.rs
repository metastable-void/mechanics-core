use super::*;
use crate::internal::http::{
    headers::{allowlisted_header_names, validate_header_name_list},
    into_io_error,
    query::{validate_byte_len, validate_min_max_bounds, validate_query_key, validate_slot_name},
    template::{UrlTemplateChunk, parse_url_template},
};
use std::{
    collections::HashSet,
    io::{Error, ErrorKind},
};

impl HttpEndpoint {
    pub(crate) fn validate_config(&self) -> std::io::Result<()> {
        self.retry_policy.validate()?;
        if self.timeout_ms == Some(0) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "timeout_ms must be >= 1 when provided",
            ));
        }
        if self.response_max_bytes == Some(0) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "response_max_bytes must be >= 1 when provided",
            ));
        }
        validate_header_name_list(
            &self.overridable_request_headers,
            "overridable_request_headers",
        )?;
        validate_header_name_list(&self.exposed_response_headers, "exposed_response_headers")?;

        let (chunks, slot_names) = parse_url_template(&self.url_template)?;
        let slot_set: HashSet<&str> = slot_names.iter().map(String::as_str).collect();

        for slot in &slot_names {
            let spec = self.url_param_specs.get(slot).ok_or(Error::new(
                ErrorKind::InvalidInput,
                format!("missing url_param_specs entry for slot `{slot}`"),
            ))?;
            validate_min_max_bounds(slot, spec.min_bytes, spec.max_bytes)?;
            if let Some(default_value) = spec.default.as_deref() {
                validate_byte_len(slot, default_value, spec.min_bytes, spec.max_bytes)?;
            }
        }

        for configured in self.url_param_specs.keys() {
            if !slot_set.contains(configured.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "url_param_specs entry `{configured}` has no placeholder in url_template"
                    ),
                ));
            }
        }

        let mut template_probe = String::with_capacity(self.url_template.len().saturating_add(16));
        for chunk in chunks {
            match chunk {
                UrlTemplateChunk::Literal(s) => template_probe.push_str(&s),
                UrlTemplateChunk::Slot(_) => template_probe.push('x'),
            }
        }
        let url = reqwest::Url::parse(&template_probe).map_err(into_io_error)?;
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

        for spec in &self.query_specs {
            match spec {
                QuerySpec::Const { key, .. } => validate_query_key(key)?,
                QuerySpec::Slotted {
                    key,
                    slot,
                    mode,
                    default,
                    min_bytes,
                    max_bytes,
                    ..
                } => {
                    validate_query_key(key)?;
                    validate_slot_name(slot)?;
                    validate_min_max_bounds(slot, *min_bytes, *max_bytes)?;
                    if let Some(default_value) = default {
                        let should_validate_default = match mode {
                            SlottedQueryMode::Required | SlottedQueryMode::Optional => {
                                !default_value.is_empty()
                            }
                            SlottedQueryMode::RequiredAllowEmpty
                            | SlottedQueryMode::OptionalAllowEmpty => true,
                        };
                        if should_validate_default {
                            validate_byte_len(slot, default_value, *min_bytes, *max_bytes)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn prepare_runtime(&self) -> std::io::Result<PreparedHttpEndpoint> {
        let (parsed_url_chunks, url_slot_names) = parse_url_template(&self.url_template)?;
        let url_slot_set = url_slot_names.iter().cloned().collect::<HashSet<_>>();
        let allowed_query_slots = self
            .query_specs
            .iter()
            .filter_map(|spec| match spec {
                QuerySpec::Slotted { slot, .. } => Some(slot.clone()),
                QuerySpec::Const { .. } => None,
            })
            .collect::<HashSet<_>>();
        let allowed_overrides = allowlisted_header_names(
            &self.overridable_request_headers,
            "overridable_request_headers",
        )?;
        let exposed_response_allowlist =
            allowlisted_header_names(&self.exposed_response_headers, "exposed_response_headers")?;

        Ok(PreparedHttpEndpoint {
            parsed_url_chunks,
            url_slot_names,
            url_slot_set,
            allowed_query_slots,
            allowed_overrides,
            exposed_response_allowlist,
        })
    }
}
