// The logic both per-call workers share (worker-single.ts, worker-threaded.ts):
// instantiate the glue the entry imported with the module the page compiled,
// run one task, and post the outcome. The page terminates the worker when the
// call settles.
import type { WorkerError, WorkerMessage, WorkerRequest, WorkerResult } from "./worker-protocol.js";

/**
 * The surface of binding-wasm's wasm-bindgen glue (`wasm/<variant>/primeval.js`)
 * that the workers use. Written by hand: the glue and its declarations exist
 * only after `npm run build:wasm`, and a clean checkout must type-check
 * without them. Where they exist, each entry's assignment of the glue to this
 * type checks it against the generated declarations.
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
    onProgress?: (info: { step: number; total: number; score: number; shape: string }) => void,
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

export function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A thrown value as plain data; without a stable code it is internal. */
function workerError(error: unknown): WorkerError {
  const fields = (typeof error === "object" && error !== null ? error : {}) as Record<
    string,
    unknown
  >;
  return {
    code: typeof fields.code === "string" ? fields.code : "INTERNAL",
    message: messageOf(error),
    ...(typeof fields.option === "string"
      ? { option: fields.option, requirement: String(fields.requirement) }
      : {}),
  };
}

/**
 * Runs the page's one request with `glue`. `startPool`, in the threaded
 * build, starts the rayon pool once the instance and its panic hook are ready.
 */
export async function runRequest(
  glue: Glue,
  request: WorkerRequest,
  startPool?: (module: WebAssembly.Module) => Promise<void>,
): Promise<void> {
  try {
    await glue.default({ module_or_path: request.module });
    // Before the pool starts, so the hook covers every thread.
    glue.setPanicReporter(request.channel, (message) => post({ type: "panic", message }));
    await startPool?.(request.module);
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
    // After a panic on this thread, the hook's "panic" message is already
    // ahead of this one on the same port.
    post({ type: "error", error: workerError(error) });
  }
}
