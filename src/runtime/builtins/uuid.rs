use crate::{executor::CustomModuleLoader, http::into_io_error, runtime::buffer_like};
use boa_engine::{
    Context, JsArgs, JsError, JsResult, JsValue, Module, NativeFunction, js_string,
    module::SyntheticModuleInitializer, object::FunctionObjectBuilder,
};
use std::rc::Rc;
use uuid::Uuid;

fn parse_uuid_variant(args: &[JsValue], index: usize) -> JsResult<&'static str> {
    let value = args.get_or_undefined(index);
    if value.is_undefined() {
        return Ok("v4");
    }
    let Some(s) = value.as_string() else {
        return Err(buffer_like::js_type_error("variant must be a string"));
    };
    match s.to_std_string_lossy().as_str() {
        "v3" => Ok("v3"),
        "v4" => Ok("v4"),
        "v5" => Ok("v5"),
        "v6" => Ok("v6"),
        "v7" => Ok("v7"),
        "nil" => Ok("nil"),
        "max" => Ok("max"),
        _ => Err(buffer_like::js_type_error(
            "variant must be one of 'v3', 'v4', 'v5', 'v6', 'v7', 'nil', 'max'",
        )),
    }
}

fn parse_uuid_v3_v5_options(args: &[JsValue], context: &mut Context) -> JsResult<(Uuid, Vec<u8>)> {
    let value = args.get_or_undefined(1);
    let Some(options) = value.as_object() else {
        return Err(buffer_like::js_type_error(
            "options must be an object with `namespace` and `name` for v3/v5",
        ));
    };

    let namespace = options
        .get(js_string!("namespace"), context)?
        .as_string()
        .ok_or_else(|| buffer_like::js_type_error("options.namespace must be a UUID string"))?
        .to_std_string_lossy();
    let namespace = Uuid::parse_str(&namespace)
        .map_err(into_io_error)
        .map_err(JsError::from_rust)?;

    let name = options
        .get(js_string!("name"), context)?
        .as_string()
        .ok_or_else(|| buffer_like::js_type_error("options.name must be a string"))?
        .to_std_string_lossy();
    Ok((namespace, name.into_bytes()))
}

fn uuid_generate(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let variant = parse_uuid_variant(args, 0)?;
    let value = match variant {
        "v3" => {
            let (namespace, name) = parse_uuid_v3_v5_options(args, context)?;
            Uuid::new_v3(&namespace, &name)
        }
        "v4" => Uuid::new_v4(),
        "v5" => {
            let (namespace, name) = parse_uuid_v3_v5_options(args, context)?;
            Uuid::new_v5(&namespace, &name)
        }
        "v6" => {
            let mut node_id = [0_u8; 6];
            getrandom::fill(&mut node_id)
                .map_err(into_io_error)
                .map_err(JsError::from_rust)?;
            Uuid::now_v6(&node_id)
        }
        "v7" => Uuid::now_v7(),
        "nil" => Uuid::nil(),
        "max" => Uuid::max(),
        _ => return Err(buffer_like::js_type_error("invalid UUID variant")),
    };
    Ok(buffer_like::js_string_value(
        &value.hyphenated().to_string(),
    ))
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let uuid =
        FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(uuid_generate))
            .length(2)
            .name("uuid")
            .build();
    let uuid_module = Module::synthetic(
        &[js_string!("default")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, f, _ctx| module.set_export(&js_string!("default"), f.clone().into()),
            uuid,
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:uuid"), uuid_module);
}
