/**
 * Hexadecimal encode/decode helpers.
 */
declare module "mechanics:hex" {
  /**
   * Encodes bytes into lowercase hexadecimal.
   *
   * SharedArrayBuffer-backed views are not supported by the runtime.
   */
  export function encode(bufferLike: ArrayBuffer | ArrayBufferView): string;

  /**
   * Decodes hexadecimal into bytes.
   */
  export function decode(encoded: string): Uint8Array;
}
