use crate::internal::{error::MechanicsError, job::MechanicsJob};
use crossbeam_channel::Sender;
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[derive(Debug)]
pub(crate) struct PoolJob {
    job: MechanicsJob,
    reply: Sender<Result<Value, MechanicsError>>,
    canceled: Arc<AtomicBool>,
}

impl PoolJob {
    pub(crate) fn new(
        job: MechanicsJob,
        reply: Sender<Result<Value, MechanicsError>>,
        canceled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            job,
            reply,
            canceled,
        }
    }

    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    pub(crate) fn mark_canceled(&self) {
        self.canceled.store(true, Ordering::Release);
    }

    pub(crate) fn send_result(&self, result: Result<Value, MechanicsError>) {
        let _ = self.reply.send(result);
    }

    pub(crate) fn reply_sender(&self) -> Sender<Result<Value, MechanicsError>> {
        self.reply.clone()
    }

    pub(crate) fn into_job(self) -> MechanicsJob {
        self.job
    }
}

#[derive(Debug)]
pub(crate) enum PoolMessage {
    Run(PoolJob),
}

#[derive(Debug)]
pub(crate) struct WorkerExit {
    worker_id: usize,
}

impl WorkerExit {
    pub(crate) fn new(worker_id: usize) -> Self {
        Self { worker_id }
    }

    pub(crate) fn worker_id(&self) -> usize {
        self.worker_id
    }
}

#[derive(Debug)]
pub(crate) struct WorkerHandle {
    join: thread::JoinHandle<()>,
    shutdown_tx: Sender<()>,
}

impl WorkerHandle {
    pub(crate) fn new(join: thread::JoinHandle<()>, shutdown_tx: Sender<()>) -> Self {
        Self { join, shutdown_tx }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    #[cfg(test)]
    pub(crate) fn from_join_for_test(join: thread::JoinHandle<()>) -> Self {
        let (shutdown_tx, _shutdown_rx) = crossbeam_channel::bounded::<()>(1);
        Self { join, shutdown_tx }
    }

    pub(crate) fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    pub(crate) fn join(self) {
        let _ = self.join.join();
    }
}
