use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, Module,
    context::time::JsInstant,
    job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob},
    module::ModuleLoader,
};
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};
use tokio::task;

/// Job queues backing Boa's executor integration.
pub(crate) struct Queue {
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    timeout_jobs: RefCell<BTreeMap<JsInstant, Vec<TimeoutJob>>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    deadline: RefCell<Option<JsInstant>>,
    tokio_rt: tokio::runtime::Runtime,
    tokio_local: tokio::task::LocalSet,
}

impl Queue {
    /// Creates an empty job queue backing Boa's executor hooks.
    pub(crate) fn new() -> std::io::Result<Self> {
        let tokio_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let tokio_local = tokio::task::LocalSet::new();

        Ok(Self {
            async_jobs: RefCell::default(),
            promise_jobs: RefCell::default(),
            timeout_jobs: RefCell::default(),
            generic_jobs: RefCell::default(),
            deadline: RefCell::default(),
            tokio_rt,
            tokio_local,
        })
    }

    fn timeout_error() -> JsError {
        JsError::from_native(
            JsNativeError::runtime_limit().with_message("Maximum execution time exceeded"),
        )
    }

    pub(crate) fn set_deadline(&self, deadline: Option<JsInstant>) {
        *self.deadline.borrow_mut() = deadline;
    }

    pub(crate) fn clear_all(&self) {
        self.async_jobs.borrow_mut().clear();
        self.promise_jobs.borrow_mut().clear();
        self.timeout_jobs.borrow_mut().clear();
        self.generic_jobs.borrow_mut().clear();
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
        self.timeout_jobs
            .borrow()
            .first_key_value()
            .map(|(k, _)| *k)
    }

    /// Executes all due timeout jobs and keeps only future/cancel-surviving entries.
    fn drain_timeout_jobs(&self, context: &mut Context) -> JsResult<()> {
        let now = context.clock().now();

        let jobs_to_run = {
            let mut timeouts_borrow = self.timeout_jobs.borrow_mut();
            timeouts_borrow.retain(|_, jobs| {
                jobs.retain(|job| !job.is_cancelled());
                !jobs.is_empty()
            });

            let mut due = Vec::new();
            while let Some((at, _)) = timeouts_borrow.first_key_value() {
                if *at > now {
                    break;
                }
                if let Some((_, mut jobs)) = timeouts_borrow.pop_first() {
                    due.append(&mut jobs);
                }
            }
            due
        };

        for job in jobs_to_run {
            job.call(context)?;
        }
        Ok(())
    }

    /// Drains one macrotask turn in Boa order: timeout, one generic task, then promise jobs.
    fn drain_jobs(&self, context: &mut Context) -> JsResult<()> {
        self.drain_timeout_jobs(context)?;

        let job = self.generic_jobs.borrow_mut().pop_front();
        if let Some(generic) = job {
            generic.call(context)?;
        }

        let jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
        for job in jobs {
            job.call(context)?;
        }
        context.clear_kept_objects();
        Ok(())
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
                self.timeout_jobs
                    .borrow_mut()
                    .entry(now + t.timeout())
                    .or_default()
                    .push(t);
            }
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            other => {
                let realm = context.realm().clone();
                let message = format!("unsupported job type: {other:?}");
                let err = GenericJob::new(
                    move |_| {
                        Err(JsError::from_native(
                            JsNativeError::typ().with_message(message.clone()),
                        ))
                    },
                    realm,
                );
                self.generic_jobs.borrow_mut().push_back(err);
            }
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
                if self.promise_jobs.borrow().is_empty()
                    && self.generic_jobs.borrow().is_empty()
                    && let Some(next_timeout_at) = self.next_timeout_at()
                {
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
            } else {
                let deadline = *self.deadline.borrow();
                let polled = if let Some(deadline) = deadline {
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
                    return Err(err);
                }
            }

            {
                let ctx_ref = context.borrow();
                self.check_deadline(&ctx_ref)?;
            }

            self.drain_jobs(&mut context.borrow_mut())?;
            task::yield_now().await
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
    async fn load_imported_module(
        self: Rc<Self>,
        _referrer: boa_engine::module::Referrer,
        specifier: JsString,
        _context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        self.defined
            .borrow()
            .get(&specifier)
            .cloned()
            .ok_or(JsError::from_native(
                JsNativeError::reference().with_message("Module not found"),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{
        JsValue,
        job::{GenericJob, NativeAsyncJob, PromiseJob, TimeoutJob},
    };
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn timeout_jobs_at_same_instant_do_not_overwrite_each_other() {
        let queue = Queue::new().expect("queue runtime should initialize");
        let mut context = Context::default();
        let at = context.clock().now();

        let counter = Rc::new(Cell::new(0));
        let c1 = Rc::clone(&counter);
        let c2 = Rc::clone(&counter);

        let job1 = TimeoutJob::from_duration(
            move |_| {
                c1.set(c1.get() + 1);
                Ok(JsValue::undefined())
            },
            Duration::ZERO,
        );
        let job2 = TimeoutJob::from_duration(
            move |_| {
                c2.set(c2.get() + 10);
                Ok(JsValue::undefined())
            },
            Duration::ZERO,
        );

        queue.timeout_jobs.borrow_mut().insert(at, vec![job1, job2]);
        queue
            .drain_timeout_jobs(&mut context)
            .expect("timeout jobs should run without error");
        assert_eq!(counter.get(), 11);
    }

    #[test]
    fn job_routing_harness_covers_all_current_boa_job_variants() {
        // Compatibility harness: if Boa adds constructible variants in future versions,
        // extend this test to assert explicit routing behavior for the new variants.
        let queue = Rc::new(Queue::new().expect("queue runtime should initialize"));
        let mut context = Context::default();
        let realm = context.realm().clone();

        Rc::clone(&queue).enqueue_job(
            Job::PromiseJob(PromiseJob::new(|_| Ok(JsValue::undefined()))),
            &mut context,
        );
        Rc::clone(&queue).enqueue_job(
            Job::AsyncJob(NativeAsyncJob::new(async |_| Ok(JsValue::undefined()))),
            &mut context,
        );
        Rc::clone(&queue).enqueue_job(
            Job::TimeoutJob(TimeoutJob::from_duration(
                |_| Ok(JsValue::undefined()),
                Duration::from_millis(1),
            )),
            &mut context,
        );
        Rc::clone(&queue).enqueue_job(
            Job::GenericJob(GenericJob::new(|_| Ok(JsValue::undefined()), realm)),
            &mut context,
        );

        assert_eq!(queue.promise_jobs.borrow().len(), 1);
        assert_eq!(queue.async_jobs.borrow().len(), 1);
        assert_eq!(queue.timeout_jobs.borrow().values().map(Vec::len).sum::<usize>(), 1);
        assert_eq!(queue.generic_jobs.borrow().len(), 1);
    }
}
