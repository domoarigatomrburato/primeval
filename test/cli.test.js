import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();
const fixturePath = path.join(repoRoot, "docs", "readme", "originals", "monalisa.jpg");
const cliPath = path.join(repoRoot, "dist", "cli.js");

const RENDER_ARGS = ["--count", "4", "--resize-input", "8", "--output-size", "16", "--seed", "7"];
// Long enough (tens of seconds) that finishing quickly proves no render ran.
const LONG_RENDER_ARGS = ["--count", "20000", "--resize-input", "16", "--output-size", "16"];
const USAGE_HINT = "Run 'primeval --help' for usage.";

function runCli(args, options = {}) {
  return spawnSync(process.execPath, [cliPath, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
    timeout: 60_000,
    ...options,
  });
}

function makeTmpDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "primeval-node-cli-test-"));
}

function copyFixture(tmpDir, name = "monalisa.jpg") {
  const input = path.join(tmpDir, name);
  fs.copyFileSync(fixturePath, input);
  return input;
}

function assertNoStack(stderr) {
  assert.doesNotMatch(stderr, /\n\s+at /, "stderr should not contain a stack trace");
}

function assertUsageError(result, pattern) {
  assert.equal(result.error, undefined, "cli should exit without timing out");
  assert.equal(result.status, 2, result.stderr);
  assert.equal(result.stdout, "", "usage errors must not write to stdout");
  assert.match(result.stderr, pattern);
  assert.ok(result.stderr.endsWith(`${USAGE_HINT}\n`), result.stderr);
  assertNoStack(result.stderr);
}

function assertRuntimeError(result, pattern) {
  assert.equal(result.error, undefined, "cli should exit without timing out");
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, pattern);
  assertNoStack(result.stderr);
}

function isPng(bytes) {
  return bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47;
}

