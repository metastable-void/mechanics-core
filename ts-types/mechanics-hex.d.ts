declare module "mechanics:hex" {
  export function encode(bufferLike: ArrayBuffer | ArrayBufferView): string;
  export function decode(encoded: string): Uint8Array;
}

