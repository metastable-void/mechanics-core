use crate::internal::{executor::CustomModuleLoader, runtime::buffer_like};
use boa_engine::{
    Context, JsArgs, JsError, JsResult, JsString, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer, object::FunctionObjectBuilder,
};
use data_encoding::BASE64;
use std::rc::Rc;

const MIME_VERSION: &str = "MIME-Version";
const CONTENT_TYPE: &str = "Content-Type";
const CONTENT_TRANSFER_ENCODING: &str = "Content-Transfer-Encoding";
const DEFAULT_TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
const DEFAULT_MULTIPART_CONTENT_TYPE: &str = "multipart/mixed";

#[derive(Clone)]
struct Header {
    name: String,
    value: String,
}

enum MimeBody {
    Text(String),
    Bytes(Vec<u8>),
}

struct MimeMessage {
    headers: Vec<Header>,
    body: Option<MimeBody>,
    parts: Vec<MimeMessage>,
    encoding: Option<String>,
}

#[derive(Clone, Copy)]
enum TransferEncoding {
    SevenBit,
    EightBit,
    Binary,
    QuotedPrintable,
    Base64,
}

impl TransferEncoding {
    fn parse(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "7bit" => Some(Self::SevenBit),
            "8bit" => Some(Self::EightBit),
            "binary" => Some(Self::Binary),
            "quoted-printable" => Some(Self::QuotedPrintable),
            "base64" => Some(Self::Base64),
            _ => None,
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::SevenBit => "7bit",
            Self::EightBit => "8bit",
            Self::Binary => "binary",
            Self::QuotedPrintable => "quoted-printable",
            Self::Base64 => "base64",
        }
    }
}

fn type_error(message: impl AsRef<str>) -> JsError {
    buffer_like::js_type_error(message)
}

fn header_matches(name: &str, wanted: &str) -> bool {
    name.eq_ignore_ascii_case(wanted)
}

fn header_value<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|header| header_matches(&header.name, name))
        .map(|header| header.value.as_str())
}

fn set_header(headers: &mut Vec<Header>, name: &str, value: String) {
    if let Some(header) = headers
        .iter_mut()
        .find(|header| header_matches(&header.name, name))
    {
        header.value = value;
    } else {
        headers.push(Header {
            name: name.to_owned(),
            value,
        });
    }
}

fn has_header(headers: &[Header], name: &str) -> bool {
    header_value(headers, name).is_some()
}

fn validate_header_name(name: &str) -> JsResult<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| matches!(b, b'!'..=b'9' | b';'..=b'~') && b != b':')
    {
        return Err(type_error(
            "MIME header names must be non-empty RFC 5322 field names",
        ));
    }
    Ok(())
}

fn validate_header_value(value: &str) -> JsResult<()> {
    if value.bytes().any(|b| matches!(b, b'\r' | b'\n')) {
        return Err(type_error(
            "MIME header values must not contain line breaks",
        ));
    }
    Ok(())
}

fn collect_headers(value: JsValue, context: &mut Context) -> JsResult<Vec<Header>> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }

    let object = value.to_object(context)?;
    let mut headers = Vec::new();
    for key in object.own_property_keys(context)? {
        let name = key.to_string();
        validate_header_name(&name)?;
        let value = object
            .get(key, context)?
            .to_string(context)?
            .to_std_string_lossy();
        validate_header_value(&value)?;
        headers.push(Header { name, value });
    }
    Ok(headers)
}

fn collect_parts(value: JsValue, context: &mut Context) -> JsResult<Vec<MimeMessage>> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }

    let object = value.to_object(context)?;
    let len = object
        .get(js_string!("length"), context)?
        .to_length(context)?;
    let mut parts = Vec::new();
    for index in 0..len {
        parts.push(collect_message(object.get(index, context)?, context)?);
    }
    Ok(parts)
}

