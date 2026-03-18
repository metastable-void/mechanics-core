use crate::internal::{executor::CustomModuleLoader, runtime::buffer_like};
use boa_engine::{
    Context, JsArgs, JsResult, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer, object::FunctionObjectBuilder,
};
use std::rc::Rc;

fn rand_fill_random(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    buffer_like::fill_random_buffer_like(args.get_or_undefined(0), context)?;
    Ok(JsValue::undefined())
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
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
