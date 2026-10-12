// Runs a wasm32-wasip1 command under Node's WASI, with its arguments, and
// exits with its status: cargo's target runner for the kernel tests
// (`node scripts/build-wasm.mjs test-kernels`).
//
// Usage: node scripts/wasi-runner.mjs <module.wasm> [args...]
import fs from "node:fs";
import { WASI } from "node:wasi";

const [file, ...args] = process.argv.slice(2);
if (file === undefined) {
  console.error("Usage: node scripts/wasi-runner.mjs <module.wasm> [args...]");
  process.exit(2);
}

const wasi = new WASI({ version: "preview1", args: [file, ...args], env: process.env });
const module = await WebAssembly.compile(fs.readFileSync(file));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
