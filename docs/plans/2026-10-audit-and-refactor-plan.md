# Audit And Refactoring Plan (October 2026)

| | |
| --- | --- |
| Audited commit | `3e872be` (`main`, 2026-10-01) |
| Branch for the first refactoring | `refactor/audit-2026-10` |
| Scope | Rust engine, render facade, napi binding, TypeScript wrapper, CLI, tests, CI/release, docs, repo hygiene |
| Supersedes | `docs/pre-launch-audit.md` (stale; deleted) |
| Method | Full static review, runtime probes against a freshly built addon, disassembly of release artifacts, benchmarks, comparison with a second independent review shared by the maintainer |

This document is the single source of truth for the refactoring work that starts on this branch. Per `AGENTS.md`, prune it as items land.

## Contents

0. [Executive summary](#0-executive-summary)
1. [Method, environment and legend](#1-method-environment-and-legend)
2. [Release and distribution (REL)](#2-release-and-distribution-rel)
3. [Runtime robustness (RT)](#3-runtime-robustness-rt)
4. [Node binding, TypeScript wrapper and CLI (NODE, CLI)](#4-node-binding-typescript-wrapper-and-cli-node-cli)
5. [Engine correctness (ENG)](#5-engine-correctness-eng)
6. [Performance (PERF)](#6-performance-perf)
7. [API and engineering quality (API)](#7-api-and-engineering-quality-api)
8. [Tests (TEST)](#8-tests-test)
9. [Tooling, CI and supply chain (TOOL)](#9-tooling-ci-and-supply-chain-tool)
10. [Documentation and repository content (DOC)](#10-documentation-and-repository-content-doc)
11. [Removals (RM)](#11-removals-rm)
12. [What to keep](#12-what-to-keep)
13. [Explicit non-goals](#13-explicit-non-goals)
14. [Roadmap](#14-roadmap)
15. [Carry-over to a redesigned engine ("next")](#15-carry-over-to-a-redesigned-engine-next)
16. [Future: the algorithm leap](#16-future-the-algorithm-leap)
17. [Appendix A: measurements and reproduction](#appendix-a-measurements-and-reproduction)
18. [Appendix B: cross-check with the second review](#appendix-b-cross-check-with-the-second-review)

---

## 0. Executive summary

The architecture is sound and most of the engineering choices are worth keeping:

- the `core → render → binding → TS/CLI` layering;
- Rust as the only owner of defaults;
- `napi.targets` as the single source for release metadata;
- the packed-tarball test;
- an energy loop that does not allocate.

The problems are concentrated at the edges: how binaries are built, how failures reach the host process, unbounded inputs, and a few engine bugs.

**Decisions recorded with the maintainer**

- The package has never been published. Breaking changes are free; no migration paths, deprecations or compatibility shims are needed.
- GPU acceleration (Metal or wgpu) is out of scope for now. It is revisited only together with the algorithm change in section 16.
- The algorithm change ("next") happens after everything in this plan is fixed.
- Proposing removal of obsolete or low-value formats and features is explicitly welcome (section 11).
- Formats: output is SVG and PNG only (JPG and GIF output are removed); input is JPEG, PNG and WebP (GIF input is removed).
- The engine works in RGB only (RM-4, PERF-4): backgrounds must be opaque, transparent inputs are composited at decode time, and buffers and kernels are three-channel (16% faster end to end). The score is the RGB RMSE divided by 255.

**Top findings:** none open. The largest measured hotspot (PERF-5, anti-aliased rasterizer interiors) landed in T6: single-threaded `model_step` is 46% faster for polygons and 59% for rotated ellipses, with bit-identical output.

**Carry-over:** of the 100 action items the audit identified, 65 carry over unchanged to a redesigned engine and 13 more partially (section 15, a snapshot taken at audit time). This argues for doing the transferable work first and capping the investment in performance tuning of the current engine.

---

## 1. Method, environment and legend

**Environment**

- Apple M3 (8 cores: 4 performance + 4 efficiency), macOS (Darwin 27), Node 24.18.1, npm 11.16.
- Rust 1.93.0 (pinned by `rust-toolchain.toml`) and 1.99.0, installed via rustup during the audit.

**Baseline gate.** `npm run verify` passes in 35 s:

- 132 Rust tests (112 core, 7 render, 12 binding, 1 doctest);
- 34 package tests;
- 17 tooling tests;
- a 13.1 kB tarball with 10 files.

`npm audit` reports 0 vulnerabilities across 117 packages.

**What was done**

- Every file in `crates/`, `binding/`, `src/`, `scripts/` and `test/` was read in full, plus all workflows and configuration files.
- Runtime probes ran against the addon built by `npm run build:node`. The scripts are described in Appendix A.
- The release artifacts of v0.1.1 found in the gitignored `artifacts/` directory (dated 2026-03-24) were disassembled.
- Benchmarks used a scratch harness outside the repository, linked to the workspace crates by path, with the same release profile (`lto = "fat"`, `codegen-units = 1`).

**Status legend**

| Status | Meaning |
| --- | --- |
| Reproduced | Triggered at runtime during the audit. |
| Measured | Numbers come from the benchmarks in Appendix A. |
| Verified | Confirmed by reading the code at the cited lines; not executed. |
| Estimated | Reasoned from the code. Benchmark it before acting on it. |
| Reported | Found by a review pass and plausible, but not independently re-checked. |

**Severity:** Critical (crashes, unusable binaries, unsoundness reachable by users), High, Medium, Low, Nit.

**Carry-over tag:** `next: yes | partial | no`. It says whether the item still applies if the optimization algorithm is redesigned from scratch (section 15).

---

## 2. Release and distribution (REL)

REL-1, REL-2, REL-5 and REL-6 landed in T4:

- no `target-cpu` anywhere (`.cargo/config.toml` is gone; `CONTRIBUTING.md` documents the local opt-in);
- `scripts/check-artifact.mjs` fails a release artifact on AVX-512 (x86_64), SVE (aarch64) or a `GLIBC_` symbol above 2.17 (Linux gnu, built with `--use-napi-cross`);
- `scripts/smoke-test-addon.mjs` loads each artifact on its own runner and renders SVG and PNG; the quality workflow runs `verify:rust` on `macos-15` too, so the NEON paths are tested;
- the Cargo version is shared with npm (`bump:version` updates both; `check-package` fails on drift); publishing skips versions already on the registry, publishes the root package last, verifies with `npm view`, and creates the GitHub Release.

**Only CI can confirm** (first release run, and the first PR run for the quality matrix): `--use-napi-cross` on both Linux runners, `llvm-objdump` from `llvm-tools` on every runner, the smoke test on all five runners (`macos-15-intel` for x86_64 macOS), and the publish/verify/release steps.

**Done with the maintainer:** the unpublished `v0.1.1` tag is deleted (the first release uses a version never tagged). npm trusted publishing cannot create a package, so the publish step accepts an optional `NPM_TOKEN` secret for the first release; `RELEASING.md` ("First Release") lists the one-time steps the maintainer takes on npmjs.com.

### REL-7: No musl targets

- **Decision:** add `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` after the first release has run green end to end, as a change of its own validated with a `workflow_dispatch` run (Alpine smoke test in a container). The loader already detects musl.

---

## 3. Runtime robustness (RT)

All RT items landed in T3: release builds unwind and the binding maps panics to `INTERNAL`; inputs below 2×2 are rejected and thumbnails keep at least 2 px per side; `count` (1..=100000), `resizeInput` (2..=2048) and `outputSize` (2..=8192) are bounded in Rust; decoding uses `ImageReader` limits (16384 px per side, 512 MiB) and frees the full image before the search (9000×9000 PNG: 646 MB → 322 MB peak RSS).

---

## 4. Node binding, TypeScript wrapper and CLI (NODE, CLI)

All NODE items landed in T3: `approximate()` only rejects (typed `PrimevalError` subclasses with `code`, `cause`, and `option`/`requirement` for invalid options); a throwing `onProgress` cancels and rejects with its error; abort is checked before any native work and on settle; numbers are range-checked in Rust; renders run on `spawn_blocking` and cancel through a `NativeTask`; JSDoc, per-output overloads and a native-type drift check are in place.

### CLI decisions (T2)

CLI-1 to CLI-4 landed with RM-7. Deliberate choices: stdout output is SVG only (the format comes from the extension, and there is no `--format`); `--help` lists no defaults, because they belong to Rust and the README; progress is shown only on a TTY.

---

## 5. Engine correctness (ENG)

All ENG items landed in T5 (see the roadmap). The geometry tests in `crates/primeval-core/src/shapes/geometry_tests.rs`, the NEON-vs-scalar parity tests and the per-kind score parity tests keep them fixed.

---

## 6. Performance (PERF)

All numbers are from Appendix A on the M3. Treat them as relative, not absolute.

### Where the time goes

Single worker, Mona Lisa at 169×256, 30 steps per shape:

| Shape | ms/step | Evaluations/step | ns/evaluation | Pixels/candidate | ns/pixel | Hill-climb share of evaluations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| any | 75.1 | 20,928 | 3,588 | 486 | 7.4 | 24% |
| triangle | 36.3 | 21,258 | 1,709 | 261 | 6.5 | 25% |
| rectangle | 22.8 | 20,171 | 1,132 | 290 | 3.9 | 21% |
| ellipse | 48.2 | 21,420 | 2,251 | 789 | 2.9 | 25% |
| circle | 50.5 | 20,204 | 2,499 | 1,044 | 2.4 | 21% |
| rotated-rectangle | 35.6 | 20,867 | 1,707 | 297 | 5.7 | 23% |
| quadratic | 37.5 | 26,483 | 1,414 | 44 | **32.1** | 46% |
| rotated-ellipse | 196.5 | 20,468 | 9,603 | 911 | **10.5** | 22% |
| polygon | 150.1 | 23,739 | 6,324 | 198 | **31.9** | 33% |

Default 8 workers, American Gothic, 200 steps, SVG output:

| Shape | Portable build | `target-cpu=native` |
| --- | ---: | ---: |
| any | 2.96 s | 3.13 s |
| triangle | 1.46 s | 1.53 s |
| rectangle | 0.93 s | 0.98 s |
| ellipse | 1.86 s | 1.99 s |
| circle | 2.28 s | 2.32 s |
| rotated-rectangle | 1.43 s | 1.45 s |
| quadratic | 2.18 s | 2.30 s |
| rotated-ellipse | **7.77 s** | 7.88 s |
| polygon | **5.98 s** | 6.03 s |
| **Total** | **26.84 s** | 27.61 s |

Takeaways:

1. **Each step is about 21k evaluations of tiny candidates.** A candidate covers 0.1–2.4% of the working image; the total pixel work per step is about 10M blend+square operations.
2. **75–80% of the evaluations are the independent random phase** (16 rounds × 1000 candidates, 900 for quadratic); the rest is sequential hill climbing.
3. **The slowest shapes are rasterization-bound, not scoring-bound.** Polygon and quadratic cost 32 ns/pixel and rotated-ellipse 10.5, against 2.4–3.9 for circle and rectangle. Rotated-ellipse and polygon alone are 51% of the total run time.
4. **Thread scaling is weak.** 1→8 workers gives 3.9× (3.475 s → 0.893 s, `any`, 50 steps), and 4→8 workers only +27% (1.136 s → 0.893 s). 12 workers on 8 cores is slower (1.273 s) because it runs 24 rounds instead of 16 and oversubscribes.

### PERF-0: Benchmark and quality infrastructure (landed)

Divan benches live behind each crate's non-default `bench` feature; `examples/quality.rs` in `primeval-render` prints a time and quality table (engine score and output RMSE) over two public-domain photos and three synthetic images. `CONTRIBUTING.md` ("Benchmarks") explains how to run them. Baseline at `425201e` on the M3 (8 logical cores):

- `Model::step`, first step of a fresh 256×256 model, one worker: rectangle 118 ms, quadratic 61 ms, circle 133 ms, triangle 151 ms, ellipse 162 ms, `any` 174 ms, rotated rectangle 197 ms, rotated ellipse 620 ms, polygon 778 ms.
- Rasterizing 256 shapes: rectangle 3.3 µs, circle 6.5 µs, ellipse 6.7 µs, triangle 12.5 µs, quadratic 49 µs, rotated rectangle 73 µs, polygon 640 µs, rotated ellipse 1.29 ms.
- Writers for a 200-shape drawing at 1024 px: SVG 0.22 ms, PNG (render and encode) 32 ms.
- Full runner: 228.6 s over 90 runs. Quadratic quality is far behind every other kind (score 0.12–0.24 against 0.03–0.05 on the photos, on the RGBA scale used before PERF-4; multiply by √(4/3) for today's RGB score).

Since ENG-4 (T5), runner quality is identical across thread counts; times still depend on the machine.

### PERF-2: x86 has no SIMD path and its loops are bounds-checked

- **Severity / status:** Medium on x86. Verified. `next: no`.
- **Where:** the scalar loops (`score.rs:63-76`, `:130-165`) index `c_pix[i] ... t_pix[i + 3]` with a manual `i += 4`: about 8 bounds checks per pixel, which blocks auto-vectorization. Linux, Windows and macOS x64 users (most servers) run this path.
- **Fix:**
  - First slice each line once and iterate `as_chunks::<4>()`, as T1 already does in `difference_full_raw_pixels`.
  - Then, only if PERF-0 shows it is worth it, add AVX2/SSE4.1 kernels chosen at runtime (`multiversion`, or `std::arch` with `is_x86_feature_detected!` resolved once into a function table). Never through `target-cpu`.

### PERF-3: NEON reduces horizontally every 8 pixels

- **Severity / status:** Medium. Verified. `next: no`.
- **Where:** `score.rs:214-222` and `:244`. `vaddvq_u32` / `vaddvq_s32` run on every 8-pixel chunk.
- **Fix:**
  - Keep vector accumulators across the scanline and reduce once per line. The lanes are safe up to lines of about 16k pixels.
  - Compute squares as `vabdq_u8` → `vmull_u8` → `vpadalq_u16`.
  - Low priority while the bottleneck is rasterization.

---

## 7. API and engineering quality (API)

Done. The last item, API-8, is decided: the crates are not published (`publish = false` in `[workspace.package]`) while their API is expected to change; the npm package is the product.

--- | --- | --- | --- | --- | --- |

---

## 8. Tests (TEST)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |

---

## 9. Tooling, CI and supply chain (TOOL)

All TOOL items landed in T1. Follow-ups:

- `napi-prebuilds.yml` calls the quality workflow with `uses: ./...` plus `# zizmor: ignore[self-repository]`. Switch to the `$/...` syntax and drop the ignore once actionlint accepts it (1.7.12 does not).
- Dependabot does not bump the actionlint `docker://` digest in the hygiene job; update it by hand.
- Two things only CI can confirm, on the first pull request run: `rustup toolchain install` (no arguments) installs the toolchain and components from `rust-toolchain.toml`, and the pinned actionlint image works as a `docker://` step.

---

## 10. Documentation and repository content (DOC)

Done. The last item, DOC-6, landed: the copyrighted images are gone, three public-domain photographs (NPS, NASA) joined the two paintings, and `docs/readme/originals/SOURCES.md` credits every image (a test requires a row per image). The benchmark corpus stays the two paintings.

--- | --- | --- | --- | --- | --- |
| DOC-6 | Low | Partly done | yes | The copyrighted originals and their images are removed and the gallery is regenerated; only the public-domain Mona Lisa and American Gothic remain, which also form the benchmark corpus (PERF-0). | Add replacement photographs (public-domain, CC0 or the maintainer's own) to `docs/readme/originals/` and run the gallery example (`CONTRIBUTING.md`). |

---

## 11. Removals (RM)

Done. Every removal landed (the last, RM-4's RGB-only kernels, in T6).

---

## 12. What to keep

- The layering `core → render → binding → TS/CLI`, and Rust as the single owner of defaults, with omitted fields passed through. The binding test `normalize_request_uses_rust_defaults_for_omitted_render_fields` guards it.
- The fused energy evaluation with no scratch buffer, proven equal to the two-pass version and to a full recomputation by tests; exact integer blend arithmetic mirroring Go; the NEON blend checked against scalar for all 256 source values.
- An inner loop that does not allocate: per-worker scratch is reused, undo is a stack copy, and the closure-based `energy()` avoids Go's back-pointer from shapes to workers.
- `package.json` `napi.targets` as the single source for optional dependencies, lockfile validation, the release matrix and artifact verification; the packed-tarball install test.
- Loader diagnostics: musl detection, combined load errors with install guidance, unit tests in a `vm` sandbox.
- Careful details: `image`'s allocation limit stops header bombs; the abort listener is removed in `finally`; registry cleanup handles setup failures; progress events never arrive after the promise settles (0 of 600 calls); the event loop stays responsive under 16 concurrent renders.
- Quadratic mutation with bounded repair instead of unbounded retries; strict workspace lints; `util.parseArgs` in strict mode.

## 13. Explicit non-goals

Agreed with the second review (Appendix B) and the maintainer:

- No CLI framework: `node:util.parseArgs` is enough.
- No switch from `node:test` to Vitest or Jest.
- No cross-language code generation or shared schema; contracts are enforced by runtime tests (TEST-1).
- No change to the `core → render → binding → TS/CLI` layering.
- No extra supply-chain machinery beyond Dependabot, `cargo-deny` and workflow linting (no Renovate, Scorecard or extra scanners).
- **No GPU work** (Metal, wgpu) until section 16 is reached.
- No browser/WASM and no CommonJS (unchanged from `AGENTS.md`).

---

## 14. Roadmap

Every code change follows the red-green-refactor rule in `AGENTS.md`. Tickets are ordered to minimise rework:

- formatting and toolchain churn land first;
- code that is going to be deleted is deleted before anything is hardened;
- engine performance comes last and is gated by benchmarks.

### T1: Toolchain and formatting baseline (first)

Done. Every TOOL item plus REL-3 and REL-4 landed; section 9 lists the follow-ups.

### T2: Simplification and the engine boundary

Done. GIF/JPG output, GIF input, path input, `repeat` and the Rust-only knobs are gone; WebP input, the opaque RGB contract, the CLI redesign (with CLI-1 to CLI-4), dead-code removal, the `Drawing` engine boundary (API-3), the small core surface, typed alpha and the render facade ergonomics landed. `Model::step` still returns `Result<_, String>`: that goes with API-2 in T3.

### T3: Runtime robustness

Done. Every RT and NODE item, API-2, and TEST-1, TEST-2, TEST-3, TEST-5 and TEST-6 landed. TEST-5 uses seeded randomized tests on stable Rust (`crates/primeval-render/tests/robustness.rs`) instead of `cargo-fuzz`, which needs nightly. The runtime contract table also aligned the keyword and number spellings: `auto` is exact lowercase and alpha strings are plain digits on every surface.

### T4: Portable native builds and release

REL-1, REL-2, REL-5 and REL-6 landed (section 2 lists what only CI can confirm and the maintainer actions).

- [ ] REL-7 musl targets, after the first green release (decided)
- [x] API-8 Crates stay unpublished

### T5: Engine correctness

Required in any case, because the current engine becomes the reference and baseline for "next".

Done. Every ENG item and TEST-4 landed, plus two found on the way: symmetric ellipse rows (circles were one row short per side, from Go) and one-pixel quadratic strokes (sub-pixel strokes over-saturated under the coverage-weighted colour fit). Rotated rectangles deliberately keep no aspect-ratio limit, unlike Go, because the limit measurably hurt quality. Wider quadratic strokes score better still (1.5 px about 6%) but read as blobs rather than pen strokes; the width stays at 1 px (decided with the maintainer).

### T6: Performance, gated by benchmarks

Done except the deferred items. Full PERF-0 runner, 8 threads, total search time on the M3 (median of alternating runs, one core busy with another app): 161 s before PERF-1, 142 s with it (-12%), about 123 s with PERF-8 as landed (-14%; the excluded kinds were within 4% of PERF-1 alone). PERF-4 gave -16% before that, PERF-5 -46%/-59% single-threaded on polygon and rotated-ellipse steps.

- [x] PERF-5, PERF-6, PERF-10: coverage evaluated only at span-end pixels, reusable row scratch, integer error-grid sums (bit-identical output)
- [x] PERF-4 RGB-only kernels (RM-4): byte-identical output
- [x] PERF-1 Prefix sums and an exact early exit: same chosen shapes, about half the blended pixels skipped; single-threaded `model_step` 5% (quadratic) to 52% (rectangle) faster
- [x] PERF-8 Half-resolution ranking of the random phase, rescoring the best 128 at full resolution, for any, circle, ellipse, rotated ellipse and polygon only: quality equal within noise at 100–1000 steps. Rescoring only 4 lost 1.3% at 1000 steps; rectangles, rotated rectangles and triangles gained almost no time, so they rank at full resolution.
- [x] PERF-11: documented in the README's Deploying section; no CLI batch mode (decided); the README shows a batch with `xargs -P`
- [ ] Deferred until the "next" decision: PERF-2 runtime-dispatched x86 SIMD, PERF-3 NEON accumulator tuning (section 15)

### T7: Documentation

Continuous: each ticket updates the README for the behaviour it changes. This ticket is the final pass.

- [x] API-6, API-7, DOC-2 (Deploying section, with PERF-11's batch-throughput note), DOC-3, DOC-7 (metadata, badges, repository description and topics), and DOC-6's removal of the copyrighted images
- [x] Gallery generator (`examples/gallery.rs`) and every image regenerated from the final engine; DOC-4 Benchmarks from the PERF-0 runner
- [x] Replacement photographs (DOC-6): three public-domain photographs, credited in `docs/readme/originals/SOURCES.md`

---

## 15. Carry-over to a redesigned engine ("next")

**Question:** how much of this plan still applies if the optimization algorithm is redesigned from scratch (section 16)?

This section is a **snapshot taken at audit time**. It counts all 100 action items the audit identified (REL 7, RT 6, NODE 10, CLI 4, ENG 16, PERF 12, API 11, TEST 6, TOOL 12, DOC 7, RM 9), each tagged `next`. As items land they are pruned from the sections above, as `AGENTS.md` requires, but they stay in these counts. Do not recompute the table on each prune; the git history shows what has landed.

| Area | Items | Carry over fully | Partially | Not at all |
| --- | ---: | ---: | ---: | ---: |
| Release and distribution (REL) | 7 | 7 | 0 | 0 |
| Runtime robustness (RT) | 6 | 4 | 2 | 0 |
| Node binding and TS (NODE) | 10 | 10 | 0 | 0 |
| CLI | 4 | 4 | 0 | 0 |
| Engine correctness (ENG) | 16 | 0 | 4 | 12 |
| Performance (PERF) | 12 | 2 | 2 | 8 |
| API and engineering (API) | 11 | 7 | 3 | 1 |
| Tests (TEST) | 6 | 5 | 1 | 0 |
| Tooling and CI (TOOL) | 12 | 12 | 0 | 0 |
| Documentation (DOC) | 7 | 6 | 1 | 0 |
| Removals (RM) | 9 | 8 | 0 | 1 |
| **Total** | **100** | **65** | **13** | **22** |

**By count:** 65% carry over unchanged, 13% partially, 22% not at all.

**By effort:** the 22 items that do not carry over are concentrated in the engine, and they include the most expensive work in the plan: x86 SIMD runtime dispatch, NEON tuning, the rasterizer rewrites, prefix sums and multi-resolution search. Weighted by estimated effort, a rough guess is that 55–60% of the total work carries over. This is an estimate, not a measurement.

**What does not carry over** is algorithm-specific by nature:

- the current rasterizers, their bugs and their tuning (ENG-2, ENG-7 to ENG-9, ENG-13, ENG-14, PERF-5 to PERF-7);
- the integer scoring kernels and NEON (ENG-1, ENG-5, ENG-6, PERF-1 to PERF-4);
- the random search and hill climbing (ENG-10 to ENG-12, PERF-8);
- the engine's dead code (API-5, RM-9).

**What partially carries over** is mostly a principle rather than code:

- a single coordinate convention with a PNG-vs-SVG test (ENG-3);
- determinism independent of the thread count (ENG-4, PERF-9);
- minimum-size validation at the boundary (RT-2);
- output caps (RT-4);
- a small engine API, one name table, accurate rustdoc (API-1, API-6, API-7);
- property-based testing (TEST-4);
- export-layer details that survive if the exporters are reused (ENG-15, ENG-16, PERF-10);
- README claims tied to the current engine (DOC-1).

**Consequences for sequencing**

1. **API-3 (the engine boundary) is the most important single item for "next".** If the engine only produces an ordered list of committed shapes and everything else (decode, limits, replay, SVG/PNG writers, binding, TS, CLI, tests, release) lives outside it, a new engine plugs in and the 65 transferable items are not redone.
2. **Do T1–T4 fully.** They are almost entirely transferable.
3. **Do T5 (engine correctness) fully as well**, even though it does not carry over. "Next" must be compared against a correct baseline, and the current engine likely remains the fallback or the fast preset.
4. **Cap T6.** Do PERF-0 (it carries over and "next" needs it), and the cheap, high-yield items with measured hotspots: PERF-5, PERF-7 (with ENG-2), PERF-1 and PERF-9. **Defer** PERF-2 (x86 SIMD runtime dispatch) and PERF-3 (NEON tuning) until it is decided whether "next" replaces the engine.

---

## 16. Future: the algorithm leap

To be started **after** everything above is fixed. Recorded here so the refactoring keeps the door open (API-3, PERF-0, ENG-3, ENG-4, DOC-6, RM-4 are the prerequisites).

### Why

The current algorithm is a faithful evolution of Fogleman's `primitive`.

**How it works:**

- It is **greedy and sequential**: one shape per step, chosen against the current canvas and never revisited.
- Each step does random sampling (16 × 1000 candidates, error-guided) followed by hill climbing.
- The fitness is RMSE in sRGB.

**Consequences:**

- early shapes cannot be corrected once later shapes exist;
- quality per shape plateaus;
- most evaluations are spent on random proposals;
- the cost grows linearly with `count × candidates`;
- the metric is not perceptual.

For the main use case (small SVG placeholders with 50–200 shapes) every shape matters, which is exactly where joint optimization pays off most.

### Options, in increasing ambition

**A. Same paradigm, better search** (lowest risk, CPU only, fits behind the current engine boundary)

1. *Refit passes:* periodically re-optimize earlier shapes with the others fixed (coordinate descent), or remove-and-refit the worst contributors.
2. *Better proposals:* replace "1000 random + hill climb" per round with an evolution strategy per shape (e.g. (1+λ)-ES or CMA-ES), seeded from the error grid.
3. *Better acceptance:* simulated annealing or late-acceptance hill climbing instead of strict descent.
4. *Coarse to fine:* optimise on an image pyramid (PERF-8 is a first step).
5. *Perceptual fitness:* score in a perceptual colour space (e.g. OKLab) or with SSIM-weighted error. Blending must stay in the colour space that SVG renderers use (sRGB), or the exported SVG will not match what was optimised.

**B. Joint optimization with differentiable rasterization** (the real leap)

- **Method:**
  - Give each primitive a smooth coverage function: anti-aliased edges as a function of its parameters.
  - Optimise **all** shape parameters and colours together by gradient descent (e.g. Adam) against the target, keeping insertion order as layer order.
  - Initialise from the greedy engine, which becomes stage 1; differentiable refinement is stage 2.
- **References:**
  - Li, Lukáč, Gharbi, Ragan-Kelley, *Differentiable Vector Graphics Rasterization for Editing and Learning* (DiffVG), SIGGRAPH Asia 2020.
  - Ma et al., *Towards Layer-wise Image Vectorization* (LIVE), CVPR 2022.
- **Cost:** each iteration is O(shapes × covered pixels) forward plus backward. It is feasible on CPU for ≤ 200–500 shapes at 128–256 px with analytic gradients for these simple primitives. This option is also where the GPU question will come back. Per the recorded decision, it stays closed until then.
- **Risks:** local minima, tuning, and noticeably more complex code.
- **Payoff:** substantially better quality at a fixed shape count, which is the metric that matters for placeholders.

**C. Learned or amortized approaches** (out of scope)

- Examples: Huang, Heng, Zhou, *Learning to Paint*, ICCV 2019; Liu et al., *Paint Transformer*, ICCV 2021.
- Fast inference, but they need a training pipeline, distributed model weights and an ML runtime dependency. That does not fit a small native library.

### Recommendation and evaluation protocol

1. Prototype A1 + A2 behind the engine boundary first: cheap, CPU-only, measurable.
2. Then prototype B as a refinement stage after greedy initialisation.
3. Accept a new engine only on the PERF-0 corpus (licence-clean, DOC-6), with these metrics:
   - RMSE and SSIM at fixed shape counts (50, 100, 200, 500);
   - quality at fixed time budgets;
   - SVG size;
   - determinism under the ENG-4 rules.
4. Keep the current engine (fixed and correct after T5) as the baseline, and possibly as the fast preset.

### Progress

The protocol's runner is `examples/engine.rs` in `primeval-render` (feature `lab`, see `CONTRIBUTING.md`; its doc comment defines the metrics). It runs one search per image and kind and records checkpoints along it. All numbers below are from it on the M3, with seed 42 and the PERF-0 corpus of 5 images × 9 kinds (45 rows per checkpoint).

**Metrics.** Lower is better for every metric except SSIM.
- `score` is the engine's RGB RMSE on its own canvas.
- `rmse256` is the RMSE of the exported PNG at the 256 px working size against the same target. It measures what users get, so new work is judged on it.
- `gap` is `rmse256 / score − 1`.
- `ssim128` is SSIM at a placeholder-like 128 px. SSIM at the 1024 output barely moves (0.631 to 0.658 over 10× the shapes), so it is no longer used.

Medians matter because `quadratic` scores 2–4× worse than the other kinds and drags every mean.

**Greedy baseline** (`5cb04d0` search):

| Shapes | Mean score | Median score | Median rmse256 | Median gap | Mean ssim128 | SVG bytes | Search s (all rows) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 | 0.0941 | 0.0659 | 0.0661 | +0.3% | 0.678 | 5,229 | 24.5 |
| 100 | 0.0786 | 0.0512 | 0.0495 | +0.4% | 0.732 | 10,322 | 41.0 |
| 200 | 0.0649 | 0.0407 | 0.0394 | +1.0% | 0.778 | 20,542 | 71.1 |
| 500 | 0.0474 | 0.0290 | 0.0298 | +4.6% | 0.849 | 51,252 | 149.2 |

Search times are from an idle machine. Greedy score falls roughly as `shapes^-0.34` between 200 and 500 shapes, so 10% more shapes buys about 3%.

**Engine canvas against export.**
- At 100 and 200 shapes the median gap is below 5% for every kind, and below 2% for all but `quadratic` (2.0% and 4.1%).
- The kinds the engine rasterizes with anti-aliasing (polygon, rotated ellipse) show no smaller gap than the binary kinds. Circle, ellipse, triangle and rotated rectangle are often negative: the anti-aliased export fits slightly better than the engine believes.
- Binary coverage therefore costs no fidelity at placeholder counts. Neither B's smooth coverage nor anti-aliased engine rasterizers have a fidelity payoff.
- The gap grows with the layer count: +4.6% at 500 shapes, with polygon at +5.3% and `quadratic` at +7.8%. A refit pass widens it further (see below). A likely cause is the engine's per-layer integer rounding, which the exporter does not share; this is unverified.

**A1, refit passes (`Model::refine`, `refine.rs`).**

- **Method.**
  - A top-down pass re-optimises each shape at its own layer, against an affine model of the layers above. That model includes the integer pipeline's mean truncation.
  - The pass is then verified on the exact canvas and reverted if it does not improve.
  - It is deterministic across thread counts.
  - Tuned constants: 4 climbs per layer, age 25, with step-adapted moves (step 3 below).
- **Adopted** (`192714e`). `approximate` ends with one pass, cancellable between layers (`Model::refine_unless`).
  - The progress contract changed accordingly: the streamed shapes are the greedy preview, and the result keeps their number, order and kind but may revise them.
  - The demo shows "Refining" after the last step, then replaces the preview with the final SVG.
- **Mean score change** against greedy at the same shape count, with refine time as a share of greedy time:

| Shapes | 1 pass at the end | 2 passes at the end | 1 pass every 50 shapes |
| ---: | --- | --- | --- |
| 50 | −3.2% / 0.20 | −4.6% / 0.36 | −3.2% / 0.20 |
| 100 | −2.8% / 0.15 | −4.2% / 0.30 | −3.9% / 0.26 |
| 200 | −2.8% / 0.11 | −4.2% / 0.23 | −5.1% / 0.35 |
| 500 | −3.6% / 0.09 | −5.4% / 0.17 | −10.1% / 0.58 |

- **One pass at the end in the export-side metrics** (what `approximate` now does; median rmse256 change / ssim128 change):

| Shapes | All kinds | `any` | `triangle` |
| ---: | --- | --- | --- |
| 50 | −3.0% / +0.009 | −4.2% / +0.012 | −3.8% / +0.008 |
| 100 | −4.7% / +0.010 | −5.7% / +0.014 | −2.8% / +0.006 |
| 200 | −3.6% / +0.008 | −5.1% / +0.008 | −2.9% / +0.006 |
| 500 | −3.4% / +0.007 | −4.1% / +0.005 | −2.6% / +0.004 |

- **Results.**
  - Every row improves.
  - Triangles and ellipses gain most in score; `quadratic` gains least (about 2%).
  - At 500 shapes only part of the score gain reaches the export: the median score falls 6.0% but rmse256 only 3.4%, and the gap rises from 4.6% to 5.8%. The pass partly fits the engine's own rounding.
  - At a fixed shape count, A1 is a real but modest gain. Per second of compute, it is no better than adding greedy shapes, so it matters only where the shape count is fixed, which is the placeholder case.

**Step-adapted moves in the refit climb (step 3, adopted).**
- **Method.**
  - Each refit climb's moves start at the greedy search's coarse size (`σ` 16 px, 32°, alpha ±10).
  - Their size follows the 1/5th success rule: ×2 after a kept move, ×2^(−1/4) after a rejected one, down to `σ` 1 px, 2° and alpha ±3.
  - Integer offsets are rounded and never zero.
  - The age halves to 25, which keeps the refine time of the old pass.
  - Greedy's moves are unchanged and pinned by a test.
- **Median change against greedy** at about the old pass's refine time, as score / rmse256:

| Shapes | All kinds | `any` | `triangle` |
| ---: | --- | --- | --- |
| 50 | −4.7% / −4.7% | −8.6% / −8.5% | −6.1% / −6.0% |
| 100 | −5.9% / −5.6% | −10.5% / −10.5% | −6.6% / −5.5% |
| 200 | −9.6% / −5.4% | −11.0% / −10.6% | −8.4% / −6.5% |
| 500 | −11.8% / −10.2% | −12.8% / −11.0% | −10.2% / −6.9% |

- **Against the coarse pass** (rmse256 at 100 / 200 shapes):
  - `any` −5.7% / −5.1%, `triangle` −2.8% / −2.9%, all kinds −4.7% / −3.6%;
  - `any` and `triangle` gain 1.8–2.2×, polygon, rotated ellipse, rectangle and rotated rectangle 1.7–2.9×, and no kind loses;
  - circle, ellipse and `quadratic` barely react, and the median over all 45 rows falls on one of their rows at 100 shapes.
- **Variants that lost:**
  - alternating coarse moves with fixed 1.5 px moves gained less at every time;
  - `SCALE_UP = 3` and a 2 px floor gained no more;
  - the same rule at age 50 doubles the time.
- **Fine polish pays**, so step 6 stays open.
- **The engine–export gap widens.** At 200 shapes the score falls 9.6% but rmse256 only 5.4%, and the median gap at 500 rises to +6.6%. The search increasingly fits the engine's own rounding, which makes the agreement step the next one.

**Engine and export agreement (step 4, `8c1ee09`).**
- **Investigation.** Split of the canvas-to-export difference (about 2 levels RMSE at every count) into three parts: engine arithmetic, geometry (each renderer's coverage) and exporter arithmetic.
  - The engine's per-layer rounding does not drift: errors saturate within 5–10 layers. The gap grows only because the score shrinks. The step's hypothesis is ruled out.
  - At 500 shapes on the photos the gap splits into:
    - geometry, about 3.8 of 6.4 points: binary triangle edges, and the 1 px `quadratic` stroke, which tiny-skia draws as a hairline at 256 px;
    - tiny-skia's low-precision `div255`, which rounds up, about 2.2 points: a bias of +0.4 to +1.0 levels;
    - the engine's own rounding, about 0.4 points.
  - The SVG's 3-decimal opacity had no measurable effect.
- **Change.** The engine blend and the PNG output (tiny-skia's high-precision pipeline) both compute the exact composite rounded once per layer. The refit's layer model is then exact up to that rounding and drops its gain and bias.
- **Effect.**
  - The 500-shape median gap with the refit pass falls from +6.6% to +0.6%.
  - At 500 shapes with the refit, rmse256 improves by 2.8% overall, 3.8% for `any` and 3.9% for `triangle`.
  - At lower counts the changes are within search noise (+0.8% to −2.6%).
  - Energy kernels are 5–8% faster for most kinds and 7–8% slower for polygon and `quadratic`, which are mostly one-pixel lines.
- **Caveat: browsers.** Headless Chromium's software raster uses Skia's legacy source-over, which truncates downwards: −0.5 levels on average. It is unchanged overall by this change but loses 2–3% on dark images such as the Mona Lisa, which shared the old engine's downward bias. GPU raster, Firefox and Safari are unmeasured. The exact composite stays the reference.
- **The main lever left for SVG is geometry:** anti-aliased coverage for the binary kinds, and step 7's wider `quadratic` stroke, which also leaves the hairline path.

**Baseline after steps 3 and 4** (`8c1ee09`; median score / rmse256; this is what `approximate` returns):

| Shapes | Greedy, all kinds | With the pass, all kinds | With the pass, `any` | With the pass, `triangle` | Search s with the pass, all rows |
| ---: | --- | --- | --- | --- | ---: |
| 50 | 0.0647 / 0.0647 | 0.0615 / 0.0615 | 0.0517 / 0.0513 | 0.0553 / 0.0538 | 25.9 |
| 100 | 0.0519 / 0.0498 | 0.0482 / 0.0465 | 0.0394 / 0.0391 | 0.0429 / 0.0419 | 42.5 |
| 200 | 0.0406 / 0.0391 | 0.0370 / 0.0376 | 0.0304 / 0.0303 | 0.0336 / 0.0328 | 72.2 |
| 500 | 0.0291 / 0.0294 | 0.0255 / 0.0260 | 0.0220 / 0.0223 | 0.0243 / 0.0245 | 151.1 |

**Remove and re-add the weakest shapes (step 5, killed).**
- **Method.**
  - Rank shapes by their leave-one-out contribution from the refit's layer model. Its ranking matches exact removal: Spearman ≥ 0.999.
  - Remove the weakest 5–20%, re-add as many by greedy steps on top, and keep the result only if the exact score improves.
  - The patch is kept outside the repository.
- **Result.**
  - Pruning before the pass gains at most about 1% median rmse256 over one pass alone. Pruning between two passes mostly reverts.
  - At equal time, more passes win at every checkpoint:
    - `end:2` gains −1.2 / −1.2 / −3.3% over `end:1` at 50 / 100 / 200 shapes;
    - `end:3` gains −1.4 / −1.6 / −4.7%;
    - at 500 shapes, `end:3` gains −4.5% against −3.2% for the best prune variant, at the same time.
- **Why.** Greedy leaves almost no harmful shapes: 2 of 13,500 contribute negatively, and 7 after a pass. Its weakest shapes are worth about as much as one more greedy step.
- **Open option:** a second pass in `approximate` buys a further 1–3% for about 10% more time. It is a product choice between time and quality, not taken yet.

**Fix `quadratic` (step 6, `0a28286`).**
- **Change.**
  - The engine covers each pixel by the exact share of its area under the stroke, as tiny-skia's anti-aliased stroke does.
  - Butt caps cut through pixels, and joins are round where the curve turns.
  - Flattening is finer (0.25 px).
  - The width is 2 px. tiny-skia draws a stroke of 1 px or less as a hairline at the working size, so 1 px could not agree with the SVG and the larger PNGs.
- **Effect** (median rmse256 against the 1 px stroke, refit pass included):

  | Shapes | Change |
  | ---: | ---: |
  | 50 | −9% |
  | 100 | −15% |
  | 200 | −24% |
  | 500 | −49% |

  - The gap is 0.5–2.0%.
  - SVG bytes rise 2–6%.
  - Search time for `quadratic` is about 1.75×: rasterization is 3× slower, and colour fit and energy 50% slower.
  - `any` is unchanged within noise.
  - 3 px would fit better still (−26% and −54% at 100 and 200), but changes the look more.
- **Still open:** `quadratic`'s ssim128 still falls from 50 to 100 shapes.

**B pilot (step 7, branch `lab/b-pilot`, `ed099c8`; not for merging).**
- **Method.**
  - Triangles only, from greedy.
  - Adam on every vertex and alpha jointly (learning rate 1 px and 10 levels, cosine decay), for K iterations.
  - Forward model: the exact box-filtered half-plane coverage, as a product over the three edges, composited in f32. Its coverage is within 0.02–0.05 of tiny-skia's on edge pixels, against 0.24–0.30 for the engine's binary triangles.
  - Closed-form colours, refitted every iteration.
  - Reverse-mode gradients over √N checkpoints, checked against finite differences (relative error ≤ 1e-6).
  - A snap to 0.25 px for direct export, or to whole pixels followed by one A1 pass.
- **Result** (triangles, the default corpus): B's advantage over A1 at equal single-threaded time, in points of greedy's median rmse256:

| Shapes | B, 0.25 px, K = 50 | B, 0.25 px, K = 150 | B, 1 px + A1, K = 50 | B, 1 px + A1, K = 150 |
| ---: | ---: | ---: | ---: | ---: |
| 100 | +5.7 | +6.1 | +3.5 | +3.6 |
| 200 | +9.5 | +9.2 | +4.2 | +4.0 |

  - The A1 comparator is greedy plus P passes, with P from 2 to 12, matched to B's time.
  - At 0.25 px B wins on all 5 images. SVG bytes are 1–2% below greedy's, and B takes 1.2–1.8× greedy's single-threaded time. It passes every success criterion.
- **Findings.**
  - Snapping to whole pixels gives back 1–5% of rmse256, and more on sharp synthetic edges.
  - An A1 pass after B worsens rmse256 although it lowers the engine score. It fits the binary canvas and undoes geometry tuned for anti-aliased edges.
  - 0.25 px coordinates cost no SVG bytes: the writer already prints up to 3 decimals. The engine's integer triangles cannot hold them, though.
  - B barely uses threads (per-layer fork-join overhead), so the multi-threaded comparison handicaps it by about 1 point.
  - B costs 5.5–9.6 ms per iteration per image at 50–200 shapes, single-threaded.
- **Open question.** B combines three advantages: joint gradient search, anti-aliased coverage (what the export draws) and sub-pixel coordinates. The pilot does not separate them. Step 3 and the literature suggest that a step-adapted search could capture the first.

**Ablation (step 8, `lab/b-pilot`, `63adb24`).**
- **Arms.** `S-A1` runs the engine refit's search (4 step-adapted climbs per layer, age 25, closed-form colour) on B's smooth model with continuous coordinates. Its per-layer energy is exact and was checked against the forward render. `S-A1-int` is the same with integer coordinates.
- **Results** (triangles, single-threaded, points of greedy's median rmse256 at 100 / 200 shapes; lower is better):

| Arm | 100 | 200 | Extra s at 100 / 200 |
| --- | ---: | ---: | ---: |
| `end:4` (engine, binary, integer) | −9.4 | −10.9 | 3.4 / 3.7 |
| `S-A1-int:4` | −12.7 | −15.8 | 11.3 / 12.5 |
| `S-A1:4` | −14.4 | −18.8 | 18.8 / 21.4 |
| B, K = 50 | −14.1 | −19.1 | 1.8 / 2.5 |
| B, K = 150 | −16.1 | −20.8 | 5.7 / 7.2 |

- **Decomposition, at equal passes:**
  - coverage is worth 3.3–4.9 points (`S-A1-int` against `end`);
  - precision is worth 1.7–3.3 points (`S-A1` against `S-A1-int`);
  - gradients add about 2 points over the same search on the same model, while B with K = 150 takes a third of `S-A1:4`'s time.
- **At equal time** the search cannot finish one pass within B's budget, so B leads by 12–19 points. Climbs barely stop on a smooth energy, because tiny moves keep improving, and an evaluation costs 6–50 µs against 3–15 ms for a whole B iteration.
- **Confound:** B has no minimum-angle rule and makes slivers, 1–22% of its triangles. Part of the 2 points may come from that larger space.
- **Verdict:** gradients matter at equal time. Coverage and precision are most of B's quality, but only B reaches them cheaply.

**Done:**
1. The measurement fix (runner metrics and summaries).
2. A1 in `approximate` (`192714e`).
3. Step-adapted refit moves (`7250faf`).
4. Engine and export agreement (`8c1ee09`).
5. Remove and re-add (killed).
6. Fix `quadratic` (`0a28286`).
7. B pilot (succeeded).
8. Ablation (gradients matter at equal time).
9. B with the minimum angle (passed).

**Requirement (user decision): shapes must read as their kind.**
- Triangles keep the engine's 15° minimum angle (`Triangle::is_valid`) in every optimiser, B included. A sliver does not look like a triangle, and gains that come from slivers do not count.
- Other kinds get the same question when an optimiser is extended to them.

**B with the minimum angle (step 9, `lab/b-pilot`, `7476ad0`): passed.**
- **Enforcement.** Two approaches:
  - (a) a minimal Gauss–Newton projection after every Adam step, lifting violated angles to 15.5°, with Adam's first moment cleared along the projection;
  - (b) a penalty below 17° plus the same projection.

  The 0.25 px snap repairs a rounded triangle to the nearest valid lattice triangle. Every exported triangle is checked: zero violations, against 53–310 per run for unconstrained B.
- **Lead over the equal-time A1** (points of greedy's median rmse256):

| Shapes | Unconstrained | (a) projection | (b) penalty |
| ---: | ---: | ---: | ---: |
| 100 | 5.7–5.9 | 5.3 | 5.1–5.2 |
| 200 | 9.1–9.5 | 8.2–8.6 | 8.7–8.8 |

  - SVG bytes are 1.4–3.0% below greedy's, and B takes at most 1.8× greedy's single-threaded time at 100–200 shapes.
  - The rule costs 0.3–0.9 points, mostly on the synthetic texture and shapes. The paintings lose under 0.01.
- **Look.** Projected triangles cluster just above 15°, as greedy's integer triangles already do; the penalty moves that cluster to 16–18°. On the paintings the output reads as ordinary triangles with sharper features than A1's.
- **Choice.** (a) and (b) differ by at most 0.5 points; either works.

**Productising B (step 10, decided 2026-10-03 after an independent review).**
- **Shape.**
  - B is a terminal stage beside the engine, `Drawing -> Drawing`, in a new `primeval-core` module. `Drawing` is already `f64` and both writers take fractional coordinates, so the engine's integer shape types do not change.
  - `approximate` runs it after greedy, in place of A1, for the layers it covers. A1 stays for the kinds B does not cover yet, and as the fallback when B is skipped. A1 after B is out: it is measured to hurt.
  - Layers B covers: half-plane kinds (triangle, rectangle, rotated rectangle, polygon). Other layers stay fixed in geometry, but their colour and alpha are still fitted.
  - Minimum angle by projection, as arm (a) of step 9.
  - The output snaps to 0.25 px. Axis-aligned rectangles may need a coarser snap, since 0.25 px costs them about 8 bytes each.
- **Requirements.**
  - Seeded output does not depend on the thread count. The iteration count K is a function of the request only, never of elapsed time.
  - Native and wasm output stay identical (the browser tripwire test). B's arithmetic uses only `+ − × ÷` and `sqrt`: no libm calls such as `hypot`, `atan2`, `cos`, `powi`, `exp`, and no `mul_add`.
  - Cancellation between iterations.
  - Checkpoint memory capped as in A1, with A1 as the fallback above a size threshold.
  - ~~Budget: B's extra time is at most 0.5× greedy at 1 thread and at most 1× at 8 threads, at 100–200 shapes.~~ **Superseded (user decision, 2026-10-03): quality comes first.** Extra time is acceptable when it buys quality, since `main` is already about 2–14× faster than Go `primitive`.
    - Configurations are chosen by absolute quality at a fixed shape count, with pass and iteration counts set by diminishing returns. "Wins at equal time" is no longer the bar.
    - Times are still reported. A kind that would become slower than Go `primitive` is flagged.
- **User decisions.**
  - Polygons must be convex, with the 15° minimum interior angle, in every optimiser, greedy included. A crossed quad reads as two triangles.
  - Rotated rectangles get an aspect-ratio cap of 1:8, close to the flattest triangle the 15° rule allows (15°, 15°, 150°: about 1:7.5).
  - Today's greedy output, at 100 and 200 shapes over the five README images:
    - only 36–44% of the `polygon` kind's quads are convex; 23–27% are concave and 32–37% cross themselves, and 13–16% of the convex ones have an angle under 15°;
    - inside `any` the shares are about the same, and polygons are 38–42% of `any`'s layers;
    - rotated rectangles have a median aspect of 2.7–3.0, but 8–13% exceed 1:8, up to 1:97.
  - The polygon rule therefore changes much of `any`'s output, so its quality cost was measured before adopting it.
- **Rules adopted in greedy and the refit pass** (`40eb6d9`, committed by mistake under the plan's message):
  - polygons are strictly convex with every angle above 15°, and rotated rectangles have their long side at most 8× the short side;
  - the checks are libm-free (cross and dot products against a `tan 15°` literal), `Triangle::is_valid` included, which changed no output;
  - a move that breaks a rule is undone and drawn again from the original shape, and polygons lose the vertex-swap move, which on a convex quad either crosses it or changes nothing.
  - Median rmse256 change at 50 / 100 / 200 shapes, in points (refit pass included):
    - polygon +1.4 / +2.9 / +2.0;
    - rotated rectangle +0.3 / +1.2 / +0.7;
    - `any` −0.8 / −1.3 / +0.4.
  - The synthetic texture's rotated rectangles lose 39% / 23% / 7%, because its stripes were fitted with needles. The paintings lose 0.3–5.5%.
  - Time is unchanged (0.96–1.08× on an idle machine), and there are zero violations in the exported drawings.
  - **No upper angle bound** (decided): 21% of polygons at 200 shapes have an angle of 165° or more and read as triangles. A near-triangle is still a legible polygon, unlike a sliver, so a bound would cost quality for no legibility.
  - The half-plane kinds B can cover are 63–67% of `any`'s layers at 100 and 200 shapes.
  - Tiny triangles: no rule needed. B's output at 100 and 200 shapes has no triangle under 4 px² (greedy with A1 at 500 shapes had 18 of 2,481).
  - A silent "Refining" phase of 1–2 s in the single-threaded browser is acceptable, as long as it stays cancellable. `onProgress` does not change.
  - Native–wasm identity is a requirement for B.
  - Killed: anti-aliased coverage in the greedy search. Binary coverage costs no fidelity there, and B fixes coverage at the end.
- **Experiments, in order:**
  1. **Port the pilot (triangle only)** into the module, wired into `approximate` for `triangle`. Success: within 0.3 points of the pilot's `Bp50`/`Bp150` at 100 and 200 shapes, and native and wasm SVG identical. **Passed** (`8a1eab3`, `primeval_core::joint`):
     - median rmse256 at 50 / 100 / 200 shapes: K = 50 gives 0.050894 / 0.038609 / 0.029079, against the pilot's 0.050898 / 0.038616 / 0.029079; K = 150 gives 0.050451 / 0.037903 / 0.028473, against 0.050456 / 0.037844 / 0.028474. The largest difference is +0.13 points;
     - extra time against greedy at 1 thread: 0.29× / 0.25× / 0.19× with K = 50 and 0.88× / 0.75× / 0.57× with K = 150;
     - zero angle violations in 1,750 exported triangles (smallest angle 15.004°), and SVG bytes 1.5–3.2% below greedy's;
     - the arithmetic is libm-free, and the native–wasm tripwire passes on both wasm builds.
     - Open, for experiment 6: only the checkpoints are capped, not the buffers that hold the canvas under each layer of a segment. B always replaces the greedy drawing; it is not kept only if better.
  2. **Band-parallel sweep** (fixed bands, one fork-join per pass, colours refitted together, Jacobi). Success: at least 3× faster at 8 threads, at most 0.5 points lost, identical output at 1, 4 and 8 threads. Kill: Jacobi loses more than 1 point; then fall back to per-layer barriers and accept about 1× greedy at 8 threads with K = 50. **Passed** (`1eb9638`):
     - bands of 16 rows, each a rayon task over every layer reaching it, with its own checkpoints; sums reduced in band order. Each iteration runs a colour-fit pass, then a gradient pass at the fitted colours, with one fork-join each;
     - the colours are refitted together by an over-relaxed step (ω = 1.5) whose divisor, `Σ g·(1 − T)` with `T` the whole stack's transmittance, makes it unable to raise the loss. Plain Jacobi diverges undamped and loses 1.16 points at 200 shapes with damping 0.5; ω = 1.3 lost 4.9 points there, through the momentum bug below;
     - K = 50, against experiment 1: +0.42 / +0.04 / +0.10 points at 50 / 100 / 200 shapes (K = 150 at 1 thread: +0.23 / −0.27 / +0.25);
     - B's extra time at 8 threads falls from 0.92× / 0.87× / 0.72× greedy to 0.29× / 0.24× / 0.18×, 3.2–4.0× less. B itself now speeds up 3.8–4.1× from 1 to 8 threads, against 1.07–1.30×. At 1 thread it is unchanged (0.28× / 0.24× / 0.19×);
     - output is identical at 1, 2, 4 and 8 threads, and the native–wasm tripwire passes.
     - **Open bug** (also in experiment 1): the projection's momentum correction `m[k] -= md/dd · displacement[k]` can inflate a coordinate whose second moment is near zero, so Adam then takes a huge step (a 269 px jump was traced). It causes single-image outliers and makes small comparisons noisy, so it is fixed before experiment 3. **Fixed** (`17bd46b`):
       - **Cause:** the correction moved first moment into coordinates with near-zero second moment; steps reached 2e4× the learning rate, 100–550 times per run.
       - **Fix:** the correction now raises each coordinate's second moment to at least the square of its new first moment, so no step exceeds plain Adam's bound. On the corpus no step exceeds 3×.
       - **Effect:** median rmse256 changes by +0.16 / +0.13 / −0.33 points at K = 50 and +0.16 / −0.02 / −0.21 at K = 150. ω = 1.3, 1.5 and 1.7 now land within 0.21 points of each other, and 1.5 stays.
       - **Rejected:** weakening the correction instead, by resetting the moment, dropping or shrinking the correction, cost up to 19 points on the synthetic texture, which relies on momentum moving along the bound.
  3. **K per shape count**, from {32, 64, 100, 150} at 50 to 500 shapes: the smallest K within 0.5 points of 150 that meets the budget. **Done** (after `17bd46b`, triangles, the default corpus):

     | Shapes | K = 50 | K = 64 | K = 100 | K = 150 |
     | ---: | --- | --- | --- | --- |
     | 50 | +0.97 pt, 0.30× | +0.56, 0.39× | +0.03, 0.61× | 0, 0.91× |
     | 100 | +2.04, 0.27× | +1.65, 0.34× | +0.72, 0.53× | 0, 0.79× |
     | 200 | +1.43, 0.21× | +0.95, 0.27× | +0.48, 0.41× | 0, 0.62× |
     | 500 | +2.7, 0.14× | +1.8, 0.18× | +0.75, 0.28× | 0, 0.43× |

     - Each cell gives the points behind K = 150, then B's extra time over greedy's at 1 thread. Points are of greedy's median rmse256; at 500 shapes, of main's greedy median, 0.0274.
     - At 8 threads the time ratios are within 0.04 of these, since B now scales like greedy.
     - Quality does not saturate by K = 100 as expected, so the 0.5-point rule cannot be met within the budget at 50 and 100 shapes, and the budget decides.
     - **Decision:** K grows with the count, the largest that keeps B's extra time near 0.5× greedy at 1 thread: 80 up to 50 shapes, linear to 120 at 200 and to 160 at 500, then 160. It uses integer arithmetic, so native and wasm agree. Against K = 50 it gains 1–3 points.
  4. **`any`**: curved layers fixed, plus rectangle, rotated rectangle and convex polygon, each with its legibility rule. Success: at least 3 points over equal-time A1 at 100 and 200 shapes, zero violations, SVG bytes at most +3%. Kill: under 2 points; then B covers triangles and rectangles only, and the plan is revisited.
     - **4a, convex polygons and fixed layers** (`4e0c49f`):
       - **Design.** B reads the shapes and kinds from the model.
         - Fixed layers use a coverage mask from the engine's own rasterizer, so B computes no trigonometry itself.
         - Polygons keep strict convexity and every angle above 15° by projection: residuals per vertex, with angles held at or below 179.5°. A 165° upper bound would be one more residual.
         - Per-kind pixel loops keep the triangle path as fast as before; triangle rows are unchanged.
       - **Polygon passes; `approximate` runs B for it.** Points over equal-time A1 at 8 threads: +3.7 / +5.2 / +7.5 at 50 / 100 / 200 shapes. At 1 thread whole passes cannot match B's time, and interpolated it is about +3.7 at 100.
         - SVG is 13% smaller than greedy's, with zero violations; 18 of 1,750 exported quads needed the lattice repair.
         - A1 then B is worse for polygons.
       - **`any` misses.** B alone, with curved layers, rectangles and rotated rectangles fixed: −0.1 / +2.5 / +3.2 points at 8 threads, and +0.5 at 200 shapes at 1 thread. A1 then B: +0.8 / +1.6 / +2.0. `any` keeps the refit pass.
       - **Time.** B's extra time is only 0.10–0.16× greedy's for these kinds, since greedy is slower on them, so the iteration rule calibrated on triangles leaves most of the budget unused.
       - **Outlier.** synthetic-shapes has large model–export gaps in every variant, greedy included; on polygons B loses to A1 there.
       - The browser tripwire's shape list lacks `polygon`, which now runs B.
       - **More iterations do not help** (K = 200 / 320 / 480 against the default, 8 threads). B levels off fast on these kinds, while A1 keeps gaining with each pass. B's lead over equal-time A1 shrinks:
         - polygon at 100 shapes: +5.2 → +3.1 / +2.4 / +2.0;
         - `any` at 100 shapes: +2.5 → +1.3 / 0.0 / −0.7.

         The iteration rule stays as it is for every kind.
       - **R refit passes, then B, on `any`** (R = 1–4): +0.5 to +2.5 points over equal-time A1, all below B alone at 100 and 200 shapes. The fixed layers' geometry is what limits B on `any`.
     - **4b, rectangles and rotated rectangles** (`de99d66`):
       - **Axis-aligned rectangles.** Their coverage is the exact separable product of four ramps. The slope at a ramp's kink is the mean of its one-sided slopes, since greedy's sides sit on pixel boundaries and the open-interval slope there is zero. They snap to 0.5 px: 0.25 px cost +6–7% SVG bytes, and 1 px cost up to 1.7 points.
       - **Rotated rectangles.** They are parametrised by centre, half-side vector and half-width, with corners computed by `sqrt` only. They snap in parameters, so they stay exact rectangles. Greedy's integer angle is converted by a libm-free polynomial `sin_cos`.
       - **Rule.** Both kinds keep sides of at least 1 px and at most 1:8, by an exact projection.
       - **Results.** Points over equal-time A1 (+6.3 / +7.8 / +8.6 for rectangle, +5.0 / +6.3 / +8.8 for rotated rectangle) at 8 threads, with B's extra time at 0.40–0.48× greedy's. Zero violations. SVG is +1.7–2.5% against greedy for rectangles and −1.2 to −2.0% for rotated rectangles. `approximate` runs B for both kinds.
       - **The greedy 1:8 cap on axis-aligned rectangles.**
         - Median cost: rectangle +2.2 / +2.0 / +1.0 points.
         - `any` gains: −2.3 / −0.5 / −1.4.
         - One image loses badly: synthetic-shapes in `any`, +7 / +35 / +72%, because it was fitted with thin bars. Kept, for consistency with the needle rule.
       - **`any`** (B now covers about 65% of its layers): B alone gains +3.2 / +3.7 points over equal-time A1 at 100 / 200 shapes, which passes the original bar.
         - The best absolute result is `joint:2` (two refit passes, then B): 0.048368 / 0.036495 / 0.028124, or 16.2 points under greedy at 200.
         - B on synthetic-shapes in `any` stays worse than greedy, the outlier already seen in 4a.
         - Left for the quality-first pipeline to decide.
     - **Quality-first final stage, after 4b.** For each kind, search the final stage for the best absolute quality: R refit passes, then B with K iterations, or refit passes alone for kinds B does not cover. Stop at diminishing returns, by a deterministic rule rather than a time budget. Check triangles at K above 150. With time free, the iteration rule and `any`'s pipeline are revisited.
       - **First sweep** (`4e0c49f`, 8 threads, median rmse256 against today's final stage; times only indicative, since 4b ran at the same time):
         - B iterations: triangles gain at most −0.4 / −1.3 / −0.8% at K = 300, and refit passes before B add nothing. Polygons gain −1.1 / −1.2 / −0.9% at K = 480 and little beyond.
         - Refit passes for the kinds B does not cover: 4 passes give −1.0% to −4.1%, and 16 give −1.2% to −6.1%. Rotated ellipses gain most.
         - **Refit passes during the search** (`--refine every:K`, no final stage): every 10 steps gives:
           - `any` −5.3 / −5.6 / −7.4%;
           - circle −9.5 / −4.5 / −6.6%;
           - ellipse −3.0 / −4.2 / −5.8%;
           - rotated ellipse −4.4 / −5.6 / −8.7%;
           - quadratic up to −1.6%.

           That beats B on `any`. It is worse on triangles and polygons only because it replaces B there.
       - **Next, the quality-first pipeline:** refit passes during the search, combined with the final stage.
         - The final stage is B for the half-plane kinds, with its iteration count raised where it pays. For the others it is refit passes until a pass gains less than a relative threshold, with a cap.
         - Also: greedy effort (candidates per step, climb age) at a fixed shape count, and `any`'s pipeline.
         - Every stop rule depends only on deterministic scores, never on time or thread count.
     - **4b, next:** rectangles and rotated rectangles in B, both for their own kinds and inside `any`, where they raise B's share of the layers from about 52% to 65%. Then `any` is measured again.
       - A rotated rectangle is parametrised without trigonometry: centre, half-side vector and half-width, with corners computed by `sqrt` only.
       - Both kinds get the 1:8 aspect cap. It is new for axis-aligned rectangles, for consistency, and its cost is measured before it is adopted.
  5. **A1 then B** at equal time. Keep only if it gains at least 0.5 points.
  6. **Scale guard** at `resizeInput` 1024 and 2048 and counts of 500 and 2000: peak memory and time ratio. This sets the fallback threshold.
  7. **Curved kinds** with their own smooth coverage: later, and perhaps never for `quadratic`.

**Quality-first pipeline: work in progress** (2026-10-03; committed as WIP, then tested):
- **State of the code.** `crates/primeval-render/src/pipeline.rs` gives each kind a pipeline, which `approximate` and the `lab` path share:
  - refit passes during the search on a `Spaced` schedule: every `interval` steps up to `interval · divisor`, then geometrically spaced, so the cost grows linearly with the count;
  - final refit passes;
  - B with a multiple of its default iteration count.
  
  The model also gets a per-kind search effort (`Effort`: rounds and climb age, 32 rounds for `any`), with lab overrides behind `primeval-core`'s new `lab` feature. **Tested** (2026-10-03, local machine): the tests pass and `npm run verify` is green, with no code fix needed and no digest pin changed beyond the WIP's three greedy pins for the new effort (quadratic, rotated ellipse, polygon).
  - The lab identity test now covers all nine kinds and asserts that each runs at least one refit pass during the search.
  - The browser native–wasm tripwire adds ellipse and circle and runs 20 steps instead of 12, so every kind reaches its first refit pass. It now takes about 58 s on the single-threaded build.
  - Known gap: `approximate`'s cancellation test would pass even if a refit pass in the search ignored the token, since the next step returns `Aborted` anyway. The pipeline's unit test covers the pass itself.
- **The chosen pipelines,** as in `pipeline()` (updated 2026-10-07, after the retune below):

  | Kind | Search effort (rounds, climb age) | During the search | Final refit passes | B (× default K) |
  | --- | --- | --- | --- | --- |
  | `any` | 16, ×1 | every 20, then spaced (÷5) | 1 | ×1 |
  | triangle, rectangle, polygon | 16, ×1 | every 20, then spaced (÷5) | 0 | ×1 |
  | rotated rectangle | 16, ×1 | every 20, then spaced (÷2) | 0 | ×2 |
  | ellipse, circle, rotated ellipse | 16, ×1 | every 10, then spaced (÷10) | 1 | — |
  | quadratic | 16, ×2 | every 20, then spaced (÷5) | until a pass gains < 1%, at most 4 | — |

  B's result is kept only if it exports closer to the target than its input (the guard below).

  Selection rule: each stage was extended while the last extension lowered the mean of the 100- and 200-shape medians by at least 0.5% without worsening the mean of the per-image changes; otherwise the cheaper configuration stayed. Final refit passes until a pass gains less than 1%, 0.5% or 0.2% gained less than 0.5% for ellipses, circles and rotated ellipses, so one pass stays. The retune of 2026-10-07 (below) traded some quality for time on `any`, triangle, ellipse and rotated ellipse.
- **Quality of the WIP pipeline, before the guard** (median rmse256 against the previous `approximate` at 50 / 100 / 200 / 500 shapes; deterministic, so valid; polygon, rotated rectangle and quadratic are superseded by the re-measure below):
  - `any` −7.7 / −10.3 / −11.4 / −10.2%;
  - rotated ellipse −6.5 / −8.4 / −10.2 / −10.9%;
  - ellipse −5.5 / −6.9 / −7.6 / −5.5%;
  - circle −10.0 / −4.9 / −6.7 / −5.8%;
  - triangle −4.6 / −5.3 / −3.7 / −3.7%;
  - polygon −2.5 / −3.6 / −3.4 / −3.4%;
  - rectangle −1.3 / −3.1 / −4.1 / −3.8%;
  - rotated rectangle −1.8 / −2.3 / −1.0 / −0.5%;
  - quadratic −2.5 / −3.2 / −5.8 / −12.3%.
  - **Images that get worse by more than 2%** (eleven rows on the x86 re-run below, all synthetic; the M3 list above had missed the polygon and rotated-rectangle rows):
    - synthetic-shapes: ellipse 500 +19.5%, polygon 500 +17.7%, rotated rectangle 50 +9.1%, polygon 200 +9.0%, triangle 200 +8.1%, `any` 100 +5.8%, `any` 500 +5.7%;
    - synthetic-gradient: polygon 200 +3.7%, triangle 200 +3.1%, polygon 100 +2.8%, polygon 500 +2.7%.
- **Re-baseline on one dedicated machine** (2026-10-03, cloud VM: Linux x86_64 under KVM, Xeon 2.10 GHz, 4 vCPU, so the x86 path without SIMD; Rust 1.99.0 fat LTO; Go `primitive` `0373c21` with go1.26.0; runs sequential). Branch at `ae1b077`, which has the same `pipeline()` as `4ca8631`; `main` at `b9e24b4`; previous `approximate` at `b62fdf8`.
  - Quality reproduces the M3 numbers above to within 0.1 points, although greedy uses libm.
  - **The pipeline alone** (`engine --refine final`, PERF-0, 4 threads), time against the previous `approximate` at 50–500 shapes: `any` 2.7–3.2×, rotated ellipse 3.0–3.2×, ellipse 2.0–3.1×, quadratic 3.1×, triangle 2.4–2.7×, polygon 2.0–2.2×, circle 1.7–2.1×, rectangle 1.6–2.1×, rotated rectangle 1.6–1.9×.
  - **Versus Go, `main` → branch** (`versus_go` defaults, geometric means over 18 configurations): speedup 4.14× → 1.78× at 200 steps and 4.13× → 2.02× at 1000; RMSE ratio against Go 0.921 → 0.808 and 0.838 → 0.741. primeval's RMSE is lower in 36 of 36 configurations on both.
    - Per kind at 200 / 1000 steps on the branch: circle 5.2 / 6.9×, ellipse 3.6 / 4.4×, rectangle 3.0 / 3.3×, rotated rectangle 2.2 / 2.2×, triangle 2.1 / 2.4×, rotated ellipse 1.5 / 1.7×, `any` 1.3 / 1.5×, **polygon 0.83 / 0.82×, quadratic 0.42 / 0.50×**.
    - Polygon and quadratic are slower than Go on x86. Without the pipeline they would be about 1.8× and 1.3× faster. On `main`, polygon was already the closest kind (1.85 / 1.77×).
    - Quadratic's RMSE against `main` falls by 35% / 40% (the 2 px stroke plus the pipeline). Its RMSE ratio against Go is 0.56 / 0.42.
- **B acceptance guard** (2026-10-04). `final_stage` keeps B's drawing only if its PNG export at the working size (`raster::squared_error`: tiny-skia, anti-aliased, B's snap already applied) has a strictly lower sum of squared errors against the target than the drawing before B; a tie keeps the drawing before B.
  - **Why.** B returned its last iterate unconditionally. On synthetic images it worsened its own objective in 7 of 15 probed runs (+0.2% to +20.3%). Its objective also disagrees with the export on finely fitted stacks:
    - its coverage, the product of half-planes, squares where two near-collinear edges cross the same pixels (one quad with a 179.976° vertex ran about 106 px along one pixel row and held 45% of B's starting loss);
    - the 0.25 px snap alone cost +85–97%.

    On synthetic-shapes, polygon, 500 shapes (32 rounds), B's input exported at rmse256 0.003218 and its output at 0.006630 (+106%) while B reported a 42% gain. A check on B's own objective would miss exactly those cases, so the guard measures the export.
  - **Determinism.** Integer sums over the 8-bit raster, in one thread. tiny-skia's high-precision pipeline uses `f32` `+ − × ÷` and `sqrt`, `as` conversions to fixed point and round-to-nearest-even stores, alike on NEON, SSE2 and wasm's scalar path (read in tiny-skia 0.12's source, not proven by a test). The one platform libm call is `sin`/`cos` of a rotated ellipse's angle (`Transform::from_rotate`), which only `any` draws and the export already depends on. The browser native–wasm tripwire is the backstop.
  - **Tests** (`pipeline.rs`): a 24 × 24 point-sampled synthetic-shapes model where B exports 4× worse (the guard returns the input; the test fails with the guard disabled), one where B exports better (kept), and a tie (input kept). The thread-count identity test runs every kind's final stage, the guard included. No digest pin changed for the guard.
  - **Known limitation, contained by the guard:** B's coverage model double-counts near-collinear edges. Fixing it in B (an exact polygon coverage, or a penalty on near-180° vertices) would let B gain where it is now rejected.
- **Re-measure with the guard** (2026-10-04, M-series Mac, PERF-0 corpus, seed 42, `engine --refine final`). Against the previous `approximate` (`b62fdf8`, reproduced with lab flags). Cost is a deterministic proxy: greedy evaluations + refit evaluations × 7.5 (polygon) / 13.8 (rotated rectangle) / 4.6 (quadratic) + B layer-iterations × 23 (polygon) / 58 (rotated rectangle), summed over the images, against the baseline's.
  - **The guard on the baselines.** It never fired for rotated rectangles. For polygons it fired only on synthetic-shapes at 200 and 500 shapes, where B exported +12.9% / +14.3% worse than its input; the medians are unchanged and the per-image mean falls by 2.3% / 2.5%.
  - **Polygon** (16 rounds, every 20 then spaced ÷5, no final refits). Per-image changes are against the unguarded baseline.

    | Config | 50 | 100 | 200 | 500 | Per-image mean | Cost |
    | --- | --- | --- | --- | --- | --- | --- |
    | baseline (median rmse256) | 0.046673 | 0.035312 | 0.026399 | 0.018384 | — | 1 |
    | B ×1 (chosen) | −1.5% | −3.6% | −1.5% | −1.1% | −3.7 / −7.2 / −9.0 / −9.1% | 1.16–1.39× |
    | B ×2 | −1.7% | −3.6% | −2.0% | −2.5% | −5.1 / −7.5 / −9.3 / −9.8% | 1.23–1.53× |

    B ×2 lowers the mean of the 100/200 medians by only 0.24% against B ×1, so B ×1 stays. synthetic-shapes gains −42% at 200 and 500 against the previous `approximate` (the guard keeps B's input there at every count); synthetic-gradient loses +3.5 / +5.0 / +3.7% at 100 / 200 / 500.
  - **Rotated rectangle** (16 rounds, no final refits):

    | Config | 50 | 100 | 200 | 500 | Per-image mean | Cost |
    | --- | --- | --- | --- | --- | --- | --- |
    | baseline: no refits in the search, B ×1 | 0.052168 | 0.040069 | 0.030638 | 0.022085 | — | 1 |
    | no refits in the search, B ×2 | −0.0% | −0.1% | −0.8% | −0.3% | −3.0 / −2.6 / −1.9 / −0.1% | 1.18–1.31× |
    | `spaced(20, 2)`, B ×2 (chosen) | −1.8% | −2.6% | −1.6% | −2.7% | +0.0 / −3.6 / −1.9 / −4.3% | 1.36–1.57× |
    | `spaced(20, 5)`, B ×2 (the WIP) | −1.8% | −2.3% | −1.0% | −0.5% | +0.0 / −6.1 / −2.1 / −1.6% | 1.36–1.70× |

    B ×2 alone gains 0.39% (below the bar). `spaced(20, 2)` with B ×2 gains 2.16% on the mean of the 100/200 medians with a better per-image mean at 100 and 200 (+0.02% at 50, where synthetic-shapes loses 9.1%, as before). `spaced(20, 5)` costs more and is 0.43% worse.
  - **Quadratic** (16 rounds, climb age ×2, `spaced(20, 5)`, final refits until a pass gains less than 1%, at most 4): medians 0.192748 / 0.159637 / 0.113070 / 0.041770, −1.8 / −2.2 / −4.4 / −10.5% against the baseline (0.196203 / 0.163186 / 0.118244 / 0.046677); per-image mean −1.2 / −2.4 / −5.2 / −13.2%, no image worse by more than 2%; cost 1.44 / 1.48 / 1.56 / 1.61×.
  - Polygon's greedy digest returned to its value before the 32 rounds (`0xb57dc794666b2a2b`); quadratic's is new (`0xe3a5dd0d1b3341ff`).
  - Not re-measured: triangle, rectangle and `any`, whose pipelines did not change but which the guard also covers. The investigator's emulation of the guard found triangle unchanged and `any` on synthetic-shapes at 100 / 500 going from +5.8 / +5.7% to −11.0 / −29.5%, with unchanged medians.
- **Known property: ellipse on synthetic-shapes at 500 (+19.5%).** Refit passes optimise the aliased engine canvas, while rmse256 measures the anti-aliased export: on that image 12 of 54 passes raised the export's error. Seeds 1–5 range from −11% to +22% there. No cheap guard fixes it (a pass is already kept only if it lowers the engine's score); anti-aliased refits would.
- **Retune** (2026-10-07), on the measurements of `docs/algorithm-leap-review-2026-10-07.md` (Apple M2 Pro, PERF-0 corpus; mean of the 100- and 200-shape median rmse256 against the previous `approximate`, time ratio over the same rows). The passes during the search were 45–70% of the time for five kinds and the only stage that lost at equal time, and the doubled effort bought little:
  - `any`: 16 rounds and `Spaced(20, 5)`, −6.9% at 1.57× (was −10.8% at 2.78×); equal-time ratio at 200 shapes 1.01 (was 1.18);
  - triangle: `Spaced(20, 5)`, −12.9% at 2.06× (was −14.8% at 3.22×);
  - ellipse: `Spaced(10, 10)`, −5.1% at 2.15× (was −7.2% at 3.11×);
  - rotated ellipse: 16 rounds and climb age ×1, −7.0% at 1.56× (was −9.2% at 3.03×).

  Quadratics keep the age ×2 (2.6 points for 57% more time; no cheaper lever). Rotated ellipse's greedy digest is new (`0xf9f17763932224cc`); `any`'s is unchanged.
- **Next** (agreed 2026-10-08 on `docs/algorithm-leap-review-2026-10-07.md`; `versus_go`, the gallery and the README were regenerated at `646252c`):
  1. B during the search, at a `Spaced`-like schedule, in place of the refit passes for the kinds it covers. **Measured 2026-10-08** (review, section 10) with a lab schedule of the runner (`--during joint:K:C:I`, `Model::adopt`): triangles gain 1.3% at 0.70× the time with 20 iterations on `Spaced(20, 5)` and move to it; polygons wait for item 3 (B is rejected on hard edges, where the refit passes are what they need); rectangles and rotated rectangles lose B's sub-pixel gain when converted back to integer geometry and `any` needs the passes for its curved layers, so those three keep the refit passes.
  2. B for the curved kinds (ellipse, circle, rotated ellipse; quadratic last), with a smooth coverage on the boundary pixels, so that their costly passes can go.
  3. B's coverage of near-collinear edges (an exact convex-polygon pixel area, or a penalty near 180°), so that the guard rejects B less often.
  4. A per-shape stroke width for quadratic, bounded and exported as `stroke-width`; no new public option.
  5. Layer-parallel refit, verified against the sequential pass: the only large lever left on refit time (more climbs per layer do not help; see the review).
  6. A2, better greedy proposals (an evolution strategy seeded from the error grid); never tried.
  7. A second final refit pass for the kinds without B: 1–3% for about 10% time.
  8. x86 SIMD kernels, last, in a session on x86 runners where they can be measured; the NEON-against-scalar parity tests are the model.
  Also still open: experiment 6 (memory guard at `resizeInput` 1024 / 2048), the manual demo check below, and the review of the merged change.

**After the merge** (the branch was squash-merged as `16877bb` on 2026-10-07 before these were done; status as of 2026-10-08):
- a full review of the merged change (user request), with the `/simplify` and `/code-review` skills: **open**;
- regenerate the gallery, the README comparison images and the versus-Go numbers (`CONTRIBUTING.md`): **done** at `646252c`, after the retune;
- check by hand in the demo that "Refining" stays visible during a long single-threaded pass, and that Stop during it keeps the preview: **open**.

---

## Appendix A: measurements and reproduction

### A.1 Environment and baseline gate

- **Machine:** Apple M3, 8 cores (4P + 4E), macOS (Darwin 27).
- **Toolchains:** Node 24.18.1, npm 11.16.0, Rust 1.93.0 (pinned) and 1.99.0.
- **Gate:** `npm run verify` exits 0 in 34.8 s (Rust 132 tests, package 34, tooling 17; pack 13.1 kB, 10 files).
- **Rust 1.99 clippy:** `cargo +1.99.0 clippy --all-targets -- -D warnings`. 6 errors (fixed in T1).
- **Edition 2024 lints:** `cargo +1.99.0 clippy --all-targets -- -A clippy::all -W rust-2024-compatibility` gives 96 warnings.
- **Rustdoc:** `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` failed on 3 broken links (fixed in T1).
- **Missing docs:** `RUSTFLAGS="-W missing_docs" cargo check -p primeval-core -p primeval-render` reports 186 items.
- **Publish dry runs:** `cargo publish --dry-run -p primeval-core` / `-p primeval-render` (API-8).

### A.2 Disassembly of release artifacts (REL-1, REL-2)

```bash
objdump -d --no-show-raw-insn artifacts/<target>/primeval-node.<suffix>.node > out.asm
grep -c '%zmm\|zmm[0-9]' out.asm                     # AVX-512 registers
grep -cE '%k[1-7]|\{k[1-7]\}' out.asm               # AVX-512 mask registers
grep -c '%ymm\|ymm[0-9]' out.asm                     # AVX/AVX2 registers
grep -cE '\bz[0-9]+\.|\bptrue\b|\bwhilelo\b|\bp[0-7]/[zm]' out.asm   # SVE (arm64)
objdump -T artifacts/x86_64-unknown-linux-gnu/*.node | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
```

### A.3 Throughput harness

This is a scratch Cargo project outside the workspace, so `.cargo/config.toml` does not apply. It depends on `primeval-render` and `primeval-core` by path and uses the workspace release profile. It runs `approximate()` with `seed: Some(42)`, SVG output at 1024 px and default options, and reports wall time plus an FNV-1a hash of the output.

```bash
cargo build --release                                                  # portable
RUSTFLAGS="-C target-cpu=native" cargo build --release --target-dir target-native
CARGO_PROFILE_RELEASE_PANIC=unwind cargo build --release --target-dir target-unwind
./target/release/bench shapes docs/readme/originals/americangothic.jpg 200
./target/release/bench workers docs/readme/originals/monalisa.jpg         # determinism vs workers
```

Determinism (`any`, 50 steps, seed 42):

| Workers | Time | SVG hash |
| ---: | ---: | --- |
| 1 | 3.475 s | `fca32544a5ff8b04` |
| 2 | 1.952 s | `59851bb2b5cd936a` |
| 4 | 1.136 s | `c27a758652b3492a` |
| 8 | 0.893 s | `53db601d6afdaffe` (identical when repeated) |
| 12 | 1.273 s | `c00947f79b616c81` |

### A.4 Output formats (Mona Lisa, `any`, 200 steps)

Measured with `/usr/bin/time -l`:

| Output | Time | Peak RSS | Size |
| --- | ---: | ---: | ---: |
| SVG, 1024 px | 2.85 s | 11 MB | 26,947 B |
| PNG, 1024 px | 2.89 s | 11 MB | 218,062 B |
| JPG, 1024 px | 2.98 s | 11 MB | 140,391 B |
| GIF, 1024 px | 4.97 s | 152 MB | 2,288,471 B |
| GIF, 2048 px | 10.99 s | 585 MB | 5,159,259 B |

### A.5 Evaluation probe (section 6)

This is a binary in the same scratch project. It builds a `Model` with `workers: 1` and seed 42 on the 256 px thumbnail, runs N steps per shape and sums the evaluation counts returned by `Model::step`. It then rasterizes 20,000 `Shape::random` candidates against the resulting canvas to measure pixels and scanlines per candidate. The hill-climb share is `(evaluations/step − 16 × candidates) / evaluations/step`.

<details>
<summary>Probe source (scratch, to be ported into PERF-0 benches)</summary>

```rust
use primeval_core::buffer::Buffer;
use primeval_core::error_grid::ErrorGrid;
use primeval_core::export::{average_background, thumbnail};
use primeval_core::model::{Model, ModelOptions};
use primeval_core::shapes::{Shape, ShapeKind};
use primeval_core::worker::{SearchRound, WorkerCtx};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let steps: u32 = std::env::args().nth(2).unwrap().parse().unwrap();
    let img = image::open(&path).unwrap();
    let bg = average_background(&img);
    let target = Buffer::from_image(&thumbnail(&img, 256));
    let (w, h) = (target.width(), target.height());
    for name in ["any", "triangle", "rectangle", "ellipse", "circle",
                 "rotated-rectangle", "quadratic", "rotated-ellipse", "polygon"] {
        let kind: ShapeKind = name.parse().unwrap();
        let mut model = Model::new(target.clone(), bg, 1024,
            ModelOptions { seed: Some(42), workers: 1, ..ModelOptions::default() });
        let t = Instant::now();
        let mut evals = 0u64;
        for _ in 0..steps { evals += model.step(kind, 0, 0).unwrap(); }
        let secs = t.elapsed().as_secs_f64();
        let defaults = ModelOptions::default();
        let mut grid = ErrorGrid::new(w, h, defaults.grid_cols, defaults.grid_rows);
        grid.compute(&model.target, &model.current);
        let round = SearchRound { target: &model.target, current: &model.current,
                                  error_grid: &grid, score: 0 };
        let mut worker = WorkerCtx::new(w as i32, h as i32, primeval_core::rng::create_rng(7));
        let (mut px, mut lines_total, n) = (0u64, 0u64, 20_000u64);
        for _ in 0..n {
            let shape = Shape::random(kind, &mut worker, &round);
            let lines = shape.rasterize(&mut worker);
            lines_total += lines.len() as u64;
            px += lines.iter().map(|l| (l.x2 - l.x1 + 1).max(0) as u64).sum::<u64>();
        }
        println!("{name} ms/step={:.2} evals/step={:.0} ns/eval={:.0} px/cand={:.0} lines/cand={:.1}",
            secs * 1e3 / f64::from(steps), evals as f64 / f64::from(steps),
            secs * 1e9 / evals as f64, px as f64 / n as f64, lines_total as f64 / n as f64);
    }
}
```

</details>

### A.6 Duplicate-pixel check (ENG-2)

A third scratch binary rasterizes 5,000 `Shape::random` shapes per kind on a 256×256 canvas. It counts `(y, x)` pixels emitted more than once, and lines that are out of bounds or have `x1 > x2` or alpha > 0xFFFF. Results are in ENG-2; no invalid lines were found for any kind.

### A.7 Node-level probes (sections 3–4)

These are small ESM scripts that import `dist/index.js` (and run `dist/cli.js`) after `npm run build && npm run build:node`. The exit code was captured for each.

| Probe | Input | Observed |
| --- | --- | --- |
| Thin image | 1000×1 PNG, `shape: "any"` | `assertion failed: min <= max`, exit 134 |
| Multi-byte background | `background: "a€bc"` | `byte index 2 is not a char boundary`, exit 134 |
| Wide GIF | 1000×10 PNG, `outputSize: 70000`, GIF | gif frame-size assertion, exit 134 (PNG resolves) |
| Sync throw | `background: "nope"` | synchronous `Error`, `instanceof ValidationError === false` |
| Throwing callback | `onProgress` throws | process exits 1 |
| u32 wrap | `count: 2**32 + 1` | resolves with `total: 1` |
| Seed saturation | seeds 1e19, 1e20, 1e300 | identical outputs |
| Abort race | `abort()`, then 200 ms of busy main thread, 40 runs | `resolvedDespiteAbort: 40` |
| FIFO path | `{ kind: "path" }` on a FIFO, abort at 200 ms | still pending at 2000 ms |
| Large PNG | 258,606 B, 9000×9000, `count: 1` | peak RSS 950,894,592 B |
| CLI | various flags | rows CLI-1 to CLI-4 |

---

## Appendix B: cross-check with the second review

The maintainer shared a second, independent review. Where both reviews agree, the items above already reflect it. Its distinct contributions, now part of this plan, are:

- Biome as the only TS/JS tool;
- the CI matrix `[22, 24, 26]`;
- the list of non-goals (section 13);
- grouped Dependabot with no further supply-chain tooling;
- using `ImageReader` + `Limits` instead of `load_from_memory`;
- framing the first tickets as "portable native builds", "render resource limits", "streaming GIF" (here: GIF removal) and "toolchain refresh".

Corrections and differences, based on this audit's evidence:

1. **`engines: ">=22"` is not enough.** Node 22.0–22.11 lacks unflagged `require(esm)`, which the loader relies on (REL-3), so the floor must be `>=22.12`. Node 20's EOL is 2026-04-30, not March.
2. **Limits at the public boundary are not sufficient on their own.** Two of the four reproduced crashes are unrelated to size: the multi-byte background (RT-3) and the 1-pixel working side (RT-2). Since `panic = "unwind"` has no measurable cost (RT-1), it is adopted as defense in depth on top of the root-cause fixes.
3. **The current contract tests are regex checks over source.** They missed real drift (TEST-1). No code generation is agreed, but they become runtime table-driven tests.
4. **Before x86 SIMD come algorithmic wins with measured hotspots:** anti-aliased rasterizer interiors (PERF-5), quadratic rasterization (PERF-7) and prefix sums (PERF-1). x86 SIMD is deferred until the "next" decision (section 15).
5. **Issues not covered by the second review:**
   - glibc 2.34 (REL-2);
   - synchronous errors and the throwing `onProgress` (NODE-1, NODE-2);
   - NEON unsoundness (ENG-1);
   - numeric wrap at the napi boundary (NODE-3);
   - the abort race (NODE-4);
   - raster/SVG geometry mismatch and quadratic duplicates (ENG-3, ENG-2);
   - seed determinism (ENG-4);
   - sample-image licensing (DOC-6);
   - the invalid YAML indentation in `.editorconfig` (TOOL-12).
