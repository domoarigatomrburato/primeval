// Type-level checks of the browser entry's published declarations, through
// the package's own name under the `browser` condition. Type-checked by
// test/types.test.js with tsconfig.browser.json.
import * as entry from "@aleburato/primeval";
import {
  AbortError,
  type ApproximateResult,
  approximate,
  type ErrorCode,
  type ExecutionOptions,
  InternalError,
  type OptionName,
  type OutputFormat,
  type PngResult,
  PrimevalError,
  type ProgressInfo,
  type RenderOptions,
  type Shape,
  type SvgResult,
  toDataUri,
  ValidationError,
} from "@aleburato/primeval";

declare const input: Uint8Array;
declare const format: "svg" | "png";

type Equal<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

export const svg: Promise<SvgResult> = approximate({ input, output: "svg" });
export const png: Promise<PngResult> = approximate({ input, output: "png" });
export const either: Promise<ApproximateResult> = approximate({ input, output: format });

// @ts-expect-error an SVG request does not resolve to a PNG result
export const mismatched: Promise<PngResult> = approximate({ input, output: "svg" });

export const svgText = async (): Promise<string> =>
  (await approximate({ input, output: "svg" })).data;
export const pngData: Equal<PngResult["data"], Uint8Array> = true;
export const dataUri = async (): Promise<string> =>
  toDataUri(await approximate({ input, output: "png" }));

export const execution: ExecutionOptions = {
  signal: new AbortController().signal,
  onProgress: (info: ProgressInfo) => void info.step,
};
export const render: RenderOptions = { count: 10, shape: "triangle" satisfies Shape };
export const outputFormats: Equal<OutputFormat, "svg" | "png"> = true;
export const progressShape: Equal<ProgressInfo["shape"], string> = true;

export const isPrimevalError = (
  error: ValidationError | AbortError | InternalError,
): PrimevalError => error;
export const classes = [ValidationError, AbortError, InternalError, PrimevalError] as const;
export const code = (error: PrimevalError): ErrorCode => error.code;
export const option = (error: ValidationError): OptionName | undefined => error.option;

// @ts-expect-error the test-only panic path is not part of the entry
export const panic = entry.panicForTests;
