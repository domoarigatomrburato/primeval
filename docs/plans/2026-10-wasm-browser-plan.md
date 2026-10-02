# Browser support through WebAssembly, and a demo on GitHub Pages

Status: W0 done (2026-10-02), go; W1 next. Supersedes the "Browser/WASM support is explicitly out of scope" rule in `AGENTS.md` once W1 lands. Comes before the algorithm change (section 16 of `2026-10-audit-and-refactor-plan.md`); everything here except the engine itself carries over to a new engine.

## Goal

`@aleburato/primeval` runs in the browser with the same `approximate()` API as on Node, multithreaded from the first release, and a demo app on GitHub Pages shows it off. The image never leaves the browser.

## Constraints

- **Threads need nightly Rust.** Threaded WebAssembly needs the standard library rebuilt with atomics (`-Z build-std=panic_abort,std`), which is nightly-only; `wasm-bindgen-rayon` provides the rayon pool on Web Workers. The main gate stays on the pinned stable toolchain; only the threaded wasm build uses a second, dated nightly pin (`nightly-2026-09-25` works).
- **Threads need cross-origin isolation.** `SharedArrayBuffer` exists only on pages served with COOP/COEP headers (`crossOriginIsolated === true`). Many sites cannot set them. So the package ships **two wasm builds**: threaded (used when the page is isolated) and single-threaded on stable (used otherwise), chosen at runtime; only one is fetched.
- **GitHub Pages cannot set headers.** The demo uses a service worker that adds COOP/COEP (the `coi-serviceworker` approach, vendored with its licence).
- **No NEON in wasm.** The scalar kernels run (bit-identical to NEON by test). SIMD128 kernels are a later, optional step.
- **`SystemTime::now()` panics on wasm32-unknown-unknown.** The engine's default seed must come from the wasm binding there (`crypto.getRandomValues`); the engine itself is unchanged.
- **`panic = "abort"` on wasm.** A panic kills the instance, so a worker that panicked is never reused.

## W0 results (spike, 2026-10-02)

Spike sources, scripts and raw results: the session scratchpad `w0/` (copied into W1 as needed). Engine unchanged; Chromium headless shell 153 via Playwright 1.63.0; wasm-bindgen 0.2.129, wasm-bindgen-rayon 1.3.0; Apple M-series, 8 cores (4P + 4E), load average 2.5–3.7.

**Builds.** Single-threaded: `cargo +1.99.0 build --release --target wasm32-unknown-unknown`, then `wasm-bindgen --target web`. Threaded: the nightly with `-Z build-std=panic_abort,std` and

```text
RUSTFLAGS='-C target-feature=+atomics,+bulk-memory -C link-arg=--shared-memory
  -C link-arg=--max-memory=1073741824 -C link-arg=--import-memory
  -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size
  -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base'
```

