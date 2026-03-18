use super::{MechanicsState, endpoint_response_to_js_value, parse_endpoint_call_options_js};
use crate::executor::CustomModuleLoader;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer, object::FunctionObjectBuilder,
};
use std::rc::Rc;

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
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
            let (endpoint, prepared) = state.endpoint(&endpoint_name).ok_or(
                JsError::from_native(JsNativeError::typ().with_message("Endpoint not found")),
            )?;

            let res = endpoint
                .execute(
                    state.endpoint_http_client(),
                    prepared,
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
}
