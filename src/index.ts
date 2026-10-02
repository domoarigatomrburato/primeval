import { mapNativeError, ValidationError } from "./errors.js";
import {
  getNativeBinding,
  type NativeApproximateRequest,
  type NativeHandle,
  type NativeProgressInfo,
} from "./native-binding.js";

export {
  AbortError,
  type ErrorCode,
  InternalError,
  PrimevalError,
  ValidationError,
} from "./errors.js";

// --- Public types ---

export type OutputFormat = "svg" | "png";

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

export type RenderOptions = {
  /** Optimization steps, an integer `1..100000`. */
  count?: number;
  shape?: Shape;
  /** `"auto"`, or a fixed shape opacity as an integer `1..255`. */
  alpha?: "auto" | number;
  /**
   * Deterministic seed, `0..2^64 - 1`. Pass seeds above
   * `Number.MAX_SAFE_INTEGER` as a `bigint`.
   */
  seed?: number | bigint;
  background?: "auto" | string;
  /** Working resolution, an integer `2..2048`. */
  resizeInput?: number;
  /** Longest side of the output, an integer `2..8192`. */
  outputSize?: number;
};

export type ProgressInfo = {
  step: number;
  total: number;
  score: number;
};

export type ExecutionOptions = {
  onProgress?: (info: ProgressInfo) => void;
  signal?: AbortSignal;
};

export type ApproximateRequest = {
  /** Encoded image bytes (JPEG, PNG, or WebP). A `Buffer` is a `Uint8Array`. */
  input: Uint8Array;
  output: OutputFormat;
  render?: RenderOptions;
  execution?: ExecutionOptions;
};

export type SvgResult = {
  format: "svg";
  data: string;
  mimeType: "image/svg+xml";
  width: number;
  height: number;
};

export type RasterResult = {
  format: "png";
  data: Buffer;
  mimeType: "image/png";
  width: number;
  height: number;
};

export type ApproximateResult = SvgResult | RasterResult;

// --- Internal types ---

const VALID_SHAPES: readonly Shape[] = [
  "any",
  "triangle",
  "rectangle",
  "ellipse",
  "circle",
  "rotated-rectangle",
  "quadratic",
  "rotated-ellipse",
  "polygon",
];

const VALID_OUTPUTS: readonly OutputFormat[] = ["svg", "png"];

interface NormalizedRender {
  count?: number;
  shape?: Shape;
  alpha?: "auto" | number;
  seed?: number | bigint;
  background?: string;
  resizeInput?: number;
  outputSize?: number;
}

interface NormalizedRequest {
  input: Buffer;
  output: OutputFormat;
  render: NormalizedRender;
  execution: {
    onProgress?: (info: ProgressInfo) => void;
    signal?: AbortSignal;
  };
}

// --- Normalization ---

