/**
 * WHATWG-style URL and URLSearchParams helpers.
 */
declare module "mechanics:url" {
  export type URLSearchParamsInit =
    | string
    | ArrayLike<readonly [string, string]>
    | Record<string, string>
    | null;

  export class URLSearchParams {
    constructor(init?: URLSearchParamsInit);

    readonly size: number;

    append(name: string, value: string): void;
    delete(name: string, value?: string): void;
    get(name: string): string | null;
    getAll(name: string): string[];
    has(name: string, value?: string): boolean;
    set(name: string, value: string): void;
    sort(): void;
    toString(): string;
    entries(): IterableIterator<[string, string]>;
    keys(): IterableIterator<string>;
    values(): IterableIterator<string>;
    forEach(
      callback: (value: string, name: string, params: URLSearchParams) => void,
      thisArg?: unknown
    ): void;
    [Symbol.iterator](): IterableIterator<[string, string]>;
  }

  export default class URL {
    constructor(input: string, base?: string);

    href: string;
    readonly origin: string;
    protocol: string;
    username: string;
    password: string;
    host: string;
    hostname: string;
    port: string;
    pathname: string;
    search: string;
    readonly searchParams: URLSearchParams;
    hash: string;

    toString(): string;
    toJSON(): string;

    static canParse(input: string, base?: string): boolean;
    static parse(input: string, base?: string): URL | null;
  }
}
