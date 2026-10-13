// The calling worker of the single-threaded build. The glue import is static
// so that bundlers bundle it with its snippets and `.wasm` asset.
import * as glueModule from "../wasm/single/primeval.js";
import { type Glue, requestRunner, scope } from "./worker-common.js";
import type { WorkerMessage, WorkerRequest } from "./worker-protocol.js";

const glue: Glue = glueModule;

const run = requestRunner(glue);

scope.onmessage = ({ data }: MessageEvent<WorkerRequest>) => {
  void run(data);
};

// Its modules are loaded and it starts no other worker: the page can start the
// next call's worker.
scope.postMessage({ type: "started" } satisfies WorkerMessage);
