/**
 * Form URL-encoding helpers (`application/x-www-form-urlencoded`).
 */
declare module "mechanics:form-urlencoded" {
  /**
   * Encodes key/value pairs as a form-urlencoded string.
   */
  export function encode(record: Record<string, string>): string;

  /**
   * Decodes a form-urlencoded string into key/value pairs.
   *
   * Duplicate keys use last-value-wins semantics.
   */
  export function decode(params: string): Record<string, string>;
}
