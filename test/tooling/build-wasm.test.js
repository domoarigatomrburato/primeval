import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import {
  cargoBuildCommand,
  cargoClippyCommand,
  checkSizeBudget,
  GZIP_BUDGET_BYTES,
  importedMemory,
  installToolsCommands,
  NIGHTLY_TOOLCHAIN,
  OUTPUT_FILES,
  parseWasmBindgenVersion,
  THREADS_RUSTFLAGS,
  VARIANTS,
  wasmBindgenArgs,
} from "../../scripts/build-wasm.mjs";
import { lockedVersion } from "../../scripts/napi-targets.mjs";

const repoRoot = process.cwd();
const readRepoFile = (...parts) => fs.readFileSync(path.join(repoRoot, ...parts), "utf8");

// --- Tiny wasm binaries ---

function leb128(value) {
  const bytes = [];
  let rest = value;
  do {
    let byte = rest & 0x7f;
    rest >>>= 7;
    if (rest !== 0) {
      byte |= 0x80;
    }
    bytes.push(byte);
  } while (rest !== 0);
  return bytes;
}

const name = (text) => [...leb128(text.length), ...Buffer.from(text, "utf8")];
const section = (id, payload) => [id, ...leb128(payload.length), ...payload];
const vector = (items) => [...leb128(items.length), ...items.flat()];

const funcImport = [...name("env"), ...name("f"), 0x00, ...leb128(0)];
const memoryImport = (limits) => [...name("env"), ...name("memory"), 0x02, ...limits];
// One `() -> ()` function type, so the function import is valid.
const typeSection = section(1, vector([[0x60, 0x00, 0x00]]));
const customSection = section(0, [...name("producers"), 0x01, 0x02]);

function wasm(...imports) {
  return Uint8Array.from([
    ...[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
    ...customSection,
    ...typeSection,
    ...section(2, vector(imports)),
  ]);
}

test("importedMemory reads a shared memory import with a maximum", () => {
  const bytes = wasm(funcImport, memoryImport([0x03, ...leb128(17), ...leb128(16384)]));

  assert.deepEqual(importedMemory(bytes), { shared: true, minimum: 17, maximum: 16384 });
});

test("importedMemory reads a non-shared memory import", () => {
  assert.deepEqual(importedMemory(wasm(memoryImport([0x01, ...leb128(1), ...leb128(2)]))), {
    shared: false,
    minimum: 1,
    maximum: 2,
  });
  assert.deepEqual(importedMemory(wasm(funcImport, memoryImport([0x00, ...leb128(300)]))), {
    shared: false,
    minimum: 300,
    maximum: null,
  });
});

test("importedMemory returns null without a memory import", () => {
  assert.equal(importedMemory(wasm(funcImport)), null);
  assert.equal(
    importedMemory(Uint8Array.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])),
    null,
  );
});

test("importedMemory rejects bytes that are not a wasm module", () => {
  assert.throws(() => importedMemory(Uint8Array.from([0x7f, 0x45, 0x4c, 0x46])), /not a wasm/);
  assert.throws(() => importedMemory(wasm(funcImport).slice(0, -2)), /truncated/);
});

// --- Versions ---

test("each .wasm has a budget of 512 KiB gzip -9", () => {
  assert.equal(GZIP_BUDGET_BYTES, 512 * 1024);
});

test("checkSizeBudget passes a .wasm at the budget and fails one byte over", () => {
  checkSizeBudget("wasm/single/primeval_bg.wasm", GZIP_BUDGET_BYTES);
  assert.throws(() => checkSizeBudget("wasm/threaded/primeval_bg.wasm", GZIP_BUDGET_BYTES + 1), {
    message:
      "wasm/threaded/primeval_bg.wasm is 524289 bytes gzip -9, over the budget of 524288 bytes (512 KiB)",
  });
});

test("parseWasmBindgenVersion reads the CLI's --version output", () => {
  assert.equal(parseWasmBindgenVersion("wasm-bindgen 0.2.129\n"), "0.2.129");
  assert.equal(parseWasmBindgenVersion("something else"), null);
});

// --- Build commands ---

test("the threaded build uses exactly the W0 RUSTFLAGS", () => {
  assert.deepEqual(THREADS_RUSTFLAGS, [
    "-C",
    "target-feature=+atomics,+bulk-memory",
    "-C",
    "link-arg=--shared-memory",
    "-C",
    "link-arg=--max-memory=1073741824",
    "-C",
    "link-arg=--import-memory",
    "-C",
    "link-arg=--export=__wasm_init_tls",
    "-C",
    "link-arg=--export=__tls_size",
    "-C",
    "link-arg=--export=__tls_align",
    "-C",
    "link-arg=--export=__tls_base",
  ]);
});

