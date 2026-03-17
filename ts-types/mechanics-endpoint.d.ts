declare module "mechanics:endpoint" {
  export type EndpointBodyType = "json" | "utf8" | "bytes";

  export interface EndpointCallOptions {
    urlParams?: Record<string, string>;
    queries?: Record<string, string>;
    body?: unknown;
  }

  const endpoint: (
    name: string,
    options?: EndpointCallOptions
  ) => Promise<unknown | string | Uint8Array | null>;

  export default endpoint;
}

