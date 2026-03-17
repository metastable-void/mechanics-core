use crate::{
    error::MechanicsError,
    executor::{CustomModuleLoader, Queue},
    http::{
        EndpointCallBody, EndpointCallOptions, EndpointResponseBody, MechanicsConfig, into_io_error,
    },
    job::{MechanicsExecutionLimits, MechanicsJob},
};
use boa_engine::{
    Context, JsArgs, JsData, JsError, JsNativeError, JsResult, JsString, JsValue, Module,
    NativeFunction, Source, Trace,
    builtins::promise::{OperationType, PromiseState},
    context::{ContextBuilder, HostHooks, time::JsInstant},
    js_string,
    module::SyntheticModuleInitializer,
    object::{
        FunctionObjectBuilder, JsObject,
        builtins::{JsArrayBuffer, JsDataView, JsPromise, JsTypedArray, JsUint8Array},
    },
};
use boa_gc::Finalize;
use data_encoding::{
    BASE32, BASE32_NOPAD, BASE32HEX, BASE32HEX_NOPAD, BASE64, BASE64_NOPAD, BASE64URL,
    BASE64URL_NOPAD,
};
use serde_json::Value;
use std::{cell::Cell, collections::HashMap, rc::Rc, sync::Arc};
use url::form_urlencoded;

#[derive(Default, Debug)]
struct RuntimeHostHooks {
    pending_unhandled_rejections: Cell<usize>,
}

impl RuntimeHostHooks {
    fn clear(&self) {
        self.pending_unhandled_rejections.set(0);
    }

    fn has_unhandled_rejections(&self) -> bool {
        self.pending_unhandled_rejections.get() > 0
    }
}

impl HostHooks for RuntimeHostHooks {
    fn promise_rejection_tracker(
        &self,
        _promise: &JsObject,
        operation: OperationType,
        _context: &mut Context,
    ) {
        let pending = self.pending_unhandled_rejections.get();
        match operation {
            OperationType::Reject => {
                self.pending_unhandled_rejections
                    .set(pending.saturating_add(1));
            }
            OperationType::Handle => {
                self.pending_unhandled_rejections
                    .set(pending.saturating_sub(1));
            }
        }
    }
}

#[derive(JsData, Finalize, Trace, Clone, Debug)]
pub(crate) struct MechanicsState {
    #[unsafe_ignore_trace]
    config: Arc<MechanicsConfig>,

    #[unsafe_ignore_trace]
    reqwest_client: reqwest::Client,

    #[unsafe_ignore_trace]
    default_timeout_ms: Option<u64>,
}

impl MechanicsState {
    pub(crate) fn new(
        config: Arc<MechanicsConfig>,
        client: reqwest::Client,
        default_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            config,
            reqwest_client: client,
            default_timeout_ms,
        }
    }

    pub(crate) fn reqwest(&self) -> reqwest::Client {
        self.reqwest_client.clone()
    }

    pub(crate) fn default_timeout_ms(&self) -> Option<u64> {
        self.default_timeout_ms
    }
}

fn js_type_error(message: impl AsRef<str>) -> JsError {
    JsError::from_native(JsNativeError::typ().with_message(message.as_ref().to_owned()))
}

fn js_range_error(message: impl AsRef<str>) -> JsError {
    JsError::from_native(JsNativeError::range().with_message(message.as_ref().to_owned()))
}

fn parse_string_map_field(
    options: &JsObject,
    field_name: &'static str,
    context: &mut Context,
) -> JsResult<HashMap<String, String>> {
    let key = match field_name {
        "urlParams" => js_string!("urlParams"),
        "queries" => js_string!("queries"),
        _ => return Err(js_type_error("unknown endpoint options field")),
    };
    let value = options.get(key, context)?;
    if value.is_undefined() || value.is_null() {
        return Ok(HashMap::new());
    }
    let json = value
        .to_json(context)?
        .ok_or_else(|| js_type_error(format!("{field_name} must be an object")))?;
    let Value::Object(_) = json else {
        return Err(js_type_error(format!("{field_name} must be an object")));
    };
    serde_json::from_value(json)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)
}

fn read_bytes_from_array_buffer_range(
    buffer: &JsArrayBuffer,
    offset: usize,
    len: usize,
) -> JsResult<Vec<u8>> {
    let data = buffer
        .data()
        .ok_or_else(|| js_type_error("ArrayBuffer is detached"))?;
    let end = offset
        .checked_add(len)
        .ok_or_else(|| js_range_error("buffer range overflow"))?;
    let slice = data
        .get(offset..end)
        .ok_or_else(|| js_range_error("buffer range is out of bounds"))?;
    Ok(slice.to_vec())
}

