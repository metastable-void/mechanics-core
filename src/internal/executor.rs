use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, Module,
    context::time::JsInstant,
    job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob},
    module::ModuleLoader,
};
use futures_concurrency::future::FutureGroup;
use futures_lite::StreamExt;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};
use tokio::task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueSnapshot {
    pub(crate) in_flight_async_jobs: usize,
    pub(crate) promise_jobs: usize,
    pub(crate) timeout_jobs: usize,
    pub(crate) generic_jobs: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunJobsExit {
    Complete,
    DeadlineExceeded(QueueSnapshot),
}

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

    fn deadline_exceeded(&self, context: &Context) -> bool {
        let Some(deadline) = *self.deadline.borrow() else {
            return false;
        };
        context.clock().now() >= deadline
    }

    fn snapshot(&self, in_flight_async_jobs: usize) -> QueueSnapshot {
        QueueSnapshot {
            in_flight_async_jobs,
            promise_jobs: self.promise_jobs.borrow().len(),
            timeout_jobs: self.timeout_jobs.borrow().values().map(Vec::len).sum(),
            generic_jobs: self.generic_jobs.borrow().len(),
        }
    }

    pub(crate) fn run_jobs_until_then_to_quiescence<F, G>(
        self: Rc<Self>,
        context: &mut Context,
        should_stop: F,
        on_stop: G,
    ) -> JsResult<RunJobsExit>
    where
        F: FnMut() -> bool,
        G: FnOnce(&mut Context) -> JsResult<()>,
    {
        let this = Rc::clone(&self);
        self.tokio_local.block_on(
            &self.tokio_rt,
            this.run_jobs_async_until_then_to_quiescence(
                &RefCell::new(context),
                should_stop,
                on_stop,
            ),
        )
    }

    fn next_timeout_at(&self) -> Option<JsInstant> {
        self.timeout_jobs
            .borrow()
            .first_key_value()
            .map(|(k, _)| *k)
    }

    fn instant_checked_add(base: JsInstant, delta: Duration) -> Option<JsInstant> {
        let base_ms = u128::from(base.millis_since_epoch());
        let delta_ms = delta.as_millis();
        let total_ms = base_ms.checked_add(delta_ms)?;
        let total_ms = u64::try_from(total_ms).ok()?;
        Self::js_instant_from_millis(total_ms)
    }

    fn js_instant_from_millis(ms: u64) -> Option<JsInstant> {
        let nanos = (ms % 1000).checked_mul(1_000_000)?;
        let nanos = u32::try_from(nanos).ok()?;
        Some(JsInstant::new(ms / 1000, nanos))
    }

    fn millis_until_or_zero(later: JsInstant, earlier: JsInstant) -> u64 {
        later
            .millis_since_epoch()
            .saturating_sub(earlier.millis_since_epoch())
    }

    fn has_due_timeout_job(&self, now: JsInstant) -> bool {
        self.next_timeout_at().is_some_and(|at| at <= now)
    }

    fn wait_budget(&self, now: JsInstant) -> Option<Duration> {
        let timeout_budget = self.next_timeout_at().map(|next_timeout_at| {
            Duration::from_millis(Self::millis_until_or_zero(next_timeout_at, now))
        });
        let deadline_budget = (*self.deadline.borrow())
            .map(|deadline| Duration::from_millis(Self::millis_until_or_zero(deadline, now)));
        match (timeout_budget, deadline_budget) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
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

    async fn run_jobs_async_until_then_to_quiescence<F, G>(
        self: Rc<Self>,
        context: &RefCell<&mut Context>,
        mut should_stop: F,
        on_stop: G,
    ) -> JsResult<RunJobsExit>
    where
        F: FnMut() -> bool,
        G: FnOnce(&mut Context) -> JsResult<()>,
    {
        let mut group = FutureGroup::new();
        let mut in_flight_async_jobs = 0_usize;
        let mut stopped = false;
        let mut on_stop = Some(on_stop);
        loop {
            if !stopped && should_stop() {
                if let Some(on_stop) = on_stop.take() {
                    on_stop(&mut context.borrow_mut())?;
                }
                stopped = true;
            }

            {
                let ctx_ref = context.borrow();
                if self.deadline_exceeded(&ctx_ref) {
                    return Ok(RunJobsExit::DeadlineExceeded(
                        self.snapshot(in_flight_async_jobs),
                    ));
                }
            }

            for job in std::mem::take(&mut *self.async_jobs.borrow_mut()) {
                group.insert(job.call(context));
                in_flight_async_jobs = in_flight_async_jobs.saturating_add(1);
            }

            if group.is_empty()
                && self.promise_jobs.borrow().is_empty()
                && self.timeout_jobs.borrow().is_empty()
                && self.generic_jobs.borrow().is_empty()
            {
                return Ok(RunJobsExit::Complete);
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
                            let mut d = Duration::from_millis(Self::millis_until_or_zero(
                                next_timeout_at,
                                now,
                            ));
                            if let Some(deadline) = *self.deadline.borrow() {
                                let remaining = if deadline <= now {
                                    Duration::ZERO
                                } else {
                                    Duration::from_millis(Self::millis_until_or_zero(deadline, now))
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
                let (has_sync_ready_jobs, wait_budget) = {
                    let ctx_ref = context.borrow();
                    let now = ctx_ref.clock().now();
                    (
                        !self.promise_jobs.borrow().is_empty()
                            || !self.generic_jobs.borrow().is_empty()
                            || self.has_due_timeout_job(now),
                        self.wait_budget(now),
                    )
                };

                let next_result = if has_sync_ready_jobs {
                    None
                } else if let Some(wait_budget) = wait_budget {
                    if wait_budget.is_zero() {
                        None
                    } else {
                        tokio::time::timeout(wait_budget, group.next())
                            .await
                            .unwrap_or_default()
                    }
                } else {
                    group.next().await
                };

                match next_result {
                    Some(Ok(_)) => {
                        in_flight_async_jobs = in_flight_async_jobs.saturating_sub(1);
                    }
                    Some(Err(err)) => return Err(err),
                    None => {}
                }
            }

            {
                let ctx_ref = context.borrow();
                if self.deadline_exceeded(&ctx_ref) {
                    return Ok(RunJobsExit::DeadlineExceeded(
                        self.snapshot(in_flight_async_jobs),
                    ));
                }
            }

            self.drain_jobs(&mut context.borrow_mut())?;
            task::yield_now().await
        }
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
                let Some(at) = Self::instant_checked_add(now, t.timeout().into()) else {
                    // Previously the overflow path was clamped to the
                    // `u64::MAX` sentinel, which placed the timer at
                    // an unreachable position in the BTreeMap — the
                    // callback never fired and the `t` closure was
                    // retained until job teardown. Route the failure
                    // through the runtime as a synchronous
                    // `RangeError` so the script sees a catchable JS
                    // error instead of a silently-dropped timer.
                    let realm = context.realm().clone();
                    let err = GenericJob::new(
                        move |_| {
                            Err(JsError::from_native(JsNativeError::range().with_message(
                                "setTimeout delay is too large for the current platform clock",
                            )))
                        },
                        realm,
                    );
                    self.generic_jobs.borrow_mut().push_back(err);
                    return;
                };
                self.timeout_jobs
                    .borrow_mut()
                    .entry(at)
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
        match self
            .run_jobs_async_until_then_to_quiescence(context, || false, |_| Ok(()))
            .await?
        {
            RunJobsExit::Complete => Ok(()),
            RunJobsExit::DeadlineExceeded(_) => Err(Self::timeout_error()),
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
        assert_eq!(
            queue
                .timeout_jobs
                .borrow()
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            1
        );
        assert_eq!(queue.generic_jobs.borrow().len(), 1);
    }
}
