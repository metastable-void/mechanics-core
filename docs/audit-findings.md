# mechanics-core audit findings (2026-03-18)

This report supersedes the previous content of this file. Prior versions are archived in git history.

## Scope
- Runtime/pool execution paths: `src/pool.rs`, `src/runtime.rs`, `src/executor.rs`, `src/job.rs`, `src/error.rs`.
- HTTP/config and endpoint protocol: `src/http.rs`, `src/runtime/synthetic_modules.rs`, `src/http/tests/*`, `src/pool/tests/*`.
- Documentation/type contracts: `README.md`, `docs/behavior.md`, `ts-types/*.d.ts`.

## Verification performed
- `cargo test --all-targets`
- Result: pass (`64 passed`, `0 failed`, `20 ignored`).
- `cargo clippy --all-targets --all-features -- -D warnings`
- Result: fail on style-only `clippy::derivable_impls` in `src/http.rs` (`EndpointBodyType`, `SlottedQueryMode`).

## Findings

### 1) Restart limiter can permanently brick the pool after a crash burst
- Severity: high
- Category: potential runtime bug / availability
- Status: done (2026-03-18)
- Evidence:
- `restart_blocked` is set when restart attempts are blocked (`src/pool.rs:434`, `src/pool.rs:430`).
- Restart attempts are only triggered while processing `exit_rx` events (`src/pool.rs:404` to `src/pool.rs:438`).
- If all workers are already gone and rate-limit is hit, no new exit event arrives to re-attempt restart.
- `run`/`run_try_enqueue` then fail with `WorkerUnavailable` when `restart_blocked && live_workers()==0` (`src/pool.rs:469`, `src/pool.rs:553`).
- Impact:
- Pool may remain permanently unavailable even after `restart_window` elapses.
- Resolution:
- Added periodic worker reconciliation in supervisor loop (`MechanicsPoolShared::reconcile_workers`) so restart attempts are re-evaluated on timeout ticks, not only on new exit events.
- Added desired worker target tracking (`desired_worker_count`) and recovery logic to refill missing workers and clear `restart_blocked` on success.
- Verification:
- Added regression test: `src/pool/tests/lifecycle.rs` (`reconcile_workers_recovers_after_restart_window_without_new_exit_events`).

### 2) VM job queue state can leak across job boundaries after early termination
- Severity: high
- Category: potential runtime bug / isolation contract
- Status: done (2026-03-18)
- Evidence:
- Queue state is long-lived in `Queue` (`src/executor.rs:19` to `src/executor.rs:24`) and reused by `RuntimeInternal` (`src/runtime.rs:105`).
- `run_source_inner` cleanup only removes context data, deadline, host hook state, and realm (`src/runtime.rs:256` to `src/runtime.rs:259`).
- No explicit queue reset/clear exists after execution errors/timeouts.
- Impact:
- If `ctx.run_jobs()` returns early on timeout/error, remaining queued async/promise/timeout jobs may execute during a later job, violating per-job isolation.
- Resolution:
- Added `Queue::clear_all()` to drain async/promise/timeout/generic job queues.
- Added runtime finalization cleanup to call `self.queue.clear_all()` in `run_source_inner` after each run (success and error).
- Verification:
- Added regression test: `src/pool/tests/runtime_behavior.rs` (`timed_out_job_does_not_leak_pending_timeout_tasks_into_next_job`).

### 3) `MechanicsPool::drop` can block for a long or unbounded duration
- Severity: medium
- Category: lifecycle/runtime behavior
- Status: done (2026-03-18)
- Evidence:
- `Drop` sends one `Shutdown` message per live worker using blocking `send` on bounded queue (`src/pool.rs:625` to `src/pool.rs:627`).
- Workers only receive shutdown when they return to queue receive loop; busy workers delay consumption.
- Impact:
- Drop/join latency can become very long under stuck/long-running workers.
- Resolution:
- Removed blocking shutdown-message sends from `Drop`.
- Worker loop now uses bounded `recv_timeout` polling and exits on observed pool-closed state.
- Verification:
- Added regression test: `src/pool/tests/lifecycle.rs` (`drop_does_not_block_when_queue_is_full_and_worker_is_not_receiving`).

