use crate::http::into_io_error;
use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, JsValue,
    object::builtins::{JsArrayBuffer, JsDataView, JsTypedArray, JsUint8Array},
};

pub(super) fn js_type_error(message: impl AsRef<str>) -> JsError {
    JsError::from_native(JsNativeError::typ().with_message(message.as_ref().to_owned()))
}

pub(super) fn js_range_error(message: impl AsRef<str>) -> JsError {
    JsError::from_native(JsNativeError::range().with_message(message.as_ref().to_owned()))
}

fn u64_to_usize(value: u64, field: &str) -> JsResult<usize> {
    usize::try_from(value).map_err(|_| js_range_error(format!("{field} exceeds usize range")))
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

pub(super) fn try_extract_buffer_like_bytes(
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
        let offset = u64_to_usize(data_view.byte_offset(context)?, "DataView byte_offset")?;
        let len = u64_to_usize(data_view.byte_length(context)?, "DataView byte_length")?;
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

pub(super) fn fill_random_buffer_like(value: &JsValue, context: &mut Context) -> JsResult<()> {
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
        let offset = u64_to_usize(data_view.byte_offset(context)?, "DataView byte_offset")?;
        let len = u64_to_usize(data_view.byte_length(context)?, "DataView byte_length")?;
        return fill_random_in_array_buffer_range(
            &array_buffer,
            offset,
            len,
        );
    }

    if let Ok(array_buffer) = JsArrayBuffer::from_object(object) {
        return fill_random_in_array_buffer_range(&array_buffer, 0, array_buffer.byte_length());
    }

    Err(js_type_error(
        "bufferLike must be a TypedArray, ArrayBuffer, or DataView",
    ))
}

pub(super) fn bytes_to_uint8_array_value(bytes: &[u8], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsUint8Array::from_iter(bytes.iter().copied(), context)?.into())
}

pub(super) fn js_string_value(s: &str) -> JsValue {
    JsValue::from(JsString::from(s))
}
