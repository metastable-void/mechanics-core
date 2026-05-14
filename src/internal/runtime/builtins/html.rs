use super::required_string_arg;
use crate::internal::{executor::CustomModuleLoader, runtime::buffer_like};
use boa_engine::{
    Context, JsResult, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer, object::FunctionObjectBuilder,
};
use std::{borrow::Cow, rc::Rc};

fn html_string_result(value: Cow<'_, str>) -> JsValue {
    buffer_like::js_string_value(&value)
}

fn escape_text(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let text = required_string_arg(args, 0, "text")?;
    Ok(html_string_result(htmlize::escape_text(text)))
}

fn escape_attribute(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let text = required_string_arg(args, 0, "text")?;
    Ok(html_string_result(htmlize::escape_all_quotes(text)))
}

fn unescape_text(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let text = required_string_arg(args, 0, "text")?;
    Ok(html_string_result(htmlize::unescape(text)))
}

fn unescape_attribute(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let text = required_string_arg(args, 0, "text")?;
    Ok(html_string_result(htmlize::unescape_attribute(text)))
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let escape_text =
        FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(escape_text))
            .length(1)
            .name("escapeText")
            .build();
    let escape_attribute = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(escape_attribute),
    )
    .length(1)
    .name("escapeAttribute")
    .build();
    let unescape_text =
        FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(unescape_text))
            .length(1)
            .name("unescapeText")
            .build();
    let unescape_attribute = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(unescape_attribute),
    )
    .length(1)
    .name("unescapeAttribute")
    .build();

    let html_module = Module::synthetic(
        &[
            js_string!("escapeText"),
            js_string!("escapeAttribute"),
            js_string!("unescapeText"),
            js_string!("unescapeAttribute"),
        ],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, funcs, _ctx| {
                module.set_export(&js_string!("escapeText"), funcs.0.clone().into())?;
                module.set_export(&js_string!("escapeAttribute"), funcs.1.clone().into())?;
                module.set_export(&js_string!("unescapeText"), funcs.2.clone().into())?;
                module.set_export(&js_string!("unescapeAttribute"), funcs.3.clone().into())
            },
            (
                escape_text,
                escape_attribute,
                unescape_text,
                unescape_attribute,
            ),
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:html"), html_module);
}
