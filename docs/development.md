# mechanics-core development workflow

## Setup
Prerequisites:
- Rust toolchain that supports edition `2024`.

Initial verification from repository root:

```bash
cargo check
cargo test --all-targets
cargo clippy --all-targets --all-features
```

Optional test suite:

```bash
cargo test --all-targets -- --ignored
```

Ignored tests currently cover network-bound endpoint behavior and require local socket bind permission (`src/pool/tests/endpoint_network.rs`).

## Common local loop
1. Make a small change.
2. Run `cargo check`.
3. Run targeted tests (for example `cargo test pool::tests::queue`), then `cargo test --all-targets`.
4. Run `cargo clippy --all-targets --all-features`.
5. Update docs and/or declarations when public behavior changes.

Recommended stricter periodic lint pass for runtime safety:
```bash
cargo clippy --lib --all-features -- -D warnings -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic -W clippy::todo -W clippy::unimplemented -W clippy::dbg_macro
```

Maximal periodic lint profile (excluding tests):
```bash
cargo clippy --workspace --all-features --lib --bins --examples -- -D warnings -W clippy::all -W clippy::pedantic -W clippy::nursery -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic -W clippy::todo -W clippy::unimplemented -W clippy::dbg_macro
```

Current status (2026-03-18):
- This maximal profile is feasible to run and should be run periodically.
- It is currently non-gating and fails with many pedantic/nursery findings in non-test code, so use it as an audit signal rather than a required CI gate.
- Keep `cargo clippy --all-targets --all-features -- -D warnings` as the blocking baseline.

## Running the example
The example binary at `examples/test-script.rs` executes a JS module with JSON config:

```bash
cargo run --example test-script -- <json_config_path> <js_path>
```

Notes:
- `<json_config_path>` must deserialize into `MechanicsConfig`.
- `<js_path>` must export a callable default function.

## Change checklist for contributors
Audit documentation routine (`docs/audit-findings.md`):
- Keep active/open findings first.
- Keep resolved findings summarized concisely at the bottom under `Done summary`.
- When a finding is resolved, move it to `Done summary` in the same change.

When changing runtime-facing behavior:
- Keep `docs/behavior.md` aligned with actual behavior.
- Keep `ts-types/*.d.ts` and `ts-types/README.md` policy expectations aligned for synthetic modules.
- Keep examples in docs valid against current API names (`run_try_enqueue`, `stats`, `MechanicsConfig::new`, endpoint body-type fields, timeout/response-limit fields, and `retry_policy`).
- Preserve JSON-first API guarantees: serde JSON parseability of runtime-facing config/job types is a feature and must remain first-class.
- If adding Rust-side constructors/builders or tightening visibility, keep serde and constructor validation behavior aligned and avoid breaking JSON ingestion flows.
- Keep the boundary explicit: endpoint transport injection (`MechanicsPoolConfig.endpoint_http_client`) is a Rust-side pool config concern, not a JSON job config field.
- If upgrading `boa_engine`, update `src/executor.rs` job routing and keep `job_routing_harness_covers_all_current_boa_job_variants` passing with explicit coverage for any newly constructible job variants.

Runtime builtins layout:
- Synthetic runtime modules are defined under `src/runtime/builtins/*.rs`.
- Register all builtins from `src/runtime/builtins/mod.rs` via `bundle_builtin_modules(...)`.
- Keep `src/runtime/synthetic_modules.rs` as a thin adapter that only calls the bundle helper.
- When adding a new builtin module, add a focused file in `src/runtime/builtins/`, expose a `register(...)` function there, and wire it into `bundle_builtin_modules(...)`.

Synchronization primitives policy:
- Use `parking_lot` exclusively for `Mutex` and `RwLock` in production code.
- Do not introduce `std::sync::Mutex` or `std::sync::RwLock` in non-test paths.

When changing config validation or endpoint behavior:
- Add or update tests under `src/http/tests/` and `src/pool/tests/`.
- Re-check ignored endpoint network tests if the behavior involves HTTP timeout/status/size handling.
