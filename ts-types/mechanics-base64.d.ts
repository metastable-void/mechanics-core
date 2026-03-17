/**
 * Base64 encode/decode helpers.
 */
declare module "mechanics:base64" {
  /** Supported base64 alphabets. */
  export type Base64Variant = "base64" | "base64url";

  /**
   * Encodes bytes into a base64 string.
   *
   * `bufferLike` accepts `ArrayBuffer` and all typed-array/DataView views.
   * SharedArrayBuffer-backed views are not supported by the runtime.
   */
  export function encode(
    bufferLike: ArrayBuffer | ArrayBufferView,
    variant?: Base64Variant
  ): string;

  /**
   * Decodes a base64 string into bytes.
   */
  export function decode(
    encoded: string,
    variant?: Base64Variant
  ): Uint8Array;
}
