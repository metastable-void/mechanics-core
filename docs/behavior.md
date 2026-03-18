# mechanics-core behavior and usage

## Purpose
`mechanics-core` executes user-provided JavaScript modules inside Boa (`boa_engine`) with:
- per-job execution limits,
- a built-in `mechanics:endpoint` helper for preconfigured HTTP calls,
- a worker pool for concurrent job execution.

The crate API is exported from `src/lib.rs`:
- `MechanicsPool`, `MechanicsPoolConfig`, `MechanicsPoolStats`
- `MechanicsJob`, `MechanicsExecutionLimits`
- `MechanicsConfig`, `HttpEndpoint`, `HttpMethod`, `EndpointBodyType`, `EndpointRetryPolicy`
- `UrlParamSpec`, `QuerySpec`, `SlottedQueryMode`
- `MechanicsError`

## High-level model
1. You build a `MechanicsPool`.
2. You submit a `MechanicsJob` containing:
- module source (`mod_source`),
- JSON argument (`arg`),
- endpoint config (`config`).
3. If a `MechanicsJob` is deserialized from JSON, `mod_source` must be non-empty.
4. `MechanicsConfig` validation is fail-fast:
- `MechanicsConfig::new(...)` validates endpoint configuration before returning.
- `serde` deserialization into `MechanicsConfig` also validates and fails on invalid endpoint config.
5. A worker creates/uses a runtime (`RuntimeInternal`) and executes the module.
6. Each job executes inside a fresh JavaScript Realm within that runtime context.
7. Global mutations (for example `globalThis.foo = ...`) do not persist to later jobs.
8. The module default export is invoked with one argument.
9. Result is converted to JSON and returned.

## JavaScript contract
Your module should export a callable default export.

```js
export default function main(arg) {
  return { ok: true, got: arg };
}
```

At runtime:
- `default` export is resolved and invoked.
- Job code runs in an isolated Realm created per job.
- If return value is not a Promise, it is wrapped in a resolved Promise.
- Job queue drains once after invocation.
- If module evaluation Promise or default export Promise is still pending after queue drain, execution fails.
- Final value is converted to `serde_json::Value`.
- Unhandled async job errors (including unhandled Promise rejections) are treated as fatal for the current job.

If JSON conversion fails, execution fails with `MechanicsError::Execution`.

## Built-in module: `mechanics:endpoint`
Runtime registers a synthetic module named `mechanics:endpoint` with default export `endpoint(name, options)`.

```js
import endpoint from "mechanics:endpoint";

export default async function main(arg) {
  const res = await endpoint("primary", {
    urlParams: { user_id: "u-123" },
    queries: { page: "1", filter: "active" },
    headers: { "x-request-id": "req-1" },
    body: arg
  });
  return res.body;
}
```

