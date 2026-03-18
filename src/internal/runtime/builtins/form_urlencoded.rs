use super::{CodecModuleSpec, register_codec_module};
use crate::internal::{executor::CustomModuleLoader, http::into_io_error, runtime::buffer_like};
use boa_engine::{Context, JsArgs, JsError, JsResult, JsValue, NativeFunction};
use serde_json::Value;
use std::{collections::BTreeMap, rc::Rc};
use url::form_urlencoded;

fn parse_form_record_arg(
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<BTreeMap<String, String>> {
    let value = args.get_or_undefined(0);
    let json = value
        .to_json(context)?
        .ok_or_else(|| buffer_like::js_type_error("record must be an object"))?;
    let Value::Object(_) = json else {
        return Err(buffer_like::js_type_error("record must be an object"));
    };
    serde_json::from_value(json)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)
}

fn form_urlencode_encode(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let record = parse_form_record_arg(args, context)?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (k, v) in record {
        serializer.append_pair(&k, &v);
    }
    let encoded = serializer.finish();
    Ok(buffer_like::js_string_value(&encoded))
}

fn form_urlencode_decode(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let params = args
        .get_or_undefined(0)
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| buffer_like::js_type_error("params must be a string"))?;
    let params = params.strip_prefix('?').unwrap_or(&params);
    let mut record = BTreeMap::new();
    for (k, v) in form_urlencoded::parse(params.as_bytes()) {
        record.insert(k.into_owned(), v.into_owned());
    }
    let value = Value::Object(
        record
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect::<serde_json::Map<String, Value>>(),
    );
    JsValue::from_json(&value, context)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    register_codec_module(
        loader,
        context,
        CodecModuleSpec {
            module_name: "mechanics:form-urlencoded",
            encode_name: "encode",
            encode_fn: NativeFunction::from_fn_ptr(form_urlencode_encode),
            encode_length: 1,
            decode_name: "decode",
            decode_fn: NativeFunction::from_fn_ptr(form_urlencode_decode),
            decode_length: 1,
        },
    );
}
