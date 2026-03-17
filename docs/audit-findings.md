# mechanics-core audit findings (2026-03-17)

This report lists inconsistencies, strange implementations, and redundant/unused parts found by reading the full codebase and running targeted checks.

## Critical / high

1. [DONE] Pending Promise results were returned as `{}` instead of timing out or failing.
- Fixed in: `src/runtime.rs` (pending state now returns execution error), `src/pool.rs` (regression test).
- Previous location: `src/runtime.rs:192-216`, `src/runtime.rs:226-233`
- Previous behavior: `PromiseState::Pending` was treated as success (`Ok(res.into())`) and converted to JSON object.
- Previous impact: jobs that never resolved could appear successful and bypass expected timeout semantics.

## Medium

2. [DONE] Unhandled async errors can be logged but not surfaced as job failure.
- Fixed in: `src/executor.rs` (job errors now propagate through `run_jobs`), `src/pool.rs` (regression test).
- Previous location: `src/executor.rs:99-103`, `src/executor.rs:117-123`, `src/executor.rs:222-224`
- Previous issue: errors in queue draining paths were printed with `eprintln!` and execution could still appear successful.

3. [DONE] `try_run` name/expectation mismatch.
- Fixed in: `src/pool.rs` (`try_run` renamed to `run_try_enqueue`).
- Previous location: `src/pool.rs:396-446`
- Previous issue: enqueue was non-blocking, but method still blocked waiting for result while name implied full non-blocking behavior.

4. [DONE] Reply-timeout model is heuristic and can become very large.
- Fixed in: `src/pool.rs` (`run_timeout` config added, heuristic removed, `RunTimeout` introduced).
- Previous location: `src/pool.rs:204-217`
- Previous issue: timeout budget scaled with `queue_capacity + worker_count + 1`.

5. [DONE] HTTP status codes are not validated before JSON parse.
- Fixed in: `src/http.rs` (`error_for_status` by default, endpoint opt-in to allow non-success status), `src/pool.rs` (regression tests).
- Previous location: `src/http.rs:67-68`
- Previous issue: response was parsed as JSON regardless of status.

6. [DONE] Invalid configured headers are silently dropped.
- Fixed in: `src/http.rs` (strict header validation and explicit `InvalidInput` errors), `src/pool.rs` (regression test).
- Previous location: `src/http.rs:55-58`
- Previous issue: invalid header key/value parse failures were ignored.

## Low

7. [DONE] Panics/unwraps remain in non-test runtime paths.
- Fixed in: `src/runtime.rs` (`ContextBuilder::build` errors are mapped to `MechanicsError::RuntimePool`), `src/executor.rs` (Tokio runtime build returns `Result`; unsupported job types become JS errors), `src/http.rs` (default header values validated without panic paths), `src/pool.rs` (fallible `thread::Builder::spawn` replaces panic-prone `thread::spawn` in runtime paths; pool locks migrated to `parking_lot`).
- Previous issue: panic-prone convenience methods (`unwrap`, explicit `panic!`, panic-capable header insertion helpers) were used in runtime paths.

8. Worker startup handshake sends rendezvous signal while holding worker map lock.
- Location: `src/pool.rs:193-196`
- Why: `start_tx.send(())` on zero-capacity channel while lock is held.
- Impact: not incorrect, but can increase lock contention during worker startup.

## Redundant / unused

9. [DONE] Unused dependency: `parking_lot`.
- Fixed in: `src/pool.rs` (runtime pool synchronization now uses `parking_lot::Mutex` / `parking_lot::RwLock`).
- Previous location: `Cargo.toml:19`
- Previous issue: dependency was present but not used.

10. Potentially over-broad Tokio feature set.
- Location: `Cargo.toml:24`
- Current: `tokio = { features = ["full"] }`
- Observation: code uses current-thread runtime, local set, sleep, and task yield; `full` may be broader than needed.

## Suggested fix order
1. Fix pending Promise behavior (item 1).
2. Decide and implement policy for uncaught queued job errors (item 2).
3. Tighten HTTP semantics (items 5, 6).
4. Reduce Tokio feature set if desired (item 10).
