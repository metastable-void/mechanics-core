use crate::{
    error::MechanicsError, http::EndpointHttpClient, job::MechanicsExecutionLimits,
    runtime::RuntimeInternal,
};
use crossbeam_channel::{Receiver, Sender, bounded, select};
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

use super::{
    config::MechanicsPoolConfig,
    restart_guard::RestartGuard,
    worker::{PoolMessage, WorkerExit, WorkerHandle},
};

#[derive(Debug)]
pub(crate) struct MechanicsPoolShared {
    tx: Sender<PoolMessage>,
    rx: Receiver<PoolMessage>,
    exit_tx: Sender<WorkerExit>,
    exit_rx: Receiver<WorkerExit>,
    workers: RwLock<HashMap<usize, WorkerHandle>>,
    next_worker_id: AtomicUsize,
    desired_worker_count: usize,
    closed: AtomicBool,
    restart_blocked: AtomicBool,
    restart_guard: Mutex<RestartGuard>,
    execution_limits: MechanicsExecutionLimits,
    default_http_timeout_ms: Option<u64>,
    default_http_response_max_bytes: Option<usize>,
    endpoint_http_client: Arc<dyn EndpointHttpClient>,
    #[cfg(test)]
    pub(crate) force_worker_runtime_init_failure: bool,
}

impl MechanicsPoolShared {
    pub(crate) fn new(
        config: &MechanicsPoolConfig,
        endpoint_http_client: Arc<dyn EndpointHttpClient>,
        tx: Sender<PoolMessage>,
        rx: Receiver<PoolMessage>,
        exit_tx: Sender<WorkerExit>,
        exit_rx: Receiver<WorkerExit>,
    ) -> Self {
        Self {
            tx,
            rx,
            exit_tx,
            exit_rx,
            workers: parking_lot::RwLock::new(HashMap::new()),
            next_worker_id: AtomicUsize::new(0),
            desired_worker_count: config.worker_count(),
            closed: AtomicBool::new(false),
            restart_blocked: AtomicBool::new(false),
            restart_guard: parking_lot::Mutex::new(RestartGuard::new(
                config.restart_window(),
                config.max_restarts_in_window(),
            )),
            execution_limits: config.execution_limits(),
            default_http_timeout_ms: config.default_http_timeout_ms(),
            default_http_response_max_bytes: config.default_http_response_max_bytes(),
            endpoint_http_client,
            #[cfg(test)]
            force_worker_runtime_init_failure: config.force_worker_runtime_init_failure,
        }
    }

