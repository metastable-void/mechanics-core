declare module "mechanics:form-urlencoded" {
  export function encode(record: Record<string, string>): string;
  export function decode(params: string): Record<string, string>;
}

