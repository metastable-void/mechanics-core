use super::SlottedQueryMode;
use std::io::{Error, ErrorKind};

pub(super) fn resolve_slotted_query_value(
    slot: &str,
    mode: SlottedQueryMode,
    default: Option<&str>,
    provided: Option<&str>,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<Option<String>> {
    let value = match mode {
        SlottedQueryMode::Required => {
            let candidate = match provided {
                Some(v) if !v.is_empty() => Some(v),
                Some(_) | None => default.filter(|v| !v.is_empty()),
            };
            let Some(candidate) = candidate else {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("required query slot `{slot}` is missing or empty"),
                ));
            };
            Some(candidate.to_owned())
        }
        SlottedQueryMode::RequiredAllowEmpty => {
            let candidate = match provided {
                Some(v) => Some(v),
                None => default,
            };
            let Some(candidate) = candidate else {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("required query slot `{slot}` is missing"),
                ));
            };
            Some(candidate.to_owned())
        }
        SlottedQueryMode::Optional => match provided {
            Some(v) if !v.is_empty() => Some(v.to_owned()),
            Some(_) | None => default.filter(|v| !v.is_empty()).map(ToOwned::to_owned),
        },
        SlottedQueryMode::OptionalAllowEmpty => match provided {
            Some(v) => Some(v.to_owned()),
            None => default.map(ToOwned::to_owned),
        },
    };

    if let Some(ref value) = value {
        validate_byte_len(slot, value, min_bytes, max_bytes)?;
    }

    Ok(value)
}

pub(super) fn validate_slot_name(slot: &str) -> std::io::Result<()> {
    if slot.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "slot name must not be empty",
        ));
    }

    if !slot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "slot name `{slot}` is invalid: only ASCII letters, digits, and `_` are allowed"
            ),
        ));
    }

    Ok(())
}

pub(super) fn validate_query_key(key: &str) -> std::io::Result<()> {
    if key.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "query key must not be empty",
        ));
    }
    Ok(())
}

pub(super) fn validate_min_max_bounds(
    slot: &str,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<()> {
    if let (Some(min), Some(max)) = (min_bytes, max_bytes)
        && min > max
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` has invalid byte bounds: min_bytes ({min}) > max_bytes ({max})"),
        ));
    }
    Ok(())
}

pub(super) fn validate_byte_len(
    slot: &str,
    value: &str,
    min_bytes: Option<usize>,
    max_bytes: Option<usize>,
) -> std::io::Result<()> {
    let len = value.len();
    if let Some(min) = min_bytes
        && len < min
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` is too short: {len} bytes < min_bytes ({min})"),
        ));
    }
    if let Some(max) = max_bytes
        && len > max
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("slot `{slot}` is too long: {len} bytes > max_bytes ({max})"),
        ));
    }
    Ok(())
}
