use super::query::validate_slot_name;
use std::{
    collections::HashSet,
    io::{Error, ErrorKind},
};

#[derive(Clone, Debug)]
pub(super) enum UrlTemplateChunk {
    Literal(String),
    Slot(String),
}

pub(super) fn parse_url_template(
    template: &str,
) -> std::io::Result<(Vec<UrlTemplateChunk>, Vec<String>)> {
    let mut chunks = Vec::new();
    let mut slots = Vec::new();
    let mut seen_slots = HashSet::new();

    let mut cursor = 0usize;
    loop {
        let Some(open_rel) = template[cursor..].find('{') else {
            break;
        };

        let open = cursor.checked_add(open_rel).ok_or(Error::new(
            ErrorKind::InvalidInput,
            "url_template index overflow",
        ))?;
        if open > cursor {
            chunks.push(UrlTemplateChunk::Literal(template[cursor..open].to_owned()));
        }

        let after_open = open.checked_add(1).ok_or(Error::new(
            ErrorKind::InvalidInput,
            "url_template index overflow",
        ))?;
        let Some(close_rel) = template[after_open..].find('}') else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains unmatched `{`",
            ));
        };

        let close = after_open.checked_add(close_rel).ok_or(Error::new(
            ErrorKind::InvalidInput,
            "url_template index overflow",
        ))?;
        let slot = &template[after_open..close];
        if slot.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains empty `{}` placeholder",
            ));
        }
        if slot.contains('{') {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "url_template contains nested `{` in placeholder",
            ));
        }
        validate_slot_name(slot)?;

        if !seen_slots.insert(slot.to_owned()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("url_template contains duplicate slot `{slot}`"),
            ));
        }

        let slot_owned = slot.to_owned();
        slots.push(slot_owned.clone());
        chunks.push(UrlTemplateChunk::Slot(slot_owned));
        cursor = close.checked_add(1).ok_or(Error::new(
            ErrorKind::InvalidInput,
            "url_template index overflow",
        ))?;
    }

    if let Some(stray) = template[cursor..].find('}') {
        let idx = cursor.checked_add(stray).ok_or(Error::new(
            ErrorKind::InvalidInput,
            "url_template index overflow",
        ))?;
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("url_template contains unmatched `}}` at byte index {idx}"),
        ));
    }

    if cursor < template.len() {
        chunks.push(UrlTemplateChunk::Literal(template[cursor..].to_owned()));
    }

    Ok((chunks, slots))
}

#[allow(clippy::indexing_slicing)]
pub(super) fn percent_encode_component(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        let is_unreserved = matches!(
            b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        );
        if is_unreserved {
            out.push(char::from(b));
            continue;
        }
        let hi = usize::from(b >> 4);
        let lo = usize::from(b & 0x0F);
        // SAFETY: both nibbles are guaranteed in `0..=15`, matching `HEX` length.
        debug_assert!(hi < HEX.len() && lo < HEX.len());
        out.push('%');
        out.push(char::from(HEX[hi]));
        out.push(char::from(HEX[lo]));
    }
    out
}
