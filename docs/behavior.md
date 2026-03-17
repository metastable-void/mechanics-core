# mechanics-core behavior and usage

## Purpose
`mechanics-core` executes user-provided JavaScript modules inside Boa (`boa_engine`) with:
- per-job execution limits,
- a built-in `mechanics:endpoint` helper for preconfigured HTTP calls,
- a worker pool for concurrent job execution.

The crate API is exported from `src/lib.rs`:
- `MechanicsPool`, `MechanicsPoolConfig`
- `MechanicsJob`, `MechanicsExecutionLimits`
- `MechanicsConfig`, `HttpEndpoint`, `HttpMethod`
- `UrlParamSpec`, `QuerySpec`, `SlottedQueryMode`
- `MechanicsError`

## High-level model
1. You build a `MechanicsPool`.
2. You submit a `MechanicsJob` containing:
- module source (`mod_source`),
- JSON argument (`arg`),
- endpoint config (`config`).
3. A worker creates/uses a runtime (`RuntimeInternal`) and executes the module.
4. The module default export is invoked with one argument.
5. Result is converted to JSON and returned.

## JavaScript contract
Your module should export a callable default export.

```js
export default function main(arg) {
  return { ok: true, got: arg };
}
```

At runtime:
- `default` export is resolved and invoked.
- If return value is not a Promise, it is wrapped in a resolved Promise.
- Job queue drains once after invocation.
- Final value is converted to `serde_json::Value`.
- Unhandled async job errors (including unhandled Promise rejections) are treated as fatal for the current job.

If JSON conversion fails, execution fails with `MechanicsError::Execution`.

## Built-in module: `mechanics:endpoint`
Runtime registers a synthetic module named `mechanics:endpoint` with default export `endpoint(name, options)`.

```js
import endpoint from "mechanics:endpoint";

export default async function main(arg) {
  return await endpoint("primary", {
    urlParams: { user_id: "u-123" },
    queries: { page: "1", filter: "active" },
    body: arg
  });
}
```

Resolution behavior:
- `name` must match a key in `MechanicsConfig.endpoints`.
- Endpoint config controls HTTP method (`GET`/`POST`/`PUT`/`DELETE`), URL template, URL slot rules, query emission rules, headers, timeout, and status policy.
- URL template placeholders (`{slot}`) are resolved from JS `options.urlParams` using configured `url_param_specs`.
- Query string is built algorithmically from configured `query_specs` using JS `options.queries`.
- Configured headers are validated; invalid names/values fail the call.
- By default, non-2xx HTTP statuses fail the call.
- `HttpEndpoint::with_allow_non_success_status(true)` opt-in allows JSON parsing on non-2xx statuses.
- Response body is parsed as JSON and returned to JS.

`endpoint(name, options)` payload shape (camelCase):
- `urlParams`: object of string slot values for URL template substitution.
- `queries`: object of string slot values used by configured slotted query specs.
- `body`: JSON value payload (`POST`/`PUT`); for `GET`/`DELETE` this must be `null` or omitted.

Config shape is JSON-friendly and snake_case (`serde`):
- endpoint definitions use `method`, `url_template`, `url_param_specs`, and `query_specs`.
- `url_template` is a full URL template string and placeholder names must be unique.
- `url_param_specs` maps placeholder names to constraints and optional defaults.
- `query_specs` is an ordered list with `type: "const" | "slotted"`.
- slotted query `mode` values:
- `required`: query value must resolve and must be non-empty.
- `required_allow_empty`: query value must resolve and may be empty.
- `optional`: missing/empty is treated as omitted.
- `optional_allow_empty`: missing is omitted; if provided, empty is emitted.
- URL param default behavior:
- if `default` exists, missing/empty JS value uses `default`.
- if `default` is absent, missing/empty JS value resolves as empty.

Byte-length validation:
- `min_bytes` / `max_bytes` for URL/query slots are validated against raw UTF-8 byte length.

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
      "timeout_ms": 5000,
      "allow_non_success_status": false
    }
  }
}
```

Timeout behavior:
- Endpoint timeout = `HttpEndpoint::with_timeout_ms(...)` if set,
- else pool default `MechanicsPoolConfig.default_http_timeout_ms`.

## Pool and queue behavior
`MechanicsPool::new(config)` creates:
- bounded job queue (`queue_capacity`),
- N worker threads (`worker_count`),
- supervisor thread with restart rate limiter (`restart_window`, `max_restarts_in_window`).
- If any worker fails during startup runtime initialization, construction fails with `MechanicsError::RuntimePool` (no partial usable pool is returned).

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

### Shutdown
Dropping `MechanicsPool`:
- marks pool closed,
- cancels queued jobs best-effort,
- reaps already-finished worker handles before shutdown signaling,
- sends shutdown messages to workers,
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
`MechanicsError` variants:
- `Execution`
- `QueueFull`
- `QueueTimeout`
- `RunTimeout`
- `PoolClosed`
- `WorkerUnavailable`
- `Canceled`
- `Panic`
- `RuntimePool`

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

    let config = MechanicsConfig::new(endpoints);
    let pool = MechanicsPool::new(MechanicsPoolConfig::default())?;

    let job = MechanicsJob {
        mod_source: Arc::from(
            r#"
            import endpoint from \"mechanics:endpoint\";
            export default async function main(arg) {
                return await endpoint(\"primary\", { body: arg });
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
- Only `mechanics:endpoint` is provided as importable synthetic module by default.
- Results must be JSON-convertible to be returned successfully.
- Queue cancellation is best-effort; jobs already executing continue until runtime completion/limits.
- HTTP helper is JSON-out only; request body is JSON for `POST`/`PUT`.
- URL/query value sources are constrained to configured slots (no arbitrary URL/method/header override from JS).
- This crate currently does not include persistent module caching (source is parsed per job).

## Test coverage shape
Unit tests in `src/pool.rs` cover:
- config validation,
- closed/unavailable pool behavior,
- queue-full and enqueue-timeout paths,
- loop-limit and conversion errors,
- HTTP timeout override logic,
- optional network/internet scenarios (ignored by default in constrained environments).
