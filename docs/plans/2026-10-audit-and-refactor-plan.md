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
- The engine works in RGB only: backgrounds must be opaque, and transparent inputs are composited at decode time (RM-4).

**Top findings**

| # | ID | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| 1 | PERF-5 | Polygon and rotated-ellipse are rasterization-bound (10–32 ns/pixel versus 2–4 for rectangles) and dominate run time. Benchmarks now exist (PERF-0). | Medium | Measured |

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

**Maintainer actions:**

- Delete the `v0.1.1` tag (never published); versioning restarts at the first real release.
- npm trusted publishing (OIDC) may not be able to create a package that does not exist yet: the first publish of each platform package may need a token or pre-created packages. This may also explain why the `v0.1.1` run published nothing.

### REL-7: No musl targets

- **Severity / status:** Low (product decision). `next: yes`.
- **Fix:** Alpine is common in containers. Consider `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` once REL-1, REL-2 and REL-5 are in place. The loader already detects musl.

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

### Smaller correctness items

| ID | Severity | Status | `next` | Where | Problem | Fix |
| --- | --- | --- | --- | --- | --- | --- |
| ENG-6 | Low | Verified | no | `score.rs:58-76` | `compute_color` ignores `line.alpha` (coverage), so anti-aliased edges and quadratic pixels are fitted as fully covered and the colour comes out under-saturated. This matches Go; the quality impact is unmeasured. | Weighted least squares: `s* = Σw(t−(1−w)c) / Σw²` with `w = (alpha/255)·(ma/65535)`. |
| ENG-10 | Low | Reported | no | `error_grid.rs:80-97`, `:141-144`, `:172-173` | Biased sampling only covers `cell_w × cell_h` per cell, but the last row/column absorbs the remainder, which is then reached only by the 20% uniform samples. | Sample within each cell's real bounds. |

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
- Full runner: 228.6 s over 90 runs. Quadratic quality is far behind every other kind (score 0.12–0.24 against 0.03–0.05 on the photos).

Since ENG-4 (T5), runner quality is identical across thread counts; times still depend on the machine.

### PERF-1: Per-step precomputation and an exact early exit

- **Severity / status:** Medium. Estimated (about 2× fewer per-pixel operations on scoring-bound shapes). `next: no`.
- **Where:** `compute_color` (scalar `score.rs:39-88`, NEON `:247-308`) re-sums Σ(t−c) and Σc for every candidate. `energy_from_lines_raw` (scalar `:108-169`, NEON `:330-414`) recomputes the "before" error (t−c)² for every candidate. Yet `target` and `current` are constant for the whole step.
- **Fix:**
  - Once per step (it can be folded into `ErrorGrid::compute`), build per-row prefix sums of (t−c) and c per channel, and of the per-pixel error.
  - `compute_color` becomes O(scanlines).
  - The energy becomes `score − Σ before(prefix) + Σ after`, so only the blend plus one square remains per pixel.
  - Because `after ≥ 0`, the running total only grows once "before" is subtracted, so the loop can stop as soon as `total ≥ best_energy`. Hill climbing rejects most moves, so this prunes well; how well is unmeasured.

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

### PERF-4: The alpha channel is processed even when it cannot change

- **Severity / status:** Medium. Estimated (about 25% of per-pixel lanes). `next: no` (the kernels); the product decision lives in RM-4.
- **Detail:** with an opaque target and background, the blended alpha provably stays 255, yet `score.rs` blends and squares channel A everywhere. JPEG inputs and the auto background are always opaque.
- **Fix:** with RM-4 (work in RGB only), use RGB-only kernels (`vld3_u8`) and 3-byte buffers: about 25% less arithmetic and memory traffic.

### PERF-5: Anti-aliased rasterizers compute coverage for interior pixels

