//! RAII guard covering partial pool construction.
//!
//! `MechanicsPool::new` allocates state in several stages:
//!   1. Build shared state (channels, worker map).
//!   2. Spawn each requested worker thread.
//!   3. Spawn the supervisor thread.
//!
//! If stage 2 fails after some workers have spawned, or stage 3
//! fails after stage 2 succeeded, the already-running threads
//! must be cleanly torn down — otherwise they leak past
//! `MechanicsPool::new`'s `Err(...)` return (no `MechanicsPool`
//! ever exists for `Drop` to fire on).
//!
//! `PoolConstructor` is the RAII guard that handles that cleanup.
//! `new` registers the guard against the partially-built `shared`
//! state. Each successful `spawn_worker` is implicitly covered
//! because the worker is in `shared.workers`. On `Drop`, the
//! guard performs the same shutdown sequence as
//! `impl Drop for MechanicsPool` (mark closed, drain pending
//! jobs, request worker shutdown, join supervisor, join worker
//! handles).
//!
//! On the success path, `commit` returns the (shared, supervisor,
//! supervisor_shutdown_tx) tuple back to the caller and sets
//! `committed = true` so `Drop` becomes a no-op.

use std::{sync::Arc, thread::JoinHandle};

use crossbeam_channel::Sender;

use super::{metrics as pool_metrics, shared::MechanicsPoolShared, worker::PoolMessage};
use crate::internal::error::MechanicsError;

pub(super) struct PoolConstructor {
    shared: Arc<MechanicsPoolShared>,
    supervisor: Option<JoinHandle<()>>,
    supervisor_shutdown_tx: Option<Sender<()>>,
    committed: bool,
}

impl PoolConstructor {
    /// Begin guarding a partial pool construction. The caller has
    /// already created the shared state and may go on to spawn
    /// workers / supervisor; the guard's Drop will tear those
    /// down if construction fails before [`Self::commit`].
    pub(super) fn new(shared: Arc<MechanicsPoolShared>) -> Self {
        Self {
            shared,
            supervisor: None,
            supervisor_shutdown_tx: None,
            committed: false,
        }
    }

    /// Register the supervisor handle + shutdown channel so the
    /// guard can join them on Drop if a later step fails.
    pub(super) fn attach_supervisor(
        &mut self,
        supervisor: JoinHandle<()>,
        supervisor_shutdown_tx: Sender<()>,
    ) {
        self.supervisor = Some(supervisor);
        self.supervisor_shutdown_tx = Some(supervisor_shutdown_tx);
    }

    /// Commit the guard: construction succeeded, so on Drop we
    /// should NOT tear anything down. Returns the fields the
    /// caller needs to populate the `MechanicsPool` struct.
    pub(super) fn commit(
        mut self,
    ) -> (
        Arc<MechanicsPoolShared>,
        Option<JoinHandle<()>>,
        Option<Sender<()>>,
    ) {
        self.committed = true;
        let shared = Arc::clone(&self.shared);
        let supervisor = self.supervisor.take();
        let supervisor_shutdown_tx = self.supervisor_shutdown_tx.take();
        (shared, supervisor, supervisor_shutdown_tx)
    }
}

impl Drop for PoolConstructor {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Mirror `impl Drop for MechanicsPool` in
        // `pool/drop_impl.rs`. Order matters: mark closed first
        // so reconciliation loops exit; drain in-flight jobs so
        // their reply channels close cleanly; signal supervisor
        // and workers to exit; join.
        self.shared.mark_closed();

        while let Ok(PoolMessage::Run(job)) = self.shared.job_receiver().try_recv() {
            job.send_result(Err(MechanicsError::canceled(
                "pool construction failed before job execution",
            )));
            pool_metrics::record_queue_depth(self.shared.queue_depth());
        }

        {
            let workers = self.shared.workers_read();
            for handle in workers.values() {
                handle.request_shutdown();
            }
        }

        if let Some(tx) = self.supervisor_shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }

        let mut workers = self.shared.workers_write();
        for (_, handle) in workers.drain() {
            handle.join();
        }
        pool_metrics::record_pool_workers_total(0);
    }
}
