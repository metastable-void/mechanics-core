use std::collections::HashMap;

use mechanics_core::{
    MechanicsPool, MechanicsPoolConfig,
    job::{MechanicsConfig, MechanicsJob},
};
use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use serde_json::json;

type SnapshotEntry = (
    metrics_util::CompositeKey,
    Option<metrics::Unit>,
    Option<metrics::SharedString>,
    DebugValue,
);

#[test]
fn pool_job_roundtrip_records_metrics() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install().expect("install metrics recorder");

    let pool =
        MechanicsPool::new(MechanicsPoolConfig::new().with_worker_count(1)).expect("create pool");

    let job = MechanicsJob::new(
        r#"
            export default function main(arg) {
                return { ok: arg.ok };
            }
        "#,
        json!({ "ok": true }),
        MechanicsConfig::new(HashMap::new()).expect("create config"),
    )
    .expect("construct job");

    let value = pool.run(job).expect("run job");
    assert_eq!(value, json!({ "ok": true }));

    let snapshot = snapshotter.snapshot().into_vec();
    assert_counter(&snapshot, "mechanics_jobs_accepted_total", &[], 1);
    assert_counter(
        &snapshot,
        "mechanics_jobs_completed_total",
        &[("outcome", "ok")],
        1,
    );
    assert_gauge(&snapshot, "mechanics_jobs_queue_depth", &[]);
    assert_gauge(&snapshot, "mechanics_pool_workers_total", &[]);
    assert_gauge(&snapshot, "mechanics_pool_workers_busy", &[]);
    assert_histogram_count(&snapshot, "mechanics_job_duration_seconds", &[], 1);
}

fn assert_counter(snapshot: &[SnapshotEntry], name: &str, labels: &[(&str, &str)], expected: u64) {
    let value = metric_value(snapshot, name, labels).unwrap_or_else(|| {
        panic!("missing counter metric `{name}` with labels {labels:?}");
    });
    match value {
        DebugValue::Counter(actual) => assert_eq!(*actual, expected),
        other => panic!("metric `{name}` was not a counter: {other:?}"),
    }
}

fn assert_gauge(snapshot: &[SnapshotEntry], name: &str, labels: &[(&str, &str)]) {
    let value = metric_value(snapshot, name, labels).unwrap_or_else(|| {
        panic!("missing gauge metric `{name}` with labels {labels:?}");
    });
    match value {
        DebugValue::Gauge(_) => {}
        other => panic!("metric `{name}` was not a gauge: {other:?}"),
    }
}

fn assert_histogram_count(
    snapshot: &[SnapshotEntry],
    name: &str,
    labels: &[(&str, &str)],
    expected: usize,
) {
    let value = metric_value(snapshot, name, labels).unwrap_or_else(|| {
        panic!("missing histogram metric `{name}` with labels {labels:?}");
    });
    match value {
        DebugValue::Histogram(values) => assert_eq!(values.len(), expected),
        other => panic!("metric `{name}` was not a histogram: {other:?}"),
    }
}

fn metric_value<'a>(
    snapshot: &'a [SnapshotEntry],
    name: &str,
    labels: &[(&str, &str)],
) -> Option<&'a DebugValue> {
    snapshot
        .iter()
        .find(|(key, _, _, _)| {
            key.key().name() == name
                && labels.iter().all(|(label_key, label_value)| {
                    key.key()
                        .labels()
                        .any(|label| label.key() == *label_key && label.value() == *label_value)
                })
        })
        .map(|(_, _, _, value)| value)
}
