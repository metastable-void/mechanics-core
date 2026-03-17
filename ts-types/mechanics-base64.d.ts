declare module "mechanics:base64" {
  export type Base64Variant = "base64" | "base64url";

  export function encode(
    bufferLike: ArrayBuffer | ArrayBufferView,
    variant?: Base64Variant
  ): string;

  export function decode(
    encoded: string,
    variant?: Base64Variant
  ): Uint8Array;
}

