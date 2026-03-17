/**
 * Base32 encode/decode helpers.
 */
declare module "mechanics:base32" {
  /** Supported base32 alphabets. */
  export type Base32Variant = "base32" | "base32hex";

  /**
   * Encodes bytes into a base32 string.
   *
   * `bufferLike` accepts `ArrayBuffer` and all typed-array/DataView views.
   * SharedArrayBuffer-backed views are not supported by the runtime.
   */
  export function encode(
    bufferLike: ArrayBuffer | ArrayBufferView,
    variant?: Base32Variant
  ): string;

  /**
   * Decodes a base32 string into bytes.
   */
  export function decode(
    encoded: string,
    variant?: Base32Variant
  ): Uint8Array;
}
