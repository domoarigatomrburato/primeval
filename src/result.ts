// The result shape shared by the Node and browser entries; each passes its
// own SVG decoder and keeps its own PNG `data` type.
import type { SvgResult } from "./types.js";

/** A PNG result whose `data` is the entry's byte type. */
export type PngResultOf<Data> = {
  format: "png";
  data: Data;
  mimeType: "image/png";
  width: number;
  height: number;
};

/** The public result of a binding's output; an SVG's bytes are decoded with `decodeSvg`. */
export function toResult<Data>(
  result: { format: string; data: Data; width: number; height: number },
  decodeSvg: (data: Data) => string,
): SvgResult | PngResultOf<Data> {
  const { width, height } = result;
  return result.format === "svg"
    ? { format: "svg", data: decodeSvg(result.data), mimeType: "image/svg+xml", width, height }
    : { format: "png", data: result.data, mimeType: "image/png", width, height };
}
