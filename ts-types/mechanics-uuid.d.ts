/**
 * UUID helper module.
 */
declare module "mechanics:uuid" {
  export type UuidVariant = "v3" | "v4" | "v5" | "v6" | "v7" | "nil" | "max";

  export interface NameBasedUuidOptions {
    /** Namespace UUID in canonical string format. */
    namespace: string;
    /** Name bytes source (UTF-8). */
    name: string;
  }

  /**
   * Generates a UUID string in lowercase canonical hyphenated format.
   *
   * - default variant: `"v4"`
   * - `v3`/`v5` require `options.namespace` and `options.name`
   */
  const uuid: (variant?: UuidVariant, options?: NameBasedUuidOptions) => string;

  export default uuid;
}
