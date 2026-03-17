/**
 * Form URL-encoding helpers (`application/x-www-form-urlencoded`).
 */
declare module "mechanics:form-urlencoded" {
  /**
   * Encodes key/value pairs as a form-urlencoded string.
   *
   * Output uses deterministic lexical key ordering.
   */
  export function encode(record: Record<string, string>): string;

  /**
   * Decodes a form-urlencoded string into key/value pairs.
   *
   * An optional leading `?` is accepted.
   * Duplicate keys use last-value-wins semantics.
   */
  export function decode(params: string): Record<string, string>;
}
