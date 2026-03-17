# mechanics-core audit findings (2026-03-18)

## Scope and verification
Audit pass covered:
- Runtime code paths in `src/pool.rs`, `src/runtime.rs`, `src/http.rs`, `src/executor.rs`.
- Synthetic modules and JS boundary handling in `src/runtime/synthetic_modules.rs`.
- Public docs/declarations consistency (`README.md`, `docs/behavior.md`, `ts-types/*.d.ts`).

Validation run:
- `cargo clippy --all-targets --all-features` (pass).
- `cargo test --all-targets` (pass: 55 passed, 0 failed, 20 ignored).
- `cargo test --all-targets -- --ignored` (pass: 20 passed, 0 failed; run outside sandbox restrictions).

## Current findings

### 1) Unbounded HTTP response buffering can cause memory blow-up
- Severity: medium
- Status: done (2026-03-18)
- Resolution:
- Added pool-level default response cap `MechanicsPoolConfig.default_http_response_max_bytes` (default: `8 MiB`).
- Added per-endpoint override `HttpEndpoint::with_response_max_bytes(...)` / `response_max_bytes` config field.
- Switched response read path to chunked accumulation with limit enforcement, plus early `Content-Length` guard.
- Verification:
- Unit tests: `src/http/tests/response_limit.rs`
- Integration tests: `src/pool/tests/endpoint_network.rs` (`endpoint_uses_pool_default_response_max_bytes`, `endpoint_response_max_bytes_overrides_pool_default`)

### 2) `mechanics:form-urlencoded.encode` output order is non-deterministic
- Severity: low
- Status: done (2026-03-18)
- Resolution:
- Switched form-urlencode record handling to ordered-map semantics (`BTreeMap`) in synthetic module encode/decode paths.
- `encode` now emits key-value pairs in lexical key order deterministically.
- Verification:
- Regression test: `src/pool/tests/synthetic_modules.rs` (`form_urlencoded_module_encode_is_key_ordered`)

### 3) Endpoint configuration validation is fail-late (first call), not fail-fast
- Severity: low
- Status: done (2026-03-18)
- Resolution:
- Added endpoint static validator (`HttpEndpoint::validate_config`) covering URL template/spec consistency and query/default bounds checks.
- `MechanicsConfig::new(...)` is now fallible and validates all endpoints before returning.
- `MechanicsConfig` deserialization is now validation-backed (invalid configs fail during deserialization).
- Validation is intentionally non-caching and does not introduce cross-job state.
- Verification:
- New tests in `src/http/tests/serde_config.rs`:
- `mechanics_config_new_rejects_invalid_endpoint_configuration`
- `mechanics_config_deserialize_rejects_invalid_endpoint_configuration`

### 4) Fail-fast validator over-rejected empty defaults for `optional` slotted queries
- Severity: medium
- Status: done (2026-03-18)
- Regression introduced by:
- Earlier fail-fast config validation pass.
- Issue:
- Empty defaults for `SlottedQueryMode::Optional` / `Required` are treated as absent at runtime, but were being rejected by config validation when `min_bytes` was set.
- Resolution:
- Validator now mirrors runtime semantics:
- For `required` / `optional`, empty `default` is treated as absent and is not byte-length validated.
- For `required_allow_empty` / `optional_allow_empty`, `default` is always validated.
- Verification:
- New tests in `src/http/tests/serde_config.rs`:
- `mechanics_config_allows_empty_default_for_optional_query_with_min_bytes`
- `mechanics_config_rejects_empty_default_for_optional_allow_empty_with_min_bytes`

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
