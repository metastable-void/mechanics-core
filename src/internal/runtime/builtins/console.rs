use crate::internal::executor::CustomModuleLoader;
use boa_engine::{
    Context, JsResult, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer,
    object::{FunctionObjectBuilder, JsObject},
};
use std::rc::Rc;

fn console_noop(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn console_method(context: &mut Context, name: &'static str) -> JsObject {
    FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(console_noop))
        .length(0)
        .name(name)
        .build()
        .into()
}

fn console_object(context: &mut Context) -> JsResult<JsObject> {
    let console = JsObject::default(context.intrinsics());
    for name in ["log", "info", "warn", "error", "debug"] {
        let method = console_method(context, name);
        console.set(js_string!(name), method, true, context)?;
    }
    Ok(console)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let Ok(console) = console_object(context) else {
        return;
    };

    let console_module = Module::synthetic(
        &[js_string!("default")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, console, _ctx| {
                module.set_export(&js_string!("default"), console.clone().into())
            },
            console,
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:console"), console_module);
}
