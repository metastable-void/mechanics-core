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
  Transport execution runs on an internal Tokio runtime.
- Per-job execution limits (`MechanicsExecutionLimits`).
- Synthetic JS modules:
  `mechanics:endpoint` for preconfigured HTTP calls (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`/`HEAD`/`OPTIONS`),
  `mechanics:form-urlencoded`, `mechanics:base64`, `mechanics:hex`, `mechanics:base32`, `mechanics:rand`, and `mechanics:uuid`.
- Endpoint config supports JSON-deserializable resilience policy (`retry_policy`) for retries/backoff/rate-limit handling.
- Structured error model (`MechanicsError`, marked `#[non_exhaustive]`) with stable symbolic kind enum (`MechanicsErrorKind`, `#[repr(u8)]`).

Public API is organized by module in `src/lib.rs`:
- root: `MechanicsPool`, `MechanicsPoolConfig`, `MechanicsPoolStats`, `MechanicsError`, `MechanicsErrorKind`
- `mechanics_core::job`: `MechanicsJob`, `MechanicsExecutionLimits`, `MechanicsConfig`
- `mechanics_core::endpoint`: `HttpEndpoint`, `HttpMethod`, `EndpointBodyType`, `EndpointRetryPolicy`, `UrlParamSpec`, `QuerySpec`, `SlottedQueryMode`
- `mechanics_core::endpoint::http_client`: `EndpointHttpClient`, `ReqwestEndpointHttpClient`, `EndpointHttpRequest`, `EndpointHttpRequestBody`, `EndpointHttpResponse`, `EndpointHttpHeaders`

## Scopes
This crate is intended to be integrated into systems as automation/orchestration layers.
While using JavaScript, this crate itself is not a Web thing.

### In scope
- Execute user-provided JavaScript modules safely inside isolated Boa realms.
- Provide a synchronous Rust API (`MechanicsPool`) for bounded worker-pool execution.
- Keep runtime behavior stateless across jobs; no cross-job mutable runtime carryover.
- Offer JSON-first runtime/job configuration (`serde` parseability is a first-class feature).
- Provide preconfigured outbound HTTP endpoint execution via `mechanics:endpoint`, with URL/query/header policy enforcement from Rust config.
- Support request/response body modes with explicit size/time limits.
- Support endpoint-level retry/backoff/rate-limit policy.
- Expose a minimal set of built-in utility modules useful for automation/orchestration scripts (`form-urlencoded`, `base64`, `hex`, `base32`, `rand`, `uuid`).
- Support pluggable pool-level HTTP transport for deterministic/mock testing.

### Out of scope (for now)
- Full async-first Rust API surface (Tokio-agnostic async abstractions).
- General-purpose workflow/orchestration DSL beyond JavaScript execution itself.
- Distributed scheduling, persistence, and cross-process job coordination.
- Cross-job in-process cache guarantees or sticky-worker semantics.
- Turnkey observability backend integrations (metrics/traces exporters).
- HTTP API hosting concerns (service routing, auth middleware, request admission).

### Planned integration architecture
- This crate is planned to be embedded behind a separate Rust HTTP server that exposes automation-as-a-service endpoints.
- Bearer-token authentication/authorization is planned to be enforced in that server layer, not inside this crate.
- Metrics exporters and service-level telemetry pipelines are also planned for that server layer, not in `mechanics-core`.

### Maturity path toward crates.io
- Pre-publication: favor correctness, safety, and API clarity; breaking Rust API changes are allowed.
- Stabilization phase (before crates.io release): tighten compatibility guarantees, freeze core config/job wire contracts, and provide migration notes for any remaining breaking changes.

## API design constraint
- JSON-first is a core API constraint.
- `MechanicsJob`, `MechanicsConfig`, `HttpEndpoint`, and related runtime-facing config types are intended to be first-class `serde_json` inputs.
- Unknown JSON fields are rejected for these runtime-facing payload types.
- Rust-side builder/constructor helpers should complement JSON ingestion, not replace it.
- When tightening encapsulation/visibility, preserve non-breaking JSON parseability and keep validation behavior aligned between serde and Rust-native construction paths.
- This repository is public and is intended for crates.io publication once implementation matures.
- Until publication/stabilization, compatibility-breaking Rust API changes are acceptable when they improve API safety/clarity.

## Contributor quick start
Prerequisites:
- Rust toolchain with support for edition `2024` (`rustup default stable` is usually sufficient).

From repo root:
```bash
cargo check
cargo test --all-targets
cargo clippy --all-targets --all-features
```

Contract consistency gate (recommended before release):
```bash
./scripts/check-contract-consistency.sh
```
Keep this script updated periodically as API names, module paths, and contract checks evolve.

Optional (environment-dependent) tests:
```bash
cargo test --all-targets -- --ignored
```
Ignored tests in `src/internal/pool/tests/endpoint_network.rs` require local socket bind permission.

## Quick usage
Run the example runner:
```bash
cargo run --example test-script -- <json_config_path> <js_path>
```

Minimal Rust imports:
```rust
use mechanics_core::{MechanicsPool, MechanicsPoolConfig};
use mechanics_core::job::{MechanicsConfig, MechanicsJob};
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
- In project docs, record file paths as project-relative paths (for example, `src/internal/http/mod.rs`).
- Do not record arbitrary absolute filesystem paths.

## Documentation map
Canonical project documentation is maintained in this GitHub repository.

- [docs/behavior.md](docs/behavior.md): runtime behavior and API semantics.
- [docs/development.md](docs/development.md): contributor workflow, checks, and change checklist.
- [docs/audit-findings.md](docs/audit-findings.md): latest audit and resolved findings.
- [ts-types/README.md](ts-types/README.md): TypeScript declaration maintenance policy.
- `ts-types/mechanics-json-shapes.d.ts`: TypeScript interfaces for serde JSON payloads (`MechanicsJob`, `MechanicsConfig`, endpoint config).
- `json-schema/*.schema.json`: JSON Schemas for canonical job/config payload shapes.
