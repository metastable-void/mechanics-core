use boa_engine::{
    Context, JsArgs, JsData, JsError, JsNativeError, JsResult, JsString, JsValue, Module, NativeFunction, Source, Trace, builtins::promise::PromiseState, context::{ContextBuilder, time::JsInstant}, job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob}, js_string, module::{ModuleLoader, SyntheticModuleInitializer}, object::{FunctionObjectBuilder, builtins::JsPromise}
};

use boa_gc::Finalize;
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};
use parking_lot::RwLock;
use reqwest::header::{HeaderMap, HeaderName};
use serde::{Serialize, Deserialize};
use serde_json::Value;
use tokio::task;
use std::{collections::{BTreeMap, HashMap, VecDeque}, cell::RefCell, rc::Rc, sync::{Arc, Mutex}};
use std::ops::DerefMut;

pub(crate) fn into_io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> std::io::Error {
    std::io::Error::other(e)
}

#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone)]
pub struct HttpEndpoint {
    url: String,
    headers: HashMap<String, String>,
}

impl HttpEndpoint {
    pub const USER_AGENT: &str = concat!("Mozilla/5.0 (compatible; mechanics/", env!("CARGO_PKG_VERSION"), ")");

    pub fn new(url: &str, headers: HashMap<String, String>) -> Self {
        Self {
            url: url.to_owned(),
            headers,
        }
    }

    pub async fn post<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(&self, req_data: &Req) -> std::io::Result<Res> {
        // Serialize once here so callers can pass any serde-compatible request type.
        let json = serde_json::to_string(req_data).map_err(into_io_error)?;
        let url = reqwest::Url::parse(&self.url).map_err(into_io_error)?;
        let client = reqwest::Client::builder().build().map_err(into_io_error)?;
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
        // `body(json)` sends raw bytes; set JSON content type explicitly for strict servers.
        headers.insert("Content-Type", "application/json".try_into().unwrap());
        let res = client.post(url).headers(headers).body(json)
            .send().await.map_err(into_io_error)?;
        let res: Res = res.json().await.map_err(into_io_error)?;
        Ok(res)
    }
}

pub(crate) struct Queue {
    async_jobs: Arc<RwLock<VecDeque<NativeAsyncJob>>>,
    promise_jobs: Arc<RwLock<VecDeque<PromiseJob>>>,
    timeout_jobs: Arc<RwLock<BTreeMap<JsInstant, TimeoutJob>>>,
    generic_jobs: Arc<RwLock<VecDeque<GenericJob>>>,
}

impl Queue {
    pub(crate) fn new() -> Self {
        Self {
            async_jobs: Arc::new(RwLock::default()),
            promise_jobs: Arc::new(RwLock::default()),
            timeout_jobs: Arc::new(RwLock::default()),
            generic_jobs: Arc::new(RwLock::default()),
        }
    }

    fn drain_timeout_jobs(&self, context: &mut Context) {
        let now = context.clock().now();

        let mut timeouts_borrow = self.timeout_jobs.write();
        // `split_off(now)` keeps future jobs in the map and returns due jobs to run now.
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

    fn drain_jobs(&self, context: &mut Context) {
        // Run the timeout jobs first.
        self.drain_timeout_jobs(context);

        let job = self.generic_jobs.write().pop_front();
        if let Some(generic) = job
            && let Err(err) = generic.call(context)
        {
            eprintln!("Uncaught {err}");
        }

        let jobs = std::mem::take(&mut *self.promise_jobs.write());
        for job in jobs {
            if let Err(e) = job.call(context) {
                eprintln!("Uncaught {e}");
            }
        }
        context.clear_kept_objects();
    }
}

impl JobExecutor for Queue {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.write().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.write().push_back(job),
            Job::TimeoutJob(t) => {
                let now = context.clock().now();
                self.timeout_jobs.write().insert(now + t.timeout(), t);
            }
            Job::GenericJob(g) => self.generic_jobs.write().push_back(g),
            _ => panic!("unsupported job type"),
        }
    }

    // While the sync flavor of `run_jobs` will block the current thread until all the jobs have finished...
    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        task::LocalSet::default().block_on(&runtime, self.run_jobs_async(&RefCell::new(context)))
    }

    // ...the async flavor won't, which allows concurrent execution with external async tasks.
    async fn run_jobs_async(self: Rc<Self>, context: &RefCell<&mut Context>) -> JsResult<()> {
        let mut group = FutureGroup::new();
        loop {
            for job in std::mem::take(&mut *self.async_jobs.write()) {
                group.insert(job.call(context));
            }

            if group.is_empty()
                && self.promise_jobs.read().is_empty()
                && self.timeout_jobs.read().is_empty()
                && self.generic_jobs.read().is_empty()
            {
                // All queues are empty. We can exit.
                return Ok(());
            }

            // We have some jobs pending on the microtask queue. Try to poll the pending
            // tasks once to see if any of them finished, and run the pending microtasks
            // otherwise.
            if let Some(Err(err)) = future::poll_once(group.next()).await.flatten() {
                eprintln!("Uncaught {err}");
            };

            // Only one macrotask can be executed before the next microtask drain.
            // Keep mutable `context` borrows scoped to this call site.
            self.drain_jobs(&mut context.borrow_mut());
            task::yield_now().await
        }
    }
}

