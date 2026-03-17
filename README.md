# mechanics-core

Core runtime for executing JavaScript automation modules with Boa in a worker pool.

## What this crate provides
- Bounded worker pool (`MechanicsPool`) for job execution.
- Per-job execution limits (`MechanicsExecutionLimits`).
- Synthetic JS module `mechanics:endpoint` for HTTP JSON POST.
- Structured error model (`MechanicsError`).

Public API re-exports are in `src/lib.rs`.

## Quick usage
See:
- [docs/behavior.md](docs/behavior.md) for runtime behavior, API semantics, assumptions, and limitations.
- [examples/test-script.rs](examples/test-script.rs) for CLI-style invocation.

## Current audit report
A full inconsistency/redundancy report is maintained at:
- [docs/audit-findings.md](docs/audit-findings.md)
