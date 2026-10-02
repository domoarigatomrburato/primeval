import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();
const fixturePath = path.join(repoRoot, "docs", "readme", "originals", "monalisa.jpg");
const cliPath = path.join(repoRoot, "dist", "cli.js");

function runCli(args) {
  return spawnSync(process.execPath, [cliPath, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
  });
}

function makeTmpDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "primeval-node-cli-test-"));
}

test("cli writes svg output file", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([
    fixturePath,
    "--output",
    output,
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  const svg = fs.readFileSync(output, "utf8");
  assert.match(svg, /^<svg\b/);
});

test("cli supports stdout output", () => {
  const result = runCli([
    fixturePath,
    "--output",
    "-",
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^<svg\b/);
});

test("cli suppresses progress with --progress off", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([
    fixturePath,
    "--output",
    output,
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stderr.trim(), "");
});

test("cli suppresses progress with --progress auto when stderr is not a tty", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([
    fixturePath,
    "--output",
    output,
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "auto",
  ]);

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stderr.trim(), "");
});

test("cli treats --alpha 0 as auto", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([
    fixturePath,
    "--output",
    output,
    "--count",
    "4",
    "--alpha",
    "0",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  const svg = fs.readFileSync(output, "utf8");
  assert.match(svg, /^<svg\b/);
});

const RENDER_ARGS = [
  "--count",
  "4",
  "--resize-input",
  "8",
  "--output-size",
  "16",
  "--seed",
  "7",
  "--progress",
  "off",
];

test("cli rejects removed output formats inferred from --output", () => {
  const tmpDir = makeTmpDir();

  for (const extension of ["jpg", "jpeg", "gif"]) {
    const output = path.join(tmpDir, `out.${extension}`);
    const result = runCli([fixturePath, "--output", output, ...RENDER_ARGS]);

    assert.equal(result.status, 1, `expected failure for .${extension}`);
    assert.match(result.stderr, new RegExp(`unknown output format: ${extension}\\b`));
    assert.equal(fs.existsSync(output), false);
  }
});

test("cli rejects removed --format values", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.bin");

  for (const format of ["jpg", "jpeg", "gif"]) {
    const result = runCli([fixturePath, "--output", output, "--format", format, ...RENDER_ARGS]);

    assert.equal(result.status, 1, `expected failure for --format ${format}`);
    assert.match(result.stderr, new RegExp(`unknown output format: ${format}\\b`));
    assert.equal(fs.existsSync(output), false);
  }
});

test("cli prints help", () => {
  const result = runCli(["--help"]);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /Usage:/);
});

test("cli exits non-zero with missing args", () => {
  const result = runCli([]);
  assert.equal(result.status, 1);
});

function assertCleanFailure(result, pattern) {
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, pattern);
  assert.doesNotMatch(result.stderr, /\n\s+at /, "stderr should not contain a stack trace");
}

test("cli reports a missing input file", () => {
  const tmpDir = makeTmpDir();
  const input = path.join(tmpDir, "does-not-exist.jpg");
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([input, "--output", output, ...RENDER_ARGS]);

  assertCleanFailure(result, /^input file not found: .*does-not-exist\.jpg\n$/);
  assert.equal(fs.existsSync(output), false);
});

test("cli reports a directory input", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([tmpDir, "--output", output, ...RENDER_ARGS]);

  assertCleanFailure(result, /^input is a directory: /);
  assert.equal(fs.existsSync(output), false);
});

test("cli rejects a FIFO input without blocking", (t) => {
  if (process.platform === "win32") {
    t.skip("mkfifo is not available on Windows");
    return;
  }
  const tmpDir = makeTmpDir();
  const fifo = path.join(tmpDir, "input.fifo");
  const made = spawnSync("mkfifo", [fifo]);
  if (made.error || made.status !== 0) {
    t.skip("mkfifo is unavailable");
    return;
  }
  const output = path.join(tmpDir, "out.svg");

  const result = spawnSync(process.execPath, [cliPath, fifo, "--output", output, ...RENDER_ARGS], {
    cwd: repoRoot,
    encoding: "utf8",
    timeout: 10_000,
  });

  assert.equal(result.error, undefined, "cli should not block on a FIFO");
  assertCleanFailure(result, /^input is not a regular file: .*input\.fifo\n$/);
});

