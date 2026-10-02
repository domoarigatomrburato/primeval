// Contract tests: one table of option values and expected outcomes, run
// through the Node API and through the CLI. Rust owns the vocabularies,
// ranges and defaults; both surfaces must agree with the table.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { approximate, ValidationError } from "@aleburato/primeval";

import { rgbPng } from "./helpers/png.js";

const repoRoot = process.cwd();
const cliPath = path.join(repoRoot, "dist", "cli.js");

function readRepoFile(...segments) {
  return fs.readFileSync(path.join(repoRoot, ...segments), "utf8");
}

// --- The table ---

/** The CLI spelling of each render option. */
const FLAGS = {
  count: "--count",
  shape: "--shape",
  alpha: "--alpha",
  seed: "--seed",
  background: "--background",
  resizeInput: "--resize-input",
  outputSize: "--output-size",
};

const SHAPES = [
  "any",
  "triangle",
  "rectangle",
  "ellipse",
  "circle",
  "rotated-rectangle",
  "quadratic",
  "rotated-ellipse",
  "polygon",
];

const REQUIREMENTS = {
  count: "must be an integer from 1 to 100000",
  shape: `must be one of: ${SHAPES.join(", ")}`,
  alpha: "must be auto or an integer 1..255",
  seed: "must be an integer from 0 to 2^64 - 1 (given as a number, at most 2^53 - 1)",
  background: "must be auto or an opaque hex color (RGB or RRGGBB)",
  resizeInput: "must be an integer from 2 to 2048",
  outputSize: "must be an integer from 2 to 8192",
};

/**
 * Each case: `option`, `value` (`undefined` means omitted) and the expected
 * outcome. A rejection is an `INVALID_OPTION` error naming `option`; when
 * `requirement` is set, both surfaces must report exactly that requirement
 * (the CLI checks syntax itself for some values, with its own wording).
 * `apiOnly` marks values the CLI cannot spell (wrong types, `null`) or
 * spells differently by design.
 */
const accepts = (option, value, extra = {}) => ({ option, value, accepted: true, ...extra });
const rejects = (option, value, extra = {}) => ({ option, value, accepted: false, ...extra });
const outOfRange = (option, value) => rejects(option, value, { requirement: REQUIREMENTS[option] });
const apiOnly = (option, value) => rejects(option, value, { apiOnly: true });

const CASES = [
  accepts("output", "svg"),
  accepts("output", "png"),
  ...["gif", "jpg", "jpeg", "webp", ""].map((value) => rejects("output", value)),
  // The CLI reads the format from the extension, case-insensitively.
  apiOnly("output", "SVG"),
  apiOnly("output", 1),
  apiOnly("output", null),
  apiOnly("output", undefined),

  accepts("count", undefined),
  accepts("count", 1),
  // A full render at the maximum takes minutes; an invalid image proves the
  // options were accepted, because Rust validates them before decoding.
  accepts("count", 100000, { probe: true }),
  ...[0, 100001, 2 ** 32 + 1, 1e20].map((value) => outOfRange("count", value)),
  ...[1.5, -1, Number.NaN, Number.POSITIVE_INFINITY].map((value) => rejects("count", value)),
  apiOnly("count", "4"),
  apiOnly("count", null),

  accepts("shape", undefined),
  ...SHAPES.map((value) => accepts("shape", value)),
  ...["hexagon", "Triangle", "", " any"].map((value) => outOfRange("shape", value)),
  apiOnly("shape", 1),
  apiOnly("shape", null),

  accepts("alpha", undefined),
  ...["auto", 1, 128, 255].map((value) => accepts("alpha", value)),
  ...[0, 256, -1, 1.5, "half", ""].map((value) => outOfRange("alpha", value)),
  apiOnly("alpha", true),
  apiOnly("alpha", null),

  accepts("seed", undefined),
  ...[0, 7, 2 ** 53 - 1, 2n ** 64n - 1n].map((value) => accepts("seed", value)),
  outOfRange("seed", 2n ** 64n),
  ...[-1, 1.5, -1n].map((value) => rejects("seed", value)),
  // The CLI parses seeds as bigints, so it accepts this one.
  apiOnly("seed", 2 ** 53),
  apiOnly("seed", "7"),
  apiOnly("seed", null),

  accepts("background", undefined),
  ...["auto", "AUTO", "#abc", "abc", "#336699", "336699"].map((value) =>
    accepts("background", value),
  ),
  ...["#1234", "#11223344", "a€bc", "", "red", " #abc", "##abc"].map((value) =>
    outOfRange("background", value),
  ),
  apiOnly("background", 123),
  apiOnly("background", null),

  accepts("resizeInput", undefined),
  ...[2, 2048].map((value) => accepts("resizeInput", value)),
  ...[0, 1, 2049].map((value) => outOfRange("resizeInput", value)),
  rejects("resizeInput", 1.5),
  apiOnly("resizeInput", 8n),
  apiOnly("resizeInput", null),

  accepts("outputSize", undefined),
  ...[2, 8192].map((value) => accepts("outputSize", value)),
  ...[0, 1, 8193].map((value) => outOfRange("outputSize", value)),
  rejects("outputSize", 1.5),
  apiOnly("outputSize", {}),
  apiOnly("outputSize", null),
];

