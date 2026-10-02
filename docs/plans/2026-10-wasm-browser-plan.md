# Browser support through WebAssembly, and a demo on GitHub Pages

Status: proposed (2026-10-02). Supersedes the "Browser/WASM support is explicitly out of scope" rule in `AGENTS.md` once W1 lands. Comes before the algorithm change (section 16 of `2026-10-audit-and-refactor-plan.md`); everything here except the engine itself carries over to a new engine.

## Goal

`@aleburato/primeval` runs in the browser with the same `approximate()` API as on Node, multithreaded from the first release, and a demo app on GitHub Pages shows it off. The image never leaves the browser.

## Constraints found up front

- **Threads need nightly Rust.** Threaded WebAssembly needs the standard library rebuilt with atomics (`-Z build-std`, `+atomics,+bulk-memory`), which is nightly-only; `wasm-bindgen-rayon` provides the rayon pool on Web Workers and needs wasm-bindgen's `--target web`. Its docs recommend a fixed nightly. The main gate stays on the pinned stable toolchain; only the wasm build uses a second, dated nightly pin.
- **Threads need cross-origin isolation.** `SharedArrayBuffer` exists only on pages served with COOP/COEP headers (`crossOriginIsolated === true`). Many sites cannot set them. So the package ships **two wasm builds**: threaded (used when the page is isolated) and single-threaded on stable (used otherwise), chosen at runtime; only one is fetched.
- **GitHub Pages cannot set headers.** The demo uses a service worker that adds COOP/COEP (the `coi-serviceworker` approach, vendored with its licence).
- **No NEON in wasm.** The scalar kernels run (they are parity-tested against NEON). SIMD128 kernels are a later, optional step.
- **`SystemTime::now()` panics on wasm32-unknown-unknown.** The engine's default seed must come from the caller there (`crypto.getRandomValues`).

## Design

- **Rust:** a new crate `binding-wasm` (`primeval-wasm`, `publish = false`) over `primeval-render`, so decoding (`image`), validation, defaults, errors and the SVG/PNG writers are the same code as on Node. Rust stays the only owner of defaults and vocabularies.
- **JS API:** the same `approximate(request)` with the same options, result types, error classes and codes, `onProgress` and `AbortSignal`. In the browser it runs in a module Web Worker the package starts itself (`new Worker(new URL("./worker.js", import.meta.url), { type: "module" })`, the pattern Vite, webpack 5, Rollup and Parcel understand); the threaded build starts its rayon pool there (`initThreadPool(navigator.hardwareConcurrency)`).
- **Cancellation:** with the threaded build, a `SharedArrayBuffer` flag the render's cancellation token reads before each step (same semantics as Node); without it, the worker is terminated and a fresh one is started for the next call.
- **Input/output:** input `Uint8Array` (and `ArrayBuffer`); PNG output is a `Uint8Array` in the browser (a `Buffer` on Node). Decode limits and option bounds are the Rust ones.
- **Packaging:** conditional exports in the one root package: `"node"` → the native addon as today, `"browser"` → the wasm entry. The wasm files ship in the root package (not as platform packages); a size budget is enforced in CI.
- **Determinism:** the same seed gives the same shapes for any thread count (ENG-4). Whether wasm and native match bit for bit depends on `f64` transcendental functions (`sin`, `cos`, …): measure it in W0; if they differ, decide whether to route them through the `libm` crate everywhere for cross-platform identical output.

## Proposed public addition (needs the maintainer's yes)

For a demo that draws shapes as they are found, `onProgress` info gains the shape just added, as an SVG element string in working-resolution coordinates (`shape`). It applies to Node and the browser alike (not the CLI). Without it, the demo shows progress and the final SVG only.

## Slices

| Slice | Content | Done when |
| --- | --- | --- |
| W0 spike | Compile the engine to both wasm builds; measure wasm size (raw and gzip), time per step in headless Chromium at 1 thread and N threads vs native, and native/wasm SVG equality for fixed seeds. | Numbers recorded here; go/no-go on the design. |
| W1 wasm crate | `binding-wasm`, the build script (pinned nightly for the threaded build, stable for the other), caller-supplied seed, `AGENTS.md` direction updated, CI builds both. | Both builds produced in CI; Rust tests unchanged. |
| W2 browser runtime | Worker, build selection, thread pool, progress, cancellation, error mapping; tests in headless Chromium (Playwright, pinned) with and without cross-origin isolation. | API parity tests pass in the browser; the Node test suite is untouched. |
| W3 packaging | Conditional exports, types, packed-install test for the browser entry, a Vite and a webpack fixture build, size budget, README "Browser" section. | `npm run verify` covers it; the tarball contains both builds. |
| W4 live shapes | `onProgress` `shape` (if approved), in Rust render, binding, wasm and TypeScript, documented. | Contract tests cover it. |
| W5 demo | `demo/`: drop an image, pick shape, count and alpha, watch it draw, compare with the original, download SVG/PNG; deployed to GitHub Pages by a workflow, with the COOP/COEP service worker. | Live at `https://domoarigatomrburato.github.io/primeval/`, linked from the README. |
| Later | WASM fallback on Node where no native prebuild exists (StackBlitz/WebContainers, musl before REL-7), SIMD128 kernels. | Separate decisions. |

## Risks

- Bundler compatibility for wasm plus workers is the main maintenance cost; the fixture builds in W3 exist to catch regressions.
- The nightly pin can break with wasm-bindgen updates; bump it deliberately, like the stable pin.
- Threaded builds roughly double the shipped wasm; only one is downloaded at runtime.
