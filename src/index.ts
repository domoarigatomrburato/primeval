import {
  getNativeBinding,
  type NativeApproximateRequest,
  type NativeProgressInfo,
} from "./native-binding.js";

// --- Public types ---

export type InputSource =
  | { kind: "path"; path: string }
  | { kind: "bytes"; data: Buffer | Uint8Array };

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
  count?: number;
  shape?: Shape;
  alpha?: number;
  repeat?: number;
  seed?: number;
  background?: "auto" | string;
  resizeInput?: number;
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
  input: InputSource;
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

// --- Error classes ---

class PrimevalError extends Error {
  constructor(name: string, message: string) {
    super(message);
    this.name = name;
  }
}

export class ValidationError extends PrimevalError {
  declare name: "ValidationError";
  constructor(message: string) {
    super("ValidationError", message);
  }
}

export class NotFoundError extends PrimevalError {
  declare name: "NotFoundError";
  constructor(message: string) {
    super("NotFoundError", message);
  }
}

export class AbortError extends PrimevalError {
  declare name: "AbortError";
  constructor(message: string) {
    super("AbortError", message);
  }
}

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
  alpha?: number;
  repeat?: number;
  seed?: number;
  background?: string;
  resizeInput?: number;
  outputSize?: number;
}

interface NormalizedInput {
  kind: "path" | "bytes";
  path?: string;
  data?: Buffer;
}

interface NormalizedRequest {
  input: NormalizedInput;
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

function normalizeInput(input: unknown): NormalizedInput {
  if (!input || typeof input !== "object") {
    throw new ValidationError("input is required");
  }
  const inp = input as Record<string, unknown>;

  if (inp.kind === "path") {
    if (typeof inp.path !== "string" || inp.path.length === 0) {
      throw new ValidationError("path input requires a path");
    }
    return { kind: "path", path: inp.path };
  }

  if (inp.kind === "bytes") {
    if (Buffer.isBuffer(inp.data)) {
      return { kind: "bytes", data: inp.data };
    }
    if (inp.data instanceof Uint8Array) {
      return { kind: "bytes", data: Buffer.from(inp.data) };
    }
    throw new ValidationError("bytes input requires data");
  }

  throw new ValidationError(`unknown input kind: ${String(inp.kind)}`);
}

function normalizeRender(render?: Record<string, unknown>): NormalizedRender {
  const r = render ?? {};
  const count = r.count == null ? undefined : (r.count as number);
  const shape = r.shape == null ? undefined : (r.shape as Shape);
  const alpha = r.alpha == null ? undefined : (r.alpha as number);
  const repeat = r.repeat == null ? undefined : (r.repeat as number);
  const background = r.background == null ? undefined : (r.background as string);
  const resizeInput = r.resizeInput == null ? undefined : (r.resizeInput as number);
  const outputSize = r.outputSize == null ? undefined : (r.outputSize as number);
  const seed = r.seed == null ? undefined : (r.seed as number);

  if (count !== undefined && (!Number.isInteger(count) || count < 1)) {
    throw new ValidationError("count must be at least 1");
  }
  if (shape !== undefined && !(VALID_SHAPES as readonly string[]).includes(shape)) {
    throw new ValidationError(`unknown shape: ${shape}`);
  }
  if (alpha !== undefined && (!Number.isInteger(alpha) || alpha < 0 || alpha > 255)) {
    throw new ValidationError("alpha must be 0..255 where 0 means auto");
  }
  if (repeat !== undefined && (!Number.isInteger(repeat) || repeat < 0)) {
    throw new ValidationError("repeat must be at least 0");
  }
  if (seed !== undefined && (!Number.isInteger(seed) || seed < 0)) {
    throw new ValidationError("seed must be a positive integer");
  }
  if (resizeInput !== undefined && (!Number.isInteger(resizeInput) || resizeInput < 1)) {
    throw new ValidationError("resizeInput must be at least 1");
  }
  if (outputSize !== undefined && (!Number.isInteger(outputSize) || outputSize < 1)) {
    throw new ValidationError("outputSize must be at least 1");
  }

  return { count, shape, alpha, repeat, seed, background, resizeInput, outputSize };
}

function normalizeRequest(request: unknown): NormalizedRequest {
  if (!request || typeof request !== "object") {
    throw new ValidationError("request is required");
  }
  const req = request as Record<string, unknown>;

  const input = normalizeInput(req.input);
  const output = req.output as string;
  if (!(VALID_OUTPUTS as readonly string[]).includes(output)) {
    throw new ValidationError(`unknown output format: ${String(output)}`);
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

// --- Error mapping ---

function mapNativeError(error: unknown): Error {
  const message = error instanceof Error ? error.message : String(error);
  const nameMatch = message.match(/^\[([^\]]+)\]\s*(.*)$/);
  if (!nameMatch) {
    return new Error(message);
  }

  const [, name, detail] = nameMatch;
  switch (name) {
    case "ValidationError":
      return new ValidationError(detail);
    case "NotFoundError":
      return new NotFoundError(detail);
    case "AbortError":
      return new AbortError(detail);
    default:
      return new Error(detail);
  }
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
      ...(normalized.render.alpha === undefined ? {} : { alpha: String(normalized.render.alpha) }),
      ...(normalized.render.repeat === undefined ? {} : { repeat: normalized.render.repeat }),
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
  const handle = nativeBinding.startApproximate(nativeRequest);

  const cancel = (): void => nativeBinding.cancelApproximate(handle.taskId);
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