function isAbortSignal(value: unknown): value is AbortSignal {
  return value instanceof AbortSignal;
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

// Type checks only: Rust owns the integer ranges and the alpha and
// background vocabularies.
function optionalNumber(r: Record<string, unknown>, name: string): number | undefined {
  const value = r[name];
  if (value == null) {
    return undefined;
  }
  if (typeof value !== "number") {
    throw new ValidationError(`${name} must be a number`);
  }
  return value;
}

function normalizeRender(render?: Record<string, unknown>): NormalizedRender {
  const r = render ?? {};
  const count = optionalNumber(r, "count");
  const resizeInput = optionalNumber(r, "resizeInput");
  const outputSize = optionalNumber(r, "outputSize");
  const shape = r.shape == null ? undefined : (r.shape as Shape);
  const alpha = r.alpha == null ? undefined : r.alpha;
  const background = r.background == null ? undefined : (r.background as string);
  const seed = r.seed == null ? undefined : r.seed;

  if (shape !== undefined && !(VALID_SHAPES as readonly string[]).includes(shape)) {
    throw new ValidationError(`shape must be one of: ${VALID_SHAPES.join(", ")}`);
  }
  if (alpha !== undefined && typeof alpha !== "string" && typeof alpha !== "number") {
    throw new ValidationError("alpha must be auto or an integer 1..255");
  }
  if (seed !== undefined && typeof seed !== "number" && typeof seed !== "bigint") {
    throw new ValidationError("seed must be a number or a bigint");
  }

  return {
    count,
    shape,
    alpha: alpha as NormalizedRender["alpha"],
    seed: seed as NormalizedRender["seed"],
    background,
    resizeInput,
    outputSize,
  };
}

function normalizeRequest(request: unknown): NormalizedRequest {
  if (!request || typeof request !== "object") {
    throw new ValidationError("request is required");
  }
  const req = request as Record<string, unknown>;

  const input = normalizeInput(req.input);
  const output = req.output as string;
  if (!(VALID_OUTPUTS as readonly string[]).includes(output)) {
    throw new ValidationError(`output must be one of: ${VALID_OUTPUTS.join(", ")}`);
  }

  const execution = (req.execution ?? {}) as Record<string, unknown>;
  if (execution.onProgress !== undefined && typeof execution.onProgress !== "function") {
    throw new ValidationError("execution.onProgress must be a function");
  }
  if (execution.signal !== undefined && !isAbortSignal(execution.signal)) {
    throw new ValidationError("execution.signal must be an AbortSignal");
  }

  return {
    input,
    output: output as OutputFormat,
    render: normalizeRender(req.render as Record<string, unknown> | undefined),
    execution: {
      onProgress: execution.onProgress as ((info: ProgressInfo) => void) | undefined,
      signal: execution.signal as AbortSignal | undefined,
    },
  };
}

// --- Core API ---

function startApproximate(request: ApproximateRequest): {
  promise: Promise<ApproximateResult>;
  cancel: () => void;
} {
  const normalized = normalizeRequest(request);
  const nativeBinding = getNativeBinding();
  const onProgress =
    normalized.execution.onProgress &&
    ((_: unknown, info: NativeProgressInfo | null): void => {
      if (info) {
        normalized.execution.onProgress?.(info);
      }
    });
  const nativeRequest: NativeApproximateRequest = {
    input: normalized.input,
    output: normalized.output,
    render: {
      ...(normalized.render.count === undefined ? {} : { count: normalized.render.count }),
      ...(normalized.render.shape === undefined ? {} : { shape: normalized.render.shape }),
      ...(normalized.render.alpha === undefined ? {} : { alpha: normalized.render.alpha }),
      ...(normalized.render.seed === undefined ? {} : { seed: normalized.render.seed }),
      ...(normalized.render.background === undefined
        ? {}
        : { background: normalized.render.background }),
      ...(normalized.render.resizeInput === undefined
        ? {}
        : { resizeInput: normalized.render.resizeInput }),
      ...(normalized.render.outputSize === undefined
        ? {}
        : { outputSize: normalized.render.outputSize }),
    },
    execution: onProgress ? { onProgress } : undefined,
  };
  let handle: NativeHandle;
  try {
    handle = nativeBinding.startApproximate(nativeRequest);
  } catch (error) {
    // Request validation in Rust (e.g. `background`) fails synchronously.
    throw mapNativeError(error);
  }

  const cancel = (): void => handle.task.cancel();
  const signal = normalized.execution.signal;
  let onAbort: (() => void) | undefined;
  if (signal) {
    onAbort = () => cancel();
    if (signal.aborted) {
      onAbort();
    } else {
      signal.addEventListener("abort", onAbort, { once: true });
    }
  }

  const promise = handle.promise
    .then((result): ApproximateResult => {
      if (result.format === "svg") {
        return {
          format: "svg",
          data: Buffer.isBuffer(result.data) ? result.data.toString("utf8") : String(result.data),
          mimeType: result.mimeType as "image/svg+xml",
          width: result.width,
          height: result.height,
        };
      }

      return {
        format: result.format as "png",
        data: Buffer.isBuffer(result.data) ? result.data : Buffer.from(result.data),
        mimeType: result.mimeType as "image/png",
        width: result.width,
        height: result.height,
      };
    })
    .catch((error: unknown) => {
      throw mapNativeError(error);
    })
    .finally(() => {
      if (signal && onAbort) signal.removeEventListener("abort", onAbort);
    });

  return { promise, cancel };
}

export function approximate(request: ApproximateRequest): Promise<ApproximateResult> {
  return startApproximate(request).promise;
}

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
