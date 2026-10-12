// Builds the two WebAssembly variants of `primeval-wasm` into `wasm/`:
//
// - `wasm/single/`: single-threaded, on the pinned stable toolchain
//   (rust-toolchain.toml);
// - `wasm/threaded/`: rayon on Web Workers, on the dated nightly below, with
//   the standard library rebuilt for atomics.
//
// Both require WebAssembly SIMD (simd128), for the scoring kernels of
// `primeval-core`; `test-kernels` runs their parity tests on wasm32-wasip1
// under Node's WASI.
//
// This file is the single source of truth for the nightly pin, the SIMD and
// threaded RUSTFLAGS, the build-std flags, the output layout and the size
// budget; CI and the docs call its subcommands (`install-tools` installs what
// the builds need) instead of repeating them, and the other scripts import
// the layout.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

import { lockedVersion } from "./napi-targets.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TARGET = "wasm32-unknown-unknown";
/** The target the kernel tests run on: libtest needs WASI for its output. */
const TEST_TARGET = "wasm32-wasip1";
const PROFILE = "wasm-release";
const CRATE = "primeval-wasm";
const OUT_NAME = "primeval";

/**
 * The files wasm-bindgen writes into each variant's `outDir`, besides the
 * `snippets/` the glue imports.
 */
export const OUTPUT_FILES = { glue: `${OUT_NAME}.js`, wasm: `${OUT_NAME}_bg.wasm` };

/**
 * The nightly for the threaded build, which needs `-Z build-std`. Bump it
 * deliberately, like the stable pin, and re-check the `atomics` target
 * feature, which nightly is phasing out (rust-lang/rust#162235).
 */
export const NIGHTLY_TOOLCHAIN = "nightly-2026-09-25";

/**
 * Every wasm32 build enables SIMD, which every current browser supports
 * (Chrome 91, Firefox 89, Safari 16.4); `primeval-core` then compiles its
 * simd128 scoring kernels.
 */
export const SIMD_RUSTFLAGS = ["-C", "target-feature=+simd128"];

/**
 * Threads need atomics, and a memory that is imported, shared and capped
 * (1 GiB), with the TLS exports wasm-bindgen uses to start each thread.
 */
export const THREADS_RUSTFLAGS = [
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
];

/** The most each `.wasm` may weigh, gzip -9, since the page downloads one. */
export const GZIP_BUDGET_BYTES = 512 * 1024;

/** Fails if `output`, `gzipBytes` long after gzip -9, is over the budget. */
export function checkSizeBudget(output, gzipBytes) {
  if (gzipBytes > GZIP_BUDGET_BYTES) {
    throw new Error(
      `${output} is ${gzipBytes} bytes gzip -9, over the budget of ${GZIP_BUDGET_BYTES} bytes ` +
        `(${GZIP_BUDGET_BYTES / 1024} KiB)`,
    );
  }
}

/** Rebuilds std with the atomics flags; nightly-only. */
const BUILD_STD = ["-Z", "build-std=panic_abort,std"];

function variant({ name, targetDir, toolchain, cargoArgs, env, shared }) {
  return {
    name,
    targetDir,
    toolchain,
    cargoArgs,
    env,
    shared,
    wasm: path.join(targetDir, TARGET, PROFILE, `${CRATE.replaceAll("-", "_")}.wasm`),
    // POSIX separators: also the path inside the npm tarball.
    outDir: `wasm/${name}`,
  };
}

/** The two builds, by name; paths are relative to the repository root. */
export const VARIANTS = {
  single: variant({
    name: "single",
    targetDir: "target",
    toolchain: null,
    cargoArgs: [],
    env: { RUSTFLAGS: SIMD_RUSTFLAGS.join(" ") },
    shared: false,
  }),
  // A target dir of its own, so the two builds do not invalidate each other.
  threaded: variant({
    name: "threaded",
    targetDir: path.join("target", "wasm-threads"),
    toolchain: NIGHTLY_TOOLCHAIN,
    cargoArgs: ["--features", "threads", ...BUILD_STD],
    env: { RUSTFLAGS: [...SIMD_RUSTFLAGS, ...THREADS_RUSTFLAGS].join(" ") },
    shared: true,
  }),
};

