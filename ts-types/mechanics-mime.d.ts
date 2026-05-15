/**
 * MIME compose/parse helpers.
 */
declare module "mechanics:mime" {
  export type MimeTransferEncoding =
    | "7bit"
    | "8bit"
    | "binary"
    | "quoted-printable"
    | "base64";

  export type MimeHeaders = Record<string, string>;

  export interface MimeLeafMessage {
    headers?: MimeHeaders | null;
    body?: string | ArrayBuffer | ArrayBufferView | null;
    encoding?: MimeTransferEncoding | null;
  }

  export interface MimeMultipartMessage {
    headers?: MimeHeaders | null;
    parts: MimeMessage[];
    encoding?: MimeTransferEncoding | null;
  }

  export type MimeMessage = MimeLeafMessage | MimeMultipartMessage;

  export interface ParsedMimeLeafMessage {
    headers: MimeHeaders;
    body: string | Uint8Array;
  }

  export interface ParsedMimeMultipartMessage {
    headers: MimeHeaders;
    parts: ParsedMimeMessage[];
  }

  export type ParsedMimeMessage =
    | ParsedMimeLeafMessage
    | ParsedMimeMultipartMessage;

  /** Composes a structured MIME message into a CRLF-normalized raw message string. */
  export function compose(message: MimeMessage): string;

  /** Parses a raw MIME message string or UTF-8 byte buffer. */
  export function parse(raw: string | ArrayBuffer | ArrayBufferView): ParsedMimeMessage;
}
