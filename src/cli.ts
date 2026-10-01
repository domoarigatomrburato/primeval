#!/usr/bin/env node

import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";

import {
  AbortError,
  approximate,
  NotFoundError,
  type OutputFormat,
  type Shape,
  ValidationError,
} from "./index.js";

const require = createRequire(import.meta.url);
const packageJson = require("../package.json") as { version: string };

const VALID_FORMATS: readonly OutputFormat[] = ["svg", "png", "jpg", "gif"];
const VALID_SHAPES = [
  "any",
  "triangle",
  "rectangle",
  "ellipse",
  "circle",
  "rotated-rectangle",
  "quadratic",
  "rotated-ellipse",
  "polygon",
] as const;

type ProgressMode = "auto" | "plain" | "off";

function parseOutputFormat(value: string): OutputFormat | undefined {
  const normalized = value.toLowerCase();
  if (normalized === "jpeg") {
    return "jpg";
  }
  if ((VALID_FORMATS as readonly string[]).includes(normalized)) {
    return normalized as OutputFormat;
  }
  return undefined;
}

function printUsage(): void {
  process.stdout.write(
    [
      "Usage: primeval <input> [--output <path|->] [options]",
      "",
      "Options:",
      "  -o, --output <path|->      Output path (or - for stdout); defaults to <input-stem>_primitive.<ext>",
      "      --format <fmt>         svg|png|jpg|jpeg|gif (optional override)",
      "      --count <n>            Number of optimization steps",
      "      --shape <kind>         any|triangle|rectangle|ellipse|circle|rotated-rectangle|quadratic|rotated-ellipse|polygon",
      "      --alpha <n>            Alpha 0..255 where 0 means auto",
      "      --background <value>   auto or a color value",
      "      --resize-input <n>     Working resolution",
      "      --output-size <n>      Final replay resolution",
      "      --seed <n>             Deterministic seed",
      "      --repeat <n>           Extra candidates per step",
      "      --progress <mode>      auto|plain|off (default: auto)",
      "      --version              Print package version",
      "  -h, --help                 Show this help",
      "",
    ].join("\n"),
  );
}