// The options every case starts from, so each render is a single cheap step.
const BASE_RENDER = { count: 1, resizeInput: 8, outputSize: 16, seed: 1 };
const TINY_IMAGE = rgbPng(4, 4, (x, y) => [x * 60, y * 60, 128]);
const NOT_AN_IMAGE = Buffer.from("not an image");

function describeCase({ option, value }) {
  const shown = typeof value === "bigint" ? `${value}n` : JSON.stringify(value);
  return `${option} = ${shown ?? "undefined"}`;
}

function caseRequest({ option, value, probe }) {
  const render = { ...BASE_RENDER };
  delete render[option];
  if (option !== "output" && value !== undefined) {
    render[option] = value;
  }
  return {
    input: probe ? NOT_AN_IMAGE : TINY_IMAGE,
    output: option === "output" ? value : "svg",
    render,
  };
}

function isPng(bytes) {
  return bytes.subarray(0, 4).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47]));
}

// --- Node API ---

async function checkApiCase(testCase) {
  const name = describeCase(testCase);
  const request = caseRequest(testCase);
  if (testCase.probe) {
    await assert.rejects(
      approximate(request),
      (error) => error instanceof ValidationError && error.code === "INVALID_IMAGE",
      name,
    );
  } else if (testCase.accepted) {
    const result = await approximate(request);
    assert.equal(result.format, request.output, name);
    if (result.format === "svg") {
      assert.match(result.data, /^<svg\b/, name);
    } else {
      assert.ok(isPng(result.data), name);
    }
  } else {
    await assert.rejects(
      approximate(request),
      (error) => {
        assert.ok(error instanceof ValidationError, name);
        assert.equal(error.code, "INVALID_OPTION", name);
        assert.equal(error.option, testCase.option, name);
        assert.equal(error.message, `${error.option} ${error.requirement}`, name);
        if (testCase.requirement !== undefined) {
          assert.equal(error.requirement, testCase.requirement, name);
        }
        return true;
      },
      name,
    );
  }
}

test("the contract table holds through the Node API", async () => {
  for (const testCase of CASES) {
    await checkApiCase(testCase);
  }
});

// --- CLI ---

function runCli(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cliPath, ...args], {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout = [];
    let stderr = "";
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout: Buffer.concat(stdout), stderr }));
  });
}

function renderArgs(render) {
  return Object.entries(render).map(([option, value]) => `${FLAGS[option]}=${value}`);
}

async function checkCliCase(testCase, dir) {
  const name = describeCase(testCase);
  const { option, value } = testCase;
  const request = caseRequest(testCase);
  const input = path.join(dir, testCase.probe ? "not-an-image.png" : "tiny.png");
  const output = option === "output" ? path.join(dir, `out-${value}.${value}`) : "-";
  const result = await runCli([input, "-o", output, ...renderArgs(request.render)]);
  const [firstLine] = result.stderr.split("\n");

  if (testCase.probe) {
    assert.equal(result.status, 1, `${name}: ${result.stderr}`);
    assert.match(firstLine, /^invalid image data/, name);
  } else if (testCase.accepted) {
    assert.equal(result.status, 0, `${name}: ${result.stderr}`);
    assert.equal(result.stderr, "", name);
    const data = output === "-" ? result.stdout : fs.readFileSync(output);
    if (request.output === "png") {
      assert.ok(isPng(data), name);
    } else {
      assert.match(data.toString("utf8"), /^<svg\b/, name);
    }
  } else {
    // 2 for a value the CLI rejects itself, 1 for one Rust rejects.
    assert.ok(result.status === 1 || result.status === 2, `${name}: ${result.status}`);
    assert.equal(result.stdout.length, 0, name);
    assert.doesNotMatch(result.stderr, /\n\s+at /, `${name}: stack trace`);
    if (option === "output") {
      assert.match(firstLine, /^unsupported output extension: /, name);
      assert.equal(fs.existsSync(output), false, name);
    } else if (testCase.requirement !== undefined) {
      assert.equal(firstLine, `${FLAGS[option]} ${testCase.requirement}`, name);
    } else {
      assert.ok(firstLine.startsWith(`${FLAGS[option]} `), `${name}: ${firstLine}`);
    }
  }
}

test("the contract table holds through the CLI", async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-contracts-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.writeFileSync(path.join(dir, "tiny.png"), TINY_IMAGE);
  fs.writeFileSync(path.join(dir, "not-an-image.png"), NOT_AN_IMAGE);

  const pending = CASES.filter((testCase) => !testCase.apiOnly);
  const workers = Array.from({ length: Math.min(8, os.availableParallelism()) }, async () => {
    for (let next = pending.shift(); next !== undefined; next = pending.shift()) {
      await checkCliCase(next, dir);
    }
  });
  await Promise.all(workers);
});

