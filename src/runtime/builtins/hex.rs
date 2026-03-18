use super::{
    CodecModuleSpec, register_codec_module, required_buffer_like_arg, required_string_arg,
};
use crate::{executor::CustomModuleLoader, http::into_io_error, runtime::buffer_like};
use boa_engine::{Context, JsError, JsResult, JsValue, NativeFunction};
use std::rc::Rc;

fn hex_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let encoded = hex::encode(bytes);
    Ok(buffer_like::js_string_value(&encoded))
}

fn hex_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let encoded = required_string_arg(args, 0, "encoded")?;
    let decoded = hex::decode(encoded)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)?;
    buffer_like::bytes_to_uint8_array_value(&decoded, context)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    register_codec_module(
        loader,
        context,
        CodecModuleSpec {
            module_name: "mechanics:hex",
            encode_name: "encode",
            encode_fn: NativeFunction::from_fn_ptr(hex_encode),
            encode_length: 1,
            decode_name: "decode",
            decode_fn: NativeFunction::from_fn_ptr(hex_decode),
            decode_length: 1,
        },
    );
}
