// The messages between the browser runtime (browser-runtime.ts), its per-call
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

/** The one message the page sends a worker. */
export interface WorkerRequest {
  type: "run";
  module: WebAssembly.Module;
  /** The page's per-call `BroadcastChannel` for panics on pool threads. */
  channel: string;
  /** Whether to forward progress. */
  progress: boolean;
  task: WorkerTask;
}

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

export type WorkerMessage =
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

/** What a pool worker posts once, before it blocks in its rayon thread. */
export type PoolWorkerMessage = { type: "ready" } | { type: "failed"; message: string };