- **Severity / status:** High for those shapes. Verified, consistent with the measured ns/pixel. `next: no`.
- **Where:** `raster.rs:271-285` (polygon) and `:384-397` (rotated ellipse) run scalar `f64` min/max/multiply/convert over all sub-row spans for **every** pixel in `[ix_min, ix_max]`. The replay also uses this path for Ellipse and Circle at scale > 1 (`model.rs:262-292`).
- **Fix:**
  - Emit the interior run `[max ceil(l_s), min floor(r_s))` at full coverage directly; evaluate per-pixel coverage only in the two edge bands.
  - Expected to cut most of the 51% of time these two shapes take (estimate).

### PERF-6: Per-row stack arrays are zeroed

- **Severity / status:** Low. Verified. `next: no`.
- **Where:** `raster.rs:202` `[(f64, usize); 64]` and a `[_; 32]` array around `:248`. That is about 1.8 KB of memset per polygon row for at most 16 hits.
- **Fix:** size them to `4 * order`, or use a scratch `Vec` in `WorkerCtx`. The unused `scratch_vertices` field suggests this was the intent.

### PERF-8: Score the random phase at reduced resolution

- **Severity / status:** Medium–High. Estimated. `next: no` (unless "next" keeps candidate search).
- **Idea:**
  - Evaluate the 16k random candidates on a 2× downsampled target and canvas (4× fewer pixels), then hill-climb the best k at full resolution.
  - It targets the 75–80% of evaluations that are independent.
  - It changes search behaviour, so it needs PERF-0's quality metrics to accept.

### PERF-10: Copies outside the hot path

- **Severity / status:** Low. Verified. `next: partial` (the render/export layer is reused).
- **Where:**
  - `buffer.rs:38-40` fills pixel by pixel (use `repeat`);
  - `buffer.rs:111` and `:123` clone full buffers (take the image by value, `into_raw`);
  - `export.rs:65` converts `to_image()` only to encode;
  - `export.rs:173-179` copies for the average background;
  - `error_grid.rs:102-109` accumulates `f64` per pixel with bounds checks.
- **Fix:** together with RT-5.

### PERF-11: Batch throughput

- **Severity / status:** Medium. Measured indirectly (3.9× scaling on 8 cores). `next: yes`.
- **Detail:** for the build-time use case (many placeholders), throughput across images matters more than the latency of one image. One image per core, single-threaded, beats one image at a time across all cores.
- **Fix:** document it; optionally add a batch mode to the CLI (`primeval *.jpg --out-dir ...`) that runs images in parallel with `workers: 1`. Note that adding CLI behaviour requires a deliberate decision under `AGENTS.md`.

---

## 7. API and engineering quality (API)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |
| API-6 | Low | Verified | partial | `ShapeKind` keeps parallel name tables (`shapes.rs:194-254`: `variants()`, `FromStr`, display). | One `const` table. |
| API-7 | Low | Measured | partial | 186 public items lack docs (`-W missing_docs`). Some docs are wrong: `difference_full_raw` claims a normalised RMS but returns a raw `u64`; `raster.rs:8` and `:172` mention a "tiny-skia pipeline" that does not exist; `raster.rs:177` says "non-zero winding" while the code uses even-odd. | `#![warn(missing_docs)]` on the public surface and fix the wrong docs. Broken links, module docs and the rustdoc gate landed in T1. |
| API-8 | Medium | Measured | yes | Not publishable: `cargo publish --dry-run` warns "manifest has no description" for core and **fails** for render (path dependency without `version`). `rust-version`, `readme`, `keywords`, `categories` and `documentation` are missing. `binding` lacks `publish = false`. Crate versions (0.1.0) are not aligned with npm. | Decide whether the crates are public. If yes, add the metadata, versioned path deps and version alignment (REL-6); if not, `publish = false` everywhere. |

---

## 8. Tests (TEST)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |
| TEST-4 | Medium | Verified | partial | Engine tests (112) have gaps: some are weak or circular (`worker.rs:521-548` asserts nothing; a "keeps radius equal" test only checks r ≥ 1; the replay test in `model.rs` is circular; a score test only checks > 0). Missing: tiny images, per-shape score parity, NEON vs scalar parity, seed determinism, PNG vs SVG geometry. | Add `proptest`: rasterizer invariants (in bounds, `x1 ≤ x2`, alpha ≤ 0xFFFF, no duplicate pixels, odd and tiny sizes), fused energy = full recomputation after drawing, the blend bound, `clamp_line` vs `crop_scanlines`, hex colour round-trip, error-grid samples in bounds. |

