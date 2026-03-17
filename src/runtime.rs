use crate::{
    error::MechanicsError,
    executor::{CustomModuleLoader, Queue},
    http::MechanicsConfig,
    job::{MechanicsExecutionLimits, MechanicsJob},
};
use boa_engine::{
    Context, JsData, JsError, JsNativeError, JsResult, JsValue, Module, Source, Trace,
    builtins::promise::{OperationType, PromiseState},
    context::{ContextBuilder, HostHooks, time::JsInstant},
    js_string,
    object::{JsObject, builtins::JsPromise},
};
use boa_gc::Finalize;
use serde_json::Value;
use std::{cell::Cell, rc::Rc, sync::Arc};

mod buffer_like;
mod synthetic_modules;

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
    pub(crate) config: Arc<MechanicsConfig>,

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

    /// Builds a Boa context, injects runtime state, and exposes runtime synthetic modules.
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

        synthetic_modules::install_synthetic_modules(&loader, &mut context);

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
