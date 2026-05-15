/**
 * HTML escaping and unescaping helpers.
 */
declare module "mechanics:html" {
  /** Escapes text-node-sensitive HTML characters. */
  export function escapeText(text: string): string;

  /** Escapes HTML attribute-sensitive characters, including quotes. */
  export function escapeAttribute(text: string): string;

  /** Unescapes HTML text entities. */
  export function unescapeText(text: string): string;

  /** Unescapes HTML attribute entities. */
  export function unescapeAttribute(text: string): string;
}