---

## 9. Tooling, CI and supply chain (TOOL)

All TOOL items landed in T1. Follow-ups:

- `napi-prebuilds.yml` calls the quality workflow with `uses: ./...` plus `# zizmor: ignore[self-repository]`. Switch to the `$/...` syntax and drop the ignore once actionlint accepts it (1.7.12 does not).
- Dependabot does not bump the actionlint `docker://` digest in the hygiene job; update it by hand.
- Two things only CI can confirm, on the first pull request run: `rustup toolchain install` (no arguments) installs the toolchain and components from `rust-toolchain.toml`, and the pinned actionlint image works as a `docker://` step.

---

## 10. Documentation and repository content (DOC)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |
| DOC-2 | Medium | Verified | yes | Missing operational documentation: minimum glibc, CPU baseline, memory sizing (per-format peaks), concurrency guidance for servers, untrusted-input guidance, the limits introduced by RT-5. | A "Deploying" section in the README. |
| DOC-3 | Low | Verified | yes | The README examples read `docs/readme/originals/monalisa.jpg`, which does not exist for npm consumers. | Use `photo.jpg` with a note, or `process.argv[2]`. |
| DOC-4 | Low | Verified | yes | The Benchmarks section cannot be reproduced (the script was removed in `e24492d`). | Replace with the PERF-0 script and its output, or remove the section. |
| DOC-6 | Medium | Verified | yes | **Licensing of sample images:** `docs/readme/originals/spongebob.jpg` (Nickelodeon artwork) and `kenna-fiume-po.jpg` (a Michael Kenna photograph), plus every derived gallery image, are copyrighted works in a public MIT repository. Mona Lisa and American Gothic are public domain. | Replace them with public-domain, CC0 or the maintainer's own photographs; regenerate the gallery; this corpus is also the benchmark corpus (PERF-0). |
| DOC-7 | Low | Verified | yes | Package and repository metadata: `package.json` has a weak description ("TypeScript-first Node package...") and lacks `keywords`, `homepage`, `bugs` and `author`; there are no README badges (CI, licence); the GitHub repository has no topics. | Fill them in. |

---

## 11. Removals (RM)

The project has never been published, so every removal is free.

| ID | `next` | Remove | Why | What it simplifies |
| --- | --- | --- | --- | --- |
| RM-4 | yes (contract landed in T2; kernels are PERF-4) | **Alpha channel in the engine**: work in RGB, composite transparent inputs onto the background at decode time, accept only opaque backgrounds (`RGB` / `RRGGBB`) | ENG-16 inconsistency; about 25% of per-pixel work (PERF-4); simpler kernels; transparent output has little value for this product. PERF-0 measures the gain when PERF-4 lands. | 3-byte buffers, RGB-only NEON (`vld3_u8`), one background rule across SVG and PNG. |

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

- [ ] REL-7 Decide on musl
- [ ] API-8 Crate publishability decision

### T5: Engine correctness

Required in any case, because the current engine becomes the reference and baseline for "next".

- [ ] ENG-6, ENG-10
- [ ] TEST-4 Property tests for rasterizers and scoring

### T6: Performance, gated by benchmarks

- [ ] PERF-5 Anti-aliased rasterizer interiors (largest measured hotspot)
- [ ] PERF-1 Prefix sums + early exit
- [ ] PERF-4 RGB-only kernels (RM-4)
- [ ] PERF-8 Reduced-resolution random phase (needs quality metrics)
- [ ] PERF-6, PERF-11
- [ ] Deferred until the "next" decision: PERF-2 runtime-dispatched x86 SIMD, PERF-3 NEON accumulator tuning (section 15)

### T7: Documentation

Continuous: each ticket updates the README for the behaviour it changes. This ticket is the final pass.

- [ ] DOC-2 to DOC-4, DOC-6, DOC-7, API-7; regenerate the README and gallery images, which predate the T2–T5 engine and output changes

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