function cargoCommand(build, subcommand, args) {
  return {
    command: "cargo",
    args: [
      ...(build.toolchain ? [`+${build.toolchain}`] : []),
      subcommand,
      "-p",
      CRATE,
      ...args,
      "--target",
      TARGET,
      "--target-dir",
      build.targetDir,
      ...build.cargoArgs,
    ],
    env: build.env,
  };
}

/** The cargo invocation that builds one variant's `.wasm`. */
export function cargoBuildCommand(build) {
  return cargoCommand(build, "build", ["--profile", PROFILE]);
}

/**
 * Clippy for wasm32 on the pinned stable toolchain, as the single-threaded
 * build compiles it. The threaded variant adds only the `threads` re-export,
 * which its nightly build compiles; nightly clippy is not used, so a nightly
 * bump cannot bring new lints to the whole workspace.
 */
export function cargoClippyCommand() {
  const command = cargoCommand(VARIANTS.single, "clippy", []);
  command.args.push("--all-targets", "--", "-D", "warnings");
  return command;
}

/**
 * The tests of `primeval-core`'s `score` module, its scoring kernels, on
 * wasm32-wasip1 with SIMD: the simd128 kernels against the scalar ones, and a
 * test that fails without the flag. Cargo runs the test binary with
 * `wasi-runner.mjs` under this Node. The rest of the suite needs threads,
 * which WASI preview 1 lacks.
 */
export function cargoKernelTestCommand() {
  const runner = [
    process.execPath,
    "--disable-warning=ExperimentalWarning",
    path.join(REPO_ROOT, "scripts", "wasi-runner.mjs"),
  ];
  return {
    command: "cargo",
    args: [
      "test",
      "-p",
      "primeval-core",
      "--lib",
      "--target",
      TEST_TARGET,
      "--config",
      `target.${TEST_TARGET}.runner=${JSON.stringify(runner)}`,
      "--",
      "score::",
      // A failing test aborts the run (panic = "abort"), so print its
      // message as it happens.
      "--nocapture",
    ],
    env: VARIANTS.single.env,
  };
}

/** wasm-bindgen arguments: ES-module glue for the web, name section removed. */
export function wasmBindgenArgs(build) {
  return [
    "--target",
    "web",
    "--remove-name-section",
    "--out-dir",
    build.outDir,
    "--out-name",
    OUT_NAME,
    build.wasm,
  ];
}

/** The version in `wasm-bindgen --version` output, or null. */
export function parseWasmBindgenVersion(output) {
  return output.match(/^wasm-bindgen (\d+\.\d+\.\d+\S*)\s*$/m)?.[1] ?? null;
}

/**
 * What `install-tools` runs: the nightly with rust-src (for build-std) and
 * the wasm target, then wasm-bindgen-cli at `required` (the crate's version
 * in Cargo.lock) built with its own lockfile, unless `installed` (the CLI's
 * version, or null) already matches.
 */
export function installToolsCommands({ required, installed }) {
  const commands = [
    {
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
        TARGET,
      ],
    },
  ];
  if (installed !== required) {
    commands.push({
      command: "cargo",
      args: ["install", "wasm-bindgen-cli", "--locked", "--version", required],
    });
  }
  return commands;
}

/** A reader of the LEB128 integers and byte runs of a wasm module. */
function wasmReader(bytes) {
  const magic = [0x00, 0x61, 0x73, 0x6d];
  if (bytes.length < 8 || magic.some((byte, index) => bytes[index] !== byte)) {
    throw new Error("not a wasm module");
  }
  const reader = {
    offset: 8,
    done: () => reader.offset >= bytes.length,
    byte() {
      if (reader.offset >= bytes.length) {
        throw new Error("truncated wasm module");
      }
      return bytes[reader.offset++];
    },
    u32() {
      let result = 0;
      let shift = 0;
      let next;
      do {
        next = reader.byte();
        result += (next & 0x7f) * 2 ** shift;
        shift += 7;
      } while (next & 0x80);
      return result;
    },
    skip(length) {
      if (reader.offset + length > bytes.length) {
        throw new Error("truncated wasm module");
      }
      reader.offset += length;
    },
    string() {
      const length = reader.u32();
      const start = reader.offset;
      reader.skip(length);
      return new TextDecoder().decode(bytes.subarray(start, start + length));
    },
  };
  return reader;
}

