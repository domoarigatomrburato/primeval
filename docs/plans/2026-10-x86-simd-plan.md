# x86_64 SIMD scoring kernels

Status: planned (2026-10-12), roadmap item 8. Meant for a cloud session on an x86_64 machine: the maintainer's machine is an Apple M2 Pro, where x86_64 code runs only under Rosetta, which is fine for correctness but useless for timing.

## Goal

The native addon on x86_64 (Linux x64, macOS x64, Windows x64) scores candidates with scalar code today; aarch64 has NEON kernels and the browser has simd128 kernels. Give x86_64 SIMD kernels too, with output bit for bit identical on every CPU. The speed review of 2026-10-12 estimated 15–35% of the total time for the cheap kinds (rectangle, ellipse, circle, triangle), 10–25% for `any`, 5–15% for polygons and 2–8% for quadratics. The simd128 port measured 15–20% on circles, ellipses and rectangles in single-threaded wasm, which is in line.

## Constraints

- **Same output everywhere.** The engine gives the same output at any thread count and on aarch64, x86_64 and wasm32, and pinned digests check it (`seeded_greedy_output_is_pinned`, `joint_output_is_pinned`, `fixed_width_quadratics_keep_their_random_streams` in `primeval-core`, `raster_output_is_pinned` in `primeval-render`). No pin may move. The scoring kernels are integer code, so a SIMD kernel that equals the scalar one for every input keeps that.
- **Portable binaries.** The prebuilt addons target the baseline instruction set of their architecture (`README.md`); `scripts/check-artifact.mjs` rejects AVX-512 outside an allowlist of dependency functions. No `target-cpu` flags. On x86_64 the baseline is SSE2, so anything newer (SSE4.1, AVX2) needs runtime detection (`is_x86_feature_detected!`) with a scalar fallback. AVX2 is allowed in the artifacts.
- **No floating point in the kernels**, no relaxed or fused operations; the engine's floating-point code stays as it is. `crates/primeval-core/clippy.toml` bans `mul_add` and the platform transcendentals in production code.

## Where the time goes

`crates/primeval-core/src/score.rs`:

- `trait LineKernels` has `line_sums`, `line_error` (built on `difference_bytes`), `difference_bytes` and `line_after`; `struct Scalar`, `struct Neon` (aarch64) and `struct Simd128` (wasm32 with `simd128`) implement it, and `type Native` picks one per target at compile time.
- The hot path is `line_after`, the squared error of a scanline after blending it with a candidate's colour, called by `energy_with::<Native>` (through `energy` and `energy_below`) for every line of every candidate the search and the refit climbs evaluate. `fit_with` mostly reads the step's prefix sums (`prefix.rs`), and `line_sums`/`line_error` serve only the one fit per committed shape and full-canvas scores: the simd128 port keeps SIMD for `line_after` alone and delegates the rest to the scalar kernels, and x86 should do the same unless a measurement says otherwise.
- The blend is fixed point: `M = 0xFFFF`, `LineBlend { keep: M − k, terms: [k · s + (M − 1) / 2; 3] }`, each channel becomes `div_by_m(c · keep + term)` with `div_by_m(v) = (v + 1 + (v >> 16)) >> 16`, exact below `BLEND_BOUND`. `blend_channel_scalar` is the reference.

## Design