fn try_extract_buffer_like_bytes(
    value: &JsValue,
    context: &mut Context,
) -> JsResult<Option<Vec<u8>>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    if let Ok(typed) = JsTypedArray::from_object(object.clone()) {
        let buffer = typed.buffer(context)?;
        let Some(buffer_object) = buffer.as_object() else {
            return Err(js_type_error("TypedArray buffer is not an ArrayBuffer"));
        };
        let array_buffer = JsArrayBuffer::from_object(buffer_object.clone()).map_err(|_| {
            js_type_error("TypedArray backed by SharedArrayBuffer is not supported")
        })?;
        let offset = typed.byte_offset(context)?;
        let len = typed.byte_length(context)?;
        return read_bytes_from_array_buffer_range(&array_buffer, offset, len).map(Some);
    }

    if let Ok(data_view) = JsDataView::from_object(object.clone()) {
        let buffer = data_view.buffer(context)?;
        let Some(buffer_object) = buffer.as_object() else {
            return Err(js_type_error("DataView buffer is not an ArrayBuffer"));
        };
        let array_buffer = JsArrayBuffer::from_object(buffer_object.clone())
            .map_err(|_| js_type_error("DataView backed by SharedArrayBuffer is not supported"))?;
        let offset = data_view.byte_offset(context)? as usize;
        let len = data_view.byte_length(context)? as usize;
        return read_bytes_from_array_buffer_range(&array_buffer, offset, len).map(Some);
    }

    if let Ok(array_buffer) = JsArrayBuffer::from_object(object) {
        let len = array_buffer.byte_length();
        return read_bytes_from_array_buffer_range(&array_buffer, 0, len).map(Some);
    }

    Ok(None)
}

fn fill_random_in_array_buffer_range(
    buffer: &JsArrayBuffer,
    offset: usize,
    len: usize,
) -> JsResult<()> {
    let mut data = buffer
        .data_mut()
        .ok_or_else(|| js_type_error("ArrayBuffer is detached"))?;
    let end = offset
        .checked_add(len)
        .ok_or_else(|| js_range_error("buffer range overflow"))?;
    let target = data
        .get_mut(offset..end)
        .ok_or_else(|| js_range_error("buffer range is out of bounds"))?;
    getrandom::fill(target)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)
}

fn fill_random_buffer_like(value: &JsValue, context: &mut Context) -> JsResult<()> {
    let Some(object) = value.as_object() else {
        return Err(js_type_error(
            "bufferLike must be a TypedArray, ArrayBuffer, or DataView",
        ));
    };

    if let Ok(typed) = JsTypedArray::from_object(object.clone()) {
        let buffer = typed.buffer(context)?;
        let Some(buffer_object) = buffer.as_object() else {
            return Err(js_type_error("TypedArray buffer is not an ArrayBuffer"));
        };
        let array_buffer = JsArrayBuffer::from_object(buffer_object.clone()).map_err(|_| {
            js_type_error("TypedArray backed by SharedArrayBuffer is not supported")
        })?;
        return fill_random_in_array_buffer_range(
            &array_buffer,
            typed.byte_offset(context)?,
            typed.byte_length(context)?,
        );
    }

    if let Ok(data_view) = JsDataView::from_object(object.clone()) {
        let buffer = data_view.buffer(context)?;
        let Some(buffer_object) = buffer.as_object() else {
            return Err(js_type_error("DataView buffer is not an ArrayBuffer"));
        };
        let array_buffer = JsArrayBuffer::from_object(buffer_object.clone())
            .map_err(|_| js_type_error("DataView backed by SharedArrayBuffer is not supported"))?;
        return fill_random_in_array_buffer_range(
            &array_buffer,
            data_view.byte_offset(context)? as usize,
            data_view.byte_length(context)? as usize,
        );
    }

    if let Ok(array_buffer) = JsArrayBuffer::from_object(object) {
        return fill_random_in_array_buffer_range(&array_buffer, 0, array_buffer.byte_length());
    }

    Err(js_type_error(
        "bufferLike must be a TypedArray, ArrayBuffer, or DataView",
    ))
}

fn required_string_arg(args: &[JsValue], index: usize, name: &str) -> JsResult<String> {
    args.get_or_undefined(index)
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| js_type_error(format!("{name} must be a string")))
}

fn required_buffer_like_arg(
    args: &[JsValue],
    index: usize,
    name: &str,
    context: &mut Context,
) -> JsResult<Vec<u8>> {
    try_extract_buffer_like_bytes(args.get_or_undefined(index), context)?.ok_or_else(|| {
        js_type_error(format!(
            "{name} must be a TypedArray, ArrayBuffer, or DataView"
        ))
    })
}

