import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import {
  cargoBuildCommand,
  cargoClippyCommand,
  cargoKernelTestCommand,
  checkBuildOutput,
  checkSizeBudget,
  GZIP_BUDGET_BYTES,
  importedMemory,
  installToolsCommands,
  NIGHTLY_TOOLCHAIN,
  OUTPUT_FILES,
  parseWasmBindgenVersion,
  SIMD_RUSTFLAGS,
  THREADS_RUSTFLAGS,
  targetFeatures,
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

const targetFeaturesSection = (...features) =>
  section(0, [
    ...name("target_features"),
    ...vector(features.map((feature) => [feature[0].charCodeAt(0), ...name(feature.slice(1))])),
  ]);

test("targetFeatures reads the features LLVM records in the target_features section", () => {
  const bytes = Uint8Array.from([
    ...wasm(funcImport),
    ...targetFeaturesSection("+bulk-memory", "+simd128", "-atomics"),
  ]);

  assert.deepEqual(targetFeatures(bytes), ["+bulk-memory", "+simd128", "-atomics"]);
});

test("targetFeatures returns null without a target_features section", () => {
  assert.equal(targetFeatures(wasm(funcImport)), null);
  assert.throws(() => targetFeatures(Uint8Array.from([0x7f, 0x45, 0x4c, 0x46])), /not a wasm/);
});

test("checkBuildOutput requires SIMD, and a shared memory exactly in the threaded build", () => {
  const sharedMemory = memoryImport([0x03, ...leb128(17), ...leb128(16384)]);
  const unsharedMemory = memoryImport([0x00, ...leb128(17)]);
  const simd = targetFeaturesSection("+bulk-memory", "+simd128");
  const noSimd = targetFeaturesSection("+bulk-memory");
  const module = (memory, features) => Uint8Array.from([...wasm(memory), ...features]);

  checkBuildOutput(VARIANTS.single, "single.wasm", module(unsharedMemory, simd));
  checkBuildOutput(VARIANTS.threaded, "threaded.wasm", module(sharedMemory, simd));
  assert.throws(
    () => checkBuildOutput(VARIANTS.single, "single.wasm", module(sharedMemory, simd)),
    {
      message:
        "single.wasm must not import a shared memory; " +
        "check the threaded RUSTFLAGS and that CARGO_ENCODED_RUSTFLAGS is unset",
    },
  );
  assert.throws(
    () => checkBuildOutput(VARIANTS.threaded, "threaded.wasm", module(unsharedMemory, simd)),
    { message: /^threaded\.wasm must import a shared memory; / },
  );
  for (const [build, memory] of [
    [VARIANTS.single, unsharedMemory],
    [VARIANTS.threaded, sharedMemory],
  ]) {
    for (const features of [noSimd, []]) {
      assert.throws(() => checkBuildOutput(build, "out.wasm", module(memory, features)), {
        message:
          "out.wasm must enable WebAssembly SIMD (simd128); " +
          "check SIMD_RUSTFLAGS and that CARGO_ENCODED_RUSTFLAGS is unset",
      });
    }
  }
});

// The signed LEB128 of a non-negative `i32.const` below 2^13.
const sleb128 = (value) => (value < 64 ? [value] : [0x80 | (value & 0x7f), value >> 7]);

// A WASI command whose `_start` returns, or calls `proc_exit(code)`.
function wasiCommand(code) {
  const body = code === null ? [0x00, 0x0b] : [0x00, 0x41, ...sleb128(code), 0x10, 0x00, 0x0b];
  return Uint8Array.from([
    ...[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
    ...section(
      1,
      vector([
        [0x60, 0x01, 0x7f, 0x00],
        [0x60, 0x00, 0x00],
      ]),
    ),
    ...section(2, vector([[...name("wasi_snapshot_preview1"), ...name("proc_exit"), 0x00, 0x00]])),
    ...section(3, vector([[0x01]])),
    ...section(5, vector([[0x00, 0x01]])),
    ...section(
      7,
      vector([
        [...name("_start"), 0x00, 0x01],
        [...name("memory"), 0x02, 0x00],
      ]),
    ),
    ...section(10, vector([[...leb128(body.length), ...body]])),
  ]);
}

test("the WASI runner exits with the status of the module it runs", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-wasi-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const config = cargoKernelTestCommand().args.find((arg) => arg.startsWith("target."));
  const [command, ...flags] = JSON.parse(config.slice(config.indexOf("=") + 1));
  for (const [code, status] of [
    [null, 0],
    [0, 0],
    [3, 3],
    [101, 101],
  ]) {
    const file = path.join(dir, `exit-${code}.wasm`);
    fs.writeFileSync(file, wasiCommand(code));
    const result = spawnSync(command, [...flags, file, "--some-arg"], { encoding: "utf8" });
    assert.equal(result.status, status, `${code}: ${result.stderr}`);
    assert.equal(result.stderr, "", `${code}`);
  }
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

test("both builds and clippy enable WebAssembly SIMD", () => {
  assert.deepEqual(SIMD_RUSTFLAGS, ["-C", "target-feature=+simd128"]);
  for (const build of Object.values(VARIANTS)) {
    assert.match(cargoBuildCommand(build).env.RUSTFLAGS, /^-C target-feature=\+simd128( |$)/);
  }
  assert.equal(cargoClippyCommand().env.RUSTFLAGS, "-C target-feature=+simd128");
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
  assert.deepEqual(env, { RUSTFLAGS: "-C target-feature=+simd128" });
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
  assert.deepEqual(env, { RUSTFLAGS: [...SIMD_RUSTFLAGS, ...THREADS_RUSTFLAGS].join(" ") });
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
  assert.deepEqual(command.env, { RUSTFLAGS: "-C target-feature=+simd128" });
});

test("the kernel tests run the score module's tests on wasm32-wasip1 with SIMD, under Node's WASI", () => {
  const command = cargoKernelTestCommand();
  const runner = [
    process.execPath,
    "--disable-warning=ExperimentalWarning",
    path.join(repoRoot, "scripts", "wasi-runner.mjs"),
  ];

  assert.equal(command.command, "cargo");
  assert.deepEqual(command.args, [
    "test",
    "-p",
    "primeval-core",
    "--lib",
    "--target",
    "wasm32-wasip1",
    "--config",
    `target.wasm32-wasip1.runner=${JSON.stringify(runner)}`,
    "--",
    "score::",
    "--nocapture",
  ]);
  assert.deepEqual(command.env, { RUSTFLAGS: "-C target-feature=+simd128" });
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

// The wasip1 std only the kernel tests use: added to the pinned stable here,
// not in rust-toolchain.toml, so the jobs that never run them do not download it.
const WASIP1_INSTALL = {
  command: "rustup",
  args: ["target", "add", "--toolchain", "1.99.0", "wasm32-wasip1"],
};

const CLI_INSTALL = {
  command: "cargo",
  args: ["install", "wasm-bindgen-cli", "--locked", "--version", "0.2.129"],
};

test("install-tools installs the nightly, the wasip1 target and the locked wasm-bindgen-cli", () => {
  assert.deepEqual(
    installToolsCommands({ required: "0.2.129", installed: null, stable: "1.99.0" }),
    [NIGHTLY_INSTALL, WASIP1_INSTALL, CLI_INSTALL],
  );
  assert.deepEqual(
    installToolsCommands({ required: "0.2.129", installed: "0.2.128", stable: "1.99.0" })[2],
    CLI_INSTALL,
  );
});

test("install-tools skips wasm-bindgen-cli when the installed one matches", () => {
  assert.deepEqual(
    installToolsCommands({ required: "0.2.129", installed: "0.2.129", stable: "1.99.0" }),
    [NIGHTLY_INSTALL, WASIP1_INSTALL],
  );
});

test("install-tools adds the wasip1 target to the channel rust-toolchain.toml pins", () => {
  const channel = readRepoFile("rust-toolchain.toml").match(/^channel = "([^"]+)"$/m)[1];
  const commands = installToolsCommands({
    required: "0.2.129",
    installed: "0.2.129",
    stable: channel,
  });

  assert.deepEqual(commands[1].args, ["target", "add", "--toolchain", channel, "wasm32-wasip1"]);
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

test("the pinned stable toolchain has the browser wasm target and not the kernel tests' wasip1", () => {
  const toolchain = readRepoFile("rust-toolchain.toml");

  assert.match(toolchain, /^targets = \["wasm32-unknown-unknown"\]$/m);
  assert.doesNotMatch(toolchain, /wasip1/);
});

test("verify runs the wasm checks", () => {
  const { scripts } = JSON.parse(readRepoFile("package.json"));

  assert.equal(scripts["build:wasm"], "node scripts/build-wasm.mjs");
  assert.equal(
    scripts["verify:wasm"],
    "node scripts/build-wasm.mjs clippy && node scripts/build-wasm.mjs test-kernels && npm run build:wasm && npm run build && npm run build:node && node scripts/release-packages.mjs verify-root && npm run test:browser",
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