- **Runtime dispatch, chosen once per evaluation.** `energy_with` is generic over `K: LineKernels`; monomorphise it for the x86 SIMD kernel and for `Scalar`, and branch once per call on a cached detection (`is_x86_feature_detected!` caches in an atomic; or a `OnceLock`), not per line. The x86 kernel functions carry `#[target_feature(enable = "avx2")]` (and SSE4.1 if a 128-bit path is added) and are `unsafe` to call; the dispatch establishes the feature. Keep `type Native` for the compile-time targets and add the x86 dispatch beside it, so aarch64 and wasm32 are untouched.
- **AVX2 first.** It has the 32-bit lane multiply (`_mm256_mullo_epi32`), the zero-extensions (`_mm256_cvtepu8_epi16`, `_mm256_cvtepu16_epi32`) and `_mm256_madd_epi16` (the equivalent of NEON's widening multiply-accumulate and wasm's `i32x4_dot_i16x8`). SSE2 alone lacks a 32-bit lane multiply, so a 128-bit path would need SSE4.1; add it only if a measurement on a CPU without AVX2 justifies it (almost every x86_64 CPU since 2013 has AVX2).
- **The simd128 kernel is the template.** `mod simd128` in `score.rs` processes blocks of 16 RGB pixels, three 16-byte vectors whose byte lanes have a fixed channel because a block starts on a pixel; the per-channel blend terms follow a three-vector pattern (`block_terms`); squared differences go through `i16` differences and a dot product into `u32` lanes, flushed into a `u64` total often enough that no lane can overflow (every bound proved in a comment); a tail of three or more pixels takes one masked block when the buffers extend that far, shorter tails the scalar kernel. With AVX2 the same structure works on 32 pixels, three 32-byte vectors (96 bytes, the channel pattern again repeating every three vectors). Re-derive every bound for the wider lanes.

## Tests

- `mod simd_parity` in `score.rs` checks the target's SIMD kernels against `Scalar` (every alpha, random buffers and lines, long lines past the flush counts, `fit_with`, `energy_with` with and without limits). Extend it to x86_64, testing the AVX2 kernel directly (not only through the dispatch) and skipping with a printed note when the CPU lacks AVX2; GitHub's x86_64 runners have it. Also test that the dispatch picks the scalar kernel when told the feature is absent.
- Write the parity tests first and see them fail on a deliberately wrong kernel (a wrong term lane, a flush count one too high), then restore it.
- The pins must pass unchanged on the x86 machine. The orchestrator also runs them on aarch64 and under Rosetta.

## Measurement

The engine runner is in the repository: `cargo build --release -p primeval-render --features lab --example engine`, then `RAYON_NUM_THREADS=10 ./target/release/examples/engine --refine final --steps 100,200` (all kinds, the five corpus images) prints a Markdown table per run with the time columns `search_s` and `refine_s` and the quality columns (`score`, `rmse256`, `gap`, `ssim128`, `ssim1024`, `svg_bytes`, `adopted`, `final_joint`). Use `RAYON_NUM_THREADS` equal to the machine's cores, and also 1 with `--no-synthetic` (the two paintings).

- Build the baseline runner from `main` before the change and keep its binary outside `target/`.
- Run base, new, base, new, interleaved, on an otherwise idle machine; record `lscpu` (or the equivalent), the core count and whether AVX2 is present. Cloud machines have noisy neighbours: report the two baselines' ratio as the noise.
- Every non-time column must be identical across all runs (write a small comparator that matches rows by image, shape and steps). Report per kind the ratio of the summed `search_s + refine_s`, new over base, at each checkpoint.
- If the scalar fallback is forced (for example with an environment variable read only in a test build, or by building a variant), check it is no slower than today.

## Gate and delivery

- Follow `AGENTS.md`: red-green TDD, targeted tests while working, `cargo fmt` and `cargo clippy --all-targets -- -D warnings` (with and without `--all-features`), then `npm run verify` once at the end. If the machine cannot run the browser tests (`verify:wasm` needs the pinned nightly, wasm-bindgen-cli and Playwright's Chromium: `node scripts/build-wasm.mjs install-tools` and `npx playwright install chromium`), run `npm run verify:rust && npm run verify:node && npm run verify:pack` and say so; CI runs the rest.
- Work on a branch (`x86-simd`), commit with clear messages, and open a pull request against `main` with the measurements in its description. Do not merge it: the maintainer's orchestrating session reviews it, re-checks the pins on aarch64 and under Rosetta, and merges.
- If a pin moves, a rule here contradicts an existing test, or the measured gain is under about 3% on every kind, stop and report rather than adjusting fixtures or pushing on.
- Update `README.md` only if public behaviour changes (it should not); record the result in `docs/algorithm-leap-review-2026-10-07.md` as a new section, in the style of sections 22 and 23, and prune this plan when it is done.
