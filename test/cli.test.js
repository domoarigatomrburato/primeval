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

test("cli infers jpg output from .jpeg extension", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.jpeg");

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
  const bytes = fs.readFileSync(output);
  assert.equal(bytes[0], 0xff);
  assert.equal(bytes[1], 0xd8);
});

test("cli accepts --format jpeg as an alias for jpg", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.bin");

  const result = runCli([
    fixturePath,
    "--output",
    output,
    "--format",
    "jpeg",
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
  const bytes = fs.readFileSync(output);
  assert.equal(bytes[0], 0xff);
  assert.equal(bytes[1], 0xd8);
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

test("cli exits non-zero with missing input file", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");
  const result = runCli([
    path.join(tmpDir, "does-not-exist.jpg"),
    "--output",
    output,
    "--progress",
    "off",
  ]);

  assert.equal(result.status, 1);
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
  const expected = path.join(tmpDir, "monalisa_primitive.jpg");
  assert.ok(fs.existsSync(expected), `expected output file ${expected} to exist`);
  const svg = fs.readFileSync(expected);
  // JPEG magic bytes (auto-derived format matches input extension)
  assert.equal(svg[0], 0xff);
  assert.equal(svg[1], 0xd8);
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
  // Pre-create the would-be output file (jpg, matching input extension)
  fs.writeFileSync(path.join(tmpDir, "monalisa_primitive.jpg"), "placeholder");

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
