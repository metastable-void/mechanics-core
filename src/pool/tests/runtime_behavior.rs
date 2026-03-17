use super::*;

#[test]
fn run_simple_module_returns_value() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(arg) {
                return { ok: true, got: arg };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        json!({"n": 7}),
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["got"]["n"], json!(7));
}

#[test]
fn loop_iteration_limit_stops_infinite_loop() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        execution_limits: MechanicsExecutionLimits {
            max_execution_time: Duration::from_secs(5),
            max_loop_iterations: 1_000,
            max_recursion_depth: 512,
            max_stack_size: 10 * 1024,
        },
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(_arg) {
                while (true) {}
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool.run(job).expect_err("must hit loop iteration limit");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("Maximum loop iteration limit"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn json_conversion_error_is_reported_as_execution_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(_arg) {
                return 1n;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("BigInt result should fail JSON conversion");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(
                msg.contains("BigInt")
                    || msg.contains("JSON")
                    || msg.contains("serialize")
                    || msg.contains("convert")
            );
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn pending_default_promise_is_reported_as_execution_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(_arg) {
                return new Promise(() => {});
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("pending promise should not be treated as success");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("did not settle") || msg.contains("pending"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn unhandled_async_error_is_reported_as_execution_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(_arg) {
                Promise.resolve().then(() => {
                    throw new Error("boom");
                });
                return 1;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("unhandled async error should fail current job");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("boom") || msg.contains("Error") || msg.contains("Unhandled"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
fn oversized_execution_timeout_is_reported_as_execution_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        execution_limits: MechanicsExecutionLimits {
            max_execution_time: Duration::MAX,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            export default function main(_arg) {
                return 1;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("oversized max_execution_time must not panic worker");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("max_execution_time") || msg.contains("too large"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}
