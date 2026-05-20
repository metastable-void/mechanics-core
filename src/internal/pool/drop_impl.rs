use crate::internal::error::MechanicsError;

use super::{api::MechanicsPool, metrics as pool_metrics, worker::PoolMessage};

impl Drop for MechanicsPool {
    fn drop(&mut self) {
        self.shared.mark_closed();

        while let Ok(PoolMessage::Run(job)) = self.shared.job_receiver().try_recv() {
            job.send_result(Err(MechanicsError::canceled(
                "pool dropped before job execution",
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
