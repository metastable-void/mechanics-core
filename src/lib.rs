use boa_engine::{
    Context, JsArgs, JsData, JsError, JsNativeError, JsResult, JsString, JsValue, Module, NativeFunction, Source, Trace, builtins::promise::PromiseState, context::{ContextBuilder, time::JsInstant}, job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob}, js_string, module::{ModuleLoader, SyntheticModuleInitializer}, object::{FunctionObjectBuilder, builtins::JsPromise}
};

use boa_gc::Finalize;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, SendTimeoutError, TryRecvError, TrySendError, bounded};
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};
use reqwest::header::{HeaderMap, HeaderName};
use serde::{Serialize, Deserialize};
use serde_json::Value;
use tokio::task;
use std::{borrow::Cow, cell::RefCell, collections::{BTreeMap, HashMap, VecDeque}, fmt::Display, rc::Rc, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread};
use std::ops::DerefMut;
use std::time::{Duration, Instant};

/// Normalizes arbitrary error types into `std::io::Error` for shared propagation paths.
pub(crate) fn into_io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}

/// HTTP endpoint configuration used by the runtime-provided JS helper.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
pub struct HttpEndpoint {
    url: String,
    headers: HashMap<String, String>,
    timeout_ms: Option<u64>,
}

impl HttpEndpoint {
    pub const USER_AGENT: &str = concat!("Mozilla/5.0 (compatible; mechanics/", env!("CARGO_PKG_VERSION"), ")");

    /// Constructs an endpoint definition used by runtime-owned HTTP helpers.
    pub fn new(url: &str, headers: HashMap<String, String>) -> Self {
        Self {
            url: url.to_owned(),
            headers,
            timeout_ms: None,
        }
    }

    /// Sends a JSON POST request and deserializes the JSON response into `Res`.
    pub async fn post<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        client: reqwest::Client,
        default_timeout_ms: Option<u64>,
        req_data: &Req,
    ) -> std::io::Result<Res> {
        let json = serde_json::to_string(req_data).map_err(into_io_error)?;
        let url = reqwest::Url::parse(&self.url).map_err(into_io_error)?;
        let mut headers = HeaderMap::new();
        for (k, v) in &self.headers {
            match (k.try_into() as Result<HeaderName, _>, v.try_into()) {
                (Ok(k), Ok(v)) => {
                    headers.insert(k, v);
                },

                _ => {},
            }
        }
        headers.insert("User-Agent", Self::USER_AGENT.try_into().unwrap());
        headers.insert("Content-Type", "application/json".try_into().unwrap());
        let timeout_ms = self.timeout_ms.or(default_timeout_ms);
        let mut req = client.post(url).headers(headers).body(json);
        if let Some(timeout_ms) = timeout_ms {
            req = req.timeout(Duration::from_millis(timeout_ms));
        }
        let res = req.send().await.map_err(into_io_error)?;
        let res: Res = res.json().await.map_err(into_io_error)?;
        Ok(res)
    }
}

/// Job queues backing Boa's executor integration.
pub(crate) struct Queue {
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    timeout_jobs: RefCell<BTreeMap<JsInstant, TimeoutJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    deadline: RefCell<Option<JsInstant>>,
    tokio_rt: tokio::runtime::Runtime,
    tokio_local: tokio::task::LocalSet,
}

impl Queue {
    /// Creates an empty job queue backing Boa's executor hooks.
    pub(crate) fn new() -> Self {
        let tokio_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let tokio_local = tokio::task::LocalSet::new();

        Self {
            async_jobs: RefCell::default(),
            promise_jobs: RefCell::default(),
            timeout_jobs: RefCell::default(),
            generic_jobs: RefCell::default(),
            deadline: RefCell::default(),
            tokio_rt,
            tokio_local,
        }
    }

    fn timeout_error() -> JsError {
        JsError::from_native(
            JsNativeError::runtime_limit().with_message("Maximum execution time exceeded"),
        )
    }

    pub(crate) fn set_deadline(&self, deadline: Option<JsInstant>) {
        *self.deadline.borrow_mut() = deadline;
    }

    fn check_deadline(&self, context: &Context) -> JsResult<()> {
        let Some(deadline) = *self.deadline.borrow() else {
            return Ok(());
        };
        if context.clock().now() >= deadline {
            return Err(Self::timeout_error());
        }
        Ok(())
    }

