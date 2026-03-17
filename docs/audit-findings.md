# mechanics-core audit findings (2026-03-18)

## Scope and verification
Audit pass covered:
- Runtime code paths in `src/pool.rs`, `src/runtime.rs`, `src/http.rs`, `src/executor.rs`.
- Synthetic modules and JS boundary handling in `src/runtime/synthetic_modules.rs`.
- Public docs/declarations consistency (`README.md`, `docs/behavior.md`, `ts-types/*.d.ts`).

Validation run:
- `cargo clippy --all-targets --all-features` (pass).
- `cargo test --all-targets` (pass: 42 passed, 0 failed, 17 ignored).
- `cargo test --all-targets -- --ignored` (pass: 17 passed, 0 failed; run outside sandbox restrictions).

## Current findings

### 1) Unbounded HTTP response buffering can cause memory blow-up
- Severity: medium
- Status: open
- Evidence: [`src/http.rs:503`](/home/menhera/projects/mechanics-core/src/http.rs:503) uses `res.bytes().await`, which buffers the full response body in memory before decoding.
- Impact:
- A misconfigured or hostile endpoint can return very large payloads and force high memory usage/OOM in worker threads.
- Suggested fix:
- Add configurable response size limits (pool-level default + per-endpoint override), enforce while streaming (`bytes_stream`) before full materialization.

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
