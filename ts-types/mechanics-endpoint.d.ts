/**
 * Preconfigured outbound HTTP call helper.
 *
 * This API is intentionally configuration-driven: JavaScript can only call endpoints defined by
 * Rust-side `MechanicsConfig`, and can only fill declared URL/query slots.
 */
declare module "mechanics:endpoint" {
  /** Body mode configured on the Rust `HttpEndpoint`. */
  export type EndpointBodyType = "json" | "utf8" | "bytes";

  /** Per-call values supplied to a preconfigured endpoint. */
  export interface EndpointCallOptions {
    /** Values for predeclared URL template slots (`{slot}` in `url_template`). */
    urlParams?: Record<string, string>;
    /** Values for predeclared slotted query specs. */
    queries?: Record<string, string>;
    /**
     * Request headers allowed by endpoint `overridable_request_headers`.
     *
     * Header-name matching is case-insensitive.
     */
    headers?: Record<string, string>;
    /**
     * Optional request body.
     *
     * The accepted runtime type depends on endpoint `request_body_type`:
     * - `json`: JSON-compatible values
     * - `utf8`: string
     * - `bytes`: `ArrayBuffer`/typed-array/DataView
     *
     * Omission semantics:
     * - omitted/`undefined`: no request body
     * - explicit `null`: JSON null body (for `json` request mode)
     *
     * For `GET`/`DELETE` endpoints, any provided body value (including `null`) is rejected.
     */
    body?: unknown;
  }

  /**
   * Endpoint call result.
   */
  export interface EndpointResponse {
    /**
     * Parsed response body according to endpoint `response_body_type`.
     *
     * Empty response bodies are returned as `null`.
     */
    body: unknown | string | Uint8Array | null;
    /**
     * Response headers exposed by endpoint `exposed_response_headers`.
     *
     * Keys are lowercase header names.
     * Allowlisted headers that are not present in the response are omitted.
     */
    headers: Record<string, string>;
  }

  /**
   * Executes a named preconfigured endpoint.
   */
  const endpoint: (name: string, options?: EndpointCallOptions) => Promise<EndpointResponse>;

  export default endpoint;
}