### 4) Protocol contradiction: explicit `body: null` is treated as absent, not JSON null
- Severity: medium
- Category: protocol contract mismatch
- Status: done (2026-03-18)
- Evidence:
- Parsing maps `body: null` to `EndpointCallBody::Absent` (`src/http.rs:1011`).
- Execution omits body when `Absent` (`src/http.rs:608`).
- Docs currently state JSON mode accepts “any JSON-convertible JS value” (`docs/behavior.md:97` to `docs/behavior.md:100`), which implies `null` should be sendable.
- Impact:
- Callers cannot send explicit JSON `null` payloads on `POST`/`PUT`.
- Resolution:
- Changed endpoint option parsing semantics:
- omitted/`undefined` => `EndpointCallBody::Absent`,
- explicit `null` => `EndpointCallBody::Json(Value::Null)`.
- Updated docs and TS declaration comments to describe the explicit null behavior and omission semantics.
- Verification:
- Added parser-level test: `src/http/tests/options.rs` (`parse_endpoint_call_options_treats_explicit_null_body_as_json_null`).
- Added runtime contract test: `src/pool/tests/endpoint_validation.rs` (`endpoint_get_rejects_explicit_null_body`).

### 5) Docs mismatch: non-2xx opt-in behavior is broader than documented
- Severity: medium
- Category: documentation contradiction
- Status: done (2026-03-18)
- Evidence:
- Docs say `with_allow_non_success_status(true)` “allows JSON parsing on non-2xx” (`docs/behavior.md:90`).
- Implementation allows normal downstream parsing according to `response_body_type` (`json`/`utf8`/`bytes`) (`src/http.rs:651`, `src/http.rs:682` to `src/http.rs:694`).
- Impact:
- Contract text is narrower than actual behavior.
- Resolution:
- Updated `docs/behavior.md` to describe non-2xx opt-in behavior as response-body-type-driven parsing, matching implementation.

### 6) Docs are incomplete for optional query modes when `default` is set
- Severity: medium
- Category: undocumented non-obvious behavior
- Status: done (2026-03-18)
- Evidence:
- Docs describe optional modes primarily as omission semantics (`docs/behavior.md:119` to `docs/behavior.md:120`).
- Runtime resolves missing values through `default` first (`src/http.rs:884` to `src/http.rs:891`).
- Impact:
- Users may assume missing optional values are always omitted, which is false when default exists.
- Resolution:
- Updated `docs/behavior.md` with explicit slotted query resolution precedence and empty-default behavior by mode.

### 7) Fail-fast constructor docs do not mention call-time `run_timeout` overflow failure
- Severity: low
- Category: documentation/behavior mismatch
- Evidence:
- `MechanicsPool::new` docs describe fail-fast invalid config handling (`src/pool.rs:340` to `src/pool.rs:346`).
- Constructor checks `run_timeout != 0` only (`src/pool.rs:358` to `src/pool.rs:360`).
- Overflow guard is deferred to `run`/`run_try_enqueue` deadline calculation (`src/pool.rs:323` to `src/pool.rs:326`).
- Impact:
- A pool can construct successfully but fail every run with `RuntimePool` for extreme timeout values.
- Recommendation:
- Validate overflow feasibility in constructor, or document this as call-time failure.

### 8) Unsafe tracing invariants are implicit
- Severity: low
- Category: undefined-behavior risk surface (future maintenance)
- Evidence:
- `MechanicsState` uses `#[unsafe_ignore_trace]` fields (`src/runtime.rs:59`, `src/runtime.rs:62`, `src/runtime.rs:65`, `src/runtime.rs:68`) with no nearby safety rationale.
- Impact:
- Current code appears safe, but future refactors could accidentally violate GC tracing assumptions.
- Recommendation:
- Add explicit safety comments/invariants for each ignored field.

### 9) Test coverage gaps hide several high-risk behaviors from default runs
- Severity: medium
- Category: testing gap
- Evidence:
- Queue pressure/concurrency tests are ignored (`src/pool/tests/queue.rs:83`, `src/pool/tests/queue.rs:154`).
- Network/socket integration tests are ignored by default (`src/pool/tests/endpoint_network.rs`, `src/pool/tests/internet.rs:4`).
- No test currently exercises real supervisor recovery after hitting restart guard.
- Impact:
- Regressions in availability/concurrency paths can slip through normal CI.
- Recommendation:
- Add deterministic local tests for supervisor restart recovery and queue pressure behavior that can run in standard CI.

## Requested categories explicitly checked

### Undefined behavior
- No active UB found in normal runtime paths (code is predominantly safe Rust).
- UB risk surface exists around `#[unsafe_ignore_trace]` usage (documented above as finding #8).

### Unimplemented code paths
- No `todo!`, `unimplemented!`, or `unreachable!` found in production code under `src/`.
- Fallback for unknown Boa job variants exists and returns runtime error (`src/executor.rs:137` to `src/executor.rs:148`).

### Possible runtime panics
- No obvious production-path `panic!/unwrap()/expect()` crash points found in `src/` runtime code paths reviewed.
- Panic usage observed is test-only.

## Suggested follow-up order
1. Harden lifecycle/docs/testing gaps (#7, #9).
2. Add explicit GC safety invariants (#8).