/**
 * The features LLVM recorded in a wasm module's `target_features` custom
 * section, each with its prefix (`+simd128`), or null without one;
 * wasm-bindgen keeps the section.
 */
export function targetFeatures(bytes) {
  const reader = wasmReader(bytes);
  while (!reader.done()) {
    const id = reader.byte();
    const size = reader.u32();
    const end = reader.offset + size;
    if (id === 0 && reader.string() === "target_features") {
      const features = [];
      for (let count = reader.u32(); count > 0; count--) {
        const prefix = String.fromCharCode(reader.byte());
        features.push(`${prefix}${reader.string()}`);
      }
      return features;
    }
    reader.skip(end - reader.offset);
  }
  return null;
}

/**
 * The memory a wasm module imports, as `{ shared, minimum, maximum }` (in
 * 64 KiB pages; `maximum` is null without one), or null if it imports none.
 * Reads only the import section.
 */
export function importedMemory(bytes) {
  const { byte, u32, skip, done } = wasmReader(bytes);
  const limits = () => {
    const flags = byte();
    const minimum = u32();
    const maximum = flags & 0x01 ? u32() : null;
    return { shared: (flags & 0x02) !== 0, minimum, maximum };
  };

  while (!done()) {
    const id = byte();
    const size = u32();
    if (id !== 2) {
      skip(size);
      continue;
    }
    const count = u32();
    for (let index = 0; index < count; index++) {
      skip(u32()); // module name
      skip(u32()); // field name
      const kind = byte();
      switch (kind) {
        case 0x00: // function: type index
          u32();
          break;
        case 0x01: // table: element type, limits
          byte();
          limits();
          break;
        case 0x02:
          return limits();
        case 0x03: // global: value type, mutability
          byte();
          byte();
          break;
        case 0x04: // tag: attribute, type index
          byte();
          u32();
          break;
        default:
          throw new Error(`unknown wasm import kind ${kind}`);
      }
    }
    return null;
  }
  return null;
}

/**
 * Fails if `bytes`, the `.wasm` written to `output` by `build`, lacks SIMD or
 * does not import a shared memory exactly when the build is threaded. A set
 * `CARGO_ENCODED_RUSTFLAGS` overrides `RUSTFLAGS` and would drop the flags.
 */
export function checkBuildOutput(build, output, bytes) {
  const shared = importedMemory(bytes)?.shared ?? false;
  if (shared !== build.shared) {
    throw new Error(
      `${output} must ${build.shared ? "" : "not "}import a shared memory; ` +
        "check the threaded RUSTFLAGS and that CARGO_ENCODED_RUSTFLAGS is unset",
    );
  }
  if (!(targetFeatures(bytes) ?? []).includes("+simd128")) {
    throw new Error(
      `${output} must enable WebAssembly SIMD (simd128); ` +
        "check SIMD_RUSTFLAGS and that CARGO_ENCODED_RUSTFLAGS is unset",
    );
  }
}

function run({ command, args, env = {} }) {
  console.log(
    `$ ${Object.entries(env)
      .map(([key, value]) => `${key}='${value}' `)
      .join("")}${command} ${args.join(" ")}`,
  );
  const result = spawnSync(command, args, {
    cwd: REPO_ROOT,
    env: { ...process.env, ...env },
    stdio: "inherit",
  });
  if (result.error) {
    throw new Error(`${command} could not start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args[0]} failed with status ${result.status ?? result.signal}`);
  }
}