fn bytes_to_uint8_array_value(bytes: &[u8], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsUint8Array::from_iter(bytes.iter().copied(), context)?.into())
}

fn parse_endpoint_call_options_js(
    value: JsValue,
    context: &mut Context,
) -> JsResult<EndpointCallOptions> {
    if value.is_undefined() || value.is_null() {
        return Ok(EndpointCallOptions::default());
    }

    let Some(options) = value.as_object() else {
        return Err(js_type_error(
            "endpoint options must be an object or null/undefined",
        ));
    };

    let url_params = parse_string_map_field(&options, "urlParams", context)?;
    let queries = parse_string_map_field(&options, "queries", context)?;
    let body_value = options.get(js_string!("body"), context)?;
    let body = if body_value.is_undefined() || body_value.is_null() {
        EndpointCallBody::Absent
    } else if let Some(string) = body_value.as_string() {
        EndpointCallBody::Utf8(string.to_std_string_lossy())
    } else if let Some(bytes) = try_extract_buffer_like_bytes(&body_value, context)? {
        EndpointCallBody::Bytes(bytes)
    } else {
        let body_json = body_value
            .to_json(context)?
            .ok_or_else(|| js_type_error("body is not JSON-convertible"))?;
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
        EndpointResponseBody::Utf8(s) => Ok(JsValue::from(JsString::from(s.as_str()))),
        EndpointResponseBody::Bytes(bytes) => bytes_to_uint8_array_value(&bytes, context),
        EndpointResponseBody::Empty => Ok(JsValue::null()),
    }
}

fn parse_form_record_arg(
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<HashMap<String, String>> {
    let value = args.get_or_undefined(0);
    let json = value
        .to_json(context)?
        .ok_or_else(|| js_type_error("record must be an object"))?;
    let Value::Object(_) = json else {
        return Err(js_type_error("record must be an object"));
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
    Ok(JsValue::from(JsString::from(encoded.as_str())))
}

fn form_urlencode_decode(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let params = required_string_arg(args, 0, "params")?;
    let params = params.strip_prefix('?').unwrap_or(&params);
    let mut record = HashMap::new();
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
        return Err(js_type_error("variant must be a string"));
    };
    match s.to_std_string_lossy().as_str() {
        "base64" => Ok("base64"),
        "base64url" => Ok("base64url"),
        _ => Err(js_type_error("variant must be 'base64' or 'base64url'")),
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
        return Err(js_type_error("variant must be a string"));
    };
    match s.to_std_string_lossy().as_str() {
        "base32" => Ok("base32"),
        "base32hex" => Ok("base32hex"),
        _ => Err(js_type_error("variant must be 'base32' or 'base32hex'")),
    }
}

fn base64_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let variant = parse_base64_variant(args, 1, "base64")?;
    let encoded = match variant {
        "base64" => BASE64.encode(&bytes),
        "base64url" => BASE64URL_NOPAD.encode(&bytes),
        _ => return Err(js_type_error("invalid base64 variant")),
    };
    Ok(JsValue::from(JsString::from(encoded.as_str())))
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
        _ => return Err(js_type_error("invalid base64 variant")),
    }
    .map_err(into_io_error)
    .map_err(JsError::from_rust)?;
    bytes_to_uint8_array_value(&decoded, context)
}

fn hex_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let encoded = hex::encode(bytes);
    Ok(JsValue::from(JsString::from(encoded.as_str())))
}

fn hex_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let encoded = required_string_arg(args, 0, "encoded")?;
    let decoded = hex::decode(encoded)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)?;
    bytes_to_uint8_array_value(&decoded, context)
}

fn base32_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = required_buffer_like_arg(args, 0, "bufferLike", context)?;
    let variant = parse_base32_variant(args, 1, "base32")?;
    let encoded = match variant {
        "base32" => BASE32.encode(&bytes),
        "base32hex" => BASE32HEX.encode(&bytes),
        _ => return Err(js_type_error("invalid base32 variant")),
    };
    Ok(JsValue::from(JsString::from(encoded.as_str())))
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
        _ => return Err(js_type_error("invalid base32 variant")),
    }
    .map_err(into_io_error)
    .map_err(JsError::from_rust)?;
    bytes_to_uint8_array_value(&decoded, context)
}

fn rand_fill_random(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    fill_random_buffer_like(args.get_or_undefined(0), context)?;
    Ok(JsValue::undefined())
}

