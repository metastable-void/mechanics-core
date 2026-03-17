/**
 * Hexadecimal encode/decode helpers.
 */
declare module "mechanics:hex" {
  /**
   * Encodes bytes into lowercase hexadecimal.
   */
  export function encode(bufferLike: ArrayBuffer | ArrayBufferView): string;

  /**
   * Decodes hexadecimal into bytes.
   */
  export function decode(encoded: string): Uint8Array;
}