#[derive(JsData, Trace, Finalize, Serialize, Deserialize, Clone)]
pub struct RuntimeState {
    endpoints: HashMap<String, HttpEndpoint>,
}

impl RuntimeState {
    pub fn new(endpoints: HashMap<String, HttpEndpoint>) -> Self {
        Self {
            endpoints,
        }
    }
}

pub(crate) struct CustomModuleLoader {
    defined: RefCell<HashMap<JsString, Module>>,
}

impl CustomModuleLoader {
    pub(crate) fn new() -> Self {
        Self {
            defined: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn define_module(&self, spec: JsString, module: Module) {
       self.defined.borrow_mut().insert(spec, module);
    }
}

impl ModuleLoader for CustomModuleLoader {
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

pub struct Runtime {
    ctx: Arc<Mutex<Context>>,
}

impl Runtime {
    pub fn new(state: RuntimeState) -> Self {
        let queue = Queue::new();

        let loader = Rc::new(CustomModuleLoader::new());
        let mut context = ContextBuilder::new()
            .job_executor(Rc::new(queue))
            .module_loader(loader.clone())
            .build()
            .unwrap();

        context.insert_data(state);
        let endpoint = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_async_fn(async |_, args, ctx| {
                let endpoint = args.get_or_undefined(0).as_string()
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("endpoint is not a string")))?;
                // Convert JS value to serde JSON once before the network call.
                let req_body = args.get_or_undefined(1).to_json(&mut ctx.borrow_mut())?
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("JSON error")))?;
                
                let ctx_ref = ctx.borrow();
                let state = ctx_ref.get_data::<RuntimeState>().cloned()
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("Invalid state")))?;
                
                // Do not hold a RefCell borrow across `.await`, otherwise the job loop
                // can panic when it tries to borrow the same context mutably.
                drop(ctx_ref);
                let endpoint_name = endpoint.to_std_string_lossy();
                let endpoint = state.endpoints.get(&endpoint_name)
                    .ok_or(JsError::from_native(JsNativeError::typ().with_message("Endpoint not found")))?;
                
                let res: Value = endpoint.post(&req_body).await
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
            ctx: Arc::new(Mutex::new(context)),
        }
    }

    pub(crate) fn run_source_inner<S: AsRef<str>, V: Serialize>(&self, source: S, arg: V) -> JsResult<JsValue> {
        let arg = serde_json::to_value(arg)
            .map_err(JsError::from_rust)?;
        let source = source.as_ref();
        let mut ctx = self.ctx.lock().unwrap();
        let source = Source::from_bytes(source);
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
    }

    pub fn run_source<S: AsRef<str>, V: Serialize>(&self, source: S, arg: V) -> Result<Value, String> {
        match self.run_source_inner(source, arg) {
            Ok(data) => {
                let mut ctx = self.ctx.lock().unwrap();
                match data.to_json(&mut ctx) {
                    Ok(d) => Ok(d.unwrap_or(Value::Null)),
                    _ => Ok(Value::Null),
                }
            },

            Err(e) => {
                Err(e.to_string())
            },
        }
    }
}
