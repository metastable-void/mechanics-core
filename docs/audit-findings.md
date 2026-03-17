# mechanics-core audit findings (2026-03-18)

## Scope and verification
Audit pass covered:
- Runtime code paths in `src/pool.rs`, `src/runtime.rs`, `src/http.rs`, `src/executor.rs`.
- Synthetic modules and JS boundary handling in `src/runtime/synthetic_modules.rs`.
- Public docs/declarations consistency (`README.md`, `docs/behavior.md`, `ts-types/*.d.ts`).

Validation run:
- `cargo clippy --all-targets --all-features` (pass).
- `cargo test --all-targets` (pass: 45 passed, 0 failed, 19 ignored).
- `cargo test --all-targets -- --ignored` (pass: 19 passed, 0 failed; run outside sandbox restrictions).

## Current findings

### 1) Unbounded HTTP response buffering can cause memory blow-up
- Severity: medium
- Status: done (2026-03-18)
- Resolution:
- Added pool-level default response cap `MechanicsPoolConfig.default_http_response_max_bytes` (default: `8 MiB`).
- Added per-endpoint override `HttpEndpoint::with_response_max_bytes(...)` / `response_max_bytes` config field.
- Switched response read path to chunked accumulation with limit enforcement, plus early `Content-Length` guard.
- Verification:
- Unit tests: [`src/http/tests/response_limit.rs`](/home/menhera/projects/mechanics-core/src/http/tests/response_limit.rs)
- Integration tests: [`src/pool/tests/endpoint_network.rs`](/home/menhera/projects/mechanics-core/src/pool/tests/endpoint_network.rs) (`endpoint_uses_pool_default_response_max_bytes`, `endpoint_response_max_bytes_overrides_pool_default`)

### 2) `mechanics:form-urlencoded.encode` output order is non-deterministic
- Severity: low
- Status: open
- Evidence:
- Parsed record uses `HashMap<String, String>`: [`src/runtime/synthetic_modules.rs:120`](/home/menhera/projects/mechanics-core/src/runtime/synthetic_modules.rs:120).
- Encoding iterates that `HashMap` directly: [`src/runtime/synthetic_modules.rs:140`](/home/menhera/projects/mechanics-core/src/runtime/synthetic_modules.rs:140).
- Impact:
- Query/body canonicalization that depends on stable key order (for signatures or snapshots) may be flaky across runs/processes.
- Suggested fix:
- Sort keys before encoding or switch to ordered map semantics for this module API.

### 3) Endpoint configuration validation is fail-late (first call), not fail-fast
- Severity: low
- Status: open
- Evidence:
- URL template/spec consistency checks happen during each execute path in `build_url`: [`src/http.rs:290`](/home/menhera/projects/mechanics-core/src/http.rs:290).
- `MechanicsConfig::new` currently just stores config with no upfront validation: [`src/http.rs:783`](/home/menhera/projects/mechanics-core/src/http.rs:783).
- Impact:
- Invalid endpoint configs are discovered only when invoked at runtime, not at configuration load time.
- Suggested fix:
- Add explicit config validator (or validate in `MechanicsConfig::new`) and optionally cache parsed URL templates/specs.

## Historical tracked items (resolved)
- `1` Pending Promise result handling: done.
- `2` Unhandled async error propagation: done.
- `3` `try_run` naming mismatch: done (`run_try_enqueue`).
- `4` Heuristic reply-timeout model: done (`run_timeout` + `RunTimeout`).
- `5` HTTP non-success status validation: done (strict by default, explicit opt-in).
- `6` Invalid configured header handling: done (strict validation, explicit error).
- `7` Runtime panic/unwrap paths: done (converted to structured failures).
- `8` Worker startup lock scope: done (rendezvous send moved outside map lock scope).
- `9` `parking_lot` synchronization migration: done.
- `10` Tokio feature breadth: done (`default-features = false`, minimal features).
