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
     * Optional request body.
     *
     * The accepted runtime type depends on endpoint `request_body_type`:
     * - `json`: JSON-compatible values
     * - `utf8`: string
     * - `bytes`: `ArrayBuffer`/typed-array/DataView
     */
    body?: unknown;
  }

  /**
   * Executes a named preconfigured endpoint.
   *
   * Resolves to one of:
   * - parsed JSON value
   * - UTF-8 string
   * - `Uint8Array` bytes
   * - `null` when response body is empty
   */
  const endpoint: (
    name: string,
    options?: EndpointCallOptions
  ) => Promise<unknown | string | Uint8Array | null>;

  export default endpoint;
}