fn collect_message(value: JsValue, context: &mut Context) -> JsResult<MimeMessage> {
    let object = value.to_object(context)?;
    let headers = collect_headers(object.get(js_string!("headers"), context)?, context)?;
    let parts = collect_parts(object.get(js_string!("parts"), context)?, context)?;
    let body_value = object.get(js_string!("body"), context)?;
    let body = if body_value.is_undefined() || body_value.is_null() {
        None
    } else if let Some(bytes) = buffer_like::try_extract_buffer_like_bytes(&body_value, context)? {
        Some(MimeBody::Bytes(bytes))
    } else if body_value.as_string().is_some() {
        Some(MimeBody::Text(
            body_value.to_string(context)?.to_std_string_lossy(),
        ))
    } else {
        return Err(type_error(
            "MIME body must be a string, Uint8Array, ArrayBuffer, or DataView",
        ));
    };
    let encoding_value = object.get(js_string!("encoding"), context)?;
    let encoding = if encoding_value.is_undefined() || encoding_value.is_null() {
        None
    } else {
        Some(
            encoding_value
                .to_string(context)?
                .to_std_string_lossy()
                .to_ascii_lowercase(),
        )
    };

    if body.is_some() && !parts.is_empty() {
        return Err(type_error(
            "MIME message must not contain both body and parts",
        ));
    }

    Ok(MimeMessage {
        headers,
        body,
        parts,
        encoding,
    })
}

fn content_type_base(content_type: &str) -> &str {
    content_type
        .split_once(';')
        .map_or(content_type, |(base, _)| base)
        .trim()
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type_base(content_type)
        .to_ascii_lowercase()
        .starts_with("text/")
}

fn parse_boundary(content_type: &str) -> Option<String> {
    for param in content_type.split(';').skip(1) {
        let Some((name, value)) = param.trim().split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("boundary") {
            let value = value.trim();
            return if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                Some(value[1..value.len().saturating_sub(1)].to_owned())
            } else {
                Some(value.to_owned())
            };
        }
    }
    None
}

fn content_type_with_boundary(content_type: &str, boundary: &str) -> String {
    if parse_boundary(content_type).is_some() {
        content_type.to_owned()
    } else {
        format!("{content_type}; boundary=\"{boundary}\"")
    }
}

fn normalize_text_lines(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

fn wrap_ascii_line(input: &str, width: usize) -> String {
    let mut output = String::new();
    let mut line_len = 0usize;
    for ch in input.chars() {
        if line_len >= width {
            output.push_str("\r\n");
            line_len = 0;
        }
        output.push(ch);
        line_len = line_len.saturating_add(ch.len_utf8());
    }
    output
}

fn base64_encode_wrapped(bytes: &[u8]) -> String {
    wrap_ascii_line(&BASE64.encode(bytes), 76)
}

fn base64_decode_liberal(input: &str) -> JsResult<Vec<u8>> {
    let compact: String = input
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    BASE64
        .decode(compact.as_bytes())
        .map_err(|_| type_error("invalid MIME base64 body"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_pair(byte: u8) -> [char; 2] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let hi = usize::from(byte >> 4);
    let lo = usize::from(byte & 0x0f);
    [char::from(HEX[hi]), char::from(HEX[lo])]
}

fn qp_push_token(output: &mut String, line_len: &mut usize, token: &str) {
    if line_len.saturating_add(token.len()) > 75 {
        output.push_str("=\r\n");
        *line_len = 0;
    }
    output.push_str(token);
    *line_len = line_len.saturating_add(token.len());
}

fn qp_encode(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut line_len = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\r' && bytes.get(index.saturating_add(1)) == Some(&b'\n') {
            output.push_str("\r\n");
            line_len = 0;
            index = index.saturating_add(2);
            continue;
        }
        let token = if matches!(byte, b'\t' | b' '..=b'<' | b'>'..=b'~') {
            char::from(byte).to_string()
        } else {
            let hex = hex_pair(byte);
            format!("={}{}", hex[0], hex[1])
        };
        qp_push_token(&mut output, &mut line_len, &token);
        index = index.saturating_add(1);
    }
    output
}

fn qp_decode(input: &str, encoded_word: bool) -> JsResult<Vec<u8>> {
    let bytes = input.as_bytes();
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if encoded_word && byte == b'_' {
            output.push(b' ');
            index = index.saturating_add(1);
            continue;
        }
        if byte != b'=' {
            output.push(byte);
            index = index.saturating_add(1);
            continue;
        }
        let next = index.saturating_add(1);
        if bytes.get(next) == Some(&b'\r') && bytes.get(next.saturating_add(1)) == Some(&b'\n') {
            index = index.saturating_add(3);
            continue;
        }
        if bytes.get(next) == Some(&b'\n') {
            index = index.saturating_add(2);
            continue;
        }
        let hi = bytes
            .get(next)
            .and_then(|b| hex_value(*b))
            .ok_or_else(|| type_error("invalid quoted-printable escape"))?;
        let lo = bytes
            .get(next.saturating_add(1))
            .and_then(|b| hex_value(*b))
            .ok_or_else(|| type_error("invalid quoted-printable escape"))?;
        output.push((hi << 4) | lo);
        index = index.saturating_add(3);
    }
    Ok(output)
}