Resolution behavior:
- `name` must match a key in `MechanicsConfig.endpoints`.
- Endpoint config controls HTTP method (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`/`HEAD`/`OPTIONS`), URL template, URL slot rules, query emission rules, headers, timeout, and status policy.
- Endpoint config can optionally include resilience policy (`retry_policy`) for retries/backoff/rate-limit handling.
- URL template placeholders (`{slot}`) are resolved from JS `options.urlParams` using configured `url_param_specs`.
- URL template must not contain query string or fragment; use `query_specs` for query output.
- Query string is built algorithmically from configured `query_specs` using JS `options.queries`.
- Unknown JS `queries` keys are rejected unless referenced by configured slotted query specs.
- Request body serialization uses endpoint `request_body_type`.
- Response body parsing uses endpoint `response_body_type`.
- Response body size is bounded by endpoint `response_max_bytes` if set, otherwise by pool default `default_http_response_max_bytes`.
- Empty response bodies are represented as `response.body = null`.
- Endpoint result is always an object: `{ body, headers, status, ok }`.
- `status` is the HTTP status code.
- `ok` is `true` for `2xx` statuses and `false` otherwise.
- `headers` includes only names allowlisted by endpoint `exposed_response_headers` (keys are lowercase).
- If an exposed header has multiple values, they are joined with `", "`.
- If an exposed header value is non-UTF-8, it is represented with lossy UTF-8 decoding.
- Configured headers are validated; invalid names/values fail the call.
- JS `options.headers` can override only names allowlisted by endpoint `overridable_request_headers` (case-insensitive).
- Header precedence is: auto defaults < configured endpoint headers < JS allowlisted overrides.
- If missing, `User-Agent` is injected automatically.
- If request body is present and `Content-Type` is missing, a default content type is injected based on `request_body_type`.
- By default, non-2xx HTTP statuses fail the call.
- `HttpEndpoint::with_allow_non_success_status(true)` opt-in allows non-2xx responses to proceed and be parsed according to `response_body_type` (`json`/`utf8`/`bytes`).
- Retry behavior is driven by endpoint `retry_policy`:
- retries apply to configured status codes and transport errors up to `max_attempts`,
- exponential backoff uses `base_backoff_ms` and `max_backoff_ms`,
- `429` handling can respect `Retry-After` (delta-seconds) when enabled,
- all retry waits are capped by `max_retry_delay_ms`.

`endpoint(name, options)` payload shape (camelCase):
- `urlParams`: object of string slot values for URL template substitution.
- `queries`: object of string slot values used by configured slotted query specs.
- `headers`: object of string request header overrides (must be allowlisted per endpoint).
- `body`: optional payload value.
- accepted request input types depend on endpoint `request_body_type`:
- `json`: any JSON-convertible JS value.
- `utf8`: `string`.
- `bytes`: `TypedArray | ArrayBuffer | DataView` (treated as bytes).
- `body` omission semantics:
- omitted or `undefined` means "no request body".
- explicit `null` is treated as JSON `null` (not omission) and is sent for JSON request mode.
- method/body baseline is aligned to HTTP Semantics (RFC 9110): request bodies are accepted for `POST`/`PUT`/`PATCH`.
- for `GET`/`DELETE`/`HEAD`/`OPTIONS`, any provided `body` value (including explicit `null`) is rejected.
- `SharedArrayBuffer`-backed typed arrays/DataView are not supported.

Config shape is JSON-friendly and snake_case (`serde`):
- endpoint definitions use `method`, `url_template`, `url_param_specs`, and `query_specs`.
- endpoint body directives:
- `request_body_type`: `"json" | "utf8" | "bytes"` (method defaults apply).
- `response_body_type`: `"json" | "utf8" | "bytes"` (default `"json"`).
- `response_max_bytes`: optional max response-body size in bytes (`null` means use pool default).
- `retry_policy`: optional resilience policy (JSON-first/serde-deserializable):
- `max_attempts` (default `1` means no retries),
- `base_backoff_ms`, `max_backoff_ms`, `max_retry_delay_ms`,
- `rate_limit_backoff_ms`, `retry_on_io_errors`, `retry_on_timeout`, `respect_retry_after`,
- `retry_on_status` (default `[429, 500, 502, 503, 504]`).
- `overridable_request_headers`: optional list of request header names that JS may override with `options.headers` (case-insensitive).
- `exposed_response_headers`: optional list of response header names exposed on endpoint result `headers` (case-insensitive).
- `url_template` is a full URL template string and placeholder names must be unique.
- placeholder/slot names are limited to ASCII letters, digits, and `_`.
- `url_param_specs` maps placeholder names to constraints and optional defaults.
- `query_specs` is an ordered list with `type: "const" | "slotted"`.
- slotted query `mode` values:
- `required`: query value must resolve and must be non-empty.
- `required_allow_empty`: query value must resolve and may be empty.
- `optional`: missing/empty is treated as omitted.
- `optional_allow_empty`: missing is omitted; if provided, empty is emitted.
- slotted query resolution precedence:
- use provided `queries[slot]` when present,
- otherwise use `default` when configured,
- then apply mode omission/error behavior.
- for `required` and `optional`, empty `default` is treated as absent.
- for `required_allow_empty` and `optional_allow_empty`, empty `default` remains a concrete value.
- URL param default behavior:
- if `default` exists, missing/empty JS value uses `default`.
- if `default` is absent, missing/empty JS value resolves as empty.

Byte-length validation:
- `min_bytes` / `max_bytes` for URL/query slots are validated against raw UTF-8 byte length.

Configuration validation:
- Endpoint config is validated when building/deserializing `MechanicsConfig`.
- Invalid configs fail fast (for example: malformed URL template, missing/extra `url_param_specs`, invalid slot/query rules, invalid bounds/defaults).
- No cross-job cache is introduced by this validation; each supplied config object is validated independently.
- `MechanicsConfig` composition helpers:
- `with_endpoint(name, endpoint)`: validates and inserts/replaces one endpoint.
- `with_endpoint_overrides(overrides)`: validates and applies multiple endpoint overrides.
- `without_endpoint(name)`: removes one endpoint if present.
- These helpers are for per-job config composition before submission; they do not mutate already-running worker state.

Minimal endpoint config example (JSON):

```json
{
  "endpoints": {
    "primary": {
      "method": "post",
      "url_template": "https://api.example.com/users/{user_id}/messages/{message_id}",
      "url_param_specs": {
        "user_id": {
          "min_bytes": 1,
          "max_bytes": 64
        },
        "message_id": {
          "default": "latest",
          "min_bytes": 1,
          "max_bytes": 64
        }
      },
      "query_specs": [
        { "type": "const", "key": "v", "value": "1" },
        {
          "type": "slotted",
          "key": "page",
          "slot": "page",
          "mode": "optional",
          "min_bytes": 1,
          "max_bytes": 8
        },
        {
          "type": "slotted",
          "key": "filter",
          "slot": "filter",
          "mode": "required_allow_empty",
          "default": "all"
        }
      ],
      "headers": {
        "x-api-key": "redacted"
      },
      "request_body_type": "json",
      "response_body_type": "json",
      "response_max_bytes": 1048576,
      "overridable_request_headers": ["x-request-id"],
      "exposed_response_headers": ["content-type", "x-trace-id"],
      "timeout_ms": 5000,
      "allow_non_success_status": false
    }
  }
}
```

## Built-in module: `mechanics:form-urlencoded`
Runtime registers a synthetic module named `mechanics:form-urlencoded` with:
- `encode(record: Record<string, string>): string`
- `decode(params: string): Record<string, string>`

Notes:
- UTF-8 form-urlencode algorithm (`application/x-www-form-urlencoded`).
- `encode` uses ordered-map semantics (keys are emitted in lexical order) for deterministic output.
- `decode` accepts optional leading `?`.
- duplicate decoded keys use "last one wins" semantics.

## Built-in module: `mechanics:base64`
Runtime registers a synthetic module named `mechanics:base64` with:
- `encode(bufferLike: TypedArray | ArrayBuffer | DataView, variant: "base64" | "base64url" = "base64"): string`
- `decode(encoded: string, variant: "base64" | "base64url" = "base64"): Uint8Array`

Notes:
- `base64url` encoding is emitted without padding.
- decode accepts both padded and unpadded forms.
- `SharedArrayBuffer`-backed typed arrays/DataView are not supported.

## Built-in module: `mechanics:hex`
Runtime registers a synthetic module named `mechanics:hex` with:
- `encode(bufferLike: TypedArray | ArrayBuffer | DataView): string`
- `decode(encoded: string): Uint8Array`

Notes:
- `SharedArrayBuffer`-backed typed arrays/DataView are not supported.

## Built-in module: `mechanics:base32`
Runtime registers a synthetic module named `mechanics:base32` with:
- `encode(bufferLike: TypedArray | ArrayBuffer | DataView, variant: "base32" | "base32hex" = "base32"): string`
- `decode(encoded: string, variant: "base32" | "base32hex" = "base32"): Uint8Array`

Notes:
- decode is case-insensitive for alphabetic input.
- decode accepts both padded and unpadded forms.
- `SharedArrayBuffer`-backed typed arrays/DataView are not supported.

## Built-in module: `mechanics:rand`
Runtime registers a synthetic module named `mechanics:rand` with default export:
- `fillRandom(bufferLike: TypedArray | ArrayBuffer | DataView): void`

Notes:
- `SharedArrayBuffer`-backed typed arrays/DataView are not supported.

## Type declarations
- `ts-types/` contains `.d.ts` declarations for runtime synthetic modules.
- Any public runtime API change must update `ts-types/*.d.ts` in the same change.

Timeout behavior:
- Endpoint timeout = `HttpEndpoint::with_timeout_ms(...)` if set,
- else pool default `MechanicsPoolConfig.default_http_timeout_ms`.

Response-size behavior:
- Endpoint response max bytes = `HttpEndpoint::with_response_max_bytes(...)` if set,
- else pool default `MechanicsPoolConfig.default_http_response_max_bytes` (default: `8 MiB`),
- exceeding the effective limit fails the endpoint call with an execution error.

## Pool and queue behavior
`MechanicsPool::new(config)` creates:
- bounded job queue (`queue_capacity`),
- N worker threads (`worker_count`),
- supervisor thread with restart rate limiter (`restart_window`, `max_restarts_in_window`).
- If any worker fails during startup runtime initialization, construction fails with `MechanicsError::RuntimePool` (no partial usable pool is returned).
- `run_timeout` is validated at construction and rejected if the platform clock cannot represent `Instant::now() + run_timeout`.

### `run(job)`
- Blocks waiting for enqueue up to `enqueue_timeout`.
- Entire API call is bounded by `run_timeout` (from call entry through result wait).
- Returns:
- success JSON value,
- or `MechanicsError` (`RunTimeout`, `QueueTimeout`, `Execution`, etc.).
- `QueueTimeout` means queue admission wait elapsed.
- `RunTimeout` means the overall API-call deadline elapsed (enqueue+reply path).

### `run_try_enqueue(job)`
- Non-blocking enqueue attempt.
- If enqueue succeeds, it still waits for execution result (same bounded reply timeout model as `run`).
- If queue is already full, returns `QueueFull` immediately.

### `stats()`
- Returns a synchronous, non-blocking snapshot (`MechanicsPoolStats`) of pool state.
- Snapshot includes queue depth/capacity, worker counts, desired worker target, restart-blocked state, and restart-window counters.
- `stats()` does not reap workers and does not join worker/supervisor threads.

### Async runtime interop
- Rust API is intentionally synchronous (no crate-provided async `run` API) to avoid requiring Tokio or any specific async runtime.
- `MechanicsPool::new`, `run`, and `run_try_enqueue` may block the calling thread and should not be called directly on an async executor worker thread.
- For Tokio integration, call synchronous methods inside `tokio::task::spawn_blocking(...)`.
- Current implementation does not require caller-owned Tokio runtime state for these sync APIs; using them from `spawn_blocking` is supported.

### Shutdown
Dropping `MechanicsPool`:
- marks pool closed,
- cancels queued jobs best-effort,
- reaps already-finished worker handles,
- workers observe closed state and exit when idle (bounded by worker receive poll interval),
- joins supervisor and worker threads.

## Runtime limits
`MechanicsExecutionLimits` controls:
- max wall-clock execution time,
- max loop iterations,
- max recursion depth,
- max VM stack size.

Defaults:
- `max_execution_time = 10s`
- `max_loop_iterations = 1_000_000`
- `max_recursion_depth = 512`
- `max_stack_size = 10 * 1024`

## Errors
`MechanicsError` current variants:
- `Execution`
- `QueueFull`
- `QueueTimeout`
- `RunTimeout`
- `PoolClosed`
- `WorkerUnavailable`
- `Canceled`
- `Panic`
- `RuntimePool`

Note:
- `MechanicsError` is `#[non_exhaustive]`; downstream `match` statements must include a wildcard arm.

Common user-visible trigger categories:
- `RuntimePool`: invalid pool/config values, startup/runtime initialization failures, or other pool lifecycle setup failures.
- `Execution`: script/module errors, promise lifecycle errors, JSON conversion failures, and endpoint request/response processing failures.
- `RunTimeout`: overall `run`/`run_try_enqueue` deadline elapsed.
- `QueueTimeout` / `QueueFull`: enqueue pressure (`run` wait timed out vs `run_try_enqueue` immediate full queue).
- `PoolClosed` / `WorkerUnavailable`: pool closed, queue disconnected, or no workers available under restart guard.

## Usage example (Rust)
```rust
use std::collections::HashMap;
use std::sync::Arc;

use mechanics_core::{
    HttpEndpoint, HttpMethod, MechanicsConfig, MechanicsJob, MechanicsPool, MechanicsPoolConfig,
};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut endpoints = HashMap::new();
    endpoints.insert(
        "primary".to_owned(),
        HttpEndpoint::new(HttpMethod::Post, "https://httpbin.org/post", HashMap::new()),
    );

    let config = MechanicsConfig::new(endpoints)?;
    let pool = MechanicsPool::new(MechanicsPoolConfig::default())?;

    let job = MechanicsJob {
        mod_source: Arc::from(
            r#"
            import endpoint from \"mechanics:endpoint\";
            export default async function main(arg) {
                const res = await endpoint(\"primary\", { body: arg });
                return res.body;
            }
            "#,
        ),
        arg: Arc::new(json!({"hello": "world"})),
        config: Arc::new(config),
    };

    let value = pool.run(job)?;
    println!("{value}");
    Ok(())
}
```

## Assumptions and limitations
- Synthetic modules provided by default:
- `mechanics:endpoint`
- `mechanics:form-urlencoded`
- `mechanics:base64`
- `mechanics:hex`
- `mechanics:base32`
- `mechanics:rand`
- Results must be JSON-convertible to be returned successfully.
- Queue cancellation is best-effort; jobs already executing continue until runtime completion/limits.
- `mechanics:endpoint` returns `{ body, headers }`; `body` may be JSON, UTF-8 string, bytes (`Uint8Array`), or `null` (empty body) based on endpoint configuration.
- URL/query value sources are constrained to configured slots (no arbitrary URL/method/header override from JS).
- This crate currently does not include persistent module caching (source is parsed per job).

## Test coverage shape
Unit tests under `src/pool/tests/` and `src/http/tests/` cover:
- config validation,
- closed/unavailable pool behavior,
- queue-full and enqueue-timeout paths,
- loop-limit and conversion errors,
- HTTP timeout override logic,
- optional network/internet scenarios (ignored by default in constrained environments).
