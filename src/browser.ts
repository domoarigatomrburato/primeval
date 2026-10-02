// The browser entry: the same `approximate()` and `toDataUri()` as the Node
// entry (index.ts), running the WebAssembly builds in a Web Worker per call
// (browser-runtime.ts). No Node APIs: a PNG result's `data` is a `Uint8Array`.
import { approximateInWorker } from "./browser-runtime.js";
import { toDataUriWith } from "./data-uri.js";
import type { ApproximateRequest, SvgResult } from "./types.js";
import type { WorkerResult } from "./worker-protocol.js";

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

/** A PNG result. */
export type PngResult = {
  format: "png";
  /** The encoded PNG bytes. */
  data: Uint8Array;
  mimeType: "image/png";
  /** Output width in pixels. */
  width: number;
  /** Output height in pixels. */
  height: number;
};

/** The result of `approximate()`, discriminated by `format`. */
export type ApproximateResult = SvgResult | PngResult;

function toResult(result: WorkerResult): ApproximateResult {
  if (result.format === "svg") {
    return {
      format: "svg",
      data: new TextDecoder().decode(result.data),
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

async function run(request: unknown): Promise<ApproximateResult> {
  return toResult(await approximateInWorker(request));
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

const BASE64_CHUNK = 0x8000;

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += BASE64_CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + BASE64_CHUNK));
  }
  return btoa(binary);
}

/**
 * Encodes a result as a `data:` URI (base64).
 * @throws {ValidationError} when `result` is missing.
 */
export function toDataUri(result: ApproximateResult): string {
  return toDataUriWith(result, {
    text: (text) => bytesToBase64(new TextEncoder().encode(text)),
    bytes: (data) =>
      bytesToBase64(data instanceof Uint8Array ? data : Uint8Array.from(data as ArrayLike<number>)),
  });
}