    pub(crate) fn workers_read(&self) -> RwLockReadGuard<'_, HashMap<usize, WorkerHandle>> {
        self.workers.read()
    }

    pub(crate) fn workers_write(&self) -> RwLockWriteGuard<'_, HashMap<usize, WorkerHandle>> {
        self.workers.write()
    }

    pub(crate) fn restart_guard_snapshot(&self) -> (usize, usize) {
        let guard = self.restart_guard.lock();
        (
            guard.restart_attempts_in_window(),
            guard.max_restarts_in_window(),
        )
    }

    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn is_restart_blocked(&self) -> bool {
        self.restart_blocked.load(Ordering::Acquire)
    }

    pub(crate) fn set_restart_blocked(&self, blocked: bool) {
        self.restart_blocked.store(blocked, Ordering::Release);
    }

    pub(crate) fn desired_worker_count(&self) -> usize {
        self.desired_worker_count
    }

    pub(crate) fn queue_depth(&self) -> usize {
        self.rx.len()
    }

    pub(crate) fn queue_capacity(&self) -> Option<usize> {
        self.rx.capacity()
    }

    pub(crate) fn job_sender(&self) -> &Sender<PoolMessage> {
        &self.tx
    }

    pub(crate) fn job_receiver(&self) -> &Receiver<PoolMessage> {
        &self.rx
    }

    pub(crate) fn worker_exit_receiver(&self) -> &Receiver<WorkerExit> {
        &self.exit_rx
    }

    pub(crate) fn record_restart_attempt(&self, now: Instant) -> bool {
        let mut guard = self.restart_guard.lock();
        guard.allow_restart(now)
    }

    fn remove_worker_handle(&self, worker_id: usize) -> Option<WorkerHandle> {
        let mut workers = self.workers_write();
        workers.remove(&worker_id)
    }

    fn reap_finished_workers(&self) {
        let finished_ids: Vec<usize> = {
            let workers = self.workers_read();
            workers
                .iter()
                .filter_map(|(id, handle)| handle.is_finished().then_some(*id))
                .collect()
        };
        if finished_ids.is_empty() {
            return;
        }

        let mut finished_handles = Vec::with_capacity(finished_ids.len());
        {
            let mut workers = self.workers_write();
            for id in finished_ids {
                if let Some(handle) = workers.remove(&id) {
                    finished_handles.push(handle);
                }
            }
        }
        for handle in finished_handles {
            handle.join();
        }
    }

    pub(crate) fn spawn_worker(shared: &Arc<Self>) -> Result<usize, MechanicsError> {
        let worker_id = shared.next_worker_id.fetch_add(1, Ordering::Relaxed);

        let rx = shared.rx.clone();
        let exit_tx = shared.exit_tx.clone();
        let (ready_tx, ready_rx) = bounded::<Result<(), MechanicsError>>(1);
        let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
        let endpoint_http_client = Arc::clone(&shared.endpoint_http_client);
        let execution_limits = shared.execution_limits;
        let default_http_timeout_ms = shared.default_http_timeout_ms;
        let default_http_response_max_bytes = shared.default_http_response_max_bytes;
        #[cfg(test)]
        let force_runtime_init_failure = shared.force_worker_runtime_init_failure;

        let handle = thread::Builder::new()
            .name(format!("mechanics-worker-{worker_id}"))
            .spawn(move || {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if force_runtime_init_failure {
                        let _ = ready_tx.send(Err(MechanicsError::runtime_pool(
                            "forced runtime initialization failure for tests",
                        )));
                        return;
                    }

                    let mut runtime = match RuntimeInternal::new_with_endpoint_http_client(
                        endpoint_http_client,
                    ) {
                        Ok(runtime) => {
                            let _ = ready_tx.send(Ok(()));
                            runtime
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            return;
                        }
                    };
                    runtime.set_execution_limits(execution_limits);
                    runtime.set_default_endpoint_timeout_ms(default_http_timeout_ms);
                    runtime.set_default_endpoint_response_max_bytes(default_http_response_max_bytes);

                    loop {
                        select! {
                            recv(shutdown_rx) -> _ => {
                                break;
                            }
                            recv(rx) -> msg => {
                                match msg {
                                    Ok(PoolMessage::Run(pool_job)) => {
                                        if pool_job.is_canceled() {
                                            pool_job.send_result(Err(MechanicsError::canceled(
                                                "job timed out before execution",
                                            )));
                                            continue;
                                        }
                                        let reply = pool_job.reply_sender();
                                        let job = pool_job.into_job();
                                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                            runtime.run_source(job)
                                        }));
                                        match result {
                                            Ok(result) => {
                                                let _ = reply.send(result);
                                            }
                                            Err(_) => {
                                                let _ = reply.send(Err(MechanicsError::panic(
                                                    "worker panicked while running job",
                                                )));
                                                break;
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                }));

                if run.is_err() {
                    let _ = ready_tx.send(Err(MechanicsError::panic("worker panicked during startup")));
                    let _ = exit_tx.send(WorkerExit::new(worker_id));
                    return;
                }

                let _ = exit_tx.send(WorkerExit::new(worker_id));
            })
            .map_err(|e| {
                MechanicsError::runtime_pool(format!("failed to spawn worker thread: {e}"))
            })?;

        {
            let mut workers = shared.workers_write();
            workers.insert(
                worker_id,
                WorkerHandle::new(handle, shutdown_tx),
            );
        }

        match ready_rx.recv() {
            Ok(Ok(())) => {
                shared.set_restart_blocked(false);
                Ok(worker_id)
            }
            Ok(Err(err)) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    handle.join();
                }
                Err(err)
            }
            Err(_) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    handle.join();
                }
                Err(MechanicsError::runtime_pool(
                    "worker exited before startup completed",
                ))
            }
        }
    }

    pub(crate) fn live_workers(&self) -> usize {
        self.reap_finished_workers();
        self.workers_read().len()
    }

    pub(crate) fn reconcile_workers(shared: &Arc<Self>) {
        if shared.is_closed() {
            return;
        }

        let live = shared.live_workers();
        let missing = shared.desired_worker_count().saturating_sub(live);
        if missing == 0 {
            shared.set_restart_blocked(false);
            return;
        }

        for _ in 0..missing {
            let now = Instant::now();
            let can_restart = shared.record_restart_attempt(now);
            if !can_restart {
                shared.set_restart_blocked(true);
                return;
            }

            if MechanicsPoolShared::spawn_worker(shared).is_err() {
                shared.set_restart_blocked(true);
                return;
            }
        }
    }
}