    fn next_timeout_at(&self) -> Option<JsInstant> {
        self.timeout_jobs.borrow().first_key_value().map(|(k, _)| *k)
    }

    /// Executes all due timeout jobs and keeps only future/cancel-surviving entries.
    fn drain_timeout_jobs(&self, context: &mut Context) {
        let now = context.clock().now();

        let mut timeouts_borrow = self.timeout_jobs.borrow_mut();
        let mut jobs_to_keep = timeouts_borrow.split_off(&now);
        jobs_to_keep.retain(|_, job| !job.is_cancelled());
        let jobs_to_run = std::mem::replace(timeouts_borrow.deref_mut(), jobs_to_keep);
        drop(timeouts_borrow);

        for job in jobs_to_run.into_values() {
            if let Err(e) = job.call(context) {
                eprintln!("Uncaught {e}");
            }
        }
    }

    /// Drains one macrotask turn in Boa order: timeout, one generic task, then promise jobs.
    fn drain_jobs(&self, context: &mut Context) {
        self.drain_timeout_jobs(context);

        let job = self.generic_jobs.borrow_mut().pop_front();
        if let Some(generic) = job
            && let Err(err) = generic.call(context)
        {
            eprintln!("Uncaught {err}");
        }

        let jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
        for job in jobs {
            if let Err(e) = job.call(context) {
                eprintln!("Uncaught {e}");
            }
        }
        context.clear_kept_objects();
    }
}

impl JobExecutor for Queue {
    /// Routes jobs to their corresponding internal queues.
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(t) => {
                let now = context.clock().now();
                self.timeout_jobs.borrow_mut().insert(now + t.timeout(), t);
            }
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            _ => panic!("unsupported job type"),
        }
    }

    /// Bridges Boa's synchronous API to the async scheduler by running a local Tokio runtime.
    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let this = Rc::clone(&self);
        self.tokio_local
            .block_on(&self.tokio_rt, this.run_jobs_async(&RefCell::new(context)))
    }

    /// Polls async jobs and drains task queues until no jobs remain.
    async fn run_jobs_async(self: Rc<Self>, context: &RefCell<&mut Context>) -> JsResult<()> {
        let mut group = FutureGroup::new();
        loop {
            {
                let ctx_ref = context.borrow();
                self.check_deadline(&ctx_ref)?;
            }

            for job in std::mem::take(&mut *self.async_jobs.borrow_mut()) {
                group.insert(job.call(context));
            }

            if group.is_empty()
                && self.promise_jobs.borrow().is_empty()
                && self.timeout_jobs.borrow().is_empty()
                && self.generic_jobs.borrow().is_empty()
            {
                return Ok(());
            }

            if group.is_empty() {
                if self.promise_jobs.borrow().is_empty() && self.generic_jobs.borrow().is_empty() {
                    if let Some(next_timeout_at) = self.next_timeout_at() {
                        let sleep_dur = {
                            let ctx_ref = context.borrow();
                            let now = ctx_ref.clock().now();
                            if next_timeout_at <= now {
                                Duration::ZERO
                            } else {
                                let mut d: Duration = (next_timeout_at - now).into();
                                if let Some(deadline) = *self.deadline.borrow() {
                                    let remaining = if deadline <= now {
                                        Duration::ZERO
                                    } else {
                                        (deadline - now).into()
                                    };
                                    d = d.min(remaining);
                                }
                                d
                            }
                        };

                        if !sleep_dur.is_zero() {
                            tokio::time::sleep(sleep_dur).await;
                        }
                    }
                }
            } else {
                let polled = if let Some(deadline) = *self.deadline.borrow() {
                    let remaining = {
                        let ctx_ref = context.borrow();
                        let now = ctx_ref.clock().now();
                        if deadline <= now {
                            return Err(Self::timeout_error());
                        }
                        let d: Duration = (deadline - now).into();
                        d
                    };
                    match tokio::time::timeout(remaining, future::poll_once(group.next())).await {
                        Ok(result) => result,
                        Err(_) => return Err(Self::timeout_error()),
                    }
                } else {
                    future::poll_once(group.next()).await
                };

                if let Some(Err(err)) = polled.flatten() {
                    eprintln!("Uncaught {err}");
                };
            }

            {
                let ctx_ref = context.borrow();
                self.check_deadline(&ctx_ref)?;
            }

            self.drain_jobs(&mut context.borrow_mut());
            task::yield_now().await
        }
    }
}