fn q_encode_header(value: &str) -> String {
    let mut encoded = String::from("=?UTF-8?Q?");
    for byte in value.as_bytes() {
        match *byte {
            b' ' => encoded.push('_'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'!' | b'*' | b'+' | b'-' | b'/' => {
                encoded.push(char::from(*byte));
            }
            _ => {
                let hex = hex_pair(*byte);
                encoded.push('=');
                encoded.push(hex[0]);
                encoded.push(hex[1]);
            }
        }
    }
    encoded.push_str("?=");
    encoded
}

fn maybe_encode_header_value(value: &str) -> String {
    if value.is_ascii() {
        value.to_owned()
    } else {
        q_encode_header(value)
    }
}

fn decode_encoded_words(value: &str) -> JsResult<String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("=?") {
        output.push_str(&rest[..start]);
        let after_start = &rest[start.saturating_add(2)..];
        let Some(charset_end) = after_start.find('?') else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let charset = &after_start[..charset_end];
        let after_charset = &after_start[charset_end.saturating_add(1)..];
        let Some(encoding_end) = after_charset.find('?') else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let encoding = &after_charset[..encoding_end];
        let after_encoding = &after_charset[encoding_end.saturating_add(1)..];
        let Some(text_end) = after_encoding.find("?=") else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        if !matches!(charset.to_ascii_lowercase().as_str(), "utf-8" | "utf8") {
            return Err(type_error("unsupported RFC 2047 charset"));
        }
        let encoded = &after_encoding[..text_end];
        let decoded = match encoding.to_ascii_lowercase().as_str() {
            "b" => base64_decode_liberal(encoded)?,
            "q" => qp_decode(encoded, true)?,
            _ => return Err(type_error("unsupported RFC 2047 encoding")),
        };
        let decoded =
            String::from_utf8(decoded).map_err(|_| type_error("invalid RFC 2047 UTF-8"))?;
        output.push_str(&decoded);
        rest = &after_encoding[text_end.saturating_add(2)..];
    }
    output.push_str(rest);
    Ok(output)
}

fn fold_header_line(name: &str, value: &str) -> String {
    let line = format!("{name}: {value}");
    if line.len() <= 78 {
        return line;
    }

    let mut folded = String::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let separator = if current.is_empty() { "" } else { " " };
        if !current.is_empty()
            && current
                .len()
                .saturating_add(separator.len())
                .saturating_add(word.len())
                > 78
        {
            if folded.is_empty() {
                folded.push_str(&current);
            } else {
                folded.push_str("\r\n ");
                folded.push_str(&current);
            }
            current.clear();
            current.push_str(word);
        } else {
            current.push_str(separator);
            current.push_str(word);
        }
    }
    if folded.is_empty() {
        line
    } else {
        folded.push_str("\r\n ");
        folded.push_str(&current);
        folded
    }
}

fn render_headers(headers: &[Header]) -> JsResult<String> {
    let mut output = String::new();
    for header in headers {
        validate_header_name(&header.name)?;
        validate_header_value(&header.value)?;
        let encoded = maybe_encode_header_value(&header.value);
        output.push_str(&fold_header_line(&header.name, &encoded));
        output.push_str("\r\n");
    }
    Ok(output)
}

