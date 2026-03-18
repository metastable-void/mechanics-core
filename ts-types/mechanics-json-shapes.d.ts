/**
 * JSON payload shapes for mechanics-core Rust-facing config/job inputs.
 *
 * These interfaces model serde-compatible JSON forms (snake_case keys).
 * They are complementary to runtime module declarations (for example `mechanics:endpoint`).
 */

export type HttpMethod =
  | "get"
  | "post"
  | "put"
  | "patch"
  | "delete"
  | "head"
  | "options";

export type EndpointBodyType = "json" | "utf8" | "bytes";

export type SlottedQueryMode =
  | "required"
  | "required_allow_empty"
  | "optional"
  | "optional_allow_empty";

export interface UrlParamSpecJson {
  default?: string | null;
  min_bytes?: number;
  max_bytes?: number;
}

export type QuerySpecJson =
  | {
      type: "const";
      key: string;
      value: string;
    }
  | {
      type: "slotted";
      key: string;
      slot: string;
      mode?: SlottedQueryMode;
      default?: string | null;
      min_bytes?: number;
      max_bytes?: number;
    };

export interface EndpointRetryPolicyJson {
  /** Must be >= 1 when provided. */
  max_attempts?: number;
  base_backoff_ms?: number;
  max_backoff_ms?: number;
  /** Must be >= 1 when provided. */
  max_retry_delay_ms?: number;
  rate_limit_backoff_ms?: number;
  retry_on_io_errors?: boolean;
  retry_on_timeout?: boolean;
  respect_retry_after?: boolean;
  retry_on_status?: number[];
}

export interface HttpEndpointJson {
  method: HttpMethod;
  url_template: string;
  url_param_specs?: Record<string, UrlParamSpecJson>;
  query_specs?: QuerySpecJson[];
  headers?: Record<string, string>;
  overridable_request_headers?: string[];
  exposed_response_headers?: string[];
  request_body_type?: EndpointBodyType;
  response_body_type?: EndpointBodyType;
  /** Must be >= 1 when provided. */
  response_max_bytes?: number | null;
  /** Must be >= 1 when provided. */
  timeout_ms?: number | null;
  allow_non_success_status?: boolean;
  retry_policy?: EndpointRetryPolicyJson;
}

export interface MechanicsConfigJson {
  endpoints: Record<string, HttpEndpointJson>;
}

export interface MechanicsJobJson {
  mod_source: string;
  arg: unknown;
  config: MechanicsConfigJson;
}
