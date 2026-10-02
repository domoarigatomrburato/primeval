#!/usr/bin/env node

import type fs from "node:fs";
import { lstat, mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";

import {
  AbortError,
  approximate,
  type ExecutionOptions,
  type OutputFormat,
  type RenderOptions,
  type Shape,
  ValidationError,
} from "./index.js";

const require = createRequire(import.meta.url);
const packageJson = require("../package.json") as { version: string };

const EXIT_RUNTIME = 1;
const EXIT_USAGE = 2;
const EXIT_INTERRUPTED = 130;

const STDIO = "-";
const OUTPUT_FORMATS: Readonly<Record<string, OutputFormat>> = { ".svg": "svg", ".png": "png" };
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

/** An error whose message is meant for the user, printed without a stack trace. */
class CliError extends Error {
  constructor(
    message: string,
    readonly exitCode: number,
  ) {
    super(message);
  }
}

function usageError(message: string): CliError {
  return new CliError(message, EXIT_USAGE);
}

function runtimeError(message: string): CliError {
  return new CliError(message, EXIT_RUNTIME);
}

function usage(): string {
  const rows: [string, string][] = [
    ["-o, --output <path>", "Output file: .svg or .png, or - for SVG on stdout"],
    ["", "(default: <input-stem>.svg next to the input; stdout when <input> is -)"],
    ["-f, --force", "Overwrite an existing output file"],
    ["-q, --quiet", "No progress or notices on stderr"],
    ["    --count <n>", "Number of optimization steps"],
    ["    --shape <kind>", VALID_SHAPES.join("|")],
    ["    --alpha <value>", "auto or a fixed shape opacity 1..255"],
    ["    --background <value>", "auto or an opaque hex color (RGB or RRGGBB)"],
    ["    --resize-input <n>", "Working resolution"],
    ["    --output-size <n>", "Final replay resolution"],
    ["    --seed <n>", "Deterministic seed"],
    ["-v, --version", "Print the package version"],
    ["-h, --help", "Show this help"],
  ];
  const width = 27;
  return [
    "Usage: primeval <input> [options]",
    "",
    `  ${"<input>".padEnd(width)}JPEG, PNG or WebP file, or - to read from stdin`,
    "",
    "Options:",
    ...rows.map(([flag, description]) => `  ${flag.padEnd(width)}${description}`),
    "",
  ].join("\n");
}

function parseInteger(name: string, value: string, min: number): number {
  if (!/^\d+$/.test(value)) {
    throw usageError(`${name} must be an integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min) {
    throw usageError(`${name} must be at least ${min}`);
  }
  return parsed;
}

function parseAlpha(raw: string): "auto" | number {
  if (raw === "auto") {
    return raw;
  }
  const alpha = /^\d+$/.test(raw) ? Number(raw) : Number.NaN;
  if (!(alpha >= 1 && alpha <= 255)) {
    throw usageError("alpha must be auto or an integer 1..255");
  }
  return alpha;
}

function formatFromExtension(outputPath: string): OutputFormat {
  const extension = path.extname(outputPath);
  if (!extension) {
    throw usageError("output path has no extension (use .svg or .png)");
  }
  const format = OUTPUT_FORMATS[extension.toLowerCase()];
  if (!format) {
    throw usageError(`unsupported output extension: ${extension} (use .svg or .png)`);
  }
  return format;
}

function errorCode(error: unknown): string | undefined {
  return error instanceof Error && "code" in error ? String(error.code) : undefined;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readFailureMessage(inputPath: string, error: unknown): string {
  switch (errorCode(error)) {
    case "ENOENT":
      return `input file not found: ${inputPath}`;
    case "EISDIR":
      return `input is a directory: ${inputPath}`;
    case "EACCES":
    case "EPERM":
      return `permission denied reading input: ${inputPath}`;
    default:
      return `cannot read input ${inputPath}: ${errorMessage(error)}`;
  }
}

async function readStdin(): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) {
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks);
}

// Reads only regular files: a FIFO or device could block or never end.
async function readInput(inputPath: string): Promise<Buffer> {
  if (inputPath === STDIO) {
    return readStdin();
  }
  let stats: fs.Stats;
  try {
    stats = await stat(inputPath);
  } catch (error) {
    throw runtimeError(readFailureMessage(inputPath, error));
  }
  if (stats.isDirectory()) {
    throw runtimeError(`input is a directory: ${inputPath}`);
  }
  if (!stats.isFile()) {
    throw runtimeError(`input is not a regular file: ${inputPath}`);
  }
  try {
    return await readFile(inputPath);
  } catch (error) {
    throw runtimeError(readFailureMessage(inputPath, error));
  }
}

function alreadyExists(outputPath: string): CliError {
  return runtimeError(`output file already exists: ${outputPath} (use --force to overwrite)`);
}

// Fails fast, before any render work. The final write uses the `wx` flag, so
// a file created after this check is still never overwritten without --force.
async function checkOutput(outputPath: string, force: boolean): Promise<void> {
  try {
    await lstat(outputPath);
  } catch (error) {
    if (errorCode(error) === "ENOENT") {
      return;
    }
    throw runtimeError(`cannot access output ${outputPath}: ${errorMessage(error)}`);
  }
  const target = await stat(outputPath).catch(() => undefined);
  if (target?.isDirectory()) {
    throw runtimeError(`output is a directory: ${outputPath}`);
  }
  if (!force) {
    throw alreadyExists(outputPath);
  }
}

async function writeOutput(outputPath: string, data: string | Buffer, force: boolean) {
  try {
    await mkdir(path.dirname(outputPath), { recursive: true });
    await writeFile(outputPath, data, { flag: force ? "w" : "wx" });
  } catch (error) {
    switch (errorCode(error)) {
      case "EEXIST":
        throw alreadyExists(outputPath);
      case "EISDIR":
        throw runtimeError(`output is a directory: ${outputPath}`);
      default:
        throw runtimeError(`cannot write output ${outputPath}: ${errorMessage(error)}`);
    }
  }
}

/** A single stderr line updated in place; only used when stderr is a TTY. */
function createProgressLine(): { execution: ExecutionOptions; finish(): void } {
  const start = Date.now();
  let drawn = false;
  return {
    execution: {
      onProgress(info) {
        const elapsedSeconds = (Date.now() - start) / 1000;
        process.stderr.write(
          `\r\x1b[K${info.step}/${info.total}  score ${info.score.toFixed(6)}  ${elapsedSeconds.toFixed(1)}s`,
        );
        drawn = true;
      },
    },
    finish() {
      if (drawn) {
        process.stderr.write("\n");
        drawn = false;
      }
    },
  };
}

type Interrupt = { signal: AbortSignal; interrupted(): boolean; dispose(): void };

// First SIGINT aborts the render; a second one exits immediately.
function watchInterrupt(): Interrupt {
  const controller = new AbortController();
  let count = 0;
  const onSigint = (): void => {
    count += 1;
    if (count > 1) {
      process.exit(EXIT_INTERRUPTED);
    }
    controller.abort();
  };
  process.on("SIGINT", onSigint);
  return {
    signal: controller.signal,
    interrupted: () => count > 0,
    dispose: () => process.off("SIGINT", onSigint),
  };
}

function parseCommandLine() {
  try {
    return parseArgs({
      allowPositionals: true,
      options: {
        output: { type: "string", short: "o" },
        force: { type: "boolean", short: "f" },
        quiet: { type: "boolean", short: "q" },
        count: { type: "string" },
        shape: { type: "string" },
        alpha: { type: "string" },
        background: { type: "string" },
        "resize-input": { type: "string" },
        "output-size": { type: "string" },
        seed: { type: "string" },
        help: { type: "boolean", short: "h" },
        version: { type: "boolean", short: "v" },
      },
    });
  } catch (error) {
    throw usageError(errorMessage(error));
  }
}

async function main(): Promise<number> {
  const { values, positionals } = parseCommandLine();

  if (values.help) {
    process.stdout.write(usage());
    return 0;
  }
  if (values.version) {
    process.stdout.write(`${packageJson.version}\n`);
    return 0;
  }

  const input = positionals[0];
  if (input === undefined || input === "") {
    throw usageError("missing input path");
  }
  if (positionals.length > 1) {
    throw usageError(`unexpected positional arguments: ${positionals.slice(1).join(" ")}`);
  }
  if (values.output === "") {
    throw usageError("output path must not be empty");
  }

  const outputPath =
    values.output ??
    (input === STDIO
      ? STDIO
      : path.join(path.dirname(input), `${path.basename(input, path.extname(input))}.svg`));
  const format = outputPath === STDIO ? "svg" : formatFromExtension(outputPath);

  if (values.shape !== undefined && !(VALID_SHAPES as readonly string[]).includes(values.shape)) {
    throw usageError(`unknown shape: ${values.shape}`);
  }
  const render: RenderOptions = {
    ...(values.count === undefined ? {} : { count: parseInteger("count", values.count, 1) }),
    ...(values.shape === undefined ? {} : { shape: values.shape as Shape }),
    ...(values.alpha === undefined ? {} : { alpha: parseAlpha(values.alpha) }),
    ...(values.background === undefined ? {} : { background: values.background }),
    ...(values["resize-input"] === undefined
      ? {}
      : { resizeInput: parseInteger("resize-input", values["resize-input"], 1) }),
    ...(values["output-size"] === undefined
      ? {}
      : { outputSize: parseInteger("output-size", values["output-size"], 1) }),
    ...(values.seed === undefined ? {} : { seed: parseInteger("seed", values.seed, 0) }),
  };

  const force = values.force === true;
  const quiet = values.quiet === true;
  if (outputPath !== STDIO) {
    await checkOutput(outputPath, force);
  }

  const inputBytes = await readInput(input);

  const progress = !quiet && process.stderr.isTTY ? createProgressLine() : undefined;
  const interrupt = watchInterrupt();
  let result: Awaited<ReturnType<typeof approximate>>;
  try {
    result = await approximate({
      input: inputBytes,
      output: format,
      render,
      execution: { ...progress?.execution, signal: interrupt.signal },
    });
  } catch (error) {
    if (interrupt.interrupted() && error instanceof AbortError) {
      return EXIT_INTERRUPTED;
    }
    throw error;
  } finally {
    progress?.finish();
    interrupt.dispose();
  }
  // A SIGINT that lands as the render completes still cancels the write.
  if (interrupt.interrupted()) {
    return EXIT_INTERRUPTED;
  }

  const data = result.format === "svg" ? result.data : Buffer.from(result.data);
  if (outputPath === STDIO) {
    process.stdout.write(data);
    return 0;
  }
  await writeOutput(outputPath, data, force);
  if (values.output === undefined && !quiet) {
    process.stderr.write(`output: ${outputPath}\n`);
  }
  return 0;
}

main().then(
  (code) => {
    process.exitCode = code;
  },
  (error: unknown) => {
    if (error instanceof CliError) {
      process.stderr.write(`${error.message}\n`);
      if (error.exitCode === EXIT_USAGE) {
        process.stderr.write("Run 'primeval --help' for usage.\n");
      }
      process.exitCode = error.exitCode;
      return;
    }
    // Library errors and filesystem errors (with a known `code`) are user-facing.
    if (
      error instanceof ValidationError ||
      error instanceof AbortError ||
      errorCode(error) !== undefined
    ) {
      process.stderr.write(`${errorMessage(error)}\n`);
    } else {
      process.stderr.write(
        `${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`,
      );
    }
    process.exitCode = EXIT_RUNTIME;
  },
);
