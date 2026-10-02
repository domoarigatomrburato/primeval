// Type-level checks of the published declarations in dist/. Type-checked by
// test/types.test.js.
import {
  type ApproximateResult,
  approximate,
  type OutputFormat,
  type PngResult,
  type ProgressInfo,
  type RenderOptions,
  type Shape,
  type SvgResult,
  ValidationError,
} from "../../dist/index.js";

declare const input: Uint8Array;
declare const format: "svg" | "png";

export const svg: Promise<SvgResult> = approximate({ input, output: "svg" });
export const png: Promise<PngResult> = approximate({ input, output: "png" });
export const either: Promise<ApproximateResult> = approximate({ input, output: format });

// @ts-expect-error an SVG request does not resolve to a PNG result
export const mismatched: Promise<PngResult> = approximate({ input, output: "svg" });

export const svgText = async (): Promise<string> =>
  (await approximate({ input, output: "svg" })).data;
export const pngBytes = async (): Promise<Buffer> =>
  (await approximate({ input, output: "png" })).data;

export const background: RenderOptions["background"][] = ["auto", "#336699", undefined];

export const option = (error: ValidationError): string | undefined => error.option;

// The vocabularies mirror Rust; test/contracts.test.js checks the same lists
// at runtime against the native binding and the CLI.
type Equal<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
export const shapes: Equal<
  Shape,
  | "any"
  | "triangle"
  | "rectangle"
  | "ellipse"
  | "circle"
  | "rotated-rectangle"
  | "quadratic"
  | "rotated-ellipse"
  | "polygon"
> = true;
export const outputFormats: Equal<OutputFormat, "svg" | "png"> = true;
export const progressShape: Equal<ProgressInfo["shape"], string> = true;