/// Script runtime that hosts a Boa context and exposes helper modules.
pub(crate) struct RuntimeInternal {
    ctx: Context,
    reqwest_client: reqwest::Client,
    queue: Rc<Queue>,
    hooks: Rc<RuntimeHostHooks>,
    execution_limits: MechanicsExecutionLimits,
    default_endpoint_timeout_ms: Option<u64>,
}

impl RuntimeInternal {
    fn compute_deadline(
        context: &Context,
        max_execution_time: std::time::Duration,
    ) -> JsResult<JsInstant> {
        let now_ms = u128::from(context.clock().now().millis_since_epoch());
        let timeout_ms = max_execution_time.as_millis();
        let deadline_ms = now_ms.checked_add(timeout_ms).ok_or(JsError::from_native(
            JsNativeError::range().with_message("Configured max_execution_time is too large"),
        ))?;
        if deadline_ms > u128::from(u64::MAX) {
            return Err(JsError::from_native(
                JsNativeError::range().with_message("Configured max_execution_time is too large"),
            ));
        }
        let deadline_ms = deadline_ms as u64;
        Ok(JsInstant::new(
            deadline_ms / 1000,
            ((deadline_ms % 1000) * 1_000_000) as u32,
        ))
    }

    /// Builds a Boa context, injects runtime state, and exposes `mechanics:endpoint`.
    pub(crate) fn new_with_client(reqwest_client: reqwest::Client) -> Result<Self, MechanicsError> {
        let queue = Rc::new(Queue::new().map_err(|e| {
            MechanicsError::runtime_pool(format!("failed to initialize async job runtime: {e}"))
        })?);
        let hooks = Rc::new(RuntimeHostHooks::default());

        let loader = Rc::new(CustomModuleLoader::new());
        let mut context = ContextBuilder::new()
            .job_executor(queue.clone())
            .module_loader(loader.clone())
            .host_hooks(hooks.clone())
            .build()
            .map_err(|e| {
                MechanicsError::runtime_pool(format!(
                    "failed to initialize JavaScript context: {e}"
                ))
            })?;

        let endpoint_fn = FunctionObjectBuilder::new(
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
                    .execute(state.reqwest(), state.default_timeout_ms(), &req_options)
                    .await
                    .map_err(JsError::from_rust)?;

                let res = endpoint_response_to_js_value(res, &mut ctx.borrow_mut())?;
                Ok(res)
            }),
        )
        .length(2)
        .name("endpoint")
        .build();

        let endpoint_module = Module::synthetic(
            &[js_string!("default")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, ept, _ctx| module.set_export(&js_string!("default"), ept.clone().into()),
                endpoint_fn,
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:endpoint"), endpoint_module);

        let form_encode_fn = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_fn_ptr(form_urlencode_encode),
        )
        .length(1)
        .name("encode")
        .build();
        let form_decode_fn = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_fn_ptr(form_urlencode_decode),
        )
        .length(1)
        .name("decode")
        .build();
        let form_module = Module::synthetic(
            &[js_string!("encode"), js_string!("decode")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, funcs, _ctx| {
                    module.set_export(&js_string!("encode"), funcs.0.clone().into())?;
                    module.set_export(&js_string!("decode"), funcs.1.clone().into())
                },
                (form_encode_fn, form_decode_fn),
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:form-urlencoded"), form_module);

        let base64_encode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(base64_encode))
                .length(2)
                .name("encode")
                .build();
        let base64_decode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(base64_decode))
                .length(2)
                .name("decode")
                .build();
        let base64_module = Module::synthetic(
            &[js_string!("encode"), js_string!("decode")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, funcs, _ctx| {
                    module.set_export(&js_string!("encode"), funcs.0.clone().into())?;
                    module.set_export(&js_string!("decode"), funcs.1.clone().into())
                },
                (base64_encode_fn, base64_decode_fn),
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:base64"), base64_module);

        let hex_encode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(hex_encode))
                .length(1)
                .name("encode")
                .build();
        let hex_decode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(hex_decode))
                .length(1)
                .name("decode")
                .build();
        let hex_module = Module::synthetic(
            &[js_string!("encode"), js_string!("decode")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, funcs, _ctx| {
                    module.set_export(&js_string!("encode"), funcs.0.clone().into())?;
                    module.set_export(&js_string!("decode"), funcs.1.clone().into())
                },
                (hex_encode_fn, hex_decode_fn),
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:hex"), hex_module);

        let base32_encode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(base32_encode))
                .length(2)
                .name("encode")
                .build();
        let base32_decode_fn =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(base32_decode))
                .length(2)
                .name("decode")
                .build();
        let base32_module = Module::synthetic(
            &[js_string!("encode"), js_string!("decode")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, funcs, _ctx| {
                    module.set_export(&js_string!("encode"), funcs.0.clone().into())?;
                    module.set_export(&js_string!("decode"), funcs.1.clone().into())
                },
                (base32_encode_fn, base32_decode_fn),
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:base32"), base32_module);

        let rand_fill_fn = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_fn_ptr(rand_fill_random),
        )
        .length(1)
        .name("fillRandom")
        .build();
        let rand_module = Module::synthetic(
            &[js_string!("default")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, fill_random, _ctx| {
                    module.set_export(&js_string!("default"), fill_random.clone().into())
                },
                rand_fill_fn,
            ),
            None,
            None,
            &mut context,
        );
        loader.define_module(js_string!("mechanics:rand"), rand_module);

        Ok(Self {
            ctx: context,
            reqwest_client,
            queue,
            hooks,
            execution_limits: MechanicsExecutionLimits::default(),
            default_endpoint_timeout_ms: None,
        })
    }

    pub(crate) fn set_execution_limits(&mut self, limits: MechanicsExecutionLimits) {
        self.execution_limits = limits;
    }

    pub(crate) fn set_default_endpoint_timeout_ms(&mut self, timeout_ms: Option<u64>) {
        self.default_endpoint_timeout_ms = timeout_ms;
    }

    /// Parses and evaluates a module, invokes its default export, and returns the JS result.
    pub(crate) fn run_source_inner(&mut self, job: MechanicsJob) -> JsResult<JsValue> {
        let arg = job.arg;
        let config = job.config;
        let source = job.mod_source;
        self.hooks.clear();
        let state = MechanicsState::new(
            config,
            self.reqwest_client.clone(),
            self.default_endpoint_timeout_ms,
        );

        let deadline = Self::compute_deadline(&self.ctx, self.execution_limits.max_execution_time)?;
        let source = source.as_ref();
        let ctx = &mut self.ctx;

        let runtime_limits = ctx.runtime_limits_mut();
        runtime_limits.set_loop_iteration_limit(self.execution_limits.max_loop_iterations);
        runtime_limits.set_recursion_limit(self.execution_limits.max_recursion_depth);
        runtime_limits.set_stack_size_limit(self.execution_limits.max_stack_size);

        self.queue.set_deadline(Some(deadline));
        ctx.insert_data(state);

        let source = Source::from_bytes(source);
        let result = (|| -> JsResult<JsValue> {
            let module = Module::parse(source, None, ctx)?;
            let module_eval = module.load_link_evaluate(ctx);
            ctx.run_jobs()?;
            match module_eval.state() {
                PromiseState::Fulfilled(_) => {}
                PromiseState::Pending => {
                    return Err(JsError::from_native(
                        JsNativeError::runtime_limit()
                            .with_message("Module evaluation promise did not settle"),
                    ));
                }
                PromiseState::Rejected(e) => return Err(JsError::from_opaque(e)),
            }
            if self.hooks.has_unhandled_rejections() {
                return Err(JsError::from_native(
                    JsNativeError::error().with_message("Unhandled promise rejection"),
                ));
            }

            let arg = JsValue::from_json(&arg, ctx)?;
            let main = module.get_value(js_string!("default"), ctx)?;
            let main = main.as_function().ok_or(JsError::from_native(
                JsNativeError::reference().with_message("Default export is not a function"),
            ))?;
            let res = main.call(&JsValue::null(), &[arg], ctx)?;
            let res = res.as_promise().unwrap_or(JsPromise::resolve(res, ctx));

            ctx.run_jobs()?;

            match res.state() {
                PromiseState::Fulfilled(v) => {
                    if self.hooks.has_unhandled_rejections() {
                        Err(JsError::from_native(
                            JsNativeError::error().with_message("Unhandled promise rejection"),
                        ))
                    } else {
                        Ok(v)
                    }
                }
                PromiseState::Pending => Err(JsError::from_native(
                    JsNativeError::runtime_limit()
                        .with_message("Default export promise did not settle"),
                )),
                PromiseState::Rejected(e) => Err(JsError::from_opaque(e)),
            }
        })();

        ctx.remove_data::<MechanicsState>();
        self.queue.set_deadline(None);
        self.hooks.clear();
        result
    }

    /// Runs source and converts the resulting JS value into `serde_json::Value`.
    pub(crate) fn run_source(&mut self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        match self.run_source_inner(job) {
            Ok(data) => {
                let ctx = &mut self.ctx;
                data.to_json(ctx)
                    .map(|d| d.unwrap_or(Value::Null))
                    .map_err(|e| MechanicsError::execution(e.to_string()))
            }

            Err(e) => Err(MechanicsError::execution(e.to_string())),
        }
    }
}
