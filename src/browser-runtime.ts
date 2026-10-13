// The browser runtime behind browser.ts. Internal: not a public entry point.
//
// Calls take the page's idle module worker or start one of their build
// (worker-single.ts or worker-threaded.ts). Only a delivered result keeps its
// worker idle, for up to 10 seconds; at most one is retained. Every failure
// terminates the worker, its instance, memory and thread pool immediately.
// Concurrent calls have separate workers and memory. The page picks the build
// before fetching anything and compiles its `.wasm` once. Fresh workers load
// one at a time (see `queueStart`); reused workers bypass that gate and retain
// their instance, pool and panic channel, receiving only the next task.
import { InternalError, mapNativeError, messageOf } from "./errors.js";
import { abortError, normalizeRequest } from "./request.js";
import type { ProgressInfo } from "./types.js";
import type {
  PanicSite,
  WasmVariant,
  WorkerError,
  WorkerMessage,
  WorkerRequest,
  WorkerResult,
  WorkerTask,
} from "./worker-protocol.js";

const IDLE_MS = 10_000;
let idleMs = IDLE_MS;
let workersStarted = 0;

const modules = new Map<WasmVariant, Promise<WebAssembly.Module>>();

function selectVariant(): WasmVariant {
  return globalThis.crossOriginIsolated === true ? "threaded" : "single";
}

// Literal URLs, one per variant, so bundlers can see both.
function wasmUrl(variant: WasmVariant): URL {
  return variant === "threaded"
    ? new URL("../wasm/threaded/primeval_bg.wasm", import.meta.url)
    : new URL("../wasm/single/primeval_bg.wasm", import.meta.url);
}

// Literal URLs, one per variant, so bundlers can see both workers.
function startWorker(variant: WasmVariant): Worker {
  const worker =
    variant === "threaded"
      ? new Worker(new URL("./worker-threaded.js", import.meta.url), { type: "module" })
      : new Worker(new URL("./worker-single.js", import.meta.url), { type: "module" });
  workersStarted += 1;
  return worker;
}

/**
 * The smallest module with a SIMD instruction (`i8x16.popcnt` of an
 * `i8x16.splat`), from the final SIMD proposal: browsers without WebAssembly
 * SIMD do not validate it.
 */
const SIMD_PROBE = Uint8Array.from([
  ...[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00], // magic, version
  ...[0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7b], // types: () -> v128
  ...[0x03, 0x02, 0x01, 0x00], // functions: one of that type
  ...[0x0a, 0x0a, 0x01, 0x08, 0x00, 0x41, 0x00, 0xfd, 0x0f, 0xfd, 0x62, 0x0b], // its code
]);

/**
 * The error for a `.wasm` that failed to load or compile: both builds need
 * WebAssembly SIMD, so a browser without it is told which browsers have it.
 */
function loadError(error: unknown): InternalError {
  let simd = false;
  try {
    simd = WebAssembly.validate(SIMD_PROBE);
  } catch {
    // An engine that cannot even validate the probe has no SIMD.
  }
  return simd
    ? new InternalError(`could not load the WebAssembly module: ${messageOf(error)}`, {
        cause: error,
      })
    : new InternalError(
        "this browser does not support WebAssembly SIMD, which primeval needs: " +
          "use Chrome 91, Firefox 89, Safari 16.4 or newer",
        { cause: error },
      );
}

/** The compiled module of `variant`, once per page; a failure is retried next call. */
function compiledModule(variant: WasmVariant): Promise<WebAssembly.Module> {
  let module = modules.get(variant);
  if (module === undefined) {
    const compiling = WebAssembly.compileStreaming(fetch(wasmUrl(variant)));
    compiling.catch(() => {
      if (modules.get(variant) === compiling) {
        modules.delete(variant);
      }
    });
    modules.set(variant, compiling);
    module = compiling;
  }
  return module;
}

/**
 * Resolves once the latest fresh worker has started or been terminated,
 * and so has every fresh worker before it. A worker has started once its
 * modules are loaded and, in the threaded build, those of all its pool
 * workers too.
 */
let startGate: Promise<void> = Promise.resolve();

/**
 * Queues a fresh worker at the start gate: `turn` resolves when it may start,
 * and `release`, once it has started or been terminated (or acquisition was
 * aborted), lets the next fresh worker start. So the package never has two
 * workers loading at once, which WebKit fails on a page isolated by a service
 * worker (https://bugs.webkit.org/show_bug.cgi?id=261734). A call released before its
 * turn, by an abort, still holds the next call until its own turn comes.
 */
