// The per-call module worker of the browser runtime: it instantiates the wasm
// build the page chose with the module the page compiled, runs one task, and
// posts the outcome. The page terminates it when the call settles.
import type {
  WasmVariant,
  WorkerError,
  WorkerMessage,
  WorkerRequest,
  WorkerResult,
} from "./worker-protocol.js";

/** The wasm-bindgen glue of binding-wasm (`wasm/<variant>/primeval.js`). */
interface Glue {
  default(options: { module_or_path: WebAssembly.Module }): Promise<unknown>;
  approximate(
    input: Uint8Array,
    output: string,
    render: object,
    onProgress?: (info: { step: number; total: number; score: number }) => void,
  ): WorkerResult;
  setPanicReporter(channel: string, report: (message: string) => void): void;
  __panicForTests(inPool: boolean): void;
  initThreadPool?(threads: number): Promise<unknown>;
}

interface WorkerScope {
  postMessage(message: WorkerMessage, transfer?: Transferable[]): void;
  onmessage: ((event: MessageEvent<WorkerRequest>) => void) | null;
}

const scope = globalThis as unknown as WorkerScope;
const post = (message: WorkerMessage, transfer: Transferable[] = []): void =>
  scope.postMessage(message, transfer);

// Literal URLs, one per variant, so bundlers can see both.
function glueUrl(variant: WasmVariant): URL {
  return variant === "threaded"
    ? new URL("../wasm/threaded/primeval.js", import.meta.url)
    : new URL("../wasm/single/primeval.js", import.meta.url);
}

function messageOf(error: unknown): string {
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

async function start({ variant, module, channel }: WorkerRequest): Promise<Glue> {
  const glue = (await import(glueUrl(variant).href)) as Glue;
  await glue.default({ module_or_path: module });
  // Before the pool starts, so the hook covers every thread.
  glue.setPanicReporter(channel, (message) => post({ type: "panic", message }));
  if (variant === "threaded") {
    await glue.initThreadPool?.(navigator.hardwareConcurrency);
  }
  return glue;
}

scope.onmessage = async ({ data: request }) => {
  // One task per worker.
  scope.onmessage = null;
  let glue: Glue;
  try {
    glue = await start(request);
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
};
