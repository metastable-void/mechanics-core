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

## Running the example
The example binary at `examples/test-script.rs` executes a JS module with JSON config:

```bash
cargo run --example test-script -- <json_config_path> <js_path>
```

Notes:
- `<json_config_path>` must deserialize into `MechanicsConfig`.
- `<js_path>` must export a callable default function.

## Change checklist for contributors
When changing runtime-facing behavior:
- Keep `docs/behavior.md` aligned with actual behavior.
- Keep `ts-types/*.d.ts` and `ts-types/README.md` policy expectations aligned for synthetic modules.
- Keep examples in docs valid against current API names (`run_try_enqueue`, `MechanicsConfig::new`, endpoint body-type fields, and timeout/response-limit fields).

When changing config validation or endpoint behavior:
- Add or update tests under `src/http/tests/` and `src/pool/tests/`.
- Re-check ignored endpoint network tests if the behavior involves HTTP timeout/status/size handling.