Without the link args the memory is not shared and threads cannot work, so the build must check that the output imports a shared memory. Nightly warns that the `atomics` flag is being phased out (rust-lang/rust#162235): re-check at every nightly bump.

**Size** (opt-level 3, fat LTO, name section stripped; bytes):

| File | raw | gzip -9 | brotli 11 |
| --- | ---: | ---: | ---: |
| single-threaded `.wasm` | 1,189,945 | 430,233 | 321,318 |
| threaded `.wasm` | 1,221,501 | 442,299 | 330,456 |
| JS glue (single / threaded) | 9,869 / 14,316 | 2,776 / 3,780 | 2,478 / 3,350 |

`opt-level = "s"`/`"z"` save 6–17 % compressed but cost 16–55 % speed: keep 3. About 60 % of the code is the decoders (JPEG 15 %, WebP 10 %, PNG/deflate 8 %) and tiny-skia (30 %, PNG output only); the primeval crates are 11 %.

**Time per step** (ms, count 200, `monalisa.jpg`, seed 1, minimum of 3 alternating rounds):

| Configuration | triangle | any | rotated-ellipse | quadratic | circle |
| --- | ---: | ---: | ---: | ---: | ---: |
| native, 1 thread | 10.75 | 22.46 | 45.17 | 33.72 | 8.84 |
| native, 8 threads | 2.53 | 5.69 | 10.63 | 7.78 | 2.17 |
| wasm single-threaded | 17.06 | 34.61 | 65.89 | 62.89 | 15.76 |
| wasm threaded, 1 thread | 17.18 | 34.70 | 65.82 | 62.52 | 15.85 |
| wasm threaded, 4 threads | 5.27 | 10.73 | 19.67 | 19.60 | 5.09 |
| wasm threaded, 8 threads | 4.19 | 8.99 | 15.68 | 14.89 | 4.05 |

Wasm is 1.5–1.9× slower than native per thread (quadratic the most, plausibly because `f64::mul_add` becomes a software `fma` call in wasm; not verified) and scales like native (4.1× at 8 threads vs 4.3×). The threaded build costs nothing at 1 thread. Startup: glue import 2.5–4.6 ms, fetch + compile + instantiate 4.2–5.6 ms, `initThreadPool` 6–9 ms at 1 thread, 11 ms at 4, 19 ms at 8.

**Determinism.** SVG bytes were identical between native (1 and 8 threads), wasm single-threaded and wasm threaded (1 and 8 threads) in all 152 renders compared (9 shapes × seeds 1–3 × both corpus images at count 50; 9 shapes at count 200; 4 rotating shapes × seeds 4–13 × both images at count 200). This is observed, not guaranteed: `sin_cos` (`raster.rs`, `shapes.rs`), `hypot` (`raster.rs`) and `exp`/`ln` (`rand_distr` normal sampling, `shapes.rs`) differ between macOS libm and wasm for 4–12 % of inputs, and those differences are absorbed today by rounding to pixels or integers. Guaranteeing it would mean the `libm` crate on every platform, which changes native output for existing seeds; decide in the engine redesign. Until then the contract is "same seed, same output on the same platform", and a browser test compares wasm with native output for fixed seeds as a tripwire.

**Memory.** After 200 steps: 6.4 MiB single-threaded; 11 / 17 / 25 MiB threaded at 1 / 4 / 8 threads. The threaded build's shared memory is capped at 1 GiB; 12000² input works in both builds (≈ 480 MiB), and 16384² is rejected by the engine's 512 MiB decode limit before that. Wasm memory never shrinks.

**Failures.** A Rust error is a JS `Error` with the Rust message. A panic on the calling thread is `RuntimeError: unreachable` (the message needs a panic hook). A panic inside a rayon task **hangs the threaded build forever**: that pool worker dies and the caller waits. Without isolation the threaded build still instantiates, and only `initThreadPool` fails (`DataCloneError`), after the download.

## Design

- **Rust:** a new crate `binding-wasm` (`primeval-wasm`, `publish = false`) over `primeval-render`, so decoding, validation, defaults, errors and the SVG/PNG writers are the same code as on Node. Rust stays the only owner of defaults and vocabularies; the binding fills an absent seed from `crypto.getRandomValues`, because the engine's clock seed cannot run there. Built with opt-level 3, fat LTO, `panic = "abort"`, and the name section stripped.
- **JS API:** the same `approximate(request)` with the same options, result types, error classes and codes, `onProgress` and `AbortSignal`.
- **One worker per call.** Each `approximate()` call runs in a fresh module Web Worker that the package starts (`new Worker(new URL("./worker.js", import.meta.url), { type: "module" })`) and terminates when the call settles. Startup is a few tens of milliseconds against renders of seconds, and it solves four problems at once:
  - memory that never shrinks;
  - recovery from a panic, since an aborted instance is never reused;
  - cancellation, since `AbortSignal` terminates the worker in both builds;
  - isolation between concurrent calls.
  The compiled `WebAssembly.Module` is cached on the page and posted to each worker, so it is fetched and compiled once.
- **Build selection:** `globalThis.crossOriginIsolated === true` selects the threaded build, with `initThreadPool(navigator.hardwareConcurrency)` in the worker; otherwise the single-threaded build. Selection happens before any download.
- **Panics:** a panic hook in every thread (pool workers included) reports the message to the page on a per-call `BroadcastChannel`; the page then terminates the worker and rejects with the same error class Node uses for an internal failure. This also covers the hanging rayon task.
- **Input/output:** input `Uint8Array` (and `ArrayBuffer`); PNG output is a `Uint8Array` in the browser (a `Buffer` on Node). Decode limits and option bounds are the Rust ones.
- **Packaging:** conditional exports in the one root package: `"node"` → the native addon as today, `"browser"` → the wasm entry. The wasm files ship in the root package. Size budget enforced in CI: each `.wasm` at most 512 KiB gzip. wasm-bindgen-rayon's `no-bundler` variant (pool workers from `blob:` URLs, so strict pages need CSP `worker-src blob:`) is used from W2; W3 decides between it and the bundler variant with the fixture builds.

## Proposed public addition (approved 2026-10-02)

`onProgress` info gains the shape just added, as an SVG element string in working-resolution coordinates (`shape`), on Node and in the browser alike (not the CLI), so the demo can draw shapes as they are found.

## Slices

| Slice | Content | Done when |
| --- | --- | --- |
| W0 spike | Both builds, size, time per step, native/wasm equality, memory, failure modes. | Done 2026-10-02 (above); go. |
| W1 wasm crate | `binding-wasm`, the build script (nightly pin for the threaded build, stable for the other, link args, shared-memory check, name section stripped), seed from the binding, `AGENTS.md` direction updated, CI builds both and checks the shared memory. | Both builds produced in CI; Rust tests unchanged. |
| W2 browser runtime | Worker per call, module cache, build selection, thread pool, progress, cancellation, panic hook and channel, error mapping; tests in headless Chromium (Playwright, pinned) with and without cross-origin isolation, including a panic in a pool task and the native-equality tripwire. | API parity tests pass in the browser; the Node test suite is untouched. |
| W3 packaging | Conditional exports, types, packed-install test for the browser entry, a Vite and a webpack fixture build, the rayon worker variant decision, size budget, README "Browser" section. | `npm run verify` covers it; the tarball contains both builds. |
| W4 live shapes | `onProgress` `shape`, in Rust render, binding, wasm and TypeScript, documented. | Contract tests cover it. |
| W5 demo | `demo/`: drop an image, pick shape, count and alpha, watch it draw, compare with the original, download SVG/PNG; deployed to GitHub Pages by a workflow, with the COOP/COEP service worker. | Live at `https://domoarigatomrburato.github.io/primeval/`, linked from the README. |
| Later | WASM fallback on Node where no native prebuild exists (StackBlitz/WebContainers, musl before REL-7); SIMD128 kernels; Firefox and WebKit in the browser tests (Playwright browsers not yet downloaded); `libm` everywhere for cross-platform identical output (engine redesign). | Separate decisions. |

## Risks

- Bundler compatibility for wasm plus nested workers is the main maintenance cost; the fixture builds in W3 exist to catch regressions.
- The nightly pin can break with wasm-bindgen updates or the `atomics` flag phase-out; bump it deliberately, like the stable pin.
- Only Chromium is measured. Safari and Firefox support nested workers and cross-origin isolation, but the 1 GiB shared-memory reservation on low-memory devices is unverified.
