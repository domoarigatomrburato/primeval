// Shared calling-worker logic: initialize the instance, panic hook and pool
// once, then run one task per message. Only successful workers may be reused.
import { messageOf, optionFields } from "./errors.js";
import type { ProgressInfo } from "./types.js";
import type { WorkerError, WorkerMessage, WorkerRequest, WorkerResult } from "./worker-protocol.js";

/**
 * The surface of binding-wasm's wasm-bindgen glue (`wasm/<variant>/primeval.js`)
 * that the workers use, and its only contract here. Written by hand: the glue
 * and its declarations exist only after `npm run build:wasm`, and a clean
 * checkout type-checks against the untyped stand-ins in wasm-glue.d.ts
 * instead. Where they exist, each entry's assignment of the glue to this type
 * checks it against the generated declarations.
 */
export interface Glue {
  default(options: {
    module_or_path: WebAssembly.Module;
    memory?: WebAssembly.Memory;
  }): Promise<unknown>;
  approximate(
    input: Uint8Array,
    output: string,
    render: object,
    onProgress?: (info: ProgressInfo) => void,
  ): WorkerResult;
  setPanicReporter(channel: string, report: (message: string) => void): void;
  __panicForTests(inPool: boolean): void;
}

/** The threaded build's glue adds the rayon pool (binding-wasm `src/pool.rs`). */
export interface ThreadedGlue extends Glue {
  PoolBuilder: new (
    numThreads: number,
  ) => {
    readonly numThreads: number;
    receiver(): number;
    build(): void;
  };
  startPoolWorker(receiver: number): void;
  wasmMemory(): WebAssembly.Memory;
}

export interface WorkerScope {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  onmessage: ((event: MessageEvent) => void) | null;
}

export const scope = globalThis as unknown as WorkerScope;

const post = (message: WorkerMessage, transfer: Transferable[] = []): void =>
  scope.postMessage(message, transfer);

/** A thrown value as plain data; without a stable code it is internal. */
function workerError(error: unknown): WorkerError {
  const fields = (typeof error === "object" && error !== null ? error : {}) as Record<
    string,
    unknown
  >;
  return {
    code: typeof fields.code === "string" ? fields.code : "INTERNAL",
    message: messageOf(error),
    ...optionFields(fields),
  };
}

/** Initializes once per calling worker, then runs each message's task. */
export function requestRunner(
  glue: Glue,
  startPool?: (module: WebAssembly.Module) => Promise<void>,
): (request: WorkerRequest) => Promise<void> {
  let ready: Promise<void> | undefined;
  return async (request) => {
    try {
      if (ready === undefined) {
        if (request.type !== "init") {
          throw new Error("the worker has not been initialized");
        }
        ready = (async () => {
          await glue.default({ module_or_path: request.module });
          // Before the pool starts, so the hook covers every thread.
          glue.setPanicReporter(request.channel, (message) =>
            post({ type: "error", error: { code: "INTERNAL", message } }),
          );
          await startPool?.(request.module);
        })();
      }
      await ready;
    } catch (error) {
      post({
        type: "error",
        error: {
          code: "INTERNAL",
          message: `could not start the WebAssembly module: ${messageOf(error)}`,
        },
      });
      return;
    }

    const { task } = request;
    try {
      if (task.kind === "panic") {
        glue.__panicForTests(task.site === "pool");
        throw new Error("the test panic did not panic");
      }
      const result = glue.approximate(
        task.input,
        task.output,
        task.render,
        request.progress ? (info) => post({ type: "progress", info }) : undefined,
      );
      post({ type: "result", result }, [result.data.buffer]);
    } catch (error) {
      // The panic hook's error precedes this one on the same port.
      post({ type: "error", error: workerError(error) });
    }
  };
}