test("cli writes svg output for a .svg output path", () => {
  const output = path.join(makeTmpDir(), "out.svg");

  const result = runCli([fixturePath, "--output", output, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(fs.readFileSync(output, "utf8"), /^<svg\b/);
});

test("cli writes png output for a .png output path", () => {
  const output = path.join(makeTmpDir(), "out.png");

  const result = runCli([fixturePath, "-o", output, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.ok(isPng(fs.readFileSync(output)));
});

test("cli infers the format from an upper-case extension", () => {
  const output = path.join(makeTmpDir(), "OUT.PNG");

  const result = runCli([fixturePath, "-o", output, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.ok(isPng(fs.readFileSync(output)));
});

test("cli rejects unsupported output extensions", () => {
  const tmpDir = makeTmpDir();

  for (const extension of ["jpg", "jpeg", "gif", "webp"]) {
    const output = path.join(tmpDir, `out.${extension}`);
    const result = runCli([fixturePath, "--output", output, ...RENDER_ARGS]);

    assertUsageError(
      result,
      new RegExp(`^unsupported output extension: \\.${extension} \\(use \\.svg or \\.png\\)\n`),
    );
    assert.equal(fs.existsSync(output), false);
  }
});

test("cli rejects an output path without an extension", () => {
  const output = path.join(makeTmpDir(), "out");

  const result = runCli([fixturePath, "--output", output, ...RENDER_ARGS]);

  assertUsageError(result, /^output path has no extension \(use \.svg or \.png\)\n/);
  assert.equal(fs.existsSync(output), false);
});

test("cli rejects --format as an unknown option", () => {
  const output = path.join(makeTmpDir(), "out.svg");

  const result = runCli([fixturePath, "--output", output, "--format", "png", ...RENDER_ARGS]);

  assertUsageError(result, /Unknown option '--format'/);
  assert.equal(fs.existsSync(output), false);
});

test("cli rejects --progress as an unknown option", () => {
  const result = runCli([fixturePath, "-o", "-", "--progress", "off", ...RENDER_ARGS]);

  assertUsageError(result, /Unknown option '--progress'/);
});

test("cli rejects an unknown option", () => {
  const result = runCli([fixturePath, "--nope"]);

  assertUsageError(result, /Unknown option '--nope'/);
});

test("cli rejects an empty output path", () => {
  const result = runCli([fixturePath, "--output", "", ...RENDER_ARGS]);

  assertUsageError(result, /^output path must not be empty\n/);
});

test("cli rejects extra positional arguments", () => {
  const result = runCli([fixturePath, fixturePath, "-o", "-"]);

  assertUsageError(result, /^unexpected positional arguments: /);
});

test("cli rejects invalid numeric options as usage errors", () => {
  const result = runCli([fixturePath, "-o", "-", "--count", "abc"]);

  assertUsageError(result, /^--count must be an integer\n/);

  for (const flag of ["--resize-input", "--output-size", "--seed"]) {
    assertUsageError(
      runCli([fixturePath, "-o", "-", flag, "1.5"]),
      new RegExp(`^${flag} must be an integer\n`),
    );
  }
});

test("cli rejects an unknown shape as a usage error", () => {
  const result = runCli([fixturePath, "-o", "-", "--shape", "hexagon"]);

  assertUsageError(result, /^--shape must be one of: any, triangle, .*, polygon\n/);
});

test("cli passes out-of-range numbers to Rust instead of wrapping them", () => {
  const count = runCli([fixturePath, "-o", "-", ...RENDER_ARGS, "--count", "4294967297"]);
  assertRuntimeError(count, /^--count must be an integer from 1 to 100000\n$/);

  const seed = runCli([fixturePath, "-o", "-", ...RENDER_ARGS, "--seed", "99999999999999999999"]);
  assertRuntimeError(seed, /^--seed must be an integer from 0 to 2\^64 - 1/);

  const resize = runCli([fixturePath, "-o", "-", ...RENDER_ARGS, "--resize-input", "4096"]);
  assertRuntimeError(resize, /^--resize-input must be an integer from 2 to 2048\n$/);

  const outputSize = runCli([fixturePath, "-o", "-", ...RENDER_ARGS, "--output-size", "1"]);
  assertRuntimeError(outputSize, /^--output-size must be an integer from 2 to 8192\n$/);
});

test("cli passes a full-range u64 seed exactly", () => {
  const result = runCli([fixturePath, "-o", "-", ...RENDER_ARGS, "--seed", "18446744073709551615"]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^<svg\b/);
});

test("cli reports a missing input without writing to stdout", () => {
  const result = runCli(["-o", "-"]);

  assertUsageError(result, /^missing input path\n/);
});

test("cli prints help on stdout", () => {
  const result = runCli(["--help"]);

  assert.equal(result.status, 0);
  assert.match(result.stdout, /^Usage: primeval <input> \[options\]/);
  assert.match(result.stdout, /-f, --force/);
  assert.match(result.stdout, /-q, --quiet/);
  assert.doesNotMatch(result.stdout, /--format|--progress/);
  assert.equal(result.stderr, "");
});

test("cli prints the version with -v and --version", () => {
  const { version } = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));

  for (const flag of ["-v", "--version"]) {
    const result = runCli([flag]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, `${version}\n`);
  }
});

test("cli writes svg to stdout with -o -", () => {
  const result = runCli([fixturePath, "--output", "-", ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^<svg\b/);
  assert.equal(result.stderr, "");
});

test("cli prints no progress when stderr is not a tty", () => {
  const output = path.join(makeTmpDir(), "out.svg");

  const result = runCli([fixturePath, "--output", output, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stderr, "");
});

test("cli accepts --alpha auto and a fixed alpha", () => {
  for (const alpha of ["auto", "200"]) {
    const output = path.join(makeTmpDir(), "out.svg");

    const result = runCli([fixturePath, "--output", output, "--alpha", alpha, ...RENDER_ARGS]);

    assert.equal(result.status, 0, result.stderr);
    assert.match(fs.readFileSync(output, "utf8"), /^<svg\b/);
  }
});

test("cli rejects --alpha outside auto and 1..255 as a usage error", () => {
  for (const alpha of ["0", "256", "-1", "half"]) {
    const result = runCli([fixturePath, "-o", "-", `--alpha=${alpha}`]);

    assertUsageError(result, /^--alpha must be auto or an integer 1\.\.255\n/);
  }
});

test("cli derives <input-stem>.svg next to the input when --output is omitted", () => {
  const tmpDir = makeTmpDir();
  const input = copyFixture(tmpDir);

  const result = runCli([input, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  const expected = path.join(tmpDir, "monalisa.svg");
  assert.match(fs.readFileSync(expected, "utf8"), /^<svg\b/);
  assert.equal(result.stderr, `output: ${expected}\n`);
  assert.equal(result.stdout, "");
});

test("cli derives svg output for a .png input", () => {
  const tmpDir = makeTmpDir();
  const input = path.join(tmpDir, "monalisa.png");
  fs.copyFileSync(
    path.join(repoRoot, "docs", "readme", "comparisons", "monalisa-any-200-alpha-128.png"),
    input,
  );

  const result = runCli([input, "--quiet", ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(fs.readFileSync(path.join(tmpDir, "monalisa.svg"), "utf8"), /^<svg\b/);
  assert.deepEqual(fs.readdirSync(tmpDir).sort(), ["monalisa.png", "monalisa.svg"]);
});

test("cli suppresses the output notice with --quiet and -q", () => {
  for (const flag of ["--quiet", "-q"]) {
    const tmpDir = makeTmpDir();
    const input = copyFixture(tmpDir);

    const result = runCli([input, flag, ...RENDER_ARGS]);

    assert.equal(result.status, 0, result.stderr);
    assert.ok(fs.existsSync(path.join(tmpDir, "monalisa.svg")));
    assert.equal(result.stderr, "");
  }
});

test("cli refuses to overwrite an existing derived output", () => {
  const tmpDir = makeTmpDir();
  const input = copyFixture(tmpDir);
  const derived = path.join(tmpDir, "monalisa.svg");
  fs.writeFileSync(derived, "placeholder");

  const result = runCli([input, ...LONG_RENDER_ARGS], { timeout: 10_000 });

  assertRuntimeError(
    result,
    /^output file already exists: .*monalisa\.svg \(use --force to overwrite\)\n$/,
  );
  assert.equal(fs.readFileSync(derived, "utf8"), "placeholder");
});

test("cli refuses to overwrite an existing explicit output without rendering", () => {
  const output = path.join(makeTmpDir(), "keep.svg");
  fs.writeFileSync(output, "placeholder");

  const result = runCli([fixturePath, "-o", output, ...LONG_RENDER_ARGS], { timeout: 10_000 });

  assertRuntimeError(
    result,
    /^output file already exists: .*keep\.svg \(use --force to overwrite\)\n$/,
  );
  assert.equal(fs.readFileSync(output, "utf8"), "placeholder");
});

test("cli overwrites an existing output with --force and -f", () => {
  for (const flag of ["--force", "-f"]) {
    const output = path.join(makeTmpDir(), "keep.svg");
    fs.writeFileSync(output, "placeholder");

    const result = runCli([fixturePath, "-o", output, flag, ...RENDER_ARGS]);

    assert.equal(result.status, 0, result.stderr);
    assert.match(fs.readFileSync(output, "utf8"), /^<svg\b/);
  }
});

test("cli rejects an output path that is a directory without rendering", () => {
  const output = path.join(makeTmpDir(), "dir.svg");
  fs.mkdirSync(output);

  for (const extra of [[], ["--force"]]) {
    const result = runCli([fixturePath, "-o", output, ...extra, ...LONG_RENDER_ARGS], {
      timeout: 10_000,
    });

    assertRuntimeError(result, /^output is a directory: .*dir\.svg\n$/);
  }
});

test("cli creates missing output parent directories", () => {
  const output = path.join(makeTmpDir(), "nested", "deeper", "out.svg");

  const result = runCli([fixturePath, "-o", output, ...RENDER_ARGS]);

  assert.equal(result.status, 0, result.stderr);
  assert.match(fs.readFileSync(output, "utf8"), /^<svg\b/);
});

test("cli reads stdin with - and writes svg to stdout by default", () => {
  const result = runCli(["-", ...RENDER_ARGS], { input: fs.readFileSync(fixturePath) });

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^<svg\b/);
  assert.equal(result.stderr, "");
});

test("cli reads stdin with - and writes to an output file", () => {
  const output = path.join(makeTmpDir(), "from-stdin.png");

  const result = runCli(["-", "-o", output, ...RENDER_ARGS], {
    input: fs.readFileSync(fixturePath),
  });

  assert.equal(result.status, 0, result.stderr);
  assert.ok(isPng(fs.readFileSync(output)));
  assert.equal(result.stdout, "");
});

test("cli reports invalid image data from stdin as a runtime error", () => {
  const result = runCli(["-", ...RENDER_ARGS], { input: Buffer.from("not an image") });

  assertRuntimeError(result, /^invalid image data/);
});

test("cli exits 130 on SIGINT without writing the output", async (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX signals are not available on Windows");
    return;
  }
  const output = path.join(makeTmpDir(), "interrupted.svg");
  const child = spawn(process.execPath, [cliPath, fixturePath, "-o", output, ...LONG_RENDER_ARGS], {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const exited = new Promise((resolve) => {
    child.on("exit", (code, signal) => resolve({ code, signal }));
  });
  const killer = setTimeout(() => child.kill("SIGKILL"), 30_000);

  await new Promise((resolve) => child.on("spawn", resolve));
  // Let the CLI load the native addon and start rendering.
  await new Promise((resolve) => setTimeout(resolve, 1_500));
  child.kill("SIGINT");
  const { code, signal } = await exited;
  clearTimeout(killer);

  assert.equal(signal, null, `cli was killed by ${signal}; stderr: ${stderr}`);
  assert.equal(code, 130, stderr);
  assert.equal(fs.existsSync(output), false);
  assertNoStack(stderr);
});

test("cli reports a missing input file", () => {
  const tmpDir = makeTmpDir();
  const input = path.join(tmpDir, "does-not-exist.jpg");
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([input, "--output", output, ...RENDER_ARGS]);

  assertRuntimeError(result, /^input file not found: .*does-not-exist\.jpg\n$/);
  assert.equal(fs.existsSync(output), false);
});

test("cli reports a directory input", () => {
  const tmpDir = makeTmpDir();
  const output = path.join(tmpDir, "out.svg");

  const result = runCli([tmpDir, "--output", output, ...RENDER_ARGS]);

  assertRuntimeError(result, /^input is a directory: /);
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

  const result = runCli([fifo, "--output", output, ...RENDER_ARGS], { timeout: 10_000 });

  assertRuntimeError(result, /^input is not a regular file: .*input\.fifo\n$/);
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

  assertRuntimeError(result, /^permission denied reading input: .*locked\.jpg\n$/);
});

test("cli reports an invalid background without a stack trace", () => {
  const output = path.join(makeTmpDir(), "out.svg");

  const result = runCli([fixturePath, "--output", output, "--background", "a€bc", ...RENDER_ARGS]);

  assertRuntimeError(
    result,
    /^--background must be auto or an opaque hex color \(RGB or RRGGBB\)\n$/,
  );
  assert.equal(fs.existsSync(output), false);
});
