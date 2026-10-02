// Untyped stand-ins for binding-wasm's wasm-bindgen glue
// (`wasm/<variant>/primeval.js`), which exists only after `npm run build:wasm`,
// so that a clean checkout type-checks. TypeScript uses these only when the
// glue is not built; where it is, it resolves the generated declarations
// instead. The contract is `Glue` and `ThreadedGlue` in worker-common.ts,
// which the workers' assignments check against the generated declarations.

declare module "*/wasm/single/primeval.js";
declare module "*/wasm/threaded/primeval.js";
