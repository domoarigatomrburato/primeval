import {
  AbortError,
  InternalError,
  invalidOption,
  mapNativeError,
  type OptionName,
  ValidationError,
} from "./errors.js";
import {
  getNativeBinding,
  type NativeApproximateRequest,
  type NativeApproximateResult,
  type NativeBinding,
  type NativeHandle,
  type NativeRenderOptions,
} from "./native-binding.js";

export {
  AbortError,
  type ErrorCode,
  InternalError,
  type OptionName,
  PrimevalError,
  ValidationError,
} from "./errors.js";

// --- Public types ---

/** The encoded output format: `"svg"` or `"png"`. */
export type OutputFormat = "svg" | "png";

/**
 * The shape family each step searches. `"any"` mixes all of them; `"quadratic"`
 * is a quadratic Bezier stroke and `"polygon"` a small convex polygon.
 */
export type Shape =
  | "any"
  | "triangle"
  | "rectangle"
  | "ellipse"
  | "circle"
  | "rotated-rectangle"
  | "quadratic"
  | "rotated-ellipse"
  | "polygon";

/**
 * Render options. An omitted field uses the Rust default; `null` is rejected
 * like any other value of the wrong type.
 */
export type RenderOptions = {
  /**
   * Optimization steps (shapes added), an integer `1..100000`. Higher values
   * improve quality and take longer.
   * @default 100
   */
  count?: number;
  /**
   * The shape family to search.
   * @default "any"
   */
  shape?: Shape;
  /**
   * Shape opacity: `"auto"` lets the optimizer choose each shape's opacity;
   * a number fixes it, an integer `1..255`.
   * @default "auto"
   */
  alpha?: "auto" | number;
  /**
   * Deterministic seed, an integer `0..2^64 - 1`. A `number` seed must be a
   * safe integer; pass larger seeds as a `bigint`. Omit it for a
   * non-deterministic seed.
   */
  seed?: number | bigint;
  /**
   * Opaque background color: `"auto"` (the alpha-weighted mean color of the
   * input, or white for a fully transparent input), or a hex color `RGB` or
   * `RRGGBB` with an optional leading `#`.
   * @default "auto"
   */
  background?: "auto" | (string & {});
  /**
   * Working resolution used during optimization (longest side), an integer
   * `2..2048`. Smaller values run faster but capture less detail.
   * @default 256
   */
  resizeInput?: number;
  /**
   * Longest side of the output image, an integer `2..8192`.
   * @default 1024
   */
  outputSize?: number;
};

/** Progress after one optimization step. */
export type ProgressInfo = {
  /** The step just completed, `1..total`. */
  step: number;
  /** The total number of steps, equal to the `count` option. */
  total: number;
  /** The current RMSE fit; lower is better. */
  score: number;
};

/** Progress and cancellation controls. */
export type ExecutionOptions = {
  /**
   * Called after each step. If it throws, the render is cancelled, no further
   * calls are made, and `approximate()` rejects with the thrown value.
   */
  onProgress?: (info: ProgressInfo) => void;
  /**
   * Cancels the render. Once the signal is aborted, `approximate()` rejects
   * with an `AbortError` whose `cause` is `signal.reason`, even if the render
   * had already finished.
   */
  signal?: AbortSignal;
};

/** The request `approximate()` takes. */
export type ApproximateRequest = {
  /** Encoded image bytes (JPEG, PNG, or WebP). A `Buffer` is a `Uint8Array`. */
  input: Uint8Array;
  /** The output format; it decides the result type. */
  output: OutputFormat;
  /** Render options; omitted fields use the Rust defaults. */
  render?: RenderOptions;
  /** Progress and cancellation controls. */
  execution?: ExecutionOptions;
};

/** An SVG result. */
export type SvgResult = {
  format: "svg";
  /** The SVG document. */
  data: string;
  mimeType: "image/svg+xml";
  /** Output width in pixels. */
  width: number;
  /** Output height in pixels. */
  height: number;
};

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

