import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

// Keep the generated binding boundary here so the public wrapper can stay focused
// on type checks and error mapping. test/types/native-binding.ts checks these
// types against the generated binding.d.ts.
export interface NativeApproximateResult {
  format: string;
  data: Buffer;
  mimeType: string;
  width: number;
  height: number;
}

export interface NativeProgressInfo {
  step: number;
  total: number;
  score: number;
}

/** Cancels one running render. */
export interface NativeTask {
  cancel(): void;
}

export interface NativeHandle {
  promise: Promise<NativeApproximateResult>;
  task: NativeTask;
}

// Native errors carry `code` from Rust's `ApproximateError::code` and, for
// `INVALID_OPTION`, `option` (the Node name) and `requirement`; see
// `mapNativeError`.

export interface NativeRenderOptions {
  count?: number;
  shape?: string;
  alpha?: number | string;
  seed?: number | bigint;
  background?: string;
  resizeInput?: number;
  outputSize?: number;
}

export interface NativeExecutionOptions {
  onProgress?: (error: Error | null, info: NativeProgressInfo) => void;
}

export interface NativeApproximateRequest {
  input: Buffer;
  output: string;
  render: NativeRenderOptions;
  execution?: NativeExecutionOptions;
}

export interface NativeBinding {
  startApproximate(request: NativeApproximateRequest): NativeHandle;
}

let nativeBinding: NativeBinding | undefined;

export function getNativeBinding(): NativeBinding {
  nativeBinding ??= require("../binding.js") as NativeBinding;
  return nativeBinding;
}
