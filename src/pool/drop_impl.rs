use crate::error::MechanicsError;
use crossbeam_channel::TryRecvError;

use super::{api::MechanicsPool, worker::PoolMessage};

impl Drop for MechanicsPool {
    fn drop(&mut self) {
        self.shared.mark_closed();

        loop {
            match self.shared.job_receiver().try_recv() {
                Ok(PoolMessage::Run(job)) => {
                    job.send_result(Err(MechanicsError::canceled(
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
    }
}
