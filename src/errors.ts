import type { NativeErrorCode } from "./native-binding.js";

/** Stable error codes, also set as `code` on every `PrimevalError`. */
export type ErrorCode = NativeErrorCode;

/** Base class of every error this package throws or rejects with. */
export class PrimevalError extends Error {
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
  constructor(
    message: string,
    options?: { code?: "INVALID_OPTION" | "INVALID_IMAGE"; cause?: unknown },
  ) {
    super("ValidationError", options?.code ?? "INVALID_OPTION", message, options);
  }
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
 * A failure valid input should not cause (`INTERNAL`): an encoder or
 * allocation failure, or a caught native panic.
 */
export class InternalError extends PrimevalError {
  declare name: "InternalError";
  declare readonly code: "INTERNAL";
  constructor(message: string, options?: { cause?: unknown }) {
    super("InternalError", "INTERNAL", message, options);
  }
}

// Maps on the native `code` only, never on the message text. Errors without
// a known code (e.g. napi argument conversion failures) pass through unchanged.
export function mapNativeError(error: unknown): unknown {
  if (!(error instanceof Error)) {
    return error;
  }
  const code = (error as { code?: unknown }).code;
  const options = { cause: error };
  switch (code) {
    case "INVALID_OPTION":
    case "INVALID_IMAGE":
      return new ValidationError(error.message, { ...options, code });
    case "ABORTED":
      return new AbortError(error.message, options);
    case "INTERNAL":
      return new InternalError(error.message, options);
    default:
      return error;
  }
}
