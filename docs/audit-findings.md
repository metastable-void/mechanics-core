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
- Result: pass (`88 passed`, `0 failed`, `20 ignored`).
- `cargo clippy --all-targets --all-features -- -D warnings`
- Result: pass (2026-03-18).

## Active findings
- 25) Redundant invariant checks persist on hot paths after prepared endpoint caching
  - Severity: low
  - Type: redundant code / performance
  - Evidence:
  - `validate_config` enforces URL/query structural invariants in `src/http/endpoint/validate.rs`.
  - `build_url_prepared` repeats several static checks (slot spec presence and mismatch checks) in `src/http/endpoint/request.rs`.
  - Risk:
  - Additional per-call overhead and larger maintenance surface for logically identical checks.
  - Potential drift if one validation path changes and the other is not kept in sync.
  - Proposed direction:
  - Separate one-time static validation from per-call dynamic input validation.
  - Retain defense-in-depth where safety critical, but document and centralize any intentionally duplicated checks.

- 26) Thin forwarding layer `runtime/synthetic_modules.rs` is functionally redundant
  - Severity: low
  - Type: redundant code / structure
  - Evidence:
  - `install_synthetic_modules` in `src/runtime/synthetic_modules.rs` only forwards to `builtins::bundle_builtin_modules`.
  - Risk:
  - Minor indirection and naming duplication without additional invariants or abstraction value.
  - Proposed direction:
  - Either remove the forwarding module and call `builtins::bundle_builtin_modules` directly, or keep it but explicitly document that it is a stable seam for future runtime module composition.

## Additional audit notes
- Undefined behavior: no active UB found in normal runtime paths; prior `unsafe_ignore_trace` safety notes were added.
- Unimplemented code paths: no `todo!`/`unimplemented!` in production code under `src/`.
- Panic risk: no production-path `panic!/unwrap()/expect()` crash points found in reviewed runtime code.
- JSON-first shape artifacts: maintain and update `ts-types/mechanics-json-shapes.d.ts` and `json-schema/*.schema.json` when serde-visible payload shapes change.

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
- 18) Late response-size enforcement in default reqwest transport: fixed by enforcing max-bytes during streaming body reads inside `ReqwestEndpointHttpClient` (before full buffering), with endpoint-side cap checks retained as defense-in-depth.
- 19) Endpoint hot-path parse/allowlist allocation overhead: fixed by introducing per-job prepared endpoint caches (`PreparedHttpEndpoint`) in runtime state; caches are scoped to each `MechanicsJob` and dropped with job state (no cross-job leakage).
- 20) Fixed 100ms worker/supervisor polling loops: fixed without regressing prior restart mitigations by switching workers to blocking channel select with explicit shutdown signaling, and supervisor to event-driven select plus periodic reconcile tick (for restart-window recovery logic).
- 21) Per-worker Tokio runtime footprint scaling with worker count: documented as an intended design limitation (isolation-first worker model), not an immediate bug.
- 22) Transport abstraction reqwest leak: fixed by introducing crate-owned transport-neutral endpoint types (`EndpointHttpHeaders`, string URL in `EndpointHttpRequest`) and confining reqwest conversions to `ReqwestEndpointHttpClient`; tests updated and passing.
- 23) Public mutable core config/job fields bypassed invariants: fixed by tightening external field visibility (`pub(crate)`), adding validated public constructors/builders (`MechanicsJob::new`, `MechanicsExecutionLimits::new`, `MechanicsPoolConfig` builder methods), and centralizing pool-config validation while preserving JSON-first serde ingestion semantics.
- 24) Internal pool state overexposed via wide `pub(crate)` fields: fixed by making `RestartGuard`, `PoolJob`, `WorkerExit`, `WorkerHandle`, and `MechanicsPoolShared` fields private and introducing narrow methods (`job_sender`, `mark_closed`, restart snapshots/recording, worker shutdown/join helpers, etc.); pool logic/tests now use these accessors instead of direct mutation.
