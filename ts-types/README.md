# mechanics-core runtime declarations

This directory contains TypeScript declarations for synthetic runtime modules provided by `mechanics-core`.

## Policy
- Any public API change to runtime synthetic modules must update these `.d.ts` files in the same change.
- Keep declaration names and signatures aligned with runtime behavior docs in `docs/behavior.md`.
- Keep JSDoc comments aligned with runtime behavior so editor IntelliSense stays accurate.
- Runtime-facing JSON input shapes are declared in `mechanics-json-shapes.d.ts` and should stay aligned with serde behavior.
- Keep `mechanics-json-shapes.d.ts` and `json-schema/*.schema.json` synchronized when JSON contracts change.
