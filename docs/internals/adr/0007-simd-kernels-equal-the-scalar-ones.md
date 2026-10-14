# 0007. SIMD kernels equal the scalar ones bit for bit, and WebAssembly requires SIMD

Status: Accepted

Date: 2026-10-12

## Context

Scoring a candidate is the engine's inner loop: per scanline, the sums for the colour fit and the squared error after blending (`line_after`). aarch64 had NEON kernels before the algorithm work of October 2026; x86_64 and WebAssembly ran scalar code, and on x86 the pipeline ran polygons at 0.83 and quadratics at 0.42–0.50 times Go `primitive`'s speed.

Any SIMD path must keep the output identical on every target ([0002](0002-same-output-on-every-thread-count-and-platform.md)), so a kernel that reorders or rounds differently from the scalar code is not an option. The prebuilt Node addons target each architecture's baseline instruction set, so an x86 kernel above SSE2 must be chosen at run time. Browsers support WebAssembly SIMD from Chrome 91, Firefox 89 and Safari 16.4.

## Decision

- The per-line kernels are a trait, `LineKernels` in [`score.rs`](../../../crates/primeval-core/src/score.rs), with a scalar implementation, which is the reference, and SIMD ones:
  - NEON on aarch64: line sums, byte differences and `line_after`, 8 pixels at a time;
  - simd128 on wasm32: `line_after`, 16 pixels at a time;
  - AVX2 on x86_64: `line_after`, 32 pixels at a time, which `energy` and `energy_below` choose with `is_x86_feature_detected!("avx2")` once per evaluation; a CPU without AVX2 runs the scalar kernels. No SSE4.1 path: almost every x86_64 CPU since 2013 has AVX2.
- The refit pass's sums have NEON, baseline SSE2 and simd128 versions in a normative order ([0008](0008-the-refit-sums-in-a-fixed-four-slot-order.md)).
- Every SIMD kernel computes exactly what its scalar reference computes: integer arithmetic with proven accumulator bounds (the AVX2 squares flush their 32-bit lanes every 4096 blocks, below the bound of 5504), and no reordering of floating-point additions.
- Both WebAssembly builds require SIMD (`+simd128`); there is no build without it. The wasm build checks the flag in each module, and a browser without SIMD gets an `InternalError` that names the browsers that have it.

## Consequences

- Parity tests, the `simd_parity` modules of [`score.rs`](../../../crates/primeval-core/src/score.rs) and [`refine.rs`](../../../crates/primeval-core/src/refine.rs) (`line_kernels_match_scalar_on_every_span`, `line_kernels_match_scalar_past_the_flushes`, `simd_sums_match_scalar_sums`), compare every kernel with its reference bit for bit on random and edge-case lines. Deliberate faults (a wrong term lane, a flush every 8192 blocks, a lane mistake in the sums) failed them.
- CI runs the Rust suite on `macos-15` (NEON) and `ubuntu-24.04` (AVX2), with `PRIMEVAL_REQUIRE_AVX2` set on x86 so that a runner without AVX2 fails instead of skipping. `node scripts/build-wasm.mjs test-kernels` runs the score and refit-sums tests on `wasm32-wasip1` under Node's WASI, in `verify:wasm`. Rosetta on macOS 15 executes AVX2 although CPUID hides it, so the AVX2 kernel can be tested on an Apple machine by forcing the check in an uncommitted patch.
- Measured gains, output unchanged: in single-threaded wasm 0.80–0.86 of the time for circles, ellipses and rectangles and 0.98 over all kinds, with a `.wasm` about 25 KiB smaller after gzip; on x86_64 0.917 of the time over all kinds on all cores and 0.912 on one thread.
- Costs: per-target code paths to maintain, each with a scalar reference and a parity test; browsers without SIMD are not supported. A new kernel lands only with its scalar reference, its parity test and, for wasm32, its wasip1 run.
- Removing a SIMD path is always safe, since the scalar reference gives the same bytes; adding one needs parity first.

## Evidence

- `135df40` (2026-10-12): simd128 kernels and the SIMD requirement in browsers; `559363a` (2026-10-12): AVX2 `line_after`; `b155074` (2026-10-13): the refit sums with SIMD.
- The tail variants that were tried and dropped are in [`../experiments.md`](../experiments.md); the open NEON item PERF-3 is in the [audit plan](../plans/2026-10-audit-and-refactor-plan.md).