fn choose_encoding(message: &MimeMessage, headers: &[Header]) -> JsResult<TransferEncoding> {
    if let Some(explicit) = &message.encoding {
        return TransferEncoding::parse(explicit).ok_or_else(|| {
            type_error("MIME encoding must be 7bit, 8bit, binary, quoted-printable, or base64")
        });
    }
    if let Some(header) = header_value(headers, CONTENT_TRANSFER_ENCODING) {
        return TransferEncoding::parse(header)
            .ok_or_else(|| type_error("invalid Content-Transfer-Encoding header"));
    }

    let content_type = header_value(headers, CONTENT_TYPE).unwrap_or(DEFAULT_TEXT_CONTENT_TYPE);
    let is_text = is_text_content_type(content_type);
    match &message.body {
        Some(MimeBody::Text(text)) if is_text && text.is_ascii() => Ok(TransferEncoding::SevenBit),
        Some(MimeBody::Text(_)) if is_text => Ok(TransferEncoding::QuotedPrintable),
        Some(MimeBody::Text(_)) => Ok(TransferEncoding::Base64),
        Some(MimeBody::Bytes(_)) => Ok(TransferEncoding::Base64),
        None => Ok(TransferEncoding::SevenBit),
    }
}

fn body_bytes_and_text(message: &MimeMessage) -> JsResult<(Vec<u8>, bool)> {
    match &message.body {
        Some(MimeBody::Text(text)) => Ok((normalize_text_lines(text).into_bytes(), true)),
        Some(MimeBody::Bytes(bytes)) => Ok((bytes.clone(), false)),
        None => Ok((Vec::new(), true)),
    }
}

fn render_body(message: &MimeMessage, encoding: TransferEncoding) -> JsResult<String> {
    let (bytes, text_body) = body_bytes_and_text(message)?;
    match encoding {
        TransferEncoding::SevenBit => {
            if !bytes.is_ascii() {
                return Err(type_error("7bit MIME bodies must be ASCII"));
            }
            String::from_utf8(bytes).map_err(|_| type_error("7bit MIME body is not UTF-8"))
        }
        TransferEncoding::EightBit | TransferEncoding::Binary => {
            if text_body {
                String::from_utf8(bytes).map_err(|_| type_error("MIME body is not UTF-8"))
            } else {
                Err(type_error(
                    "explicit 8bit/binary Uint8Array bodies cannot be represented in a JS string",
                ))
            }
        }
        TransferEncoding::QuotedPrintable => Ok(qp_encode(&bytes)),
        TransferEncoding::Base64 => Ok(base64_encode_wrapped(&bytes)),
    }
}

fn boundary_seed(message: &MimeMessage, state: &mut u64) {
    for header in &message.headers {
        for byte in header.name.bytes().chain(header.value.bytes()) {
            *state ^= u64::from(byte);
            *state = state.wrapping_mul(0x100000001b3);
        }
    }
    if let Some(body) = &message.body {
        match body {
            MimeBody::Text(text) => {
                for byte in text.bytes() {
                    *state ^= u64::from(byte);
                    *state = state.wrapping_mul(0x100000001b3);
                }
            }
            MimeBody::Bytes(bytes) => {
                for byte in bytes {
                    *state ^= u64::from(*byte);
                    *state = state.wrapping_mul(0x100000001b3);
                }
            }
        }
    }
    for part in &message.parts {
        boundary_seed(part, state);
    }
}

fn message_contains_boundary(message: &MimeMessage, boundary: &str) -> bool {
    let body_has_boundary = match &message.body {
        Some(MimeBody::Text(text)) => text.contains(boundary),
        Some(MimeBody::Bytes(bytes)) => bytes
            .windows(boundary.len())
            .any(|window| window == boundary.as_bytes()),
        None => false,
    };
    body_has_boundary
        || message
            .parts
            .iter()
            .any(|part| message_contains_boundary(part, boundary))
}

fn generate_boundary(message: &MimeMessage) -> String {
    let mut seed = 0xcbf29ce484222325;
    boundary_seed(message, &mut seed);
    let mut attempt = 0u64;
    loop {
        let boundary = format!("mechanics-{seed:016x}-{attempt:04x}");
        if !message_contains_boundary(message, &boundary) {
            return boundary;
        }
        attempt = attempt.saturating_add(1);
    }
}