/// Serializable runtime data injected into the JS context.
#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone, Debug)]
pub struct MechanicsConfig {
    endpoints: HashMap<String, HttpEndpoint>,
}

impl MechanicsConfig {
    /// Builds runtime state from endpoint definitions.
    pub fn new(endpoints: HashMap<String, HttpEndpoint>) -> Self {
        Self {
            endpoints,
        }
    }
}

/// In-memory module loader for synthetic runtime modules.
pub(crate) struct CustomModuleLoader {
    defined: RefCell<HashMap<JsString, Module>>,
}

impl CustomModuleLoader {
    /// Creates an empty in-memory module registry.
    pub(crate) fn new() -> Self {
        Self {
            defined: RefCell::new(HashMap::new()),
        }
    }

    /// Registers a synthetic module under a specifier for later import resolution.
    pub(crate) fn define_module(&self, spec: JsString, module: Module) {
       self.defined.borrow_mut().insert(spec, module);
    }
}

impl ModuleLoader for CustomModuleLoader {
    /// Resolves imports from the in-memory module registry.
    fn load_imported_module(
            self: Rc<Self>,
            _referrer: boa_engine::module::Referrer,
            specifier: JsString,
            _context: &RefCell<&mut Context>,
        ) -> impl Future<Output = JsResult<Module>> {
        async move {
            self.defined.borrow().get(&specifier).cloned()
                .ok_or(JsError::from_native(JsNativeError::reference().with_message("Module not found")))
        }
    }
}

