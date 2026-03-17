/**
 * Cryptographically secure random byte filler.
 */
declare module "mechanics:rand" {
  /**
   * Fills the provided buffer/view with random bytes in place.
   */
  export default function fillRandom(
    bufferLike: ArrayBuffer | ArrayBufferView
  ): void;
}
