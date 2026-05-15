# Changelog

All notable changes to this crate are documented in this file.

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- `mechanics:endpoint` now prefixes JS-visible transport
  errors with the endpoint name. Previously a job that called
  `await endpoint("llm", {...})` and timed out surfaced as
  `Error: request timed out` with no indication of which
  configured endpoint failed — operators reading the chat-
  worker logs had to correlate timing to a `tcpdump` or a
  connector-side trace to attribute the failure to a specific
  endpoint (`embed`, `vector_search`, `llm`, `db`, etc.). The
  new prefix renders as
  ``Error: endpoint `llm` request failed: request timed out``
  in the JS realm. Pure diagnostic — no transport-layer
  behaviour change. Covered by
  `endpoint_transport_errors_include_endpoint_name`.

### Fixed
- The default `EndpointHttpClient` impl in
  `internal/http/transport.rs` now wraps the entire request
  operation (build → send → status/content-length check →
  body read) in an outer `tokio::time::timeout` when the
  endpoint configures `timeout_ms`, in addition to the
  inner reqwest-equivalent per-request timeout. mhc and any
  future endpoint client should honour the inner timer, but
  a path that doesn't (e.g. an h3 stage that doesn't propagate
  the per-request timeout to a backgrounded driver task) was
  letting `endpoint("llm")` POSTs sit past the configured
  endpoint timeout and stop only at the outer mechanics
  300 s envelope. Belt-and-braces: the outer timer is the
  same `timeout_ms` value, so a well-behaved client path
  sees no observable change, while a misbehaving client
  surfaces as ``Error: endpoint `<name>` request failed:
  request timed out`` at the configured budget.

## [0.6.1] - 2026-05-14

### Changed
- Added `mime` to the default-features set. The workspace's
  pre-landing convention is "default = every feature some
  consumer actually exercises" so `cargo check --workspace`
  compiles every gated module path. Without this, the `mime`
  module code was never reached by the workspace's default-
  features check, and a regression in `mechanics:mime` would
  silently slip through the same gap that hid the mechanics
  0.5.2 `handle_h3_request` mismatch. Embedders using
  `default-features = false` are unaffected; embedders using
  default features now get `mime` automatically with no new
  transitive dependencies (`data-encoding` was already
  default-on via `encoding`).

## [0.6.0] - 2026-05-14

### Added
- Added the default-on `[features]` surface:
  `default = ["rand", "encoding", "html", "console", "url"]`.
  Consumers using default features retain the previous built-in
  module behaviour; consumers can now opt out with
  `default-features = false` and re-enable individual module
  families explicitly.
- Added `mechanics:console` behind the `console` feature. The
  module default-exports a `console` object with no-op `log`,
  `info`, `warn`, `error`, and `debug` methods. The methods do
  no I/O and emit no tracing; future capture into
  `RunJobResponse` is out of scope for this release.
- Added `mechanics:html` behind the `html` feature. The module
  wraps `htmlize` as named exports `escapeText`,
  `escapeAttribute`, `unescapeText`, and `unescapeAttribute`.
- Added `mechanics:url` behind the `url` feature. The module
  default-exports a WHATWG-style `URL` class and named-exports
  `URLSearchParams`, backed by the existing `url` crate.
- Added `mechanics:mime` behind the non-default `mime` feature.
  The module named-exports pure no-I/O `compose` and `parse`
  functions for structured MIME message objects. It emits CRLF
  line endings, auto-adds `MIME-Version: 1.0`, generates
  multipart boundaries, encodes non-ASCII headers as RFC 2047
  UTF-8 encoded-words, and handles `7bit`, `8bit`, `binary`,
  `quoted-printable`, and `base64` transfer encodings. The
  implementation is format-only, installs no globals, uses no
  per-job shared state, and is backed by in-module MIME logic
  plus the existing `data-encoding` crate for Base64.

### Changed
- `mechanics:rand` and `mechanics:uuid` are now gated by the
  default-on `rand` feature.
- `mechanics:form-urlencoded`, `mechanics:base64`,
  `mechanics:base32`, and `mechanics:hex` are now gated by the
  default-on `encoding` feature.

## [0.5.1] - 2026-05-14

### Changed
- Internal Cargo.toml audit: `default-features = false` set on
  direct dependencies with explicit feature lists for what the
  crate actually uses. No behaviour change. (D24)

### Removed (breaking for JS workloads)
- The `setTimeout(callback, delay_ms)` realm global is gone.
  It was a Web-Platform shim that violated the workspace's
  "no non-ES globals" hard rule (see
  `docs/design/06-execution-substrate.md` §"Realm surface
  (no non-ES globals)"). JS workloads should use Promise-based
  patterns instead — `Promise.resolve().then(...)` for
  microtask deferral, `(async () => { ... })()` for unawaited
  async work, endpoint promises with `.then(...)` for async
  side effects scheduled from sync `main`. The tail-promise
  polling behaviour (D17) remains unchanged for Promise-driven
  in-flight work; only the JS-visible timer-binding is
  removed. The internal `set_timeout` Rust fn,
  `install_timer_builtins` Rust fn, and both call sites in
  `internal::runtime` were deleted; the runtime no longer
  registers `setTimeout` on the default realm nor on per-job
  isolated realms.

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