test("the single-threaded build uses the pinned stable toolchain", () => {
  const { command, args, env } = cargoBuildCommand(VARIANTS.single);

  assert.equal(command, "cargo");
  assert.deepEqual(args, [
    "build",
    "-p",
    "primeval-wasm",
    "--profile",
    "wasm-release",
    "--target",
    "wasm32-unknown-unknown",
    "--target-dir",
    "target",
  ]);
  assert.deepEqual(env, {});
  assert.equal(
    VARIANTS.single.wasm,
    path.join("target", "wasm32-unknown-unknown", "wasm-release", "primeval_wasm.wasm"),
  );
  assert.equal(VARIANTS.single.outDir, "wasm/single");
  assert.equal(VARIANTS.single.shared, false);
});

test("the threaded build uses the nightly, build-std and its own target dir", () => {
  const { command, args, env } = cargoBuildCommand(VARIANTS.threaded);

  assert.equal(command, "cargo");
  assert.deepEqual(args, [
    `+${NIGHTLY_TOOLCHAIN}`,
    "build",
    "-p",
    "primeval-wasm",
    "--profile",
    "wasm-release",
    "--target",
    "wasm32-unknown-unknown",
    "--target-dir",
    path.join("target", "wasm-threads"),
    "--features",
    "threads",
    "-Z",
    "build-std=panic_abort,std",
  ]);
  assert.deepEqual(env, { RUSTFLAGS: THREADS_RUSTFLAGS.join(" ") });
  assert.equal(
    VARIANTS.threaded.wasm,
    path.join(
      "target",
      "wasm-threads",
      "wasm32-unknown-unknown",
      "wasm-release",
      "primeval_wasm.wasm",
    ),
  );
  assert.equal(VARIANTS.threaded.outDir, "wasm/threaded");
  assert.equal(VARIANTS.threaded.shared, true);
});

test("clippy checks the single-threaded variant on the pinned stable toolchain", () => {
  const command = cargoClippyCommand();
  assert.deepEqual(command.args, [
    "clippy",
    "-p",
    "primeval-wasm",
    "--target",
    "wasm32-unknown-unknown",
    "--target-dir",
    "target",
    "--all-targets",
    "--",
    "-D",
    "warnings",
  ]);
  assert.deepEqual(command.env, {});
});

test("wasm-bindgen emits web glue without the name section", () => {
  assert.deepEqual(wasmBindgenArgs(VARIANTS.single), [
    "--target",
    "web",
    "--remove-name-section",
    "--out-dir",
    "wasm/single",
    "--out-name",
    "primeval",
    VARIANTS.single.wasm,
  ]);
});

test("each variant's output is wasm-bindgen's glue and .wasm, named after OUT_NAME", () => {
  assert.deepEqual(OUTPUT_FILES, { glue: "primeval.js", wasm: "primeval_bg.wasm" });
  assert.deepEqual(Object.keys(VARIANTS), ["single", "threaded"]);
});

// --- Tool installation ---

const NIGHTLY_INSTALL = {
  command: "rustup",
  args: [
    "toolchain",
    "install",
    NIGHTLY_TOOLCHAIN,
    "--profile",
    "minimal",
    "--component",
    "rust-src",
    "--target",
    "wasm32-unknown-unknown",
  ],
};

test("install-tools installs the nightly and the locked wasm-bindgen-cli", () => {
  assert.deepEqual(installToolsCommands({ required: "0.2.129", installed: null }), [
    NIGHTLY_INSTALL,
    {
      command: "cargo",
      args: ["install", "wasm-bindgen-cli", "--locked", "--version", "0.2.129"],
    },
  ]);
  assert.deepEqual(installToolsCommands({ required: "0.2.129", installed: "0.2.128" })[1], {
    command: "cargo",
    args: ["install", "wasm-bindgen-cli", "--locked", "--version", "0.2.129"],
  });
});

test("install-tools skips wasm-bindgen-cli when the installed one matches", () => {
  assert.deepEqual(installToolsCommands({ required: "0.2.129", installed: "0.2.129" }), [
    NIGHTLY_INSTALL,
  ]);
});

