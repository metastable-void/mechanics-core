use crate::{
    error::MechanicsError,
    executor::{CustomModuleLoader, Queue},
    http::MechanicsConfig,
    job::{MechanicsExecutionLimits, MechanicsJob},
};
use boa_engine::{
    Context, JsArgs, JsData, JsError, JsNativeError, JsResult, JsValue, Module, NativeFunction,
    Source, Trace,
    builtins::promise::PromiseState,
    context::ContextBuilder,
    js_string,
    module::SyntheticModuleInitializer,
    object::{FunctionObjectBuilder, builtins::JsPromise},
};
use boa_gc::Finalize;
use serde_json::Value;
use std::{rc::Rc, sync::Arc};

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

/// Script runtime that hosts a Boa context and exposes helper modules.
pub(crate) struct RuntimeInternal {
    ctx: Context,
    reqwest_client: reqwest::Client,
    queue: Rc<Queue>,
    execution_limits: MechanicsExecutionLimits,
    default_endpoint_timeout_ms: Option<u64>,
}

impl RuntimeInternal {
    /// Builds a Boa context, injects runtime state, and exposes `mechanics:endpoint`.
    pub(crate) fn new_with_client(reqwest_client: reqwest::Client) -> Self {
        let queue = Rc::new(Queue::new());

        let loader = Rc::new(CustomModuleLoader::new());
        let mut context = ContextBuilder::new()
            .job_executor(queue.clone())
            .module_loader(loader.clone())
            .build()
            .unwrap();

        let endpoint = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_async_fn(async |_, args, ctx| {
                let endpoint = args
                    .get_or_undefined(0)
                    .as_string()
                    .ok_or(JsError::from_native(
                        JsNativeError::typ().with_message("endpoint is not a string"),
                    ))?;
                let req_body = args
                    .get_or_undefined(1)
                    .to_json(&mut ctx.borrow_mut())?
                    .ok_or(JsError::from_native(
                        JsNativeError::typ().with_message("JSON error"),
                    ))?;

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

                let res: Value = endpoint
                    .post(state.reqwest(), state.default_timeout_ms(), &req_body)
                    .await
                    .map_err(JsError::from_rust)?;

                let res = JsValue::from_json(&res, &mut ctx.borrow_mut())?;
                Ok(res)
            }),
        )
        .length(2)
        .name("endpoint")
        .build();

        let module = Module::synthetic(
            &[js_string!("default")],
            SyntheticModuleInitializer::from_copy_closure_with_captures(
                |module, ept, _ctx| module.set_export(&js_string!("default"), ept.clone().into()),
                endpoint,
            ),
            None,
            None,
            &mut context,
        );

        loader.define_module(js_string!("mechanics:endpoint"), module);

        Self {
            ctx: context,
            reqwest_client,
            queue,
            execution_limits: MechanicsExecutionLimits::default(),
            default_endpoint_timeout_ms: None,
        }
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
        let state = MechanicsState::new(
            config,
            self.reqwest_client.clone(),
            self.default_endpoint_timeout_ms,
        );

        let source = source.as_ref();
        let ctx = &mut self.ctx;

        let runtime_limits = ctx.runtime_limits_mut();
        runtime_limits.set_loop_iteration_limit(self.execution_limits.max_loop_iterations);
        runtime_limits.set_recursion_limit(self.execution_limits.max_recursion_depth);
        runtime_limits.set_stack_size_limit(self.execution_limits.max_stack_size);

        let deadline = ctx.clock().now() + self.execution_limits.max_execution_time.into();
        self.queue.set_deadline(Some(deadline));
        ctx.insert_data(state);

        let source = Source::from_bytes(source);
        let result = (|| -> JsResult<JsValue> {
            let module = Module::parse(source, None, ctx)?;
            let _ = module.load_link_evaluate(ctx);
            ctx.run_jobs()?;

            let arg = JsValue::from_json(&arg, ctx)?;
            let main = module.get_value(js_string!("default"), ctx)?;
            let main = main.as_function().ok_or(JsError::from_native(
                JsNativeError::reference().with_message("Default export is not a function"),
            ))?;
            let res = main.call(&JsValue::null(), &[arg], ctx)?;
            let res = res.as_promise().unwrap_or(JsPromise::resolve(res, ctx));

            ctx.run_jobs()?;

            match res.state() {
                PromiseState::Fulfilled(v) => Ok(v),
                PromiseState::Pending => Ok(res.into()),
                PromiseState::Rejected(e) => Err(JsError::from_opaque(e)),
            }
        })();

        ctx.remove_data::<MechanicsState>();
        self.queue.set_deadline(None);
        result
    }

    /// Runs source and converts the resulting JS value into `serde_json::Value`.
    pub(crate) fn run_source(&mut self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        match self.run_source_inner(job) {
            Ok(data) => {
                let ctx = &mut self.ctx;
                match data.to_json(ctx) {
                    Ok(d) => Ok(d.unwrap_or(Value::Null)),
                    _ => Ok(Value::Null),
                }
            }

            Err(e) => Err(MechanicsError::execution(e.to_string())),
        }
    }
}