fn render_message(message: &MimeMessage, top_level: bool) -> JsResult<String> {
    let mut headers = message.headers.clone();
    if top_level && !has_header(&headers, MIME_VERSION) {
        headers.insert(
            0,
            Header {
                name: MIME_VERSION.to_owned(),
                value: "1.0".to_owned(),
            },
        );
    }

    if !message.parts.is_empty() {
        let boundary = header_value(&headers, CONTENT_TYPE)
            .and_then(parse_boundary)
            .unwrap_or_else(|| generate_boundary(message));
        let content_type = header_value(&headers, CONTENT_TYPE)
            .map_or(DEFAULT_MULTIPART_CONTENT_TYPE, content_type_base)
            .to_owned();
        set_header(
            &mut headers,
            CONTENT_TYPE,
            content_type_with_boundary(&content_type, &boundary),
        );

        let mut output = render_headers(&headers)?;
        output.push_str("\r\n");
        for part in &message.parts {
            output.push_str("--");
            output.push_str(&boundary);
            output.push_str("\r\n");
            output.push_str(&render_message(part, false)?);
            output.push_str("\r\n");
        }
        output.push_str("--");
        output.push_str(&boundary);
        output.push_str("--\r\n");
        return Ok(output);
    }

    if !has_header(&headers, CONTENT_TYPE) {
        set_header(
            &mut headers,
            CONTENT_TYPE,
            DEFAULT_TEXT_CONTENT_TYPE.to_owned(),
        );
    }
    let encoding = choose_encoding(message, &headers)?;
    set_header(
        &mut headers,
        CONTENT_TRANSFER_ENCODING,
        encoding.token().to_owned(),
    );
    let mut output = render_headers(&headers)?;
    output.push_str("\r\n");
    output.push_str(&render_body(message, encoding)?);
    Ok(output)
}

fn compose(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let message = collect_message(args.get_or_undefined(0).clone(), context)?;
    let output = render_message(&message, true)?;
    Ok(buffer_like::js_string_value(&output))
}