// --- Normalization ---
//
// Type checks only: Rust owns the ranges and vocabularies. Only `undefined`
// means omitted.

type Fields = Record<string, unknown>;

interface NormalizedRequest {
  native: NativeApproximateRequest;
  onProgress?: (info: ProgressInfo) => void;
  signal?: AbortSignal;
}

function optionalObject(value: unknown, name: string): Fields {
  if (value === undefined) {
    return {};
  }
  if (value === null || typeof value !== "object") {
    throw new ValidationError(`${name} must be an object`);
  }
  return value as Fields;
}

function normalizeInput(input: unknown): Buffer {
  if (Buffer.isBuffer(input)) {
    return input;
  }
  if (input instanceof Uint8Array) {
    // Shares memory with the caller's array; no copy.
    return Buffer.from(input.buffer, input.byteOffset, input.byteLength);
  }
  throw new ValidationError("input must be a Uint8Array");
}

function field<T>(
  fields: Fields,
  name: OptionName,
  accepts: (value: unknown) => value is T,
  requirement: string,
): T | undefined {
  const value = fields[name];
  if (value === undefined) {
    return undefined;
  }
  if (!accepts(value)) {
    throw invalidOption(name, requirement);
  }
  return value;
}

const isNumber = (value: unknown): value is number => typeof value === "number";
const isString = (value: unknown): value is string => typeof value === "string";
const isSeed = (value: unknown): value is number | bigint =>
  typeof value === "number" || typeof value === "bigint";

function normalizeRender(render: Fields): NativeRenderOptions {
  const values: NativeRenderOptions = {
    count: field(render, "count", isNumber, "must be a number"),
    shape: field(render, "shape", isString, "must be a string"),
    // The type check shares Rust's wording; Rust checks the value.
    alpha: field(
      render,
      "alpha",
      (value) => isNumber(value) || isString(value),
      "must be auto or an integer 1..255",
    ),
    seed: field(render, "seed", isSeed, "must be a number or a bigint"),
    background: field(render, "background", isString, "must be a string"),
    resizeInput: field(render, "resizeInput", isNumber, "must be a number"),
    outputSize: field(render, "outputSize", isNumber, "must be a number"),
  };
  // Omitted fields stay absent so Rust applies its defaults.
  return Object.fromEntries(
    Object.entries(values).filter(([, value]) => value !== undefined),
  ) as NativeRenderOptions;
}

function normalizeRequest(request: unknown): NormalizedRequest {
  if (request === null || typeof request !== "object") {
    throw new ValidationError("request must be an object");
  }
  const fields = request as Fields;
  const input = normalizeInput(fields.input);
  const output = fields.output;
  if (!isString(output)) {
    throw invalidOption("output", "must be a string");
  }
  const render = normalizeRender(optionalObject(fields.render, "render"));

  const execution = optionalObject(fields.execution, "execution");
  const { onProgress, signal } = execution;
  if (onProgress !== undefined && typeof onProgress !== "function") {
    throw new ValidationError("execution.onProgress must be a function");
  }
  if (signal !== undefined && !(signal instanceof AbortSignal)) {
    throw new ValidationError("execution.signal must be an AbortSignal");
  }

  return {
    native: { input, output, render },
    onProgress: onProgress as ((info: ProgressInfo) => void) | undefined,
    signal,
  };
}

// --- Core API ---

function abortError(signal: AbortSignal): AbortError {
  return new AbortError("render aborted", { cause: signal.reason });
}

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
  const { native, onProgress, signal } = normalizeRequest(request);
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
  if (!result || typeof result !== "object") {
    throw new ValidationError("result is required");
  }

  const base64 =
    result.format === "svg"
      ? Buffer.from(
          typeof result.data === "string" ? result.data : String(result.data),
          "utf8",
        ).toString("base64")
      : Buffer.from(result.data).toString("base64");

  return `data:${result.mimeType};base64,${base64}`;
}
