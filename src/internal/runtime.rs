use crate::internal::{
    error::MechanicsError,
    executor::{CustomModuleLoader, Queue, QueueSnapshot, RunJobsExit},
    http::{BoaMechanicsConfig, EndpointHttpClient, PreparedHttpEndpoint},
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
use std::{cell::Cell, collections::HashMap, rc::Rc, sync::Arc};

mod buffer_like;
mod builtins;

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

    #[cfg(test)]
    fn pending_unhandled_rejection_count(&self) -> usize {
        self.pending_unhandled_rejections.get()
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
    // SAFETY: `BoaMechanicsConfig` is Rust-owned data and does not embed GC-traced Boa handles.
    #[unsafe_ignore_trace]
    pub(crate) config: Arc<BoaMechanicsConfig>,

    // SAFETY: `Arc<dyn EndpointHttpClient>` is Rust-owned transport state with no references into
    // Boa's GC heap.
    #[unsafe_ignore_trace]
    endpoint_http_client: Arc<dyn EndpointHttpClient>,

    // SAFETY: Primitive scalar copied into runtime config; not a GC-managed value.
    #[unsafe_ignore_trace]
    default_timeout_ms: Option<u64>,

    // SAFETY: Primitive scalar copied into runtime config; not a GC-managed value.
    #[unsafe_ignore_trace]
    default_response_max_bytes: Option<usize>,

    // SAFETY: Prepared endpoint caches are Rust-owned data with no GC-managed values and are
    // scoped to the current job state instance.
    #[unsafe_ignore_trace]
    prepared_endpoints: HashMap<String, PreparedHttpEndpoint>,
}

impl MechanicsState {
    pub(crate) fn new(
        config: Arc<BoaMechanicsConfig>,
        endpoint_http_client: Arc<dyn EndpointHttpClient>,
        default_timeout_ms: Option<u64>,
        default_response_max_bytes: Option<usize>,
        prepared_endpoints: HashMap<String, PreparedHttpEndpoint>,
    ) -> Self {
        Self {
            config,
            endpoint_http_client,
            default_timeout_ms,
            default_response_max_bytes,
            prepared_endpoints,
        }
    }

    pub(crate) fn endpoint_http_client(&self) -> Arc<dyn EndpointHttpClient> {
        Arc::clone(&self.endpoint_http_client)
    }

    pub(crate) fn default_timeout_ms(&self) -> Option<u64> {
        self.default_timeout_ms
    }

    pub(crate) fn default_response_max_bytes(&self) -> Option<usize> {
        self.default_response_max_bytes
    }

    pub(crate) fn endpoint(
        &self,
        name: &str,
    ) -> Option<(&crate::internal::http::HttpEndpoint, &PreparedHttpEndpoint)> {
        let endpoint = self.config.as_inner().endpoints().get(name)?;
        let prepared = self.prepared_endpoints.get(name)?;
        Some((endpoint, prepared))
    }
}

/// Script runtime that hosts a Boa context and exposes helper modules.
pub(crate) struct RuntimeInternal {
    ctx: Context,
    loader: Rc<CustomModuleLoader>,
    endpoint_http_client: Arc<dyn EndpointHttpClient>,
    queue: Rc<Queue>,
    hooks: Rc<RuntimeHostHooks>,
    execution_limits: MechanicsExecutionLimits,
    default_endpoint_timeout_ms: Option<u64>,
    default_endpoint_response_max_bytes: Option<usize>,
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
        let deadline_ms = u64::try_from(deadline_ms).map_err(|_| {
            JsError::from_native(
                JsNativeError::range().with_message("Configured max_execution_time is too large"),
            )
        })?;
        let nanos = (deadline_ms % 1000).checked_mul(1_000_000).ok_or_else(|| {
            JsError::from_native(
                JsNativeError::range().with_message("Configured max_execution_time is too large"),
            )
        })?;
        let nanos = u32::try_from(nanos).map_err(|_| {
            JsError::from_native(
                JsNativeError::range().with_message("Configured max_execution_time is too large"),
            )
        })?;
        Ok(JsInstant::new(deadline_ms / 1000, nanos))
    }

    /// Builds a Boa context, injects runtime state, and exposes runtime synthetic modules.
    pub(crate) fn new_with_endpoint_http_client(
        endpoint_http_client: Arc<dyn EndpointHttpClient>,
    ) -> Result<Self, MechanicsError> {
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

        builtins::bundle_builtin_modules(&loader, &mut context);

        Ok(Self {
            ctx: context,
            loader,
            endpoint_http_client,
            queue,
            hooks,
            execution_limits: MechanicsExecutionLimits::default(),
            default_endpoint_timeout_ms: None,
            default_endpoint_response_max_bytes: None,
        })
    }

    pub(crate) fn set_execution_limits(&mut self, limits: MechanicsExecutionLimits) {
        self.execution_limits = limits;
    }

    pub(crate) fn set_default_endpoint_timeout_ms(&mut self, timeout_ms: Option<u64>) {
        self.default_endpoint_timeout_ms = timeout_ms;
    }

    pub(crate) fn set_default_endpoint_response_max_bytes(&mut self, max_bytes: Option<usize>) {
        self.default_endpoint_response_max_bytes = max_bytes;
    }

    fn js_value_to_json(context: &mut Context, data: JsValue) -> Result<Value, MechanicsError> {
        data.to_json(context)
            .map(|d| d.unwrap_or(Value::Null))
            .map_err(|e| MechanicsError::execution(e.to_string()))
    }

    fn js_error_to_execution(error: JsError) -> MechanicsError {
        MechanicsError::execution(error.to_string())
    }

    fn main_pending_error() -> JsError {
        JsError::from_native(
            JsNativeError::runtime_limit().with_message("Default export promise did not settle"),
        )
    }

    fn log_tail_poll_aborted(job_id: &str, snapshot: QueueSnapshot) {
        let queued = snapshot
            .promise_jobs
            .saturating_add(snapshot.timeout_jobs)
            .saturating_add(snapshot.generic_jobs);
        tracing::warn!(
            job_id = job_id,
            in_flight = snapshot.in_flight_async_jobs,
            queued = queued,
            queued_promise = snapshot.promise_jobs,
            queued_timeout = snapshot.timeout_jobs,
            queued_generic = snapshot.generic_jobs,
            reason = "max_execution_time exceeded",
            "tail_poll_aborted"
        );

        #[cfg(test)]
        test_support::record_tail_poll_abort(job_id, snapshot);
    }

    /// Parses and evaluates a module, invokes its default export, sends the main outcome through
    /// `early_reply`, then continues polling tail jobs until quiescence or deadline.
    pub(crate) fn run_source_with_early_reply<F>(
        &mut self,
        job: MechanicsJob,
        job_id: &str,
        early_reply: F,
    ) -> Result<(), MechanicsError>
    where
        F: FnOnce(Result<Value, MechanicsError>),
    {
        let (source, arg, config) = job.into_parts();
        self.hooks.clear();
        let config_inner = Arc::unwrap_or_clone(config);
        let mut prepared_endpoints = HashMap::with_capacity(config_inner.endpoints().len());
        for (name, endpoint) in config_inner.endpoints() {
            let prepared = endpoint
                .prepare_runtime()
                .map_err(JsError::from_rust)
                .map_err(Self::js_error_to_execution)?;
            prepared_endpoints.insert(name.clone(), prepared);
        }
        let state = MechanicsState::new(
            Arc::new(config_inner.into()),
            Arc::clone(&self.endpoint_http_client),
            self.default_endpoint_timeout_ms,
            self.default_endpoint_response_max_bytes,
            prepared_endpoints,
        );

        let deadline = Self::compute_deadline(&self.ctx, self.execution_limits.max_execution_time)
            .map_err(Self::js_error_to_execution)?;
        let ctx = &mut self.ctx;
        let isolated_realm = ctx.create_realm().map_err(Self::js_error_to_execution)?;
        let previous_realm = ctx.enter_realm(isolated_realm);
        builtins::bundle_builtin_modules(&self.loader, ctx);

        let runtime_limits = ctx.runtime_limits_mut();
        runtime_limits.set_loop_iteration_limit(self.execution_limits.max_loop_iterations);
        runtime_limits.set_recursion_limit(self.execution_limits.max_recursion_depth);
        runtime_limits.set_stack_size_limit(self.execution_limits.max_stack_size);

        self.queue.set_deadline(Some(deadline));
        ctx.insert_data(state);

        let source = source.as_ref();
        let source = Source::from_bytes(source);
        let mut early_reply = Some(early_reply);
        let mut main_replied = false;
        let result = (|| -> JsResult<()> {
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

            let tail_exit = Rc::clone(&self.queue).run_jobs_until_then_to_quiescence(
                ctx,
                || !matches!(res.state(), PromiseState::Pending),
                |ctx| {
                    let main_result = match res.state() {
                        // If main fulfilled, the script's own try/catch chain already
                        // produced a successful outcome — trust it. We intentionally do
                        // NOT consult `has_unhandled_rejections()` here.
                        //
                        // Why: Boa's `NativeFunction::from_async_fn` rejects an inner
                        // promise that the await machinery wraps in an outer
                        // continuation promise. The spec-compliant
                        // `promise_rejection_tracker` fires `Reject` on the inner
                        // rejection (no handlers attached at that moment), but the
                        // matching `Handle` event for the inner promise does not
                        // reliably fire when the handler is attached to the outer
                        // wrapper rather than the inner promise. The counter then
                        // ends positive even though every JS-visible rejection was
                        // caught by the script's `await ... catch`.
                        //
                        // The strict check we used to do here produced false-positive
                        // step failures for any workflow that legitimately catches an
                        // endpoint error — including the canonical D13 chat-with-
                        // fallback pattern. Match Node's semantics: an unhandled
                        // rejection is a warning, not a process kill. Genuine
                        // script-author bugs (e.g. `Promise.resolve().then(throw)`
                        // with no catch anywhere) still produce a working but
                        // misbehaving step rather than a hard failure — the cost of
                        // that is much lower than breaking every workflow that
                        // handles errors correctly.
                        //
                        // The module-evaluation-time check above (run after the
                        // module is imported but before `main` is called) is
                        // separate and stays strict because top-level awaits in user
                        // scripts are rare and a module-load failure is a different
                        // class of problem.
                        PromiseState::Fulfilled(v) => Self::js_value_to_json(ctx, v),
                        PromiseState::Pending => {
                            Err(Self::js_error_to_execution(Self::main_pending_error()))
                        }
                        PromiseState::Rejected(e) => {
                            Err(Self::js_error_to_execution(JsError::from_opaque(e)))
                        }
                    };

                    main_replied = true;
                    if let Some(reply) = early_reply.take() {
                        reply(main_result);
                    }
                    Ok(())
                },
            )?;

            if !main_replied && matches!(res.state(), PromiseState::Pending) {
                return Err(Self::main_pending_error());
            }

            match tail_exit {
                RunJobsExit::Complete => {}
                RunJobsExit::DeadlineExceeded(snapshot) => {
                    Self::log_tail_poll_aborted(job_id, snapshot);
                }
            }
            Ok(())
        })();

        #[cfg(test)]
        let pending_unhandled_rejections = self.hooks.pending_unhandled_rejection_count();

        ctx.remove_data::<MechanicsState>();
        self.queue.set_deadline(None);
        self.queue.clear_all();
        self.hooks.clear();
        ctx.enter_realm(previous_realm);

        #[cfg(test)]
        test_support::record_unhandled_rejection_count(job_id, pending_unhandled_rejections);

        match result {
            Ok(()) => Ok(()),
            Err(e) if main_replied => {
                let _ = e;
                Ok(())
            }
            Err(e) => Err(Self::js_error_to_execution(e)),
        }
    }

    /// Runs source and converts the resulting JS value into `serde_json::Value`.
    // Kept for direct internal callers; worker dispatch uses the early-reply entry point.
    #[allow(dead_code)]
    pub(crate) fn run_source(&mut self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        let mut main_result = None;
        let tail_result = self.run_source_with_early_reply(job, "direct", |result| {
            main_result = Some(result);
        });
        match main_result {
            Some(result) => result,
            None => match tail_result {
                Ok(()) => Err(MechanicsError::execution(
                    "script completed without producing a main result",
                )),
                Err(err) => Err(err),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn recorded_unhandled_rejection_count_for_test(job_id: &str) -> Option<usize> {
        test_support::recorded_unhandled_rejection_count(job_id)
    }

    #[cfg(test)]
    pub(crate) fn clear_tail_poll_abort_records_for_test() {
        test_support::clear_tail_poll_abort_records();
    }

    #[cfg(test)]
    pub(crate) fn tail_poll_abort_records_for_test() -> Vec<test_support::TailPollAbortRecord> {
        test_support::tail_poll_abort_records()
    }
}

#[cfg(test)]
mod test_support {
    use super::QueueSnapshot;
    use std::sync::{Mutex, OnceLock};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct TailPollAbortRecord {
        pub(crate) job_id: String,
        pub(crate) snapshot: QueueSnapshot,
    }

    static TAIL_POLL_ABORTS: OnceLock<Mutex<Vec<TailPollAbortRecord>>> = OnceLock::new();
    static UNHANDLED_REJECTION_COUNTS: OnceLock<Mutex<Vec<(String, usize)>>> = OnceLock::new();

    fn tail_poll_aborts() -> &'static Mutex<Vec<TailPollAbortRecord>> {
        TAIL_POLL_ABORTS.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn unhandled_rejection_counts() -> &'static Mutex<Vec<(String, usize)>> {
        UNHANDLED_REJECTION_COUNTS.get_or_init(|| Mutex::new(Vec::new()))
    }

    pub(crate) fn record_tail_poll_abort(job_id: &str, snapshot: QueueSnapshot) {
        tail_poll_aborts()
            .lock()
            .expect("lock tail poll abort records")
            .push(TailPollAbortRecord {
                job_id: job_id.to_owned(),
                snapshot,
            });
    }

    pub(crate) fn clear_tail_poll_abort_records() {
        tail_poll_aborts()
            .lock()
            .expect("lock tail poll abort records")
            .clear();
    }

    pub(crate) fn tail_poll_abort_records() -> Vec<TailPollAbortRecord> {
        tail_poll_aborts()
            .lock()
            .expect("lock tail poll abort records")
            .clone()
    }

    pub(crate) fn record_unhandled_rejection_count(job_id: &str, count: usize) {
        unhandled_rejection_counts()
            .lock()
            .expect("lock unhandled rejection records")
            .push((job_id.to_owned(), count));
    }

    pub(crate) fn recorded_unhandled_rejection_count(job_id: &str) -> Option<usize> {
        unhandled_rejection_counts()
            .lock()
            .expect("lock unhandled rejection records")
            .iter()
            .rev()
            .find_map(|(id, count)| (id == job_id).then_some(*count))
    }
}