#[derive(Debug, Clone)]
pub struct MechanicsJob {
    pub mod_source: Arc<str>,
    pub arg: Arc<Value>,
    pub config: Arc<MechanicsConfig>,
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

#[derive(Debug, Clone, Copy)]
pub struct MechanicsExecutionLimits {
    pub max_execution_time: Duration,
    pub max_loop_iterations: u64,
    pub max_recursion_depth: usize,
    pub max_stack_size: usize,
}

impl Default for MechanicsExecutionLimits {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_secs(10),
            max_loop_iterations: 1_000_000,
            max_recursion_depth: 512,
            max_stack_size: 10 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MechanicsError {
    Execution(Cow<'static, str>),
    QueueFull(Cow<'static, str>),
    QueueTimeout(Cow<'static, str>),
    PoolClosed(Cow<'static, str>),
    WorkerUnavailable(Cow<'static, str>),
    Canceled(Cow<'static, str>),
    Panic(Cow<'static, str>),
    RuntimePool(Cow<'static, str>),
}

impl MechanicsError {
    pub fn execution<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Execution(msg.into())
    }

    pub fn runtime_pool<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::RuntimePool(msg.into())
    }

    pub fn queue_full<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::QueueFull(msg.into())
    }

    pub fn queue_timeout<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::QueueTimeout(msg.into())
    }

    pub fn pool_closed<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::PoolClosed(msg.into())
    }

    pub fn worker_unavailable<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::WorkerUnavailable(msg.into())
    }

    pub fn canceled<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Canceled(msg.into())
    }

    pub fn panic<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Panic(msg.into())
    }

    pub fn msg(&self) -> &str {
        match self {
            Self::Execution(msg) => msg.as_ref(),
            Self::QueueFull(msg) => msg.as_ref(),
            Self::QueueTimeout(msg) => msg.as_ref(),
            Self::PoolClosed(msg) => msg.as_ref(),
            Self::WorkerUnavailable(msg) => msg.as_ref(),
            Self::Canceled(msg) => msg.as_ref(),
            Self::Panic(msg) => msg.as_ref(),
            Self::RuntimePool(msg) => msg.as_ref(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match &self {
            Self::Execution(_) => "MechanicsError::Execution",
            Self::QueueFull(_) => "MechanicsError::QueueFull",
            Self::QueueTimeout(_) => "MechanicsError::QueueTimeout",
            Self::PoolClosed(_) => "MechanicsError::PoolClosed",
            Self::WorkerUnavailable(_) => "MechanicsError::WorkerUnavailable",
            Self::Canceled(_) => "MechanicsError::Canceled",
            Self::Panic(_) => "MechanicsError::Panic",
            Self::RuntimePool(_) => "MechanicsError::RuntimePool",
        }
    }
}

impl Display for MechanicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.msg())
    }
}

impl std::error::Error for MechanicsError {}

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
                let endpoint = args.get_or_undefined(0).as_string()
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("endpoint is not a string")))?;
                let req_body = args.get_or_undefined(1).to_json(&mut ctx.borrow_mut())?
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("JSON error")))?;
                
                let ctx_ref = ctx.borrow();
                let state = ctx_ref.get_data::<MechanicsState>().cloned()
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("Invalid state")))?;
                
                drop(ctx_ref);
                let endpoint_name = endpoint.to_std_string_lossy();
                let endpoint = state.config.endpoints.get(&endpoint_name)
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("Endpoint not found")))?;
                
                let res: Value = endpoint.post(state.reqwest(), state.default_timeout_ms(), &req_body).await
                    .map_err(|e| JsError::from_rust(e))?;

                let res = JsValue::from_json(&res, &mut ctx.borrow_mut())?;
                Ok(res)
            }),
        )
        .length(2)
        .name("endpoint")
        .build();

        let module = Module::synthetic(&[
            js_string!("default"),
        ], SyntheticModuleInitializer::from_copy_closure_with_captures(|module, ept, _ctx| {
            module.set_export(&js_string!("default"), ept.clone().into())
        }, endpoint), None, None, &mut context);
        
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
        let state = MechanicsState::new(config, self.reqwest_client.clone(), self.default_endpoint_timeout_ms);

        let source = source.as_ref();
        let mut ctx = &mut self.ctx;

        let runtime_limits = ctx.runtime_limits_mut();
        runtime_limits.set_loop_iteration_limit(self.execution_limits.max_loop_iterations);
        runtime_limits.set_recursion_limit(self.execution_limits.max_recursion_depth);
        runtime_limits.set_stack_size_limit(self.execution_limits.max_stack_size);

        let deadline = ctx.clock().now() + self.execution_limits.max_execution_time.into();
        self.queue.set_deadline(Some(deadline));
        ctx.insert_data(state);

        let source = Source::from_bytes(source);
        let result = (|| -> JsResult<JsValue> {
            let module = Module::parse(source, None, &mut ctx)?;
            let _ = module.load_link_evaluate(&mut ctx);
            ctx.run_jobs()?;

            let arg = JsValue::from_json(&arg, &mut ctx)?;
            let main = module.get_value(js_string!("default"), &mut ctx)?;
            let main = main.as_function()
                .ok_or(JsError::from_native(JsNativeError::reference().with_message("Default export is not a function")))?;
            let res = main.call(&JsValue::null(), &[arg], &mut ctx)?;
            let res = res.as_promise()
                .unwrap_or(JsPromise::resolve(res, &mut ctx));

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
                let mut ctx = &mut self.ctx;
                match data.to_json(&mut ctx) {
                    Ok(d) => Ok(d.unwrap_or(Value::Null)),
                    _ => Ok(Value::Null),
                }
            },

            Err(e) => {
                Err(MechanicsError::execution(e.to_string()))
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct MechanicsPoolConfig {
    pub worker_count: usize,
    pub queue_capacity: usize,
    pub enqueue_timeout: Duration,
    pub execution_limits: MechanicsExecutionLimits,
    pub default_http_timeout_ms: Option<u64>,
    pub restart_window: Duration,
    pub max_restarts_in_window: usize,
}

impl Default for MechanicsPoolConfig {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(1);
        Self {
            worker_count: workers.max(1),
            queue_capacity: workers.saturating_mul(64).max(64),
            enqueue_timeout: Duration::from_millis(500),
            execution_limits: MechanicsExecutionLimits::default(),
            default_http_timeout_ms: None,
            restart_window: Duration::from_secs(10),
            max_restarts_in_window: 16,
        }
    }
}

#[derive(Debug)]
struct RestartGuard {
    window: Duration,
    max_restarts: usize,
    restarts: VecDeque<Instant>,
}

impl RestartGuard {
    fn new(window: Duration, max_restarts: usize) -> Self {
        Self {
            window,
            max_restarts,
            restarts: VecDeque::new(),
        }
    }

    fn allow_restart(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.restarts.front() {
            if now.duration_since(*oldest) > self.window {
                self.restarts.pop_front();
            } else {
                break;
            }
        }

        if self.restarts.len() >= self.max_restarts {
            return false;
        }
        self.restarts.push_back(now);
        true
    }
}

#[derive(Debug)]
struct PoolJob {
    job: MechanicsJob,
    reply: Sender<Result<Value, MechanicsError>>,
}

#[derive(Debug)]
enum PoolMessage {
    Run(PoolJob),
    Shutdown,
}

#[derive(Debug)]
struct WorkerExit {
    worker_id: usize,
}

#[derive(Debug)]
struct MechanicsPoolShared {
    tx: Sender<PoolMessage>,
    rx: Receiver<PoolMessage>,
    exit_tx: Sender<WorkerExit>,
    exit_rx: Receiver<WorkerExit>,
    workers: Mutex<HashMap<usize, thread::JoinHandle<()>>>,
    next_worker_id: AtomicUsize,
    closed: AtomicBool,
    restart_blocked: AtomicBool,
    restart_guard: Mutex<RestartGuard>,
    execution_limits: MechanicsExecutionLimits,
    default_http_timeout_ms: Option<u64>,
    reqwest_client: reqwest::Client,
}

impl MechanicsPoolShared {
    fn spawn_worker(shared: &Arc<Self>) -> usize {
        let worker_id = shared.next_worker_id.fetch_add(1, Ordering::Relaxed);

        let rx = shared.rx.clone();
        let exit_tx = shared.exit_tx.clone();
        let reqwest_client = shared.reqwest_client.clone();
        let execution_limits = shared.execution_limits;
        let default_http_timeout_ms = shared.default_http_timeout_ms;

        let handle = thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut runtime = RuntimeInternal::new_with_client(reqwest_client);
                runtime.set_execution_limits(execution_limits);
                runtime.set_default_endpoint_timeout_ms(default_http_timeout_ms);

                loop {
                    match rx.recv() {
                        Ok(PoolMessage::Run(pool_job)) => {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                runtime.run_source(pool_job.job)
                            }));
                            match result {
                                Ok(result) => {
                                    let _ = pool_job.reply.send(result);
                                }
                                Err(_) => {
                                    let _ = pool_job.reply.send(Err(MechanicsError::panic(
                                        "worker panicked while running job",
                                    )));
                                    break;
                                }
                            }
                        }
                        Ok(PoolMessage::Shutdown) => break,
                        Err(_) => break,
                    }
                }
            }));

            if run.is_err() {
                // If the worker panicked outside task execution (runtime setup/loop),
                // notify a synthetic panic event via restart path.
                let _ = exit_tx.send(WorkerExit { worker_id });
                return;
            }

            let _ = exit_tx.send(WorkerExit { worker_id });
        });

        let mut workers = shared.workers.lock().expect("workers mutex poisoned");
        workers.insert(worker_id, handle);
        worker_id
    }

    fn live_workers(&self) -> usize {
        self.workers.lock().expect("workers mutex poisoned").len()
    }
}