// --- Pins and wiring ---

test("subcommands print the locked wasm-bindgen version and reject unknown ones", () => {
  const run = (command) =>
    spawnSync(process.execPath, ["scripts/build-wasm.mjs", command], { encoding: "utf8" });

  assert.equal(
    run("wasm-bindgen-version").stdout,
    `${lockedVersion(readRepoFile("Cargo.lock"), "wasm-bindgen")}\n`,
  );
  for (const removed of ["nightly", "install-nightly"]) {
    const result = run(removed);
    assert.equal(result.status, 1, removed);
    assert.match(result.stderr, new RegExp(`unknown command: ${removed}`));
  }
  assert.match(NIGHTLY_TOOLCHAIN, /^nightly-\d{4}-\d{2}-\d{2}$/);
});

test("the nightly and wasm-bindgen-cli pins have one source", () => {
  const sources = [
    "package.json",
    "AGENTS.md",
    "CONTRIBUTING.md",
    "RELEASING.md",
    ...fs
      .readdirSync(path.join(repoRoot, ".github", "workflows"))
      .map((file) => path.join(".github", "workflows", file)),
  ];
  for (const file of sources) {
    const source = readRepoFile(file);
    assert.doesNotMatch(source, /nightly-\d{4}-\d{2}-\d{2}/, `${file} repeats the nightly pin`);
    assert.doesNotMatch(
      source,
      /wasm-bindgen-cli[^\n]*\d+\.\d+\.\d+/,
      `${file} repeats the wasm-bindgen-cli version`,
    );
    assert.doesNotMatch(
      source,
      /install-nightly|cargo install wasm-bindgen-cli/,
      `${file} repeats the tool installation instead of running install-tools`,
    );
  }
  for (const file of ["AGENTS.md", "CONTRIBUTING.md"]) {
    assert.match(readRepoFile(file), /node scripts\/build-wasm\.mjs install-tools/, file);
  }

  const quality = readRepoFile(".github", "workflows", "quality.yml");
  const wasmJob = quality.slice(quality.indexOf("  wasm-checks:"), quality.indexOf("  hygiene:"));
  const order = [
    "- run: rustup toolchain install\n",
    "uses: Swatinem/rust-cache@",
    // After the cache restore, so a cached wasm-bindgen-cli is found and kept.
    "- run: node scripts/build-wasm.mjs install-tools\n",
    "- run: npm run verify:wasm\n",
  ];
  let last = -1;
  for (const step of order) {
    const index = wasmJob.indexOf(step);
    assert.ok(index > last, `wasm-checks: ${step.trim()} missing or out of order`);
    last = index;
  }
});

test("the pinned stable toolchain has the wasm target", () => {
  assert.match(readRepoFile("rust-toolchain.toml"), /^targets = \["wasm32-unknown-unknown"\]$/m);
});

test("verify runs the wasm checks", () => {
  const { scripts } = JSON.parse(readRepoFile("package.json"));

  assert.equal(scripts["build:wasm"], "node scripts/build-wasm.mjs");
  assert.equal(
    scripts["verify:wasm"],
    "node scripts/build-wasm.mjs clippy && npm run build:wasm && npm run build && npm run build:node && node scripts/release-packages.mjs verify-root && npm run test:browser",
  );
  assert.equal(scripts["test:browser"], "node --test test/browser/*.test.js");
  assert.match(scripts.verify, /npm run verify:wasm/);
});

test("the browser tests use a pinned Playwright and its Chromium headless shell in CI", () => {
  const { devDependencies } = JSON.parse(readRepoFile("package.json"));
  assert.match(
    devDependencies.playwright,
    /^\d+\.\d+\.\d+$/,
    "playwright must be an exact version",
  );

  const quality = readRepoFile(".github", "workflows", "quality.yml");
  const wasmJob = quality.slice(quality.indexOf("  wasm-checks:"), quality.indexOf("  hygiene:"));
  assert.match(wasmJob, /- run: npm ci\n/);
  // The locally installed (lockfile) Playwright picks the browser version.
  assert.match(wasmJob, /- run: npx playwright install --with-deps --only-shell chromium\n/);
  assert.ok(
    wasmJob.indexOf("npm ci") < wasmJob.indexOf("npx playwright install") &&
      wasmJob.indexOf("npx playwright install") < wasmJob.indexOf("npm run verify:wasm"),
    "wasm-checks must install dependencies, then the browser, then verify",
  );
});
