declare module "mechanics:base32" {
  export type Base32Variant = "base32" | "base32hex";

  export function encode(
    bufferLike: ArrayBuffer | ArrayBufferView,
    variant?: Base32Variant
  ): string;

  export function decode(
    encoded: string,
    variant?: Base32Variant
  ): Uint8Array;
}

