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

### Changed (breaking, pre-publish)
- `RuntimeInternal::run_source_with_early_reply` now returns the
  typed enum `RunSourceOutcome` (`MainReplied` |
  `MainNotReplied(MechanicsError)`) instead of `Result<(),
  MechanicsError>`. The enum makes explicit whether the
  `early_reply` closure was invoked — the worker side no longer
  needs an `Arc<AtomicBool>` to track in-band whether to send the
  error on the reply channel. Healthy paths and contract-bug
  paths are now distinguishable at the type level. Affects only
  internal `pub(crate)` API; not exposed to crate consumers.
- `EndpointHttpClient::execute` now returns
  `EndpointTransportResult<EndpointHttpResponse>` (alias for
  `Result<_, EndpointTransportError>`) instead of
  `std::io::Result<EndpointHttpResponse>`. The new typed error
  enum encodes the retryability class at the type level
  (`Network` and `Timeout` are retryable per
  `EndpointRetryPolicy`; `BodyTooLarge`, `InvalidRequest`,
  `Decode`, `Other` are terminal). Downstream impls of the trait
  must migrate; the in-tree `DefaultEndpointHttpClient` and test
  doubles are updated. Surfaces re-export
  `EndpointTransportError` / `EndpointTransportResult` through
  `crate::endpoint::http_client`.

### Fixed
- `MechanicsPool::new` no longer leaks already-spawned workers
  when a later step fails mid-construction. Previously a worker
  spawn failure (e.g. `force_worker_runtime_init_failure=true`
  on worker N>1) returned `Err` from `?`-propagation with the
  earlier workers still alive — `MechanicsPool::drop` could
  not run because no `MechanicsPool` ever existed. A new
  `PoolConstructor` RAII guard owns the cleanup contract:
  it mirrors `MechanicsPool::drop` (mark closed, drain
  pending jobs, request worker shutdown, join supervisor,
  join worker handles) and runs on partial-construction
  failure. The success path commits the guard (no-op Drop),
  transferring ownership to the new `MechanicsPool`.
- `Queue::enqueue_job` no longer silently drops a `TimeoutJob`
  whose delay overflows `JsInstant`. Previously the overflow
  clamped to the `u64::MAX` sentinel and inserted the job at
  an unreachable position in the BTreeMap, retaining the
  closure indefinitely (until job teardown). The overflow path
  now routes the failure through the runtime as a catchable
  `RangeError("setTimeout delay is too large for the current
  platform clock")`. JS-facing `setTimeout` is not exposed
  in this runtime (removed in 0.5.x); the fix targets the
  `Job::TimeoutJob` host-contract surface that Boa's internal
  sequencing may still produce.
- Endpoint `timeout_ms` is now an *aggregate* (total wall-clock)
  bound rather than a per-attempt bound. Previously
  `timeout_ms=30000, max_attempts=3` could spend ~90 s before
  returning — each attempt got a fresh full budget, so a slow
  upstream that exhausted the budget three times racked up to
  3× the configured timeout. After the change,
  `execute_endpoint` computes a single deadline at function
  entry and passes `remaining(deadline)` as each attempt's
  `timeout_ms`; retry sleeps are bounded by the same
  `remaining(deadline)`; when the deadline fires the loop
  terminates with `io::ErrorKind::TimedOut` carrying
  `endpoint call timed out across N attempt(s)`. Open-ended
  calls (no `timeout_ms` configured) preserve the previous
  unbounded shape. Pairs naturally with `Boa` runtime's
  `max_execution_time`.
- The endpoint retry loop no longer retries deterministic local
  conditions (body-cap violations, invalid-request shapes,
  decode failures) as if they were transient I/O errors.
  Previously every `Err(io::Error)` from
  `EndpointHttpClient::execute` was fed into
  `EndpointRetryPolicy::should_retry_transport_error`, whose
  `ErrorKind`-based discriminator could not tell apart a
  TCP-level corruption from an operator cap violation —
  body-cap responses retried `max_attempts` times against the
  same upstream, burning budget for no possible recovery.
  The new typed error makes the contract auditable from the
  type itself; covered by
  `mechanics-core/src/internal/http/transport.rs#retry_classification_tests`.
- `run_source_with_early_reply` no longer silently drops tail-side
  errors when the main result has already been delivered. A
  `Promise.resolve().then(() => { throw 'x' })` that fires after
  the main reply now emits a structured
  `tail promise produced an error after main resolved` warn log
  carrying the job ID and the JS error; the caller-side reply
  behaviour is unchanged (main result still wins).
- The default `EndpointHttpClient` now gives every
  `mechanics:endpoint` execution a fresh TCP/TLS hyper transport
  while preserving the shared `mechanics-http-client` HTTP/3
  discovery and negative-cache state. This prevents a stalled or
  cancelled endpoint call from poisoning the long-lived mechanics
  worker's TCP connection pool and causing later jobs from new
  workflow/chat instances to stall before any loopback packet is
  emitted.
- The default `EndpointHttpClient` impl in
  `internal/http/transport.rs` now tracks `timeout_ms` as an
  absolute per-request deadline instead of wrapping the whole
  endpoint operation in an outer timeout. The request/header
  phase receives the remaining budget through
  `mechanics-http-client`, and the response-body phase receives
  the remaining budget through an explicit body-read timeout.
  This keeps endpoint timeouts covering both phases while
  avoiding an outer future drop at an arbitrary transport await
  point.
- The runtime job executor now preserves already-started native
  async jobs across the early-reply boundary used by D17 tail
  promise polling. Previously the main-promise wait and the
  tail-poll wait owned separate in-flight future sets, so a
  sibling endpoint promise could be cancelled when the main
  promise settled even though tail polling was meant to keep
  driving it. Covered by the tightened
  `d17_fire_and_forget_endpoint_replies_before_tail_completes`
  regression.

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
