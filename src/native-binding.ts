import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

// Keep the generated binding boundary here so the public wrapper can stay focused
// on validation, normalization, and error mapping.
export interface NativeApproximateResult {
  format: string;
  data: Buffer | Uint8Array;
  mimeType: string;
  width: number;
  height: number;
}

export interface NativeProgressInfo {
  step: number;
  total: number;
  score: number;
}

export interface NativeHandle {
  promise: Promise<NativeApproximateResult>;
  taskId: number;
}

export interface NativeRenderOptions {
  count?: number;
  shape?: string;
  alpha?: number | string;
  seed?: number;
  background?: string;
  resizeInput?: number;
  outputSize?: number;
}

export interface NativeExecutionOptions {
  onProgress?: (error: Error | null, info: NativeProgressInfo | null) => void;
}

export interface NativeApproximateRequest {
  input: Buffer;
  output: string;
  render: NativeRenderOptions;
  execution?: NativeExecutionOptions;
}

export interface NativeBinding {
  startApproximate(request: NativeApproximateRequest): NativeHandle;
  cancelApproximate(taskId: number): void;
}

let nativeBinding: NativeBinding | undefined;

export function getNativeBinding(): NativeBinding {
  nativeBinding ??= require("../binding.js") as NativeBinding;
  return nativeBinding;
}
