// `toDataUri` shared by the Node and browser entries; each passes its own
// base64 encoders.
import { ValidationError } from "./errors.js";

export interface Base64Encoders {
  /** Base64 of the UTF-8 bytes of `text`. */
  text(text: string): string;
  /** Base64 of PNG result `data`. */
  bytes(data: unknown): string;
}

export function toDataUriWith(
  result: { format: string; data: unknown; mimeType: string } | null | undefined,
  base64: Base64Encoders,
): string {
  if (!result || typeof result !== "object") {
    throw new ValidationError("result is required");
  }

  const encoded =
    result.format === "svg"
      ? base64.text(typeof result.data === "string" ? result.data : String(result.data))
      : base64.bytes(result.data);

  return `data:${result.mimeType};base64,${encoded}`;
}
