use super::{
    CodecModuleSpec, register_codec_module, required_buffer_like_arg, required_string_arg,
};
use crate::{executor::CustomModuleLoader, http::into_io_error, runtime::buffer_like};
use boa_engine::{Context, JsError, JsResult, JsValue, NativeFunction};
use data_encoding::{BASE64, BASE64_NOPAD, BASE64URL, BASE64URL_NOPAD};
use std::rc::Rc;

fn parse_base64_variant(
    args: &[JsValue],
    index: usize,
    default: &'static str,
) -> JsResult<&'static str> {
    let value = args.get(index).cloned().unwrap_or_else(JsValue::undefined);
    if value.is_undefined() {
        return Ok(default);
    }
    let Some(s) = value.as_string() else {
        return Err(buffer_like::js_type_error("variant must be a string"));
    };
    match s.to_std_string_lossy().as_str() {
        "base64" => Ok("base64"),
        "base64url" => Ok("base64url"),
        _ => Err(buffer_like::js_type_error(
            "variant must be 'base64' or 'base64url'",
        )),
    }
}

fn base64_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let variant = parse_base64_variant(args, 1, "base64")?;
    let encoded = match variant {
        "base64" => BASE64.encode(&bytes),
        "base64url" => BASE64URL_NOPAD.encode(&bytes),
        _ => return Err(buffer_like::js_type_error("invalid base64 variant")),
    };
    Ok(buffer_like::js_string_value(&encoded))
}

fn base64_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let encoded = required_string_arg(args, 0, "encoded")?;
    let variant = parse_base64_variant(args, 1, "base64")?;
    let decoded = match variant {
        "base64" => BASE64
            .decode(encoded.as_bytes())
            .or_else(|_| BASE64_NOPAD.decode(encoded.as_bytes())),
        "base64url" => BASE64URL
            .decode(encoded.as_bytes())
            .or_else(|_| BASE64URL_NOPAD.decode(encoded.as_bytes())),
        _ => return Err(buffer_like::js_type_error("invalid base64 variant")),
    }
    .map_err(into_io_error)
    .map_err(JsError::from_rust)?;
    buffer_like::bytes_to_uint8_array_value(&decoded, context)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    register_codec_module(
        loader,
        context,
        CodecModuleSpec {
            module_name: "mechanics:base64",
            encode_name: "encode",
            encode_fn: NativeFunction::from_fn_ptr(base64_encode),
            encode_length: 2,
            decode_name: "decode",
            decode_fn: NativeFunction::from_fn_ptr(base64_decode),
            decode_length: 2,
        },
    );
}
