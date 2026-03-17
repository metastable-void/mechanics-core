# mechanics-core

Core runtime for executing JavaScript automation modules with Boa in a worker pool.

Stateless by design: jobs should be self-contained, and correctness must not depend on
in-process caching or sticky worker affinity. This supports horizontal scaling.

## What this crate provides
- Bounded worker pool (`MechanicsPool`) for job execution.
- Per-job execution limits (`MechanicsExecutionLimits`).
- Synthetic JS modules:
- `mechanics:endpoint` for preconfigured HTTP calls (`GET`/`POST`/`PUT`/`DELETE`).
- `mechanics:form-urlencoded`, `mechanics:base64`, `mechanics:hex`, `mechanics:base32`, `mechanics:rand`.
- Structured error model (`MechanicsError`, marked `#[non_exhaustive]`).

Public API re-exports are in `src/lib.rs`.

## Quick usage
See:
- [docs/behavior.md](docs/behavior.md) for runtime behavior, API semantics, assumptions, and limitations.
- [ts-types/](ts-types/) for bundled `.d.ts` declarations of synthetic runtime modules.
- [examples/test-script.rs](examples/test-script.rs) for CLI-style invocation.

## Current audit report
A full inconsistency/redundancy report is maintained at:
- [docs/audit-findings.md](docs/audit-findings.md)