function fail(message: string): never {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

function parsePositiveInteger(name: string, value: string, min: number): number {
  if (!/^\d+$/.test(value)) {
    fail(`${name} must be an integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min) {
    fail(`${name} must be at least ${min}`);
  }
  return parsed;
}

function parseAlpha(raw: string): number {
  const alpha = parsePositiveInteger("alpha", raw, 0);
  if (alpha > 255) {
    fail("alpha must be 0..255 where 0 means auto");
  }
  return alpha;
}

function inferFormat(outputPath: string): OutputFormat {
  if (outputPath === "-") {
    return "svg";
  }

  const extension = path.extname(outputPath).slice(1).toLowerCase();
  const format = parseOutputFormat(extension);
  if (format) {
    return format;
  }
  fail("could not infer output format from output extension; use --format");
}

function parseProgress(raw: string | undefined): ProgressMode {
  if (raw === undefined) {
    return "auto";
  }
  if (raw === "auto" || raw === "plain" || raw === "off") {
    return raw;
  }
  fail("progress must be one of: auto, plain, off");
}

async function main(): Promise<void> {
  let values: {
    output?: string;
    format?: string;
    count?: string;
    shape?: string;
    alpha?: string;
    background?: string;
    resizeInput?: string;
    outputSize?: string;
    seed?: string;
    repeat?: string;
    progress?: string;
    help?: boolean;
    version?: boolean;
  };
  let positionals: string[];

  try {
    const parsed = parseArgs({
      allowPositionals: true,
      options: {
        output: { type: "string", short: "o" },
        format: { type: "string" },
        count: { type: "string" },
        shape: { type: "string" },
        alpha: { type: "string" },
        background: { type: "string" },
        "resize-input": { type: "string" },
        "output-size": { type: "string" },
        seed: { type: "string" },
        repeat: { type: "string" },
        progress: { type: "string" },
        help: { type: "boolean", short: "h" },
        version: { type: "boolean" },
      },
    });
    values = {
      output: parsed.values.output,
      format: parsed.values.format,
      count: parsed.values.count,
      shape: parsed.values.shape,
      alpha: parsed.values.alpha,
      background: parsed.values.background,
      resizeInput: parsed.values["resize-input"],
      outputSize: parsed.values["output-size"],
      seed: parsed.values.seed,
      repeat: parsed.values.repeat,
      progress: parsed.values.progress,
      help: parsed.values.help,
      version: parsed.values.version,
    };
    positionals = parsed.positionals;
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }

  if (values.help) {
    printUsage();
    return;
  }

  if (values.version) {
    process.stdout.write(`${packageJson.version}\n`);
    return;
  }

  const input = positionals[0];
  if (!input) {
    process.stderr.write("missing input path\n\n");
    printUsage();
    process.exit(1);
  }

  if (positionals.length > 1) {
    fail(`unexpected positional arguments: ${positionals.slice(1).join(" ")}`);
  }

  // Parse --format early — needed both for validation and output path derivation.
  let format: OutputFormat | undefined;
  if (values.format !== undefined) {
    const parsedFormat = parseOutputFormat(values.format);
    if (!parsedFormat) {
      fail(`unknown output format: ${values.format}`);
    }
    format = parsedFormat;
  }

  let outputPath = values.output;
  if (!outputPath) {
    // Auto-derive: <input-stem>_primitive.<ext> in the same directory as the input.
    const ext = format ?? parseOutputFormat(path.extname(input).slice(1)) ?? "svg";
    const stem = path.basename(input, path.extname(input));
    const dir = path.dirname(input);
    const derived = path.join(dir, `${stem}_primitive.${ext}`);
    if (fs.existsSync(derived)) {
      fail(`output file already exists: ${derived} — use --output to specify a different path`);
    }
    outputPath = derived;
    process.stderr.write(`output: ${derived}\n`);
  }

  format ??= inferFormat(outputPath);

  if (outputPath === "-" && format !== "svg") {
    fail("stdout output currently supports only svg format");
  }

  if (values.shape !== undefined && !(VALID_SHAPES as readonly string[]).includes(values.shape)) {
    fail(`unknown shape: ${values.shape}`);
  }

  const progress = parseProgress(values.progress);
  const start = Date.now();
  const showProgress =
    progress === "plain" || (progress === "auto" && Boolean(process.stderr.isTTY));

  const result = await approximate({
    input: { kind: "path", path: input },
    output: format,
    render: {
      ...(values.count === undefined
        ? {}
        : { count: parsePositiveInteger("count", values.count, 1) }),
      ...(values.shape === undefined ? {} : { shape: values.shape as Shape }),
      ...(values.alpha === undefined ? {} : { alpha: parseAlpha(values.alpha) }),
      ...(values.background === undefined ? {} : { background: values.background }),
      ...(values.resizeInput === undefined
        ? {}
        : { resizeInput: parsePositiveInteger("resize-input", values.resizeInput, 1) }),
      ...(values.outputSize === undefined
        ? {}
        : { outputSize: parsePositiveInteger("output-size", values.outputSize, 1) }),
      ...(values.seed === undefined ? {} : { seed: parsePositiveInteger("seed", values.seed, 0) }),
      ...(values.repeat === undefined
        ? {}
        : { repeat: parsePositiveInteger("repeat", values.repeat, 0) }),
    },
    execution: !showProgress
      ? undefined
      : {
          onProgress(info) {
            const elapsedSeconds = (Date.now() - start) / 1000;
            process.stderr.write(
              `${String(info.step).padStart(4, " ")}: elapsed=${elapsedSeconds.toFixed(3)}s score=${info.score.toFixed(6)}\n`,
            );
          },
        },
  });

  if (outputPath === "-") {
    const stdoutBytes =
      result.format === "svg" ? Buffer.from(result.data, "utf8") : Buffer.from(result.data);
    process.stdout.write(stdoutBytes);
    return;
  }

  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  if (result.format === "svg") {
    fs.writeFileSync(outputPath, result.data, "utf8");
  } else {
    fs.writeFileSync(outputPath, result.data);
  }
}

main().catch((error: unknown) => {
  if (error instanceof AbortError) {
    process.exit(0);
  }
  if (error instanceof ValidationError || error instanceof NotFoundError) {
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  }

  process.stderr.write(
    `${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`,
  );
  process.exit(1);
});
