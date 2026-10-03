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

**Requirement (user decision): shapes must read as their kind.**
- Triangles keep the engine's 15° minimum angle (`Triangle::is_valid`) in every optimiser, B included. A sliver does not look like a triangle, and gains that come from slivers do not count.
- Other kinds get the same question when an optimiser is extended to them.

**Next:**

9. **B with the minimum angle enforced.** Re-run the pilot's protocol with the rule kept throughout and checked on every exported triangle after the snap, with zero violations. B must still beat the equal-time A1 by at least 3 points at 100 and 200 shapes.
10. **Decide how to productise B**, if step 9 passes. The user decides; an independent review is advisable first. Candidate shape:
- B becomes the final stage of `approximate` and exports its 0.25 px drawing directly, with no A1 pass after it.
- It starts with the polygonal kinds (triangle, then rectangle, rotated rectangle and convex polygon, which share the half-plane coverage). Layers of other kinds stay fixed in the composite.
- **Needed:**
  - cancellation between iterations;
  - multi-threading by image bands;
  - the minimum-angle rule (step 9);
  - deterministic reductions, which the pilot already has;
  - a time budget per shape count that stays within 2× greedy in single-threaded wasm.
- **Open questions:**
  - whether the engine's greedy search should also get anti-aliased coverage;
  - the curved kinds (ellipses, circles, `quadratic`) and `any`, the default, whose layers mix every kind;
  - whether B replaces the A1 pass or follows it.

**Before merging the branch:**
- regenerate the gallery, the README comparison images and the versus-Go numbers (`CONTRIBUTING.md`), which output changes make stale;
- check by hand in the demo that "Refining" stays visible during a long single-threaded pass, and that Stop during it keeps the preview.

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
