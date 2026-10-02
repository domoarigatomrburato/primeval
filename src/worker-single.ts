// The per-call worker of the single-threaded build. The glue import is static
// so that bundlers bundle it with its snippets and `.wasm` asset.
import * as glueModule from "../wasm/single/primeval.js";
import { type Glue, runRequest, scope } from "./worker-common.js";
import type { WorkerRequest } from "./worker-protocol.js";

const glue: Glue = glueModule;

scope.onmessage = ({ data }: MessageEvent<WorkerRequest>) => {
  // One task per worker.
  scope.onmessage = null;
  void runRequest(glue, data);
};
