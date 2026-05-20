use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::internal::error::{MechanicsError, MechanicsErrorKind};

pub(crate) const JOBS_ACCEPTED_TOTAL: &str = "mechanics_jobs_accepted_total";
pub(crate) const JOBS_COMPLETED_TOTAL: &str = "mechanics_jobs_completed_total";
pub(crate) const JOBS_QUEUE_DEPTH: &str = "mechanics_jobs_queue_depth";
pub(crate) const POOL_WORKERS_TOTAL: &str = "mechanics_pool_workers_total";
pub(crate) const POOL_WORKERS_BUSY: &str = "mechanics_pool_workers_busy";
pub(crate) const WORKER_RESTARTS_TOTAL: &str = "mechanics_worker_restarts_total";
pub(crate) const JOB_DURATION_SECONDS: &str = "mechanics_job_duration_seconds";
const JOB_OUTCOME_LABELS: &[&str] = &["ok", "failed", "timeout", "cancelled"];
const WORKER_RESTART_REASON_LABELS: &[&str] = &["panic", "timeout", "other"];

#[derive(Clone, Copy)]
pub(crate) enum JobOutcome {
    Ok,
    Failed,
    Timeout,
    Cancelled,
}

impl JobOutcome {
    pub(crate) fn from_error(error: &MechanicsError) -> Self {
        match error.kind() {
            MechanicsErrorKind::RunTimeout | MechanicsErrorKind::QueueTimeout => Self::Timeout,
            MechanicsErrorKind::Canceled => Self::Cancelled,
            MechanicsErrorKind::Execution
            | MechanicsErrorKind::QueueFull
            | MechanicsErrorKind::PoolClosed
            | MechanicsErrorKind::WorkerUnavailable
            | MechanicsErrorKind::Panic
            | MechanicsErrorKind::RuntimePool => Self::Failed,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(crate) struct WorkerBusyGuard {
    busy_workers: Arc<AtomicUsize>,
}

pub(crate) fn register_metrics() {
    let _ = ::metrics::counter!(
        description: "Jobs accepted onto the mechanics worker-pool queue.",
        JOBS_ACCEPTED_TOTAL
    );
    for outcome in JOB_OUTCOME_LABELS {
        let _ = ::metrics::counter!(
            description: "Mechanics jobs that reached a terminal state.",
            JOBS_COMPLETED_TOTAL,
            "outcome" => *outcome
        );
    }
    let _ = ::metrics::gauge!(
        description: "Current mechanics worker-pool queue depth.",
        JOBS_QUEUE_DEPTH
    );
    let _ = ::metrics::gauge!(
        description: "Current number of mechanics worker threads tracked by the pool.",
        POOL_WORKERS_TOTAL
    );
    let _ = ::metrics::gauge!(
        description: "Current number of mechanics worker threads executing jobs.",
        POOL_WORKERS_BUSY
    );
    for reason in WORKER_RESTART_REASON_LABELS {
        let _ = ::metrics::counter!(
            description: "Mechanics worker threads restarted by the pool supervisor.",
            WORKER_RESTARTS_TOTAL,
            "reason" => *reason
        );
    }
    let _ = ::metrics::histogram!(
        description: "Wall-clock mechanics job duration from queue acceptance to terminal state.",
        JOB_DURATION_SECONDS
    );
}

impl WorkerBusyGuard {
    pub(crate) fn start(busy_workers: Arc<AtomicUsize>) -> Self {
        let current =
            match busy_workers.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            }) {
                Ok(previous) => previous.saturating_add(1),
                Err(previous) => previous,
            };
        record_pool_workers_busy(current);
        Self { busy_workers }
    }
}

impl Drop for WorkerBusyGuard {
    fn drop(&mut self) {
        let current =
            match self
                .busy_workers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    Some(value.saturating_sub(1))
                }) {
                Ok(previous) => previous.saturating_sub(1),
                Err(previous) => previous,
            };
        record_pool_workers_busy(current);
    }
}

pub(crate) fn record_job_accepted(queue_depth: usize) {
    ::metrics::counter!(
        description: "Jobs accepted onto the mechanics worker-pool queue.",
        JOBS_ACCEPTED_TOTAL
    )
    .increment(1);
    record_queue_depth(queue_depth);
}

pub(crate) fn record_job_completed(accepted_at: Instant, outcome: JobOutcome) {
    ::metrics::counter!(
        description: "Mechanics jobs that reached a terminal state.",
        JOBS_COMPLETED_TOTAL,
        "outcome" => outcome.label()
    )
    .increment(1);
    record_job_duration(accepted_at.elapsed());
}

pub(crate) fn record_queue_depth(queue_depth: usize) {
    ::metrics::gauge!(
        description: "Current mechanics worker-pool queue depth.",
        JOBS_QUEUE_DEPTH
    )
    .set(usize_to_f64(queue_depth));
}

pub(crate) fn record_pool_workers_total(worker_count: usize) {
    ::metrics::gauge!(
        description: "Current number of mechanics worker threads tracked by the pool.",
        POOL_WORKERS_TOTAL
    )
    .set(usize_to_f64(worker_count));
}

pub(crate) fn record_worker_restart(reason: &'static str) {
    ::metrics::counter!(
        description: "Mechanics worker threads restarted by the pool supervisor.",
        WORKER_RESTARTS_TOTAL,
        "reason" => reason
    )
    .increment(1);
}

fn record_pool_workers_busy(worker_count: usize) {
    ::metrics::gauge!(
        description: "Current number of mechanics worker threads executing jobs.",
        POOL_WORKERS_BUSY
    )
    .set(usize_to_f64(worker_count));
}

fn record_job_duration(duration: Duration) {
    ::metrics::histogram!(
        description: "Wall-clock mechanics job duration from queue acceptance to terminal state.",
        JOB_DURATION_SECONDS
    )
    .record(duration.as_secs_f64());
}

fn usize_to_f64(value: usize) -> f64 {
    let bounded = u32::try_from(value).unwrap_or(u32::MAX);
    f64::from(bounded)
}
