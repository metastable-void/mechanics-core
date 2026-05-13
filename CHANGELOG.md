# Changelog

All notable changes to this crate are documented in this file.

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-05-13

Changed (breaking): the default endpoint HTTP transport is now
backed by `mechanics-http-client` (hyper-rustls + webpki-roots +
aws-lc-rs) instead of `reqwest`.

- Renamed: `ReqwestEndpointHttpClient` → `DefaultEndpointHttpClient`.
  Constructor signature changed from
  `ReqwestEndpointHttpClient::new(reqwest::Client)` to
  `DefaultEndpointHttpClient::new(mechanics_http_client::Client)`.
- Removed: the `reqwest` dependency. Callers that injected a
  custom `reqwest::Client` via `MechanicsPoolConfig.endpoint_http_client`
  must switch to `mechanics_http_client::Client` (or implement the
  `EndpointHttpClient` trait directly).
- Trust posture: the default transport's TLS trust store is the
  bundled Mozilla CA bundle (`webpki-roots`) only — no OS-native
  trust, no `rustls-platform-verifier`. Crypto provider is
  `aws-lc-rs`; `ring` is no longer in the dep graph.

## [0.4.1] - 2026-05-12

- Changed: the run-job response now returns when the script's
  top-level settles. Unawaited promises, endpoint calls, and
  `setTimeout` callbacks continue polling on the worker thread
  until quiescence or `max_execution_time`. Previously the
  response was held open until every queued promise / timer /
  async job drained to quiescence, so the script's `return`
  was not the response fence.
- Added: `setTimeout(callback, delayMs)` is now exposed as a
  global builtin inside the script realm.
- Tail-poll abort path emits one `tracing::warn!` line per
  job that hit the deadline before quiescence, naming the
  job ID, in-flight async-job count, and queued
  promise/timeout/generic counts at abort time.

## [0.4.0] - 2026-05-11

Changed: when the default-export `main` returns a fulfilled promise, the
runtime no longer overrides that success with an "Unhandled promise
rejection" engine error. Boa's spec-compliant
`promise_rejection_tracker` does not reliably balance `Reject`/`Handle`
events across the inner-promise / outer-await-wrapper chain that
`NativeFunction::from_async_fn` creates, so the previous strict check
produced false-positive step failures for workflows that correctly
caught endpoint errors with `try { await endpoint(...) } catch (e) {
... }`. The module-evaluation-time unhandled-rejection check (run
before `main` is called) stays strict — top-level async work in user
scripts is rare and module-load failures are a different class of
problem. A genuinely-abandoned inner rejection (e.g.
`Promise.resolve().then(() => { throw })` with no catch anywhere)
no longer fails the step; the script is responsible for handling its
own promises.

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
