/**
 * Stable error codes, also set as `code` on every `PrimevalError`:
 * `INVALID_OPTION`, `INVALID_IMAGE`, `ABORTED`, or `INTERNAL`.
 */
export type ErrorCode = "INVALID_OPTION" | "INVALID_IMAGE" | "ABORTED" | "INTERNAL";

/**
 * A request field that Rust identifies by name in an `INVALID_OPTION` error,
 * in its Node spelling: `output` or one of the `render` options.
 */
export type OptionName =
  | "output"
  | "count"
  | "shape"
  | "alpha"
  | "seed"
  | "background"
  | "resizeInput"
  | "outputSize";

/** Base class of every error `approximate()` rejects with or `toDataUri()` throws. */
export class PrimevalError extends Error {
  /** The stable error code; branch on it or on the subclass, not on the message. */
  readonly code: ErrorCode;

  constructor(name: string, code: ErrorCode, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = name;
    this.code = code;
  }
}

/** An invalid option (`INVALID_OPTION`) or unusable input image (`INVALID_IMAGE`). */
export class ValidationError extends PrimevalError {
  declare name: "ValidationError";
  declare readonly code: "INVALID_OPTION" | "INVALID_IMAGE";
  /**
   * The invalid field, when it is `output` or a `render` option. The message
   * is then `` `${option} ${requirement}` ``.
   */
  readonly option?: OptionName;
  /** What `option` accepts, e.g. `"must be an integer from 2 to 2048"`. */
  readonly requirement?: string;

  constructor(
    message: string,
    options?: {
      code?: "INVALID_OPTION" | "INVALID_IMAGE";
      cause?: unknown;
      option?: OptionName;
      requirement?: string;
    },
  ) {
    super("ValidationError", options?.code ?? "INVALID_OPTION", message, options);
    if (options?.option !== undefined) {
      this.option = options.option;
      this.requirement = options.requirement;
    }
  }
}

/** The error for an invalid value of `option`: `` `${option} ${requirement}` ``. */
export function invalidOption(option: OptionName, requirement: string): ValidationError {
  return new ValidationError(`${option} ${requirement}`, { option, requirement });
}

/** The render was cancelled through `execution.signal` (`ABORTED`). */
export class AbortError extends PrimevalError {
  declare name: "AbortError";
  declare readonly code: "ABORTED";
  constructor(message: string, options?: { cause?: unknown }) {
    super("AbortError", "ABORTED", message, options);
  }
}

/**
 * A failure valid input should not cause (`INTERNAL`): a native addon that
 * fails to load, an encoder or allocation failure, or a caught native panic.
 */
export class InternalError extends PrimevalError {
  declare name: "InternalError";
  declare readonly code: "INTERNAL";
  constructor(message: string, options?: { cause?: unknown }) {
    super("InternalError", "INTERNAL", message, options);
  }
}

// Maps on the native `code` only, never on the message text. Errors without
// a known code pass through unchanged.
export function mapNativeError(error: unknown): unknown {
  if (!(error instanceof Error)) {
    return error;
  }
  const native = error as { code?: unknown; option?: unknown; requirement?: unknown };
  const options = { cause: error };
  switch (native.code) {
    case "INVALID_OPTION":
    case "INVALID_IMAGE":
      return new ValidationError(error.message, {
        ...options,
        code: native.code,
        ...(typeof native.option === "string"
          ? { option: native.option as OptionName, requirement: String(native.requirement) }
          : {}),
      });
    case "ABORTED":
      return new AbortError(error.message, options);
    case "INTERNAL":
      return new InternalError(error.message, options);
    default:
      return error;
  }
}