function requiredWasmBindgenVersion() {
  const cargoLock = fs.readFileSync(path.join(REPO_ROOT, "Cargo.lock"), "utf8");
  const version = lockedVersion(cargoLock, "wasm-bindgen");
  if (version === null) {
    throw new Error("Cargo.lock has no wasm-bindgen package");
  }
  return version;
}

/** The wasm-bindgen CLI to run. */
function wasmBindgenCommand() {
  return process.env.WASM_BINDGEN || "wasm-bindgen";
}

/** The version of the installed wasm-bindgen CLI, or null. */
function installedWasmBindgenVersion() {
  const result = spawnSync(wasmBindgenCommand(), ["--version"], { encoding: "utf8" });
  return result.status === 0 ? parseWasmBindgenVersion(result.stdout) : null;
}

/** The wasm-bindgen CLI to run; it must match the crate in Cargo.lock. */
function checkedWasmBindgen() {
  const command = wasmBindgenCommand();
  const required = requiredWasmBindgenVersion();
  const found = installedWasmBindgenVersion();
  if (found !== required) {
    throw new Error(
      `${command} ${found ?? "is not installed"}; the wasm-bindgen crate is ${required}. ` +
        "Install the matching CLI: node scripts/build-wasm.mjs install-tools",
    );
  }
  return command;
}

function build(names) {
  const wasmBindgen = checkedWasmBindgen();
  for (const name of names) {
    const variantBuild = VARIANTS[name];
    run(cargoBuildCommand(variantBuild));
    fs.rmSync(path.join(REPO_ROOT, variantBuild.outDir), { recursive: true, force: true });
    run({ command: wasmBindgen, args: wasmBindgenArgs(variantBuild) });

    const output = `${variantBuild.outDir}/${OUTPUT_FILES.wasm}`;
    const bytes = fs.readFileSync(path.join(REPO_ROOT, output));
    const gzip = gzipSync(bytes, { level: 9 }).length;
    try {
      checkBuildOutput(variantBuild, output, bytes);
      checkSizeBudget(output, gzip);
    } catch (error) {
      // Never leave a failed build where packaging would find it.
      fs.rmSync(path.join(REPO_ROOT, variantBuild.outDir), { recursive: true, force: true });
      throw error;
    }
    console.log(
      `${output}: ${bytes.length} bytes raw, ${gzip} bytes gzip -9, ` +
        `${variantBuild.shared ? "shared" : "unshared"} memory, SIMD`,
    );
  }
}

function usage() {
  console.error(
    [
      "Usage:",
      "  node scripts/build-wasm.mjs [build [single|threaded]]  build into wasm/ (default: both)",
      "  node scripts/build-wasm.mjs clippy                     clippy for wasm32 on the pinned stable",
      "  node scripts/build-wasm.mjs test-kernels               the scoring kernels' tests on wasm32-wasip1",
      "  node scripts/build-wasm.mjs install-tools              install the nightly and wasm-bindgen-cli",
      "  node scripts/build-wasm.mjs wasm-bindgen-version       print the required wasm-bindgen-cli version",
    ].join("\n"),
  );
}

function main([command = "build", ...args]) {
  try {
    switch (command) {
      case "build": {
        const names = args.length > 0 ? args : Object.keys(VARIANTS);
        for (const name of names) {
          if (!Object.hasOwn(VARIANTS, name)) {
            throw new Error(`unknown variant: ${name}`);
          }
        }
        build(names);
        break;
      }
      case "clippy":
        run(cargoClippyCommand());
        break;
      case "test-kernels":
        run(cargoKernelTestCommand());
        break;
      case "install-tools": {
        const required = requiredWasmBindgenVersion();
        const installed = installedWasmBindgenVersion();
        for (const step of installToolsCommands({ required, installed })) {
          run(step);
        }
        if (installed === required) {
          console.log(`wasm-bindgen-cli ${required} is already installed`);
        }
        break;
      }
      case "wasm-bindgen-version":
        console.log(requiredWasmBindgenVersion());
        break;
      default:
        usage();
        throw new Error(`unknown command: ${command}`);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