pub struct MechanicsPool {
    shared: Arc<MechanicsPoolShared>,
    enqueue_timeout: Duration,
    supervisor: Option<thread::JoinHandle<()>>,
}

impl MechanicsPool {
    pub fn new(config: MechanicsPoolConfig) -> Result<Self, MechanicsError> {
        if config.worker_count == 0 {
            return Err(MechanicsError::runtime_pool("worker_count must be > 0"));
        }
        if config.queue_capacity == 0 {
            return Err(MechanicsError::runtime_pool("queue_capacity must be > 0"));
        }
        if config.max_restarts_in_window == 0 {
            return Err(MechanicsError::runtime_pool("max_restarts_in_window must be > 0"));
        }

        let reqwest_client = reqwest::Client::builder()
            .build()
            .map_err(into_io_error)
            .map_err(|e| MechanicsError::runtime_pool(e.to_string()))?;

        let (tx, rx) = bounded(config.queue_capacity);
        let (exit_tx, exit_rx) = bounded::<WorkerExit>(config.worker_count.saturating_mul(4).max(8));

        let shared = Arc::new(MechanicsPoolShared {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: Mutex::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: Mutex::new(RestartGuard::new(
                config.restart_window,
                config.max_restarts_in_window,
            )),
            execution_limits: config.execution_limits,
            default_http_timeout_ms: config.default_http_timeout_ms,
            reqwest_client,
        });

        for _ in 0..config.worker_count {
            MechanicsPoolShared::spawn_worker(&shared);
        }

        let supervisor_shared = Arc::clone(&shared);
        let supervisor = thread::spawn(move || {
            loop {
                if supervisor_shared.closed.load(Ordering::Acquire) {
                    break;
                }

                match supervisor_shared.exit_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        let maybe_old = {
                            let mut workers = supervisor_shared
                                .workers
                                .lock()
                                .expect("workers mutex poisoned");
                            workers.remove(&event.worker_id)
                        };
                        if let Some(handle) = maybe_old {
                            let _ = handle.join();
                        }

                        if supervisor_shared.closed.load(Ordering::Acquire) {
                            continue;
                        }

                        let now = Instant::now();
                        let can_restart = {
                            let mut guard = supervisor_shared
                                .restart_guard
                                .lock()
                                .expect("restart guard mutex poisoned");
                            guard.allow_restart(now)
                        };

                        if can_restart {
                            MechanicsPoolShared::spawn_worker(&supervisor_shared);
                        } else {
                            supervisor_shared.restart_blocked.store(true, Ordering::Release);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            shared,
            enqueue_timeout: config.enqueue_timeout,
            supervisor: Some(supervisor),
        })
    }

    pub fn run(&self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(MechanicsError::pool_closed("runtime pool is closed"));
        }
        if self.shared.restart_blocked.load(Ordering::Acquire) && self.shared.live_workers() == 0 {
            return Err(MechanicsError::worker_unavailable(
                "all workers are unavailable and restart guard is active",
            ));
        }

        let (reply_tx, reply_rx) = bounded(1);
        let message = PoolMessage::Run(PoolJob {
            job,
            reply: reply_tx,
        });

        match self.shared.tx.send_timeout(message, self.enqueue_timeout) {
            Ok(()) => {}
            Err(SendTimeoutError::Timeout(PoolMessage::Run(pool_job))) => {
                let _ = pool_job.reply.send(Err(MechanicsError::queue_timeout(
                    "enqueue timed out because queue is full",
                )));
                return Err(MechanicsError::queue_timeout(
                    "enqueue timed out because queue is full",
                ));
            }
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err(MechanicsError::worker_unavailable(
                    "job queue disconnected from workers",
                ));
            }
            Err(SendTimeoutError::Timeout(PoolMessage::Shutdown)) => {
                return Err(MechanicsError::runtime_pool("unexpected shutdown message timeout"));
            }
        }

        match reply_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(MechanicsError::worker_unavailable(
                "worker dropped reply channel",
            )),
        }
    }

    pub fn try_run(&self, job: MechanicsJob) -> Result<Value, MechanicsError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(MechanicsError::pool_closed("runtime pool is closed"));
        }
        if self.shared.restart_blocked.load(Ordering::Acquire) && self.shared.live_workers() == 0 {
            return Err(MechanicsError::worker_unavailable(
                "all workers are unavailable and restart guard is active",
            ));
        }

        let (reply_tx, reply_rx) = bounded(1);
        let message = PoolMessage::Run(PoolJob {
            job,
            reply: reply_tx,
        });

        match self.shared.tx.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(PoolMessage::Run(_))) => {
                return Err(MechanicsError::queue_full("queue is full"));
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(MechanicsError::worker_unavailable(
                    "job queue disconnected from workers",
                ));
            }
            Err(TrySendError::Full(PoolMessage::Shutdown)) => {
                return Err(MechanicsError::runtime_pool("unexpected shutdown queue state"));
            }
        }

        match reply_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(MechanicsError::worker_unavailable(
                "worker dropped reply channel",
            )),
        }
    }
}

impl Drop for MechanicsPool {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);

        loop {
            match self.shared.rx.try_recv() {
                Ok(PoolMessage::Run(job)) => {
                    let _ = job
                        .reply
                        .send(Err(MechanicsError::canceled("pool dropped before job execution")));
                }
                Ok(PoolMessage::Shutdown) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        let worker_count = self.shared.live_workers();
        for _ in 0..worker_count {
            let _ = self.shared.tx.send(PoolMessage::Shutdown);
        }

        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }

        let mut workers = self.shared.workers.lock().expect("workers mutex poisoned");
        for (_, handle) in workers.drain() {
            let _ = handle.join();
        }
    }
}