fn normalize_raw_message(raw: &str) -> String {
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

fn split_header_body(raw: &str) -> JsResult<(&str, &str)> {
    raw.split_once("\n\n")
        .ok_or_else(|| type_error("malformed MIME message: missing header/body separator"))
}

fn parse_headers(raw: &str) -> JsResult<Vec<Header>> {
    let mut headers = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_value = String::new();
    for line in raw.split('\n') {
        if line.starts_with(' ') || line.starts_with('\t') {
            if current_name.is_none() {
                return Err(type_error("malformed folded MIME header"));
            }
            current_value.push(' ');
            current_value.push_str(line.trim());
            continue;
        }
        if let Some(name) = current_name.take() {
            headers.push(Header {
                name,
                value: decode_encoded_words(current_value.trim())?,
            });
            current_value.clear();
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| type_error("malformed MIME header"))?;
        validate_header_name(name)?;
        current_name = Some(name.to_owned());
        current_value.push_str(value.trim());
    }
    if let Some(name) = current_name {
        headers.push(Header {
            name,
            value: decode_encoded_words(current_value.trim())?,
        });
    }
    Ok(headers)
}

fn decode_body(raw: &str, headers: &[Header]) -> JsResult<(Vec<u8>, bool)> {
    let encoding = header_value(headers, CONTENT_TRANSFER_ENCODING)
        .and_then(TransferEncoding::parse)
        .unwrap_or(TransferEncoding::SevenBit);
    let bytes = match encoding {
        TransferEncoding::SevenBit | TransferEncoding::EightBit | TransferEncoding::Binary => {
            raw.as_bytes().to_vec()
        }
        TransferEncoding::QuotedPrintable => qp_decode(raw, false)?,
        TransferEncoding::Base64 => base64_decode_liberal(raw)?,
    };
    let content_type = header_value(headers, CONTENT_TYPE).unwrap_or(DEFAULT_TEXT_CONTENT_TYPE);
    Ok((bytes, is_text_content_type(content_type)))
}

fn multipart_sections(body: &str, boundary: &str) -> JsResult<Vec<String>> {
    let marker = format!("--{boundary}");
    let close_marker = format!("--{boundary}--");
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut inside = false;
    let mut closed = false;
    for line in body.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        if trimmed == marker {
            if inside {
                sections.push(trim_part_section(&current));
                current.clear();
            }
            inside = true;
            continue;
        }
        if trimmed == close_marker {
            if inside {
                sections.push(trim_part_section(&current));
                current.clear();
            }
            closed = true;
            break;
        }
        if inside {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !closed {
        return Err(type_error(
            "malformed multipart MIME body: missing closing boundary",
        ));
    }
    Ok(sections)
}

fn trim_part_section(section: &str) -> String {
    section
        .strip_suffix('\n')
        .unwrap_or(section)
        .strip_suffix('\r')
        .unwrap_or_else(|| section.strip_suffix('\n').unwrap_or(section))
        .to_owned()
}

fn parse_message_raw(raw: &str) -> JsResult<ParsedMessage> {
    let (raw_headers, raw_body) = split_header_body(raw)?;
    let headers = parse_headers(raw_headers)?;
    let content_type = header_value(&headers, CONTENT_TYPE).unwrap_or(DEFAULT_TEXT_CONTENT_TYPE);
    if content_type_base(content_type)
        .to_ascii_lowercase()
        .starts_with("multipart/")
    {
        let boundary = parse_boundary(content_type)
            .ok_or_else(|| type_error("multipart MIME message is missing boundary"))?;
        let parts = multipart_sections(raw_body, &boundary)?
            .into_iter()
            .map(|part| parse_message_raw(&part))
            .collect::<JsResult<Vec<_>>>()?;
        Ok(ParsedMessage {
            headers,
            body: None,
            parts,
            text_body: false,
        })
    } else {
        let (body, text_body) = decode_body(raw_body, &headers)?;
        Ok(ParsedMessage {
            headers,
            body: Some(body),
            parts: Vec::new(),
            text_body,
        })
    }
}

struct ParsedMessage {
    headers: Vec<Header>,
    body: Option<Vec<u8>>,
    parts: Vec<ParsedMessage>,
    text_body: bool,
}

fn headers_to_js(headers: &[Header], context: &mut Context) -> JsResult<JsValue> {
    let object = boa_engine::object::JsObject::default(context.intrinsics());
    for header in headers {
        object.set(
            JsString::from(header.name.as_str()),
            buffer_like::js_string_value(&header.value),
            true,
            context,
        )?;
    }
    Ok(object.into())
}

fn parsed_to_js(message: ParsedMessage, context: &mut Context) -> JsResult<JsValue> {
    let object = boa_engine::object::JsObject::default(context.intrinsics());
    object.set(
        js_string!("headers"),
        headers_to_js(&message.headers, context)?,
        true,
        context,
    )?;
    if message.parts.is_empty() {
        let body = message.body.unwrap_or_default();
        let body_value = if message.text_body {
            let text =
                String::from_utf8(body).map_err(|_| type_error("MIME text body is not UTF-8"))?;
            buffer_like::js_string_value(&normalize_raw_message(&text))
        } else {
            buffer_like::bytes_to_uint8_array_value(&body, context)?
        };
        object.set(js_string!("body"), body_value, true, context)?;
    } else {
        let array = boa_engine::object::builtins::JsArray::new(context);
        for part in message.parts {
            array.push(parsed_to_js(part, context)?, context)?;
        }
        object.set(js_string!("parts"), array, true, context)?;
    }
    Ok(object.into())
}

fn parse(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let raw_value = args.get_or_undefined(0);
    let raw = if let Some(bytes) = buffer_like::try_extract_buffer_like_bytes(raw_value, context)? {
        String::from_utf8(bytes).map_err(|_| type_error("raw MIME bytes must be UTF-8"))?
    } else {
        raw_value.to_string(context)?.to_std_string_lossy()
    };
    let normalized = normalize_raw_message(&raw);
    let parsed = parse_message_raw(&normalized)?;
    parsed_to_js(parsed, context)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let compose = FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(compose))
        .length(1)
        .name("compose")
        .build();
    let parse = FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(parse))
        .length(1)
        .name("parse")
        .build();

    let module = Module::synthetic(
        &[js_string!("compose"), js_string!("parse")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, funcs, _ctx| {
                module.set_export(&js_string!("compose"), funcs.0.clone().into())?;
                module.set_export(&js_string!("parse"), funcs.1.clone().into())
            },
            (compose, parse),
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:mime"), module);
}
