# mechanics-core audit findings (2026-03-17)

This report lists inconsistencies, strange implementations, and redundant/unused parts found by reading the full codebase and running targeted checks.

## Critical / high

1. [DONE] Pending Promise results were returned as `{}` instead of timing out or failing.
- Fixed in: `src/runtime.rs` (pending state now returns execution error), `src/pool.rs` (regression test).
- Previous location: `src/runtime.rs:192-216`, `src/runtime.rs:226-233`
- Previous behavior: `PromiseState::Pending` was treated as success (`Ok(res.into())`) and converted to JSON object.
- Previous impact: jobs that never resolved could appear successful and bypass expected timeout semantics.

## Medium

2. Unhandled async errors can be logged but not surfaced as job failure.
- Location: `src/executor.rs:99-103`, `src/executor.rs:117-123`, `src/executor.rs:222-224`
- Why: job errors in queue draining paths are printed with `eprintln!` and not propagated.
- Impact: caller may receive successful result while background rejection/error happened.

3. `try_run` name/expectation mismatch.
- Location: `src/pool.rs:396-446`
- Why: enqueue is non-blocking, but method still blocks waiting for result.
- Impact: callers may expect immediate return semantics from the method name.
- Note: behavior is now partially documented, but API naming remains potentially misleading.

4. Reply-timeout model is heuristic and can become very large.
- Location: `src/pool.rs:204-217`
- Why: timeout budget scales with `queue_capacity + worker_count + 1`.
- Impact: for large queues, "bounded" wait can still be operationally near-unbounded.

5. HTTP status codes are not validated before JSON parse.
- Location: `src/http.rs:67-68`
- Why: response always parsed as JSON regardless of status.
- Impact: 4xx/5xx responses may be treated as success if JSON body parses.

6. Invalid configured headers are silently dropped.
- Location: `src/http.rs:55-58`
- Why: invalid header key/value parse failures are ignored.
- Impact: misconfiguration is hard to detect; requests may miss required auth/custom headers.

## Low

7. Panics/unwraps remain in non-test runtime paths.
- `ContextBuilder::build().unwrap()`: `src/runtime.rs:94`
- Tokio runtime build unwrap: `src/executor.rs:31-35`
- Header insertion unwraps: `src/http.rs:60-61`
- Unsupported job panic: `src/executor.rs:142`
- Impact: process-level panic risk instead of structured errors.

8. Worker startup handshake sends rendezvous signal while holding worker map mutex.
- Location: `src/pool.rs:193-196`
- Why: `start_tx.send(())` on zero-capacity channel while lock is held.
- Impact: not incorrect, but can increase lock contention during worker startup.

## Redundant / unused

9. Unused dependency: `parking_lot`.
- Location: `Cargo.toml:19`
- Evidence: no references in `src/` or `examples/`.

10. Potentially over-broad Tokio feature set.
- Location: `Cargo.toml:24`
- Current: `tokio = { features = ["full"] }`
- Observation: code uses current-thread runtime, local set, sleep, and task yield; `full` may be broader than needed.

## Suggested fix order
1. Fix pending Promise behavior (item 1).
2. Decide and implement policy for uncaught queued job errors (item 2).
3. Tighten HTTP semantics (items 5, 6).
4. Decide API strategy for `try_run` naming/behavior (item 3).
5. Remove unused dependency and reduce Tokio features if desired (items 9, 10).
