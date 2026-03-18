use crate::{
    error::MechanicsError, http::EndpointHttpClient, job::MechanicsExecutionLimits,
    runtime::RuntimeInternal,
};
use crossbeam_channel::{Receiver, Sender, bounded, select};
use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
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
    restart_guard::RestartGuard,
    worker::{PoolMessage, WorkerExit, WorkerHandle},
};

#[derive(Debug)]
pub(crate) struct MechanicsPoolShared {
    pub(crate) tx: Sender<PoolMessage>,
    pub(crate) rx: Receiver<PoolMessage>,
    pub(crate) exit_tx: Sender<WorkerExit>,
    pub(crate) exit_rx: Receiver<WorkerExit>,
    pub(crate) workers: RwLock<HashMap<usize, WorkerHandle>>,
    pub(crate) next_worker_id: AtomicUsize,
    pub(crate) desired_worker_count: usize,
    pub(crate) closed: AtomicBool,
    pub(crate) restart_blocked: AtomicBool,
    pub(crate) restart_guard: Mutex<RestartGuard>,
    pub(crate) execution_limits: MechanicsExecutionLimits,
    pub(crate) default_http_timeout_ms: Option<u64>,
    pub(crate) default_http_response_max_bytes: Option<usize>,
    pub(crate) endpoint_http_client: Arc<dyn EndpointHttpClient>,
    #[cfg(test)]
    pub(crate) force_worker_runtime_init_failure: bool,
}

impl MechanicsPoolShared {
    pub(crate) fn workers_read(&self) -> RwLockReadGuard<'_, HashMap<usize, WorkerHandle>> {
        self.workers.read()
    }

    pub(crate) fn workers_write(&self) -> RwLockWriteGuard<'_, HashMap<usize, WorkerHandle>> {
        self.workers.write()
    }

    pub(crate) fn restart_guard_guard(&self) -> MutexGuard<'_, RestartGuard> {
        self.restart_guard.lock()
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
            let _ = handle.join.join();
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
                                        if pool_job.canceled.load(Ordering::Acquire) {
                                            let _ = pool_job.reply.send(Err(MechanicsError::canceled(
                                                "job timed out before execution",
                                            )));
                                            continue;
                                        }
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
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                }));

                if run.is_err() {
                    let _ = ready_tx.send(Err(MechanicsError::panic("worker panicked during startup")));
                    let _ = exit_tx.send(WorkerExit { worker_id });
                    return;
                }

                let _ = exit_tx.send(WorkerExit { worker_id });
            })
            .map_err(|e| {
                MechanicsError::runtime_pool(format!("failed to spawn worker thread: {e}"))
            })?;

        {
            let mut workers = shared.workers_write();
            workers.insert(
                worker_id,
                WorkerHandle {
                    join: handle,
                    shutdown_tx,
                },
            );
        }

        match ready_rx.recv() {
            Ok(Ok(())) => {
                shared.restart_blocked.store(false, Ordering::Release);
                Ok(worker_id)
            }
            Ok(Err(err)) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    let _ = handle.join.join();
                }
                Err(err)
            }
            Err(_) => {
                if let Some(handle) = shared.remove_worker_handle(worker_id) {
                    let _ = handle.join.join();
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
        if shared.closed.load(Ordering::Acquire) {
            return;
        }

        let live = shared.live_workers();
        let missing = shared.desired_worker_count.saturating_sub(live);
        if missing == 0 {
            shared.restart_blocked.store(false, Ordering::Release);
            return;
        }

        for _ in 0..missing {
            let now = Instant::now();
            let can_restart = {
                let mut guard = shared.restart_guard_guard();
                guard.allow_restart(now)
            };
            if !can_restart {
                shared.restart_blocked.store(true, Ordering::Release);
                return;
            }

            if MechanicsPoolShared::spawn_worker(shared).is_err() {
                shared.restart_blocked.store(true, Ordering::Release);
                return;
            }
        }
    }
}
