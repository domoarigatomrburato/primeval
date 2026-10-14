# 0009. Browser calls run in Web Workers, started one at a time, with one idle worker reused

Status: Accepted

Date: 2026-10-13

## Context

In the browser the engine runs as WebAssembly ([0001](0001-rust-engine-behind-two-bindings.md)), synchronously, for hundreds of milliseconds to seconds. Run on the page's main thread it would freeze the page, and it could not be aborted: a synchronous render only stops when its thread is terminated.

Threads in WebAssembly need `SharedArrayBuffer`, which exists only on a cross-origin isolated page (COOP and COEP headers, `crossOriginIsolated === true`); many sites cannot set those headers, and GitHub Pages cannot. The threaded build also needs a dated nightly toolchain. A panic aborts the instance without its message, and a panic on a pool thread would leave the calling thread waiting forever.

WebKit fails module workers started together on a page that a service worker isolates (WebKit bug 261734): only the first loads, the others are refused under COEP. Started one at a time, all load.

Starting a fresh worker per call has a cost: the threaded build instantiates the module and starts one pool worker per core, one at a time, on every call, about 50 ms at 10 shapes on an Apple M2 Pro.

## Decision

- Each call runs in a module Web Worker ([`src/browser-runtime.ts`](../../../src/browser-runtime.ts)), so the main thread stays free.
- The build is chosen before anything is fetched: `threaded` on a cross-origin isolated page, with a rayon pool of `navigator.hardwareConcurrency` threads on Web Workers ([`binding-wasm/src/pool.rs`](../../../binding-wasm/src/pool.rs)); `single` otherwise. Each build's `.wasm` is compiled once per page and posted to every worker.
- **Start gate.** Fresh workers load one at a time (`queueStart`): the next starts only once the previous one, and in the threaded build all its pool workers, have loaded. The threaded worker starts its pool workers one at a time too.
- **Idle reuse.** A worker whose call delivered a result stays idle for 10 s, at most one per page, and the next call runs there with its instance, pool and panic channel; a reused worker bypasses the start gate. Every call builds its model and random streams afresh.
- **Termination.** Every other outcome (an abort, a throwing `onProgress`, an error, a panic) terminates the worker at once with its instance, memory and pool; that is how an abort stops a synchronous render. Concurrent calls get separate workers and memory.
- **Panics.** A hook reports `render panicked: <message>` before the abort, from a pool thread through a `BroadcastChannel` the page opened for that worker ([`binding-wasm/src/panic.rs`](../../../binding-wasm/src/panic.rs)), so the page rejects with `InternalError` instead of waiting.
- The demo, on GitHub Pages, isolates itself with a service worker ([`demo/coi-sw.js`](../../../demo/coi-sw.js)) that adds COOP and COEP.

## Consequences

- Options, defaults, errors and output bytes are those of Node: the same Rust code runs, and the browser tests compare SVG and PNG bytes with the native addon's.
- Reuse made a threaded call of 10 shapes take 0.81–0.82 of its time (about 50 ms less) and one of 100 shapes 0.965–0.969; the single-threaded build is unchanged (0.99–1.00). The price is an idle worker's memory for up to 10 s; WebAssembly memory never shrinks.
- The start gate makes Safari get threads under the demo's service worker, at about 20 ms to the first progress and about 105 ms more for the second of two concurrent calls in Chromium.
- Tests in [`test/browser/approximate.test.js`](../../../test/browser/approximate.test.js): "workers load one at a time, also across concurrent calls", "an aborted call waiting to start rejects at once and starts no worker", the "worker reuse" tests (fresh and reused outputs match, at most one idle worker, idle expiry, failures terminate every worker, reuse bypasses a held gate), and the abort and panic tests; [`test/browser/webkit.test.js`](../../../test/browser/webkit.test.js) covers WebKit locally (CI runs Chromium only).
- Never start the package's workers concurrently again while the WebKit bug is open. If it is fixed, the gate can go; the reuse stands on its own.

## Evidence

- `1b25053` (2026-10-02): the browser entry, the two builds, the pool and the panic channel; [`../plans/2026-10-wasm-browser-plan.md`](../plans/2026-10-wasm-browser-plan.md) records the spike's measurements.
- `9513203` (2026-10-12): the start gate; `c31551f` (2026-10-13): idle reuse; `9dd1638` (2026-10-13): the simplified worker lifecycle.
- [Runtime and release](../tuning.md#runtime-and-release) in `tuning.md`.
