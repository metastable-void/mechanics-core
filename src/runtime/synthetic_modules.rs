use super::{MechanicsState, buffer_like};
use crate::{
    executor::CustomModuleLoader,
    http::{EndpointCallBody, EndpointCallOptions, EndpointResponseBody, into_io_error},
};
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsResult, JsString, JsValue, Module, NativeFunction,
    js_string,
    module::SyntheticModuleInitializer,
    object::{FunctionObjectBuilder, JsObject},
};
use data_encoding::{
    BASE32, BASE32_NOPAD, BASE32HEX, BASE32HEX_NOPAD, BASE64, BASE64_NOPAD, BASE64URL,
    BASE64URL_NOPAD,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    rc::Rc,
};
use url::form_urlencoded;

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
    let body_value = options.get(js_string!("body"), context)?;
    let body = if body_value.is_undefined() || body_value.is_null() {
        EndpointCallBody::Absent
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
        body,
    })
}

fn endpoint_response_to_js_value(
    response: EndpointResponseBody,
    context: &mut Context,
) -> JsResult<JsValue> {
    match response {
        EndpointResponseBody::Json(v) => JsValue::from_json(&v, context),
        EndpointResponseBody::Utf8(s) => Ok(buffer_like::js_string_value(&s)),
        EndpointResponseBody::Bytes(bytes) => {
            buffer_like::bytes_to_uint8_array_value(&bytes, context)
        }
        EndpointResponseBody::Empty => Ok(JsValue::null()),
    }
}

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
    let params = required_string_arg(args, 0, "params")?;
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

fn parse_base64_variant(
    args: &[JsValue],
    index: usize,
    default: &'static str,
) -> JsResult<&'static str> {
    let value = args.get_or_undefined(index);
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

fn parse_base32_variant(
    args: &[JsValue],
    index: usize,
    default: &'static str,
) -> JsResult<&'static str> {
    let value = args.get_or_undefined(index);
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

fn rand_fill_random(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    buffer_like::fill_random_buffer_like(args.get_or_undefined(0), context)?;
    Ok(JsValue::undefined())
}

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

pub(super) fn install_synthetic_modules(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let endpoint = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_async_fn(async |_, args, ctx| {
            let endpoint = args
                .get_or_undefined(0)
                .as_string()
                .ok_or(JsError::from_native(
                    JsNativeError::typ().with_message("endpoint is not a string"),
                ))?;
            let options = args.get_or_undefined(1).clone();
            let req_options = parse_endpoint_call_options_js(options, &mut ctx.borrow_mut())?;

            let state = {
                let ctx_ref = ctx.borrow();
                ctx_ref
                    .get_data::<MechanicsState>()
                    .cloned()
                    .ok_or(JsError::from_native(
                        JsNativeError::typ().with_message("Invalid state"),
                    ))?
            };
            let endpoint_name = endpoint.to_std_string_lossy();
            let endpoint =
                state
                    .config
                    .endpoints
                    .get(&endpoint_name)
                    .ok_or(JsError::from_native(
                        JsNativeError::typ().with_message("Endpoint not found"),
                    ))?;

            let res = endpoint
                .execute(
                    state.reqwest(),
                    state.default_timeout_ms(),
                    state.default_response_max_bytes(),
                    &req_options,
                )
                .await
                .map_err(JsError::from_rust)?;

            endpoint_response_to_js_value(res, &mut ctx.borrow_mut())
        }),
    )
    .length(2)
    .name("endpoint")
    .build();

    let endpoint_module = Module::synthetic(
        &[js_string!("default")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, f, _ctx| module.set_export(&js_string!("default"), f.clone().into()),
            endpoint,
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:endpoint"), endpoint_module);

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

    let fill_random = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(rand_fill_random),
    )
    .length(1)
    .name("fillRandom")
    .build();
    let rand_module = Module::synthetic(
        &[js_string!("default")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, f, _ctx| module.set_export(&js_string!("default"), f.clone().into()),
            fill_random,
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:rand"), rand_module);
}
