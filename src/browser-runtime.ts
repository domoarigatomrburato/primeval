// The browser runtime behind browser.ts. Internal: not a public entry point.
//
// Each call runs in a fresh module worker of its build (worker-single.ts or
// worker-threaded.ts) that is terminated when the call settles, whatever the
// outcome; a terminated worker takes the instance, its memory and its thread
// pool (nested workers) with it. The page picks the build (threaded when
// cross-origin isolated, single-threaded otherwise) before fetching anything,
// compiles its `.wasm` once, and posts the compiled module to every worker.
// Calls start their workers one at a time (see `queueStart`).
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
  return variant === "threaded"
    ? new Worker(new URL("./worker-threaded.js", import.meta.url), { type: "module" })
    : new Worker(new URL("./worker-single.js", import.meta.url), { type: "module" });
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
 * Resolves once the latest call's worker has started, or that call has
 * settled, and so has every call before it. A worker has started once its
 * modules are loaded and, in the threaded build, those of all its pool
 * workers too.
 */
let startGate: Promise<void> = Promise.resolve();

/**
 * Queues a call at the start gate: `turn` resolves when the call may start its
 * worker, and `release`, once its worker has started or the call has settled,
 * lets the next call start its own. So the package never has two workers
 * loading at once, which WebKit fails on a page isolated by a service worker
 * (https://bugs.webkit.org/show_bug.cgi?id=261734). A call released before its
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

/** A name no other call or page uses, for the per-call panic channel. */
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

/**
 * Runs `task` in a fresh worker, started once the call's turn at the start
 * gate comes. The first reason to stop wins: an abort, a throwing
 * `onProgress`, a panic, a worker failure, or the worker's outcome.
 */
function runInWorker(
  task: WorkerTask,
  onProgress: ((info: ProgressInfo) => void) | undefined,
  signal: AbortSignal | undefined,
): Promise<WorkerResult> {
  const variant = selectVariant();
  const { turn, release } = queueStart();
  return new Promise<WorkerResult>((resolve, reject) => {
    let settled = false;
    let worker: Worker | undefined;
    let channel: BroadcastChannel | undefined;

    const settle = (finish: () => void): void => {
      if (settled) {
        return;
      }
      settled = true;
      worker?.terminate();
      channel?.close();
      signal?.removeEventListener("abort", onAbort);
      release();
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
          break;
        case "started":
          release();
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
            settle(() => resolve(message.result));
          }
          break;
        case "error":
          fail(errorFromWorker(message.error));
          break;
      }
    };

    // The turn never rejects: a failure is the compile's.
    Promise.all([compiledModule(variant), turn]).then(
      ([module]) => {
        if (settled) {
          return;
        }
        try {
          // Open before the worker starts, so no panic report can be missed.
          const name = channelName();
          channel = new BroadcastChannel(name);
          channel.onmessage = (event: MessageEvent<unknown>) =>
            fail(errorFromWorker({ code: "INTERNAL", message: String(event.data) }));
          worker = startWorker(variant);
          worker.onmessage = (event: MessageEvent<WorkerMessage>) => onMessage(event.data);
          worker.onerror = (event) => {
            event.preventDefault();
            fail(new InternalError("the primeval worker failed to load or crashed"));
          };
          worker.onmessageerror = () =>
            fail(new InternalError("a message from the primeval worker could not be read"));
          const request: WorkerRequest = {
            type: "run",
            module,
            channel: name,
            progress: onProgress !== undefined,
            task,
          };
          const transfer = task.kind === "approximate" ? [task.input.buffer as ArrayBuffer] : [];
          worker.postMessage(request, transfer);
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
 * Internal, for the browser tests only: panics in a fresh worker, on the
 * calling thread or in a rayon pool task, through binding-wasm's hidden
 * `__panicForTests`. Rejects with the `InternalError` a render panic gives.
 *
 * @testonly
 */
export async function panicForTests(site: PanicSite): Promise<never> {
  await runInWorker({ kind: "panic", site }, undefined, undefined);
  throw new InternalError("the test panic did not reject");
}
