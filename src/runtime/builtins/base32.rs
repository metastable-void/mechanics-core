use super::{
    CodecModuleSpec, register_codec_module, required_buffer_like_arg, required_string_arg,
};
use crate::{executor::CustomModuleLoader, http::into_io_error, runtime::buffer_like};
use boa_engine::{Context, JsError, JsResult, JsValue, NativeFunction};
use data_encoding::{BASE32, BASE32_NOPAD, BASE32HEX, BASE32HEX_NOPAD};
use std::rc::Rc;

fn parse_base32_variant(
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
        "base32" => Ok("base32"),
        "base32hex" => Ok("base32hex"),
        _ => Err(buffer_like::js_type_error(
            "variant must be 'base32' or 'base32hex'",
        )),
    }
}

fn base32_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let variant = parse_base32_variant(args, 1, "base32")?;
    let encoded = match variant {
        "base32" => BASE32.encode(&bytes),
        "base32hex" => BASE32HEX.encode(&bytes),
        _ => return Err(buffer_like::js_type_error("invalid base32 variant")),
    };
    Ok(buffer_like::js_string_value(&encoded))
}

fn base32_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let encoded = required_string_arg(args, 0, "encoded")?;
    let encoded_upper = encoded.to_ascii_uppercase();
    let variant = parse_base32_variant(args, 1, "base32")?;
    let decoded = match variant {
        "base32" => BASE32
            .decode(encoded_upper.as_bytes())
            .or_else(|_| BASE32_NOPAD.decode(encoded_upper.as_bytes())),
        "base32hex" => BASE32HEX
            .decode(encoded_upper.as_bytes())
            .or_else(|_| BASE32HEX_NOPAD.decode(encoded_upper.as_bytes())),
        _ => return Err(buffer_like::js_type_error("invalid base32 variant")),
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
            module_name: "mechanics:base32",
            encode_name: "encode",
            encode_fn: NativeFunction::from_fn_ptr(base32_encode),
            encode_length: 2,
            decode_name: "decode",
            decode_fn: NativeFunction::from_fn_ptr(base32_decode),
            decode_length: 2,
        },
    );
}
