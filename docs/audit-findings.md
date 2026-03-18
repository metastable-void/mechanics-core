# mechanics-core audit findings (2026-03-18)

This report supersedes previous content of this file. Prior versions are archived in git history.

## Report routine
- Keep active/open findings first.
- Keep resolved items only in the bottom `Done summary` section, concise (1-2 lines each).
- When an item is resolved, move it from active findings to `Done summary` in the same change.

## Scope
Update this section on code additions.

- Runtime/pool execution paths: `src/pool.rs`, `src/runtime.rs`, `src/executor.rs`, `src/job.rs`, `src/error.rs`.
- HTTP/config and endpoint protocol: `src/http.rs`, `src/runtime/synthetic_modules.rs`, `src/http/tests/*`, `src/pool/tests/*`.
- Documentation/type contracts: `README.md`, `docs/behavior.md`, `ts-types/*.d.ts`.

## Verification performed
- `cargo test --all-targets`
- Result: pass (`77 passed`, `0 failed`, `20 ignored`).
- `cargo clippy --all-targets --all-features -- -D warnings`
- Last recorded result: pass (2026-03-18).

## Active findings

### 12) HTTP resilience policies are not first-class (retry/backoff/rate-limit)
- Severity: medium
- Category: missing capability (reliability)
- Status: open
- Evidence:
- Endpoint config currently has timeout/size/status policy, but no retry/backoff/circuit-breaker controls (`src/http.rs`).
- Impact:
- Callers must re-implement retry logic in JS modules, reducing determinism and increasing duplicated policy code.
- Proposed direction:
- Add optional endpoint retry policy (`max_attempts`, `base_backoff_ms`, `max_backoff_ms`, `jitter`, `retry_on_status`, `retry_on_io_errors`).
- Retry only idempotent methods by default (`GET`/`HEAD`/`OPTIONS`) unless explicitly opted in for mutating methods.
- Emit attempt metadata on response (`attempt`, `max_attempts`) and expose terminal retry reason in execution errors.
- JSON-first requirement: policy must be fully representable in endpoint config JSON (`serde`), because parsing `MechanicsConfig` (and often whole `MechanicsJob`) from JSON is a first-class crate feature.

### 14) No public pool telemetry/stats API despite internal state tracking
- Severity: medium
- Category: missing capability (observability)
- Status: open
- Evidence:
- Pool tracks worker and restart state internally, but exposes no public stats snapshot API (`src/pool.rs`).
- Impact:
- Orchestrators cannot make informed scaling/circuit decisions from native pool state.
- Proposed direction:
- Add `MechanicsPool::stats()` snapshot with counters (`jobs_submitted`, `jobs_completed`, `jobs_failed`, `worker_restarts`) and gauges (`live_workers`, `queue_depth`, `restart_blocked`).
- Optionally add event hooks for worker crash/restart lifecycle events.

### 16) Built-in runtime modules do not expose orchestration primitives
- Severity: low
- Category: missing capability (runtime expressiveness)
- Status: open
- Evidence:
- Current synthetic modules are endpoint + codecs + RNG (`src/runtime/synthetic_modules.rs`, `src/lib.rs`).
- Impact:
- Users rebuild common orchestration helpers (IDs/events/checkpoints) per script set.
- Proposed direction:
- Consider optional feature-gated synthetic modules for deterministic IDs, step/event emission hooks, and checkpoint serialization helpers.

## Additional audit notes
- Undefined behavior: no active UB found in normal runtime paths; prior `unsafe_ignore_trace` safety notes were added.
- Unimplemented code paths: no `todo!`/`unimplemented!` in production code under `src/`.
- Panic risk: no production-path `panic!/unwrap()/expect()` crash points found in reviewed runtime code.

## Done summary

- 1) Restart limiter could permanently brick pool after crash burst: fixed with periodic worker reconciliation and restart recovery test.
- 2) VM job queue state leak across job boundaries: fixed with queue cleanup after each run and isolation regression test.
- 3) `MechanicsPool::drop` long/unbounded block risk: fixed by removing blocking shutdown-send path and validating drop behavior with tests.
- 4) Protocol contradiction for explicit `body: null`: fixed (`null` now means JSON null; `undefined`/omitted means absent), docs/tests updated.
- 5) Non-2xx opt-in docs mismatch: fixed docs to reflect response parsing by `response_body_type`.
- 6) Optional query mode docs gap with defaults: fixed docs to include explicit resolution precedence and empty-default semantics.
- 7) Constructor docs/behavior mismatch for `run_timeout` overflow: fixed with constructor-time validation and test.
- 8) Unsafe tracing invariants were implicit: fixed by adding explicit SAFETY comments for `unsafe_ignore_trace` fields.
- 9) Default test coverage gaps for high-risk behavior: fixed with deterministic local tests (queue pressure, restart recovery, disconnect paths).
- 10) Async API intent not documented: fixed docs to state sync API is intentional and Tokio usage should be through `spawn_blocking`; added interop test.
- 11) Endpoint response missing status metadata: fixed by adding `status`/`ok` fields across runtime, docs, types, and tests.
- 13) HTTP method set too narrow: fixed by adding `PATCH`/`HEAD`/`OPTIONS` and aligning body policy to RFC 9110 baseline.
- 15) Config composition helpers missing: fixed with validated `with_endpoint`, `with_endpoint_overrides`, and `without_endpoint` APIs (per-job config composition).
