// The per-call worker of the threaded build, and also each of its rayon pool
// workers: the per-call worker starts its pool workers from this same file,
// and the first message tells the two roles apart. The glue import is static
// so that bundlers bundle it with its snippets and `.wasm` asset.
import * as glueModule from "../wasm/threaded/primeval.js";
import { messageOf } from "./errors.js";
import { runRequest, scope, type ThreadedGlue } from "./worker-common.js";
import type { PoolWorkerInit, PoolWorkerMessage, WorkerRequest } from "./worker-protocol.js";

const glue: ThreadedGlue = glueModule;

// Kept for the life of this worker. The builder owns the channel whose
// receiver the pool workers block on, so it must outlive them. The `Worker`
// objects keep Firefox from collecting workers that share the memory but are
// not otherwise rooted (https://bugzilla.mozilla.org/show_bug.cgi?id=1702191).
let builder: InstanceType<ThreadedGlue["PoolBuilder"]> | undefined;
const poolWorkers: Worker[] = [];

/** Starts one pool worker; resolves once it is about to run its rayon thread. */
function spawnPoolWorker(init: PoolWorkerInit): Promise<void> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./worker-threaded.js", import.meta.url), {
      type: "module",
    });
    poolWorkers.push(worker);
    worker.onmessage = ({ data }: MessageEvent<PoolWorkerMessage>) => {
      if (data.type === "ready") {
        resolve();
      } else {
        reject(new Error(data.message));
      }
    };
    worker.onerror = (event) => {
      event.preventDefault();
      reject(new Error("a pool worker failed to load"));
    };
    worker.postMessage(init);
  });
}

/** The rayon pool, one thread per logical core, each on a pool worker. */
async function startPool(module: WebAssembly.Module): Promise<void> {
  builder = new glue.PoolBuilder(navigator.hardwareConcurrency);
  const init: PoolWorkerInit = {
    type: "pool",
    module,
    memory: glue.wasmMemory(),
    receiver: builder.receiver(),
  };
  await Promise.all(Array.from({ length: builder.numThreads }, () => spawnPoolWorker(init)));
  builder.build();
}

/** A pool worker: joins the calling worker's instance and runs one rayon thread. */
async function runPoolWorker({ module, memory, receiver }: PoolWorkerInit): Promise<void> {
  try {
    await glue.default({ module_or_path: module, memory });
  } catch (error) {
    scope.postMessage({ type: "failed", message: messageOf(error) } satisfies PoolWorkerMessage);
    return;
  }
  scope.postMessage({ type: "ready" } satisfies PoolWorkerMessage);
  // Blocks for the life of the pool. A panic here reaches the page on the
  // call's BroadcastChannel: this thread has no reporter of its own.
  glue.startPoolWorker(receiver);
}

scope.onmessage = ({ data }: MessageEvent<WorkerRequest | PoolWorkerInit>) => {
  // One task per worker.
  scope.onmessage = null;
  if (data.type === "pool") {
    void runPoolWorker(data);
  } else {
    void runRequest(glue, data, startPool);
  }
};
