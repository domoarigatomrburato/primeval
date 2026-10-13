// The messages between the browser runtime (browser-runtime.ts), its calling
// worker (worker-single.ts or worker-threaded.ts) and, in the threaded build,
// that worker's pool workers (worker-threaded.ts again). Types only.
import type { RequestRenderOptions } from "./request.js";
import type { ProgressInfo } from "./types.js";

/** `threaded` on a cross-origin isolated page, `single` otherwise. */
export type WasmVariant = "single" | "threaded";

/** What one worker runs: a render, or (internal) a test panic. */
export type WorkerTask =
  | { kind: "approximate"; input: Uint8Array; output: string; render: RequestRenderOptions }
  | { kind: "panic"; site: PanicSite };

/** Where the internal test panic happens. */
export type PanicSite = "caller" | "pool";

/** A task sent to an already initialized calling worker. */
interface WorkerRun {
  type: "run";
  progress: boolean;
  task: WorkerTask;
}

/** The first task also initializes the instance, panic hook and thread pool. */
export type WorkerRequest =
  | WorkerRun
  | {
      type: "init";
      module: WebAssembly.Module;
      /** The page's worker-lifetime `BroadcastChannel` for pool panics. */
      channel: string;
      progress: boolean;
      task: WorkerTask;
    };

/**
 * A Rust error as plain data, because structured clone drops an error's own
 * properties. `code` is a stable error code; an invalid option also has
 * `option` and `requirement`.
 */
export interface WorkerError {
  code: string;
  message: string;
  option?: string;
  requirement?: string;
}

/** The bytes and metadata of a finished render; `data` is transferred. */
export interface WorkerResult {
  format: string;
  mimeType: string;
  width: number;
  height: number;
  data: Uint8Array;
}

/**
 * What every worker of the threaded build posts as soon as its modules are
 * loaded, before it knows its role. A pool worker's caller waits for it before
 * starting the next pool worker; the page ignores it.
 */
export interface LoadedMessage {
  type: "loaded";
}

export type WorkerMessage =
  | LoadedMessage
  /**
   * The worker no longer loads anything: its modules are loaded and, in the
   * threaded build, so are all its pool workers. The page then lets the next
   * call start its worker.
   */
  | { type: "started" }
  | { type: "progress"; info: ProgressInfo }
  | { type: "result"; result: WorkerResult }
  /** Also posted by the panic hook, before the trap, for a panic on the worker's own thread. */
  | { type: "error"; error: WorkerError };

/** The one message a threaded worker sends each of its pool workers. */
export interface PoolWorkerInit {
  type: "pool";
  /** The page's compiled module. */
  module: WebAssembly.Module;
  /** The calling worker's shared memory. */
  memory: WebAssembly.Memory;
  /** The pool channel's receiver, a pointer into `memory`. */
  receiver: number;
}

/**
 * What a pool worker posts: `loaded` once its modules are loaded, then
 * `ready` or `failed` once, before it blocks in its rayon thread.
 */
export type PoolWorkerMessage =
  | LoadedMessage
  | { type: "ready" }
  | { type: "failed"; message: string };
