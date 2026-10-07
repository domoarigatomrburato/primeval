// The public types shared by the Node entry (index.ts) and the browser entry
// (browser.ts). Only the PNG result differs between them (`Buffer` on Node,
// `Uint8Array` in the browser), so each entry declares that one itself.

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
  /**
   * The current fit, after this step and the refit passes before it: the
   * RMSE between the working canvas and the resized input over the RGB
   * channels, divided by 255, from 0 (exact) to 1; lower is better. The
   * final stage can lower it further, so the result can fit better than the
   * last step reports.
   */
  score: number;
  /**
   * The SVG element of the shape this step's search added, formatted exactly
   * as a shape line of the SVG output, without the newline, whatever the
   * `output` format. Its coordinates are in the final SVG's `viewBox`, the
   * working canvas.
   *
   * The shapes of every step, in order, draw a live preview. The render
   * revises shapes it has already reported: after some steps a refit pass,
   * which re-optimises one shape at a time, can move, resize and recolour
   * any shape so far, and after the last step a final stage can revise
   * every shape, so the result's shape lines can differ from the preview.
   * For triangles, polygons, rectangles and rotated rectangles the final
   * stage is a joint gradient optimisation of every shape at once, whose
   * coordinates are multiples of a quarter of a `viewBox` unit, half a unit
   * for rectangles, while a rotated rectangle's corners are computed from
   * such values, so they can be fractional. For `any` it is one refit pass,
   * then the same optimisation of those shapes, the others keeping their
   * geometry. The optimisation's result is kept only if the PNG output at
   * the working size is closer to the resized input than with the shapes
   * before it; otherwise the stage keeps those, with coordinates not
   * rounded to that grid. For quadratics it is refit passes until one
   * improves the fit by less than 1%, at most four; for the other shapes
   * it is one refit pass. The passes and
   * the stage keep the shapes' number, their order and each shape's kind,
   * though an ellipse can turn into a `<circle>` or back when its radii
   * become equal or unequal.
   *
   * To draw the preview, wrap the shapes received so far in an `<svg>` with
   * the final document's `viewBox` and background, for example taken from a
   * `count: 1` render with the same options: the background and canvas size
   * do not depend on `count`. When the result arrives, show the result.
   */
  shape: string;
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