// --- Defaults ---

// The one remaining source parse: the field initializers of
// `RenderOptions::default()`, e.g. `count: 100,` or `shape: ShapeKind::Any,`.
// `None` (the seed) has no default value.
function rustRenderDefaults() {
  const source = readRepoFile("crates", "primeval-render", "src", "lib.rs");
  const body = source.match(
    /^impl Default for RenderOptions \{\n.*?\n {8}Self \{\n(.*?)\n {8}\}/ms,
  );
  assert.ok(body, "missing RenderOptions::default");
  const defaults = {};
  for (const [, field, value] of body[1].matchAll(/^ +(\w+): (\d+|\w+::\w+|None),$/gm)) {
    const name = field.replace(/_(\w)/g, (_, letter) => letter.toUpperCase());
    if (value === "None") {
      continue;
    }
    defaults[name] = /^\d+$/.test(value)
      ? Number(value)
      : value
          .split("::")[1]
          .replace(/(?<=.)([A-Z])/g, "-$1")
          .toLowerCase();
  }
  assert.deepEqual(
    Object.keys(defaults).sort(),
    Object.keys(REQUIREMENTS)
      .sort()
      .filter((name) => name !== "seed"),
  );
  return defaults;
}

// The `@default` tag of each field in `export type RenderOptions = { ... }`.
function jsdocDefaults() {
  const source = readRepoFile("src", "index.ts");
  const block = source.match(/export type RenderOptions = \{(.*?)\n\};/s);
  assert.ok(block, "missing RenderOptions type");
  const defaults = {};
  for (const [, doc, field] of block[1].matchAll(/\/\*\*((?:(?!\*\/).)*)\*\/\s*(\w+)\?:/gs)) {
    const tag = doc.match(/@default\s+(\S+)/);
    if (tag) {
      defaults[field] = JSON.parse(tag[1]);
    }
  }
  return defaults;
}

function readmeDefaults() {
  const line = readRepoFile("README.md").match(/^- Current Rust defaults are (.*)$/m);
  assert.ok(line, "missing README defaults line");
  const defaults = {};
  for (const [, field, value] of line[1].matchAll(/`(\w+): ([^`]+)`/g)) {
    defaults[field] = JSON.parse(value);
  }
  return defaults;
}

test("README and JSDoc defaults match the Rust render defaults", () => {
  const defaults = rustRenderDefaults();

  assert.deepEqual(jsdocDefaults(), defaults);
  assert.deepEqual(readmeDefaults(), defaults);
});

// Larger than the default working resolution and not uniform, so every
// default shows in the output: the step count, shape kinds, opacities,
// background, viewBox (working size) and output size.
const DEFAULTS_IMAGE = rgbPng(300, 200, (x, y) => [x % 256, y, (x * y) % 256]);

// One cheap step at a small working size, except for the option under test.
function defaultsBase(option) {
  const render = { count: 1, resizeInput: 16, seed: 7 };
  delete render[option];
  return render;
}

test("an omitted option renders like its explicit Rust default through the API", async () => {
  for (const [option, value] of Object.entries(rustRenderDefaults())) {
    const render = defaultsBase(option);
    const omitted = await approximate({ input: DEFAULTS_IMAGE, output: "svg", render });
    const explicit = await approximate({
      input: DEFAULTS_IMAGE,
      output: "svg",
      render: { ...render, [option]: value },
    });

    assert.deepEqual(omitted, explicit, option);
  }
});

test("an omitted option renders like its explicit Rust default through the CLI", async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-defaults-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const input = path.join(dir, "gradient.png");
  fs.writeFileSync(input, DEFAULTS_IMAGE);

  await Promise.all(
    Object.entries(rustRenderDefaults()).map(async ([option, value]) => {
      const render = defaultsBase(option);
      const [omitted, explicit] = await Promise.all([
        runCli([input, "-o", "-", ...renderArgs(render)]),
        runCli([input, "-o", "-", ...renderArgs({ ...render, [option]: value })]),
      ]);

      assert.equal(omitted.status, 0, omitted.stderr);
      assert.equal(explicit.status, 0, explicit.stderr);
      assert.match(omitted.stdout.toString("utf8"), /^<svg\b/, option);
      assert.ok(omitted.stdout.equals(explicit.stdout), option);
    }),
  );
});

// --- Checks without a runtime equivalent ---

// The binding's dependency graph is an architecture rule, not observable
// behavior.
test("binding depends on primeval-render only", () => {
  const manifest = readRepoFile("binding", "Cargo.toml");

  assert.match(manifest, /^primeval-render = /m);
  assert.doesNotMatch(manifest, /primeval-core/);
});

test("obsolete rust cli tree is absent", () => {
  assert.equal(fs.existsSync(path.join(repoRoot, "crates", "primeval-cli")), false);
});

test("package test script exercises the native package path", () => {
  const pkg = JSON.parse(readRepoFile("package.json"));

  assert.match(pkg.scripts.test, /test\/native\.test\.js/);
});
