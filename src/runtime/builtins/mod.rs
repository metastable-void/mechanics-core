use super::{MechanicsState, buffer_like};
use crate::{
    executor::CustomModuleLoader,
    http::{
        EndpointCallBody, EndpointCallOptions, EndpointResponse, EndpointResponseBody,
        into_io_error,
    },
};
use boa_engine::{
    Context, JsArgs, JsError, JsResult, JsString, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer,
    object::{FunctionObjectBuilder, JsObject},
};
use serde_json::Value;
use std::{collections::HashMap, rc::Rc};

mod base32;
mod base64;
mod endpoint;
mod form_urlencoded;
mod hex;
mod rand;
mod uuid;

struct CodecModuleSpec {
    module_name: &'static str,
    encode_name: &'static str,
    encode_fn: NativeFunction,
    encode_length: usize,
    decode_name: &'static str,
    decode_fn: NativeFunction,
    decode_length: usize,
}

fn register_codec_module(
    loader: &Rc<CustomModuleLoader>,
    context: &mut Context,
    spec: CodecModuleSpec,
) {
    let CodecModuleSpec {
        module_name,
        encode_name,
        encode_fn,
        encode_length,
        decode_name,
        decode_fn,
        decode_length,
    } = spec;

    let encode = FunctionObjectBuilder::new(context.realm(), encode_fn)
        .length(encode_length)
        .name(encode_name)
        .build();
    let decode = FunctionObjectBuilder::new(context.realm(), decode_fn)
        .length(decode_length)
        .name(decode_name)
        .build();

    let module = Module::synthetic(
        &[js_string!(encode_name), js_string!(decode_name)],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            move |module, funcs, _ctx| {
                module.set_export(&js_string!(encode_name), funcs.0.clone().into())?;
                module.set_export(&js_string!(decode_name), funcs.1.clone().into())
            },
            (encode, decode),
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!(module_name), module);
}

fn parse_string_map_field(
    options: &JsObject,
    key: JsString,
    field_name: &'static str,
    context: &mut Context,
) -> JsResult<HashMap<String, String>> {
    let value = options.get(key, context)?;
    if value.is_undefined() || value.is_null() {
        return Ok(HashMap::new());
    }
    let json = value
        .to_json(context)?
        .ok_or_else(|| buffer_like::js_type_error(format!("{field_name} must be an object")))?;
    let Value::Object(_) = json else {
        return Err(buffer_like::js_type_error(format!(
            "{field_name} must be an object"
        )));
    };
    serde_json::from_value(json)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)
}

fn required_string_arg(args: &[JsValue], index: usize, name: &str) -> JsResult<String> {
    args.get_or_undefined(index)
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| buffer_like::js_type_error(format!("{name} must be a string")))
}

fn required_buffer_like_arg(
    args: &[JsValue],
    index: usize,
    name: &str,
    context: &mut Context,
) -> JsResult<Vec<u8>> {
    buffer_like::try_extract_buffer_like_bytes(args.get_or_undefined(index), context)?.ok_or_else(
        || {
            buffer_like::js_type_error(format!(
                "{name} must be a TypedArray, ArrayBuffer, or DataView"
            ))
        },
    )
}

fn parse_endpoint_call_options_js(
    value: JsValue,
    context: &mut Context,
) -> JsResult<EndpointCallOptions> {
    if value.is_undefined() || value.is_null() {
        return Ok(EndpointCallOptions::default());
    }

    let Some(options) = value.as_object() else {
        return Err(buffer_like::js_type_error(
            "endpoint options must be an object or null/undefined",
        ));
    };

    let url_params =
        parse_string_map_field(&options, js_string!("urlParams"), "urlParams", context)?;
    let queries = parse_string_map_field(&options, js_string!("queries"), "queries", context)?;
    let headers = parse_string_map_field(&options, js_string!("headers"), "headers", context)?;
    let body_value = options.get(js_string!("body"), context)?;
    let body = if body_value.is_undefined() {
        EndpointCallBody::Absent
    } else if body_value.is_null() {
        EndpointCallBody::Json(Value::Null)
    } else if let Some(string) = body_value.as_string() {
        EndpointCallBody::Utf8(string.to_std_string_lossy())
    } else if let Some(bytes) = buffer_like::try_extract_buffer_like_bytes(&body_value, context)? {
        EndpointCallBody::Bytes(bytes)
    } else {
        let body_json = body_value
            .to_json(context)?
            .ok_or_else(|| buffer_like::js_type_error("body is not JSON-convertible"))?;
        EndpointCallBody::Json(body_json)
    };

    Ok(EndpointCallOptions {
        url_params,
        queries,
        headers,
        body,
    })
}

fn endpoint_response_to_js_value(
    response: EndpointResponse,
    context: &mut Context,
) -> JsResult<JsValue> {
    let body = match response.body {
        EndpointResponseBody::Json(v) => JsValue::from_json(&v, context),
        EndpointResponseBody::Utf8(s) => Ok(buffer_like::js_string_value(&s)),
        EndpointResponseBody::Bytes(bytes) => {
            buffer_like::bytes_to_uint8_array_value(&bytes, context)
        }
        EndpointResponseBody::Empty => Ok(JsValue::null()),
    }?;

    let headers_value = Value::Object(
        response
            .headers
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect(),
    );
    let headers = JsValue::from_json(&headers_value, context)?;

    let object = JsObject::default(context.intrinsics());
    object.set(js_string!("body"), body, true, context)?;
    object.set(js_string!("headers"), headers, true, context)?;
    object.set(
        js_string!("status"),
        JsValue::new(i32::from(response.status)),
        true,
        context,
    )?;
    object.set(js_string!("ok"), JsValue::new(response.ok), true, context)?;
    Ok(object.into())
}

pub(super) fn bundle_builtin_modules(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    endpoint::register(loader, context);
    form_urlencoded::register(loader, context);
    base64::register(loader, context);
    hex::register(loader, context);
    base32::register(loader, context);
    rand::register(loader, context);
    uuid::register(loader, context);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoint_response_to_js_value_includes_status_and_ok() {
        let mut context = Context::default();
        let mut headers = HashMap::new();
        headers.insert("x-trace-id".to_owned(), "abc".to_owned());
        let response = EndpointResponse {
            body: EndpointResponseBody::Json(json!({"n": 1})),
            headers,
            status: 202,
            ok: false,
        };

        let value =
            endpoint_response_to_js_value(response, &mut context).expect("convert response");
        let as_json = value
            .to_json(&mut context)
            .expect("json conversion should succeed")
            .expect("converted response should be JSON object");
        assert_eq!(as_json["status"], json!(202));
        assert_eq!(as_json["ok"], json!(false));
        assert_eq!(as_json["body"]["n"], json!(1));
        assert_eq!(as_json["headers"]["x-trace-id"], json!("abc"));
    }
}
