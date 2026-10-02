// Stand-ins for binding-wasm's wasm-bindgen glue (`wasm/<variant>/primeval.js`),
// which exists only after `npm run build:wasm`, so that a clean checkout
// type-checks. TypeScript uses these only when the glue is not built; where it
// is, it resolves the generated declarations instead, and the workers'
// assignments to `Glue` and `ThreadedGlue` (worker-common.ts) check those.

declare module "*/wasm/single/primeval.js" {
  export default function init(options: {
    module_or_path: WebAssembly.Module;
    memory?: WebAssembly.Memory;
  }): Promise<unknown>;
  export function approximate(
    input: Uint8Array,
    output: string,
    render: object,
    onProgress?: (info: { step: number; total: number; score: number; shape: string }) => void,
  ): { format: string; mimeType: string; width: number; height: number; data: Uint8Array };
  export function setPanicReporter(channel: string, report: (message: string) => void): void;
  export function __panicForTests(inPool: boolean): void;
}

declare module "*/wasm/threaded/primeval.js" {
  export default function init(options: {
    module_or_path: WebAssembly.Module;
    memory?: WebAssembly.Memory;
  }): Promise<unknown>;
  export function approximate(
    input: Uint8Array,
    output: string,
    render: object,
    onProgress?: (info: { step: number; total: number; score: number; shape: string }) => void,
  ): { format: string; mimeType: string; width: number; height: number; data: Uint8Array };
  export function setPanicReporter(channel: string, report: (message: string) => void): void;
  export function __panicForTests(inPool: boolean): void;
  export class PoolBuilder {
    constructor(numThreads: number);
    readonly numThreads: number;
    receiver(): number;
    build(): void;
  }
  export function startPoolWorker(receiver: number): void;
  export function wasmMemory(): WebAssembly.Memory;
}
