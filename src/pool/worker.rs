use crate::{error::MechanicsError, job::MechanicsJob};
use crossbeam_channel::Sender;
use serde_json::Value;
use std::{
    sync::{Arc, atomic::AtomicBool},
    thread,
};

#[derive(Debug)]
pub(crate) struct PoolJob {
    pub(crate) job: MechanicsJob,
    pub(crate) reply: Sender<Result<Value, MechanicsError>>,
    pub(crate) canceled: Arc<AtomicBool>,
}

#[derive(Debug)]
pub(crate) enum PoolMessage {
    Run(PoolJob),
}

#[derive(Debug)]
pub(crate) struct WorkerExit {
    pub(crate) worker_id: usize,
}

#[derive(Debug)]
pub(crate) struct WorkerHandle {
    pub(crate) join: thread::JoinHandle<()>,
    pub(crate) shutdown_tx: Sender<()>,
}

impl WorkerHandle {
    pub(crate) fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    #[cfg(test)]
    pub(crate) fn from_join_for_test(join: thread::JoinHandle<()>) -> Self {
        let (shutdown_tx, _shutdown_rx) = crossbeam_channel::bounded::<()>(1);
        Self { join, shutdown_tx }
    }
}
