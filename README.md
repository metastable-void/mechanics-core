# mechanics-core

Core runtime for executing JavaScript automation modules with Boa in a worker pool.

Stateless by design: jobs should be self-contained, and correctness must not depend on
in-process caching or sticky worker affinity. This supports horizontal scaling.
Each job executes in an isolated JavaScript Realm, so `globalThis` mutations do not carry
across jobs.

## What this crate provides
- Bounded worker pool (`MechanicsPool`) for job execution.
- Synchronous non-blocking pool state snapshots (`MechanicsPool::stats`, `MechanicsPoolStats`).
- Swappable pool-level endpoint HTTP transport via `MechanicsPoolConfig.endpoint_http_client` (`reqwest` wrapper by default), useful for deterministic/mock testing.
- Per-job execution limits (`MechanicsExecutionLimits`).
- Synthetic JS modules:
  `mechanics:endpoint` for preconfigured HTTP calls (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`/`HEAD`/`OPTIONS`),
  `mechanics:form-urlencoded`, `mechanics:base64`, `mechanics:hex`, `mechanics:base32`, `mechanics:rand`, and `mechanics:uuid`.
- Endpoint config supports JSON-deserializable resilience policy (`retry_policy`) for retries/backoff/rate-limit handling.
- Structured error model (`MechanicsError`, marked `#[non_exhaustive]`).

Public API re-exports are in `src/lib.rs`.

## Contributor quick start
Prerequisites:
- Rust toolchain with support for edition `2024` (`rustup default stable` is usually sufficient).

From repo root:
```bash
cargo check
cargo test --all-targets
cargo clippy --all-targets --all-features
```

Optional (environment-dependent) tests:
```bash
cargo test --all-targets -- --ignored
```
Ignored tests in `src/pool/tests/endpoint_network.rs` require local socket bind permission.

## Quick usage
Run the example runner:
```bash
cargo run --example test-script -- <json_config_path> <js_path>
```

Minimal files:

`config.json`
```json
{
  "endpoints": {}
}
```

`main.js`
```js
export default function main(arg) {
  return { ok: true, got: arg };
}
```

## Documentation path policy
- In project docs, record file paths as project-relative paths (for example, `src/http.rs`).
- Do not record arbitrary absolute filesystem paths.

## Documentation map
- [docs/behavior.md](docs/behavior.md): runtime behavior and API semantics.
- [docs/development.md](docs/development.md): contributor workflow, checks, and change checklist.
- [docs/audit-findings.md](docs/audit-findings.md): latest audit and resolved findings.
- [ts-types/README.md](ts-types/README.md): TypeScript declaration maintenance policy.
