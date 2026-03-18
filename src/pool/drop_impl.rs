use crate::error::MechanicsError;
use crossbeam_channel::TryRecvError;
use std::sync::atomic::Ordering;

use super::{api::MechanicsPool, worker::PoolMessage};

impl Drop for MechanicsPool {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);

        loop {
            match self.shared.rx.try_recv() {
                Ok(PoolMessage::Run(job)) => {
                    let _ = job.reply.send(Err(MechanicsError::canceled(
                        "pool dropped before job execution",
                    )));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        {
            let workers = self.shared.workers_read();
            for handle in workers.values() {
                let _ = handle.shutdown_tx.send(());
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
            let _ = handle.join.join();
        }
    }
}
