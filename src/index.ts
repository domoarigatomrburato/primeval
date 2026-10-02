import { toDataUriWith } from "./data-uri.js";
import { InternalError, mapNativeError } from "./errors.js";
import {
  getNativeBinding,
  type NativeApproximateRequest,
  type NativeApproximateResult,
  type NativeBinding,
  type NativeHandle,
} from "./native-binding.js";
import { abortError, normalizeRequest } from "./request.js";
import type { ApproximateRequest, SvgResult } from "./types.js";

export {
  AbortError,
  type ErrorCode,
  InternalError,
  type OptionName,
  PrimevalError,
  ValidationError,
} from "./errors.js";
export type {
  ApproximateRequest,
  ExecutionOptions,
  OutputFormat,
  ProgressInfo,
  RenderOptions,
  Shape,
  SvgResult,
} from "./types.js";

// --- Public types ---
//
// The shared ones are in types.ts; the PNG result is the Node one.

/** A PNG result. */
export type PngResult = {
  format: "png";
  /** The encoded PNG bytes. */
  data: Buffer;
  mimeType: "image/png";
  /** Output width in pixels. */
  width: number;
  /** Output height in pixels. */
  height: number;
};

/** The result of `approximate()`, discriminated by `format`. */
export type ApproximateResult = SvgResult | PngResult;

// --- Core API ---

function loadNativeBinding(): NativeBinding {
  try {
    return getNativeBinding();
  } catch (error) {
    throw new InternalError(error instanceof Error ? error.message : String(error), {
      cause: error,
    });
  }
}

function toResult(result: NativeApproximateResult): ApproximateResult {
  if (result.format === "svg") {
    return {
      format: "svg",
      data: result.data.toString("utf8"),
      mimeType: "image/svg+xml",
      width: result.width,
      height: result.height,
    };
  }
  return {
    format: "png",
    data: result.data,
    mimeType: "image/png",
    width: result.width,
    height: result.height,
  };
}

// Async, so every failure, including validation and a native load failure,
// is a rejection.
async function run(request: unknown): Promise<ApproximateResult> {
  const { input, output, render, onProgress, signal } = normalizeRequest(request);
  const native: NativeApproximateRequest = {
    // Shares memory with the caller's array; no copy.
    input: Buffer.isBuffer(input)
      ? input
      : Buffer.from(input.buffer, input.byteOffset, input.byteLength),
    output,
    render,
  };
  if (signal?.aborted) {
    throw abortError(signal);
  }
  const binding = loadNativeBinding();

  // The first reason to stop early: a throwing `onProgress` or the signal.
  // It wins over whatever the native promise settles with.
  let stopped: { error: unknown } | undefined;
  let handle: NativeHandle | undefined;
  const stop = (error: unknown): void => {
    if (stopped === undefined) {
      stopped = { error };
      handle?.task.cancel();
    }
  };

  if (onProgress) {
    native.execution = {
      onProgress(_error, info) {
        if (stopped !== undefined) {
          return;
        }
        try {
          onProgress(info);
        } catch (error) {
          stop(error);
        }
      },
    };
  }

  try {
    handle = binding.startApproximate(native);
  } catch (error) {
    throw mapNativeError(error);
  }

  const onAbort = (): void => stop(abortError(signal as AbortSignal));
  signal?.addEventListener("abort", onAbort, { once: true });
  let outcome: { result: NativeApproximateResult } | { error: unknown };
  try {
    outcome = { result: await handle.promise };
  } catch (error) {
    outcome = { error };
  } finally {
    signal?.removeEventListener("abort", onAbort);
  }

  if (signal?.aborted) {
    stop(abortError(signal));
  }
  if (stopped !== undefined) {
    throw stopped.error;
  }
  if ("error" in outcome) {
    throw mapNativeError(outcome.error);
  }
  return toResult(outcome.result);
}

/**
 * Approximates an image with shapes and encodes the result as SVG or PNG.
 *
 * Never throws synchronously: every failure is a rejection with a
 * `PrimevalError` subclass (`ValidationError`, `AbortError`, or
 * `InternalError`), except that a throwing `execution.onProgress` rejects
 * with the value it threw.
 */
export function approximate(request: ApproximateRequest & { output: "svg" }): Promise<SvgResult>;
export function approximate(request: ApproximateRequest & { output: "png" }): Promise<PngResult>;
export function approximate(request: ApproximateRequest): Promise<ApproximateResult>;
export function approximate(request: ApproximateRequest): Promise<ApproximateResult> {
  return run(request);
}

/**
 * Encodes a result as a `data:` URI (base64).
 * @throws {ValidationError} when `result` is missing.
 */
export function toDataUri(result: ApproximateResult): string {
  return toDataUriWith(result, {
    text: (text) => Buffer.from(text, "utf8").toString("base64"),
    bytes: (data) => Buffer.from(data as Uint8Array).toString("base64"),
  });
}