function queueStart(): { turn: Promise<void>; release: () => void } {
  const turn = startGate;
  let release: () => void = () => undefined;
  const started = new Promise<void>((resolve) => {
    release = resolve;
  });
  startGate = Promise.all([turn, started]).then(() => undefined);
  return { turn, release };
}

/** A name no other worker or page uses, for the worker's panic channel. */
function channelName(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `primeval-panic-${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

/**
 * The error Node rejects with for the same Rust error: the worker's plain
 * fields become a native-like `Error`, mapped by the Node mapping.
 */
function errorFromWorker({ code, message, option, requirement }: WorkerError): unknown {
  const cause = Object.assign(
    new Error(message),
    { code },
    option === undefined ? {} : { option, requirement },
  );
  return mapNativeError(cause);
}

/** A calling worker's lifetime state, shared by its successive calls. */
interface RuntimeWorker {
  /** Calling worker that owns the wasm instance and any pool workers. */
  worker: Worker;
  /** The page's panic channel, open until this worker is terminated. */
  channel: BroadcastChannel;
  /** Lets the next fresh worker load once this one has started or terminated. */
  releaseStart: () => void;
  /** Active call's message handler; set before posting, cleared at settle or termination. */
  onMessage?: (message: WorkerMessage) => void;
  /** Active call's rejection handler; set before posting, cleared at settle or termination. */
  fail?: (error: unknown) => void;
}

/** The page's sole successful worker waiting for reuse, or no idle worker. */
let idle:
  | {
      /** Worker whose most recent call delivered a result. */
      runtime: RuntimeWorker;
      /** Terminates it after idleMs; cleared on reuse or termination. */
      timer: ReturnType<typeof setTimeout>;
    }
  | undefined;

/** Clears the call's handlers, retaining only a worker that delivered a result. */
function release(runtime: RuntimeWorker, reusable: boolean): void {
  runtime.onMessage = undefined;
  runtime.fail = undefined;
  if (reusable) {
    retain(runtime);
  } else {
    if (idle?.runtime === runtime) {
      clearTimeout(idle.timer);
      idle = undefined;
    }
    runtime.worker.terminate();
    runtime.channel.close();
    runtime.releaseStart();
  }
}

/** Ends the entire worker lifetime, including its panic channel. */
function terminate(runtime: RuntimeWorker): void {
  release(runtime, false);
}

function retain(runtime: RuntimeWorker): void {
  if (idle !== undefined || idleMs === 0) {
    terminate(runtime);
    return;
  }
  idle = { runtime, timer: setTimeout(() => terminate(runtime), idleMs) };
}

function takeIdle(): RuntimeWorker | undefined {
  if (idle === undefined) {
    return undefined;
  }
  const { runtime, timer } = idle;
  clearTimeout(timer);
  idle = undefined;
  return runtime;
}

/** Open the panic channel before starting the worker; it belongs to its life. */
function createWorker(
  variant: WasmVariant,
  module: WebAssembly.Module,
  releaseStart: () => void,
): RuntimeWorker {
  const channel = new BroadcastChannel(channelName());
  let worker: Worker;
  try {
    worker = startWorker(variant);
  } catch (error) {
    channel.close();
    throw error;
  }
  const runtime: RuntimeWorker = { worker, channel, releaseStart };
  const fail = (error: unknown): void => {
    if (runtime.fail !== undefined) {
      runtime.fail(error);
    } else {
      terminate(runtime);
    }
  };
  channel.onmessage = (event: MessageEvent<unknown>) =>
    fail(errorFromWorker({ code: "INTERNAL", message: String(event.data) }));
  worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
    if (event.data.type === "started") {
      releaseStart();
    } else if (runtime.onMessage !== undefined) {
      runtime.onMessage(event.data);
    } else if (event.data.type === "error") {
      fail(errorFromWorker(event.data.error));
    }
  };
  worker.onerror = (event) => {
    event.preventDefault();
    fail(new InternalError("the primeval worker failed to load or crashed"));
  };
  worker.onmessageerror = () =>
    fail(new InternalError("a message from the primeval worker could not be read"));
  try {
    worker.postMessage({ type: "init", module, channel: channel.name } satisfies WorkerRequest);
  } catch (error) {
    terminate(runtime);
    throw error;
  }
  return runtime;
}

/** Takes the idle worker, or compiles and starts a fresh worker at its gate turn. */
function acquire(variant: WasmVariant, signal: AbortSignal | undefined): Promise<RuntimeWorker> {
  const runtime = takeIdle();
  if (runtime !== undefined) {
    return Promise.resolve(runtime);
  }
  const gate = queueStart();
  return new Promise<RuntimeWorker>((resolve, reject) => {
    const fail = (error: unknown): void => {
      signal?.removeEventListener("abort", onAbort);
      gate.release();
      reject(error);
    };
    const onAbort = (): void => fail(abortError(signal as AbortSignal));
    signal?.addEventListener("abort", onAbort, { once: true });
    // The turn never rejects: a failure is the compile's.
    let module: Promise<WebAssembly.Module>;
    try {
      module = compiledModule(variant);
    } catch (error) {
      fail(loadError(error));
      return;
    }
    Promise.all([module, gate.turn]).then(
      ([module]) => {
        if (signal?.aborted) {
          return;
        }
        try {
          const runtime = createWorker(variant, module, gate.release);
          signal?.removeEventListener("abort", onAbort);
          resolve(runtime);
        } catch (error) {
          fail(
            new InternalError(`could not start the primeval worker: ${messageOf(error)}`, {
              cause: error,
            }),
          );
        }
      },
      (error: unknown) => fail(loadError(error)),
    );
  });
}

/**
 * Runs one task in an idle worker or a fresh one through the start gate.
 * The first reason to stop wins. Only a delivered result permits reuse.
 */
async function runInWorker(
  task: WorkerTask,
  onProgress: ((info: ProgressInfo) => void) | undefined,
  signal: AbortSignal | undefined,
): Promise<WorkerResult> {
  const runtime = await acquire(selectVariant(), signal);
  if (signal?.aborted) {
    release(runtime, false);
    throw abortError(signal);
  }
  return new Promise<WorkerResult>((resolve, reject) => {
    let settled = false;
    const settle = (finish: () => void, reusable = false): void => {
      if (settled) {
        return;
      }
      settled = true;
      release(runtime, reusable);
      signal?.removeEventListener("abort", onAbort);
      finish();
    };
    const fail = (error: unknown): void => settle(() => reject(error));
    const onAbort = (): void => fail(abortError(signal as AbortSignal));
    signal?.addEventListener("abort", onAbort, { once: true });

    const onMessage = (message: WorkerMessage): void => {
      if (settled) {
        return;
      }
      switch (message.type) {
        case "loaded":
        case "started":
          break;
        case "progress":
          try {
            onProgress?.(message.info);
          } catch (error) {
            fail(error);
          }
          break;
        case "result":
          // An abort before the result is delivered wins, as on Node.
          if (signal?.aborted) {
            fail(abortError(signal));
          } else {
            settle(() => resolve(message.result), true);
          }
          break;
        case "error":
          fail(errorFromWorker(message.error));
          break;
      }
    };

    runtime.onMessage = onMessage;
    runtime.fail = fail;
    try {
      const request: WorkerRequest = { type: "run", progress: onProgress !== undefined, task };
      const transfer = task.kind === "approximate" ? [task.input.buffer as ArrayBuffer] : [];
      runtime.worker.postMessage(request, transfer);
    } catch (error) {
      fail(
        new InternalError(`could not start the primeval worker: ${messageOf(error)}`, {
          cause: error,
        }),
      );
    }
  });
}

/** Validates `request` and renders it in a worker. Used by browser.ts. */
export async function approximateInWorker(request: unknown): Promise<WorkerResult> {
  const { input, output, render, onProgress, signal } = normalizeRequest(request);
  if (signal?.aborted) {
    throw abortError(signal);
  }
  // A copy of only the viewed bytes, transferred to the worker; the caller's
  // array is left as it is.
  const task: WorkerTask = { kind: "approximate", input: input.slice(), output, render };
  return runInWorker(task, onProgress, signal);
}

/**
 * Internal, for the browser tests only: panics in a calling worker, on the
 * calling thread or in a rayon pool task, through binding-wasm's hidden
 * `__panicForTests`. Rejects with the `InternalError` a render panic gives.
 *
 * @testonly
 */
export async function panicForTests(site: PanicSite): Promise<never> {
  await runInWorker({ kind: "panic", site }, undefined, undefined);
  throw new InternalError("the test panic did not reject");
}

/**
 * Clears the idle slot and sets its lifetime; omitted restores the default.
 * Zero terminates successful workers at settle, disabling reuse.
 * @testonly
 */
export function setIdleMsForTests(ms: number = IDLE_MS): void {
  if (idle !== undefined) {
    terminate(idle.runtime);
  }
  idleMs = ms;
}

/**
 * Number of calling workers started by this page runtime (not pool workers).
 * @testonly
 */
export function workersStartedForTests(): number {
  return workersStarted;
}