test("cli reports an unreadable input file", (t) => {
  if (process.platform === "win32" || process.getuid?.() === 0) {
    t.skip("file permissions are not enforced here");
    return;
  }
  const tmpDir = makeTmpDir();
  const input = path.join(tmpDir, "locked.jpg");
  fs.copyFileSync(fixturePath, input);
  fs.chmodSync(input, 0o000);
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([input, "--output", output, ...RENDER_ARGS]);

  assertCleanFailure(result, /^permission denied reading input: .*locked\.jpg\n$/);
});

test("cli reports an invalid background without a stack trace", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([fixturePath, "--output", output, "--background", "a€bc", ...RENDER_ARGS]);

  assertCleanFailure(
    result,
    /^background must be auto or an opaque hex color \(RGB or RRGGBB\)\n$/,
  );
});

test("cli auto-derives output filename when --output is omitted", () => {
  const tmpDir = makeTmpDir();
  // Copy fixture into tmpDir so auto-derived file lands alongside it
  const inputCopy = path.join(tmpDir, "monalisa.jpg");
  fs.copyFileSync(fixturePath, inputCopy);

  const result = runCli([
    inputCopy,
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  const expected = path.join(tmpDir, "monalisa_primitive.svg");
  assert.ok(fs.existsSync(expected), `expected output file ${expected} to exist`);
  // JPEG is not an output format, so a .jpg input derives an SVG output.
  assert.match(fs.readFileSync(expected, "utf8"), /^<svg\b/);
  assert.equal(fs.existsSync(path.join(tmpDir, "monalisa_primitive.jpg")), false);
});

test("cli auto-derives png output from a .png input", () => {
  const tmpDir = makeTmpDir();
  const pngInput = path.join(tmpDir, "monalisa.png");
  fs.copyFileSync(
    path.join(repoRoot, "docs", "readme", "comparisons", "monalisa-any-200-alpha-128.png"),
    pngInput,
  );

  const result = runCli([pngInput, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  const expected = path.join(tmpDir, "monalisa_primitive.png");
  assert.ok(fs.existsSync(expected), `expected output file ${expected} to exist`);
  const bytes = fs.readFileSync(expected);
  assert.equal(bytes[0], 0x89);
  assert.equal(bytes[1], 0x50);
});

test("cli auto-derives output with correct format when --format is given", () => {
  const tmpDir = makeTmpDir();
  const inputCopy = path.join(tmpDir, "monalisa.jpg");
  fs.copyFileSync(fixturePath, inputCopy);

  const result = runCli([
    inputCopy,
    "--format",
    "png",
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 0, result.stderr);
  const expected = path.join(tmpDir, "monalisa_primitive.png");
  assert.ok(fs.existsSync(expected), `expected output file ${expected} to exist`);
  const bytes = fs.readFileSync(expected);
  // PNG magic bytes
  assert.equal(bytes[0], 0x89);
  assert.equal(bytes[1], 0x50);
});

test("cli fails with collision when auto-derived output already exists", () => {
  const tmpDir = makeTmpDir();
  const inputCopy = path.join(tmpDir, "monalisa.jpg");
  fs.copyFileSync(fixturePath, inputCopy);
  // Pre-create the would-be output file (svg, derived for a .jpg input)
  fs.writeFileSync(path.join(tmpDir, "monalisa_primitive.svg"), "placeholder");

  const result = runCli([
    inputCopy,
    "--count",
    "4",
    "--resize-input",
    "8",
    "--output-size",
    "16",
    "--seed",
    "7",
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /already exists/);
});
