# Changelog

All notable changes to this crate are documented in this file.

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.2]

Added an optional `MechanicsJob::run_timeout` override for the Rust-side pool wait deadline, including a `with_run_timeout` builder and accessor. The serde field is optional with a default of `None`, so existing serialized jobs without `run_timeout` remain valid, and the public `MechanicsJob::new` signature is unchanged.

## [0.3.1]

- Added doc comments.

## [0.3.0]

Originally prepared as `0.2.3`. Re-cut as `0.3.0` after
`cargo-semver-checks` correctly flagged the type-identity change
(schema types now defined in `mechanics-config`, re-exported here)
as a breaking change under cargo's pre-1.0 semver rules. Call-site
usage is preserved by the re-exports, but the defining crate
moved, so the bump opts downstreams in explicitly rather than
arriving under a caret-range silent upgrade.

- Extracted schema/config types into `mechanics-config` and now depend on
  `mechanics-config = "0.1.0"`.
- Added Boa GC wrapper newtypes over extracted schema types via
  `#[unsafe_ignore_trace]`.
- Preserved path-level compatibility by re-exporting endpoint/config types
  at `mechanics_core::endpoint::*` and `mechanics_core::job::MechanicsConfig`.
  Call-site code using those import paths compiles unchanged; only the
  type-identity-sensitive minority (patterns touching `std::any::TypeId`
  of these types, or depending on the defining-crate identity for some
  other reason) sees a difference.
- **Behavior change:** schema validation now fails at config-construction
  time instead of at job call time. Callers that previously constructed
  intentionally-invalid `MechanicsConfig` or `HttpEndpoint` values and
  relied on errors surfacing lazily will now see those errors at the
  construction site. Intentional; matches the design.

## [0.2.2]

Current published baseline. Git history is the authoritative
record for this and earlier releases; future releases will be
documented going forward in this file.
