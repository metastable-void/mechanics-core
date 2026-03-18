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
- Result: pass (`85 passed`, `0 failed`, `20 ignored`).
- `cargo clippy --all-targets --all-features -- -D warnings`
- Result: pass (2026-03-18).

## Active findings
- None currently open.

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
- 12) HTTP resilience policies missing: fixed with JSON-deserializable `retry_policy` on endpoints (retry/backoff/rate-limit behavior, config validation, and tests).
- 13) HTTP method set too narrow: fixed by adding `PATCH`/`HEAD`/`OPTIONS` and aligning body policy to RFC 9110 baseline.
- 14) Public pool stats API missing: fixed with synchronous non-blocking `MechanicsPool::stats()` returning `MechanicsPoolStats` (worker/queue/restart snapshot) and non-blocking behavior test.
- 15) Config composition helpers missing: fixed with validated `with_endpoint`, `with_endpoint_overrides`, and `without_endpoint` APIs (per-job config composition).
- 16) Orchestration-primitives gap reframed and addressed: generalized orchestration module deemed unnecessary for this crate scope; added focused `mechanics:uuid` utility module (`v3`/`v4`/`v5`/`v6`/`v7`/`nil`/`max`) with docs/types/tests.
- 17) Endpoint transport was fixed to reqwest internals: fixed with pool-level pluggable transport (`EndpointHttpClient`) plus default `ReqwestEndpointHttpClient`, including deterministic injected-client test coverage and docs.
