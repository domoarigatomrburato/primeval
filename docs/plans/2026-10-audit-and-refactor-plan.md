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

**Top findings**

| # | ID | Finding | Severity | Status |
| --- | --- | --- | --- | --- |
| 1 | REL-1 | `target-cpu=native` leaks into release prebuilds. The Windows x64 DLL contains AVX-512 and the Linux arm64 addon contains SVE, so both crash with an illegal-instruction signal on common CPUs. On ARM the flag gives zero speedup. | Critical | Reproduced, measured |
| 2 | RT-1 | `panic = "abort"` turns every Rust panic into the death of the host Node process. Switching to `unwind` costs nothing measurable. | Critical | Reproduced, measured |
| 3 | RT-2, RT-3, RT-4 | Panics reachable from ordinary input: a 1-pixel working side (e.g. a 2000×5 banner), `background: "a€bc"`, a huge `outputSize`, GIF output wider than 65535 px. | Critical | Reproduced |
| 4 | REL-2 | Linux prebuilds require glibc ≥ 2.34 (no Debian 11, Ubuntu 20.04, Amazon Linux 2, RHEL 8). | High | Verified |
| 5 | NODE-1, NODE-2 | Some errors are thrown synchronously and skip the error-class mapping; a throwing `onProgress` crashes the process. | High | Reproduced |
| 6 | RT-5 | Memory is unbounded: a 258 KB PNG (9000×9000) peaks at 951 MB RSS; GIF at 2048 px peaks at 585 MB. | High | Reproduced, measured |
| 7 | ENG-1 | The NEON code inside safe public functions can read out of bounds. Their `// SAFETY:` comments (added in T1) say the invariant is assumed, not checked. | High | Verified |
| 8 | ENG-2, ENG-3, ENG-4 | Quadratic strokes paint pixels twice (15.4% of shapes). PNG/JPG/GIF geometry differs from the SVG. "Deterministic" seeds depend on the CPU core count. | Medium | Reproduced, verified |
| 9 | PERF-* | No benchmarks exist. Polygon and rotated-ellipse take 51% of the total time and are rasterization-bound (10–32 ns/pixel versus 2–4 for rectangles). | Medium | Measured |

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

### REL-1: `target-cpu=native` is compiled into the published binaries

- **Severity / status:** Critical. Reproduced (disassembly), measured. `next: yes`.
- **Where:**
  - `.cargo/config.toml:2` sets `rustflags = ["-C", "target-cpu=native"]` for every build.
  - `.github/workflows/napi-prebuilds.yml` builds with `npx napi build ... --release --target <triple>` and sets no `RUSTFLAGS`.
  - `@napi-rs/cli` only sets `RUSTFLAGS` for musl targets or `--strip`, so the config value applies.
- **Why it matters:** the binaries are compiled for the CPU of whichever runner built them. The quality CI runs on the same runner class, so it can never catch the problem.
- **Evidence:** v0.1.1 artifacts, `objdump -d --no-show-raw-insn`.

  | Artifact | AVX-512: `zmm` / k-mask / `vpternlog` / `vmovdqu8/16/32/64` | AVX/AVX2 (`ymm`) | Notes |
  | --- | --- | --- | --- |
  | `win32-x64-msvc` | 582 / 577 / 132 / 1084 | 59,609 | Crashes on any CPU without AVX-512 (most Intel Core 12th–14th gen, AMD Zen 1–3). |
  | `linux-x64-gnu` | 0 | 30,706 | AVX2 everywhere; crashes on pre-Haswell CPUs, some Atom/Celeron parts and VMs with masked CPUID. |
  | `darwin-x64` | 0 | 749 | Cross-compiled on an arm64 runner, so `native` was ignored. The 749 come from runtime-dispatched code in dependencies. This is the portable baseline. |
  | `linux-arm64-gnu` | n/a | n/a | About 843 SVE instructions (`z` registers, `ptrue`, `whilelo`), 224 LSE atomics, 88 `ldapr`. Crashes on Graviton2, Ampere Altra, Raspberry Pi and Docker on Apple Silicon. |

- **Benefit measured on the M3:** none. Over 9 shapes at 200 steps, the portable build took 26.8 s and the native build 27.6 s, with identical output hashes. On aarch64 NEON is baseline and already hand-written. On x86 the hot loops are scalar and bounds-checked (PERF-2), so the gain would be small there too.
- **Fix:**
  1. Delete `.cargo/config.toml`, or replace it with nothing global. Document local opt-in instead: `RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling`.
  2. If a raised baseline is ever wanted, set an explicit and documented one (e.g. `x86-64-v2`) per target, never `native`.
  3. Add a CI gate on release artifacts that fails if `objdump` finds `zmm` or SVE registers.
  4. Optionally run each artifact under `qemu-x86_64 -cpu Nehalem` or `qemu-aarch64 -cpu cortex-a72`.

### REL-2: Linux prebuilds require glibc 2.34

- **Severity / status:** High. Verified (`objdump -T` shows a maximum of `GLIBC_2.34`). `next: yes`.
- **Impact:** the addon fails to load on Debian 11, Ubuntu 20.04, Amazon Linux 2, RHEL/Alma/Rocky 8 and `node:*-bullseye` images. The README does not state a minimum.
- **Fix:**
  - Build Linux targets with `napi build --use-napi-cross`, or with a zig/cross toolchain targeting glibc 2.17.
  - Add the same ISA/symbol gate as REL-1, with a maximum `GLIBC_` version.
  - Document the minimum.

### REL-5: Prebuilds are never executed on their own platform

- **Severity / status:** High. Verified. `next: yes`.
- **Where:**
  - `quality.yml` runs only on `ubuntu-latest` x64.
  - The release `build` job compiles and uploads, but never loads the `.node` file.
  - As a consequence the aarch64 NEON code paths (`score.rs`, `#[cfg(target_arch = "aarch64")]`) are never tested in CI. The scalar fallbacks are compiled out on aarch64.
- **Fix:**
  - After each target build, run a smoke test on the target's native runner: load the addon, render a tiny fixture to SVG and PNG, assert non-empty output and exit code 0. Run `x86_64-apple-darwin` under Rosetta on `macos-14`.
  - Add a `macos-14` (arm64) leg to the Rust test job so NEON is tested.

### REL-6: Release process gaps

- **Severity / status:** Medium. Verified. `next: yes`.
- **Gaps:**
  - No CHANGELOG and no GitHub Release creation.
  - `npm run bump:version` updates `package.json` and the lockfile but not the Cargo crate versions. They are 0.1.0 while npm is 0.1.1.
  - Publishing is not idempotent. Platform packages are published before the root package, so a partial failure leaves a state a re-run cannot fix.
  - The `v0.1.1` tag exists and its `publish` job is green, yet no `@aleburato/*` package exists on the registry. The maintainer confirms nothing was ever published. The run logs have expired (HTTP 410), so the cause cannot be determined now.
- **Fix:**
  - Generate release notes (`gh release create --generate-notes`, or a CHANGELOG checked by CI).
  - Have `bump:version` also bump the Cargo versions and validate them.
  - Make publish re-runnable (skip versions already published).
  - Add a post-publish verification step (`npm view <pkg>@<version>`) so a green job proves a published package.
  - Delete the `v0.1.1` tag and restart versioning at the first real release.

### REL-7: No musl targets

- **Severity / status:** Low (product decision). `next: yes`.
- **Fix:** Alpine is common in containers. Consider `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` once REL-1, REL-2 and REL-5 are in place. The loader already detects musl.

---

## 3. Runtime robustness (RT)

### RT-1: `panic = "abort"` kills the host process

- **Severity / status:** Critical. Reproduced, measured. `next: yes`.
- **Where:** `Cargo.toml` `[profile.release] panic = "abort"`, inherited by `napi build --release`.
- **Evidence:** every panic in RT-2, RT-3 and RT-4 ends the Node process with exit code 134, even with a `process.on("uncaughtException")` handler installed.
- **Cost of switching to `unwind`:** 5 interleaved runs of 9 shapes × 100 steps.

  | Run order | abort | unwind |
  | --- | --- | --- |
  | abort first | 14.56 s | 15.29 s |
  | abort first | 15.59 s | 15.85 s |
  | abort first | 16.48 s | 17.69 s |
  | unwind first | 17.16 s | 16.21 s |
  | unwind first | 17.24 s | 16.88 s |

  Whichever build runs first wins (thermal drift), so the difference is noise. The binary grows by 10% (1,038,448 to 1,143,984 bytes).
- **Fix:**
  1. Use a binding profile with `panic = "unwind"` (`[profile.release-napi] inherits = "release"`, `panic = "unwind"`, `napi build --profile release-napi`), or switch the release profile itself.
  2. Wrap the native entry point in `std::panic::catch_unwind` and map a panic to an internal-error rejection. rayon propagates worker panics to the caller, so this catches them.
  3. Keep the root-cause fixes in RT-2, RT-3 and RT-4. `unwind` is defense in depth, not a substitute.

### RT-2: A 1-pixel working dimension panics

- **Severity / status:** Critical. Reproduced. `next: partial`: rejecting tiny working sizes at the boundary carries over; the clamp fixes are specific to this engine.
- **Where:**
  - `shapes.rs:486` / `:490` (Ellipse `clamp(1, width - 1)` / `clamp(1, height - 1)`);
  - `:536` (Circle);
  - `:654` / `:656` (RotatedRectangle);
  - `:982` / `:984` (RotatedEllipse).
  - `Ord::clamp` and `f64::clamp` panic when `min > max`.
- **Reachability:** `export::thumbnail` (`export.rs:128-139`) keeps 1×N inputs and squashes extreme aspect ratios to N×1 with `.max(1)`. A 2000×5 banner becomes 256×1. The default `any` shape samples the affected kinds.
- **Evidence:** a 1000×1 PNG with `shape: any` gives `assertion failed: min <= max`, exit 134. `triangle`, `rectangle`, `quadratic`, `rotated-ellipse` and `polygon` survive 1000×1. `export.rs:191` has a test asserting the thumbnail clamps to 1 px, which is exactly the size that then crashes.
- **Fix:**
  - Validate working dimensions ≥ 2×2 in `primeval-render`, returning a `Validation` error, or upscale tiny inputs.
  - Use `(dim - 1).max(1)` upper bounds in the mutations.
  - Add a regression test per `ShapeKind` for 1×1, 1×N, N×1 and 2×2.

### RT-3: A multi-byte background string panics

- **Severity / status:** Critical. Reproduced. `next: yes` (colour parsing is reused).
- **Where:** `color.rs:49` `parse_hex_pair(&s[0..2])` dispatches on the byte length (`s.len()`) and then slices a `str`.
- **Evidence:** `background: "a€bc"` (6 bytes) gives `byte index 2 is not a char boundary; it is inside '€'`, exit 134. The CLI `--background 'a€bc'` crashes the same way.
- **Fix:**
  - Reject non-ASCII input first (`if !s.is_ascii()`), then parse bytes.
  - Add a regression test and a fuzz target (TEST-5).

### RT-4: Output-size overflows panic

- **Severity / status:** High. Reproduced. `next: partial` (output caps carry over; GIF removal makes half of this moot).
- **Where:**
  - `buffer.rs:160` `.expect("buffer pixel byte length must not overflow usize")`: `outputSize: 4294967295` with raster output panics on a `tokio-rt-worker` thread.
  - `export.rs:105`, `:110-111` cast `width() as u16`: a 1000×10 PNG with `outputSize: 70000` resolves for PNG, but GIF hits the gif crate's frame-size assertion, exit 134.
- **Fix:**
  - Cap `output_size` in `validate_options` (e.g. ≤ 16384).
  - Use `u16::try_from` with an error, or remove GIF (RM-1).
  - Prefer `try_reserve` for large buffers.

### RT-5: Unbounded memory and work

- **Severity / status:** High. Reproduced, measured. `next: yes`.
- **Evidence:**
  - A 258,606-byte 9000×9000 PNG with `count: 1` and `resizeInput: 64` peaks at **951 MB RSS**.
  - GIF output peaks at 152 MB (200 steps at 1024 px) and 585 MB (200 steps at 2048 px).
  - A reviewer probe with 600 steps at 2048 px peaked at 765 MB.
  - A 30000×30000 PNG header bomb is rejected by `image`'s default 512 MiB allocation limit, so that part works.
- **Causes:**
  - `render::prepare` keeps the full decoded `DynamicImage` alive for the whole optimisation (`crates/primeval-render/src/lib.rs:363` onward).
  - `average_background` (`export.rs:173-174`) makes full-resolution RGBA copies.
  - `thumbnail` returns `to_rgba8()` copies (`export.rs:131`), and `Buffer::from_image` clones again (`buffer.rs:111`).
  - GIF frames are all materialised before encoding (`model.rs:224-255`, `result.push(output.clone())`). The frame step is applied only afterwards (`lib.rs:468-479`).
  - Nothing caps `count`, `resizeInput`, `outputSize` or `repeat`. The Rust-only `workers` option has no upper bound.
- **Fix:**
  - Decode with `image::ImageReader` and explicit `image::Limits` (`max_image_width`, `max_image_height`, `max_alloc`).
  - Compute the background from the thumbnail.
  - `drop` the full image right after taking the thumbnail.
  - Take `RgbaImage` by value (`into_raw`).
  - Add upper bounds in `validate_options`, document them, and test them at every layer.
  - Remove GIF (RM-1) or stream it.

### RT-6: Path input is a liability

- **Severity / status:** High. Reproduced. `next: yes`. Resolved by RM-2.
- **Where:** `crates/primeval-render/src/lib.rs:450`, `std::fs::read(path).map_err(|_| ApproximateError::NotFound(path.clone()))`.
- **Problems:**
  - It reads the whole file with no size limit (`/dev/zero` grows until out of memory; reasoned, not run).
  - A FIFO blocks forever and cannot be aborted. Probe result: still pending 2000 ms after abort, and a tokio worker is lost.
  - Every IO error becomes `NotFound`: a directory or a mode-000 file reports "does not exist or is not readable".
  - Paths cross napi as `String`, so non-UTF-8 paths cannot be expressed.
  - On a server, untrusted `kind: "path"` is a file-existence and file-read oracle.
- **Fix:** remove path input from the Node API and the Rust facade (RM-2). Node's `fs` gives precise errors and the CLI can read the file itself.

---

## 4. Node binding, TypeScript wrapper and CLI (NODE, CLI)

### NODE-1: Errors thrown synchronously and left unmapped

- **Severity / status:** High. Reproduced. `next: yes`.
- **Where:**
  - `src/index.ts:305` `const handle = nativeBinding.startApproximate(nativeRequest);` runs outside the `.catch(mapNativeError)` at `:341`.
  - `approximate()` (`:351`) is not `async`.
- **Evidence:**
  - Invalid background: a synchronous `Error "[ValidationError] invalid background color"`, `instanceof ValidationError === false`, `code: "GenericFailure"`.
  - `count: 0` (checked in TypeScript): a synchronous `ValidationError`.
  - `background: 123`: an unmapped napi conversion error that leaks internal names (`... on NativeRenderOptions.background on NativeApproximateRequest.render`).
  - A native-load failure also throws synchronously.
  - The CLI prints a full stack trace for `--background zzz`.
  - So `approximate(x).catch(...)` misses whole classes of errors, and the README promise that errors map to the typed classes is false.
- **Fix:**
  - Make `approximate` `async`, or wrap the whole body so every failure becomes a rejected promise passed through `mapNativeError`.
  - Validate `background`'s type in TypeScript.

### NODE-2: A throwing `onProgress` crashes the process

- **Severity / status:** High. Reproduced. `next: yes`.
- **Where:** `src/index.ts`; the user callback is invoked directly from the threadsafe-function callback.
- **Evidence:** `onProgress() { throw new Error("boom") }` ends the process with `Error: boom at 1`, exit 1. With an `uncaughtException` handler the render still resolves.
- **Fix:** catch inside the wrapper. On error, cancel the task and reject the promise with that error as `cause`.

### NODE-3: Numbers wrap at the napi boundary

- **Severity / status:** High. Reproduced. `next: yes`.
- **Where:**
  - `binding/src/binding.rs:83-92` take `count`, `repeat`, `resizeInput` and `outputSize` as `Option<u32>`, and `seed` as `Option<i64>`.
  - TypeScript checks only `Number.isInteger` and the lower bound.
- **Evidence:**
  - `count: 2**32 + 1` resolves with `total: 1`.
  - `count: 2**32` gives "count must be at least 1".
  - `outputSize: 2**32 + 16` renders at 16 px.
  - Seeds 1e19, 1e20 and 1e300 produce identical output (they saturate).
  - CLI `--count 4294967297` exits 0 after one shape.
  - CLI `--seed 99999999999999999999` reports "seed must be at least 0".
- **Fix:**
  - Accept `f64` (or `BigInt` for `seed`) in the binding and range-check in Rust, so one layer owns the rule.
  - Restrict `seed` to `Number.MAX_SAFE_INTEGER` or accept `bigint`.

### NODE-4: Abort is not guaranteed

- **Severity / status:** Medium–High. Reproduced. `next: yes`.
- **Evidence:**
  - With the main thread busy for 200 ms after `controller.abort()`, 40 of 40 renders **resolved** instead of rejecting. The wrapper never re-checks `signal.aborted` when the native promise settles.
  - An already-aborted signal still starts native work: decoding happens before the first flag check.
  - Cancellation latency is one optimization step (a reviewer measured 640 ms for `polygon` at `resizeInput: 1024`).
  - Reading and decoding cannot be cancelled (FIFO, RT-6).
- **Fix:**
  - Reject immediately when `signal.aborted`.
  - On settle, reject with `AbortError` (`cause: signal.reason`) if the signal fired.
  - Check the flag before and after decoding and before encoding.
  - Remove path input (RM-2).

### NODE-5: CPU-bound work runs on tokio worker threads

- **Severity / status:** Medium. Verified, and probed by a reviewer. `next: yes`.
- **Where:** `binding/src/binding.rs:141` `env.spawn_future_with_callback(async move { ... approximate(...) ... })`. The future never awaits; panics show thread `tokio-rt-worker`.
- **Evidence:** 16 concurrent renders on 8 cores. The first 8 report progress at 91–166 ms and the rest only at 1215–1848 ms; the event loop p99 is 19.5 ms; peak RSS is 109 MB. The CPU is not oversubscribed (rayon's pool is shared), but queued work is invisible and anything else on the runtime is starved.
- **Fix:**
  - Use `tokio::task::spawn_blocking` (or a dedicated pool) with an optional documented concurrency limit.
  - Do not use napi `AsyncTask`, which runs on the libuv pool and would block `fs`/`crypto`.

### NODE-6: Error objects carry little information

- **Severity / status:** Medium. Reproduced. `next: yes`.
- **Where:** `mapNativeError` (`src/index.ts:248-266`) parses a `[Name] message` prefix and builds new errors.
- **Evidence:**
  - No `code` and no `cause`; `NotFoundError` has `keys: ['name']`, and the stack starts at `mapNativeError`.
  - A message containing a newline (e.g. a path `a\nb.png`) is not matched and leaks as a plain `Error`.
  - `PrimevalError` (`:76`) is not exported.
- **Fix:**
  - Throw napi errors with a status/code (e.g. `Error::new("ValidationError", msg)`, readable as `err.code`) and map on `code`.
  - Always set `cause`.
  - Export the base class and add an `InternalError` for panics and encoder failures.

### NODE-7: Vocabulary drift between layers

- **Severity / status:** Medium. Reproduced. `next: yes`.
- **Evidence:**
  - Rust `OutputFormat::from_str` and the CLI accept `jpeg`, but the API rejects `output: "jpeg"`.
  - Rust `parse_alpha_str` accepts `"auto"`, but CLI `--alpha auto` gives "alpha must be an integer".
  - GIF input decodes (`primeval-render` enables `image/gif`) although the README says JPEG/PNG only.
  - The same rule is spelled three ways: "resize_input must be at least 1" (Rust), "resizeInput ..." (TypeScript) and "resize-input ..." (CLI).
  - The seed message says "positive integer" although 0 is accepted.
  - `shape: null` is accepted at runtime although the type forbids it.
- **Fix:**
  - Pick one vocabulary in Rust and make every layer consume it.
  - Report errors with the public (camelCase) field names, mapped in exactly one place.
  - Most of the drift disappears with RM-1, RM-5 and RM-7.

### NODE-8: Typings and packaging details

- **Severity / status:** Low. Verified. `next: yes`.
- **Items:**
  - No JSDoc in `dist/index.d.ts`; defaults and meanings live only in the README.
  - No per-output overloads, so callers must narrow `result.format` to get `data: string`.
  - `background?: "auto" | string` collapses to `string`; `"auto" | (string & {})` keeps autocomplete.
  - `Uint8Array` input is copied twice (`Buffer.from` at `src/index.ts:171`, then into a `Vec`).
  - `ArrayBuffer` input is rejected with the misleading "bytes input requires data".
  - The hand-written native types in `src/native-binding.ts` duplicate the generated `binding.d.ts`, which is neither used nor shipped, with no drift check.
  - `dist/cli.d.ts` and `dist/native-binding.d.ts` ship as noise.
  - No source maps or declaration maps.
  - The exports map is correct (`types` before `import`).

### NODE-9: The cancellation registry is global

- **Severity / status:** Low. Verified. `next: yes`.
- **Where:** `binding.rs:15-16`, a global `Mutex<HashMap<u32, Arc<AtomicBool>>>` keyed by a wrapping `AtomicU32`.
- **Fix:** a `#[napi]` class holding `Arc<AtomicBool>` with a `cancel()` method. Cleanup is currently correct and tested, so this is simplification, not a bug fix.

### NODE-10: Loader detail

- **Severity / status:** Low. Reported, speculative. `next: yes`.
- **Detail:** on Linux the loader calls `process.report.getReport()` without first setting `process.report.excludeNetwork = true`, which the napi-rs template sets for speed. Otherwise the diagnostics are good: musl detection, combined load errors, install guidance, sandboxed unit tests.

### CLI-1: Usage text goes to stdout on errors

- **Severity / status:** Medium. Reproduced. `next: yes`.
- **Where:** `src/cli.ts:187-188` prints "missing input path" to stderr, then `printUsage()` to stdout.
- **Evidence:** `primeval -o - > out.svg` writes 16 lines of help text into `out.svg`.
- **Fix:** print usage to stderr on errors; only `--help` prints to stdout.

### CLI-2: Inconsistent output handling

- **Severity / status:** Medium. Reproduced. `next: yes`.
- **Evidence:**
  - Explicit `--output` silently overwrites (`keep.svg` was replaced), while a derived output path refuses to overwrite (`cli.ts:213`).
  - The derived-path check is `existsSync` followed by `writeFileSync` (`:283-285`): racy, and it follows dangling symlinks.
  - `-o mismatch.png --format svg` writes SVG into a `.png`.
  - `-o <directory>` runs the full render and then dies with an `EISDIR` stack trace.
  - `--output ""` is treated as omitted.
- **Fix:**
  - Decide one overwrite policy (refuse unless `--force`).
  - Write with `{ flag: "wx" }`.
  - Validate the output destination before rendering.
  - Infer the format from the extension only (RM-7).

### CLI-3: Error and exit-code conventions

- **Severity / status:** Low. Reproduced. `next: yes`.
- **Items:**
  - User errors print stack traces (`--background zzz`, `EISDIR`).
  - Usage errors exit 1; 2 is conventional.
  - The `AbortError` branch (`cli.ts:290-292`) is unreachable because the CLI never passes a signal.
  - SIGINT does not abort the render.
- **Fix:**
  - Print `message` only for `PrimevalError`s, with exit code 2 for usage and 1 for runtime errors.
  - Wire SIGINT to an `AbortController` and exit 130.

### CLI-4: Missing conveniences

- **Severity / status:** Low. Reproduced. `next: yes`.
- **Items:**
  - No stdin input (`-` gives "- does not exist or is not readable").
  - Raster output to stdout is refused although `cli.ts:275-276` already handles buffers.
  - No `-v`.
  - Progress lines lack the step total and don't update a single TTY line.
  - `--help` shows no defaults.
- **Already good:** strict `util.parseArgs`, clear unknown-option errors, EPIPE handled (`| head` exits 0).

---

## 5. Engine correctness (ENG)

### ENG-1: Out-of-bounds reads reachable from safe public functions on aarch64

- **Severity / status:** High (soundness). Verified. `next: no`.
- **Where:** `score.rs:422` `pub fn compute_color` and the public `energy_from_lines_raw` call `unsafe { neon::... }`.
  - The NEON code loads with `vld4_u8(c_pix.as_ptr().add(byte_index))` (around `score.rs:274-275` and `:357-358`).
  - The offsets are clipped against `target`'s dimensions only, and `current` is never checked.
  - `difference_full_raw` does assert equal dimensions.
  - Since T1 every `unsafe` block has a `// SAFETY:` comment; the two at these call sites state that `current`'s dimensions are assumed, not checked.
  - The comments rely on the `Buffer` invariant `pixels.len() == width * height * 4`, which `Buffer::from_image` checks only with `debug_assert!`.
- **Fix:**
  - `assert_eq!` dimensions at entry, or make these functions `pub(crate)` (API-1).
  - Make the `Buffer` length invariant a hard check in `from_image`.
  - Add a NEON-vs-scalar parity test that runs in CI on arm64 (REL-5).

### ENG-2: Quadratic strokes paint pixels twice

- **Severity / status:** Medium. Reproduced in Rust (Appendix A.6). `next: no`.
- **Where:** `raster.rs`, quadratic stroke. The curve is subdivided into segments (around `:90-91`), and each segment's columns are emitted as a closed range (around `:136-140`), so neighbouring segments share integer columns.
- **Evidence:**

  | Shape (5000 random shapes, 256×256) | Shapes with a duplicated pixel | Duplicated / total pixels |
  | --- | --- | --- |
  | quadratic | 15.4% | 0.413% |
  | triangle, rectangle, ellipse, circle, rotated-rectangle, rotated-ellipse, polygon | 0% | 0% |

- **Impact:**
  - Energy assumes each pixel is blended once, but `draw_lines` blends duplicates twice. The score stored by `Model::add` (`model.rs:177-191`) drifts away from `difference_full_raw`.
  - This affects reported scores and frame selection, and `compute_color` double-weights those pixels.
  - In debug builds `total -=` can transiently underflow.
  - The test `add_score_matches_full_recomputation` covers only Rectangle.
- **Fix:**
  - Use half-open column ranges per segment and dedupe at joins, or accumulate coverage per row.
  - Property test "no pixel emitted twice" for every rasterizer.
  - Run the add-score parity test for every `ShapeKind`.

### ENG-3: Raster output geometry does not match the SVG

- **Severity / status:** Medium. Verified. `next: partial`: the rule "one coordinate convention, plus a PNG-vs-SVG test" carries over.
- **Where:**
  - The SVG wraps every shape in `scale(s) translate(0.5 0.5)` (`model.rs:313-316`).
  - The raster replay (`model.rs:258-296`) offsets Ellipse and Circle by `(v + 0.5) * scale`, but every other kind goes through `shape.scaled(scale).rasterize()`.
  - `scaled()` uses `scale_i32 = (v * scale).round()` (`shapes.rs:1111-1113`), with no pixel-centre offset and with the inclusive `x2` scaled as a point.
- **Impact** (reasoned at the default scale 4): a working-resolution rectangle covering columns 1..=3 should be 12 output pixels.
  - The replay paints 4..=12, i.e. 9 pixels; the SVG covers [6, 18).
  - Rectangles that touch at working resolution leave 3-pixel gaps in PNG/JPG/GIF.
  - Triangles and rotated rectangles sit about 1.5 px off from circles in the same `any` image.
  - The existing replay test (`model.rs:347`) is circular: it compares against the same `scaled().rasterize()`.
- **Fix:**
  - Pick one convention: continuous coordinates, rectangles covering `[x1·s, (x2+1)·s)`, integer vertices mapped to `(v + 0.5)·s`.
  - Add a test that rasterizes the SVG semantics and compares with the PNG.

### ENG-4: Seeded output depends on the core count

- **Severity / status:** Medium. Reproduced. `next: partial`: the design principle carries over; this implementation does not.
- **Where:**
  - `model.rs:87` seeds each worker with `create_rng(seed + index)`.
  - `model.rs:130` runs `worker_rounds = 16.div_ceil(worker_count)` rounds per worker.
  - Render defaults `workers` to `available_parallelism()`.
- **Evidence:** seed 42, `any`, 50 steps: workers 1, 2, 4, 8 and 12 produce 5 different SVGs. The same configuration repeated gives identical output.
- **Impact:**
  - The README promises "`--seed <N>` for deterministic output".
  - The total search effort also varies: `W · ceil(16/W)` rounds, e.g. 24 with 12 workers.
  - `seed + index` overflows in debug builds near `u64::MAX`.
  - The streams overlap: worker 1 with seed s equals worker 0 with seed s+1.
  - Bit-identical results across platforms are not guaranteed (`sin_cos`, `acos` and `rand_distr` use the platform libm); this part is speculative.
- **Fix:**
  - Exactly 16 rounds as independent tasks, each with an RNG derived from `(seed, step, round)`, e.g. ChaCha `set_stream`. The output is then independent of thread count, and the load balances better (PERF-9).
  - Document the guarantee as "same seed, same version, same platform".

### ENG-5 to ENG-16: Smaller correctness items

| ID | Severity | Status | `next` | Where | Problem | Fix |
| --- | --- | --- | --- | --- | --- | --- |
| ENG-5 | Low–Med | Verified | no | `score.rs:53` | `0x101 * 255 / alpha` divides by zero for `alpha == 0` through the public `Model::add(shape, 0)`; `alpha as u8` (around `:87`) truncates values above 255. | Typed alpha (API-4); restrict visibility. |
| ENG-6 | Low | Verified | no | `score.rs:58-76` | `compute_color` ignores `line.alpha` (coverage), so anti-aliased edges and quadratic pixels are fitted as fully covered and the colour comes out under-saturated. This matches Go; the quality impact is unmeasured. | Weighted least squares: `s* = Σw(t−(1−w)c) / Σw²` with `w = (alpha/255)·(ma/65535)`. |
| ENG-7 | Low | Verified | no | `scanline.rs:57-66` | `clamp_line` turns lines entirely outside the image into one-pixel lines at the border; `w = 0` panics. Unreachable from render (callers clip first), reachable from the public API. | Return `None` when `x2 < 0 \|\| x1 >= w`. |
| ENG-8 | Low | Verified | no | `shapes.rs:600-606` | RotatedRectangle starts `rect_max` at 0, so rows whose edges are all at negative x emit a stray (0..0) pixel. Inherited from Go. | Start at `i32::MIN` / `i32::MAX`. |
| ENG-9 | Low | Verified | no | `shapes.rs:983-984` | RotatedEllipse clamps `ry` to `width - 1`; Ellipse uses the height. | Clamp to `height - 1`. |
| ENG-10 | Low | Reported | no | `error_grid.rs:80-97`, `:141-144`, `:172-173` | Biased sampling only covers `cell_w × cell_h` per cell, but the last row/column absorbs the remainder, which is then reached only by the 20% uniform samples. | Sample within each cell's real bounds. |
| ENG-11 | Low | Verified | no | `model.rs:67` | A zero dimension gives a NaN or infinite aspect ratio, `random_range(0..0)` panics, and the score becomes NaN. Public API only. | Validate in the constructor. |
| ENG-12 | Low | Verified | no | `model.rs:157-171` | The `repeat` loop takes `before` from an energy cached against the previous canvas, which happens to equal the new score. Fragile. | Removed with RM-3. |
| ENG-13 | Low | Verified | no | `shapes.rs:100-101`, `raster.rs:244`, `score.rs:129`, `state.rs:16` | Public fields with unchecked invariants: `Polygon.order > 4` or `0` panics; `partial_cmp().unwrap()` panics on NaN vertices; `Scanline.alpha > 0xFFFF` overflows `M - sa*ma/M`; `State.cached_energy` is public and mutable. | Restrict visibility (API-1); use `total_cmp`. |
| ENG-14 | Info | Verified | no | `score.rs` blend | The blend arithmetic is exactly at the overflow bound (`v < M(M+1)`, `div_by_m` exact). Full anti-aliased coverage sums to 65532, not 65535, so "fully covered" pixels are never exactly opaque. | Document the bound, add a `debug_assert`, normalise coverage. |
| ENG-15 | Nit | Verified | partial | `export.rs:135-137`, `error_grid.rs:37` | `max_size * height / width` and `(cols * rows) as usize` are computed in `u32`. Unreachable with decode limits. | Compute in `u64`. |
| ENG-16 | Low | Verified | partial | `model.rs:305-312` | The SVG background ignores the background's alpha while the PNG keeps it. | Resolved by RM-4 (opaque backgrounds only). |

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

### PERF-0: Benchmark and quality infrastructure

- **Severity / status:** High (enabling). Verified absent. `next: yes`.
- **Problem:** there is no `benches/`, Criterion or Divan. The README's comparison with Go comes from a script that was removed (`e24492d`), so it cannot be reproduced.
- **Fix:**
  - Add Divan (or Criterion) benches: rasterizer per shape, `compute_color`, `energy_from_lines_raw`, `difference_full_raw`, a seeded `Model::step` per shape with one worker, `render_output` at 1024, and encoders.
  - Add an end-to-end script that records **time and quality** (RMSE, and SSIM later) at fixed step counts on a licence-clean corpus (DOC-6), with output as a stable table for diffing.
  - Run it on arm64 and x86. The harness in Appendix A is the starting point.

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

### PERF-7: Quadratic emits one scanline per pixel

- **Severity / status:** Medium. Measured (44 lines for 44 pixels per candidate, 32 ns/pixel). `next: no`.
- **Where:** `raster.rs`, quadratic band test (around `:134`, `:143-153`). It scans at least 5 rows when about 1.5 have coverage, and per-line overhead (`clamp_line`, offset computation) dominates. The NEON 8-pixel path is never used.
- **Fix:** compute the exact row range analytically, merge horizontal runs, and fix ENG-2 in the same change.

### PERF-8: Score the random phase at reduced resolution

- **Severity / status:** Medium–High. Estimated. `next: no` (unless "next" keeps candidate search).
- **Idea:**
  - Evaluate the 16k random candidates on a 2× downsampled target and canvas (4× fewer pixels), then hill-climb the best k at full resolution.
  - It targets the 75–80% of evaluations that are independent.
  - It changes search behaviour, so it needs PERF-0's quality metrics to accept.

### PERF-9: Task structure and P/E-core imbalance

- **Severity / status:** Medium. Measured (scaling numbers above). `next: partial`: the principle of task-based search decoupled from the thread count carries over.
- **Where:** `model.rs:129-144` runs one rayon task per worker, each doing `ceil(16/W)` rounds, with a barrier per step. Performance cores wait for efficiency cores, and variable hill-climb lengths leave threads idle.
- **Fix:** `par_iter` over a fixed `0..16` with `map_init` for a per-thread `WorkerCtx` and per-round RNGs (ENG-4). Work stealing balances P and E cores and makes output independent of the thread count. Optionally accept a caller-provided `ThreadPool`.

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
| API-1 | Medium | Verified | partial | `crates/primeval-core/src/lib.rs:7-20` makes every module public. That exposes `WorkerCtx` (with public `lines`, `rng`, `rect_*`, `scratch_vertices`), `SearchRound`, `State`, `hill_climb`, `raster::*`, profiling hooks (`worker.rs:93-139`), and mutable `Model.target` / `Model.current` while `score` is private. Because everything is public, the workspace's `unreachable_pub` / `dead_code = "deny"` lints cannot find dead items. | Expose a small surface (shape types, `Color`, `ShapeKind`, a `Model` facade or an engine trait, the committed-shape IR); make the rest `pub(crate)`; `#[non_exhaustive]` on public enums and option structs. |
| API-2 | Medium | Verified | yes | Errors are stringly typed: `Model::step` returns `Result<u64, String>` (`model.rs:114`, and that error cannot happen); `FromStr` uses `Err = String` (`shapes.rs:238`, `export.rs:50`); encoders return `Box<dyn Error>`, which is not `Send + Sync` (`export.rs:64`, `:78`, `:98`); render's `ApproximateError` (`lib.rs:144-148`) has no `source()` and turns image errors into strings. | One typed error per crate with `std::error::Error` + `source`, `#[non_exhaustive]`, keeping `io::ErrorKind`. |
| API-3 | Medium | Verified | **yes (key)** | Output concerns live in the engine: `export.rs` (PNG/JPEG/GIF encoders, `thumbnail`, `average_background`, CLI file naming in `output_paths`) is in core, and `Model` mixes search with `output_size`, `scale`, `svg()`, `render_output()` and `frames()`. | **Define the engine boundary:** the engine takes a target buffer plus options and produces an ordered list of committed shapes (shape, colour, alpha) with canvas metadata. Decode, resize, SVG/PNG writing and replay live in `primeval-render`. This is what lets a future engine replace the current one without touching render, binding, TS or CLI (section 15). Core then drops `image` and `gif`. |
| API-4 | Low | Verified | yes | Alpha is a magic number: `alpha: i32` with 0 meaning auto (`model.rs:114`, `state.rs:14-15`), sent as a number from TypeScript, stringified (`src/index.ts` around `:290`) and re-parsed in Rust. | An `Alpha::{Auto, Fixed(u8)}` enum end to end; `alpha?: "auto" \| number` in TypeScript. |
| API-5 | Low | Verified | no | Dead or vestigial code (see RM-9). | Delete. |
| API-6 | Low | Verified | partial | `ShapeKind` keeps parallel name tables (`shapes.rs:194-254`: `variants()`, `FromStr`, display). | One `const` table. |
| API-7 | Low | Measured | partial | 186 public items lack docs (`-W missing_docs`). Some docs are wrong: `difference_full_raw` claims a normalised RMS but returns a raw `u64`; `raster.rs:8` and `:172` mention a "tiny-skia pipeline" that does not exist; `raster.rs:177` says "non-zero winding" while the code uses even-odd. | `#![warn(missing_docs)]` on the public surface and fix the wrong docs. Broken links, module docs and the rustdoc gate landed in T1. |
| API-8 | Medium | Measured | yes | Not publishable: `cargo publish --dry-run` warns "manifest has no description" for core and **fails** for render (path dependency without `version`). `rust-version`, `readme`, `keywords`, `categories` and `documentation` are missing. `binding` lacks `publish = false`. Crate versions (0.1.0) are not aligned with npm. | Decide whether the crates are public. If yes, add the metadata, versioned path deps and version alignment (REL-6); if not, `publish = false` everywhere. |
| API-9 | Medium | Verified | yes | Render facade ergonomics: `RenderOptions` and `ApproximateError` are not `#[non_exhaustive]`; `ApproximateResult::Raster { format }` can hold `Svg`; `approximate(req, Option<&dyn Fn(ProgressInfo)>, &AtomicBool)` is positional and takes `Fn` rather than `FnMut`, forcing `Arc<Mutex<_>>` in callers; `ShapeKind` and `Color` appear in the API but are not re-exported (the binding imports `primeval_core::shapes::ShapeKind`); the binding helper parsers (`parse_alpha_str`, `parse_alpha_u32`, `parse_background_str`, `parse_seed_i64`) are public and return `String` errors. | A builder or options struct with `progress: Option<&mut dyn FnMut>` and a `CancellationToken`; re-export the types used in the API; move binding helpers behind `pub(crate)` or into the binding. |
| API-10 | Low | Verified | yes | The binding merges defaults itself (`binding.rs:211-257`, `unwrap_or(defaults.x)`). This complies with `AGENTS.md`, since the defaults come from Rust, but every new surface would have to repeat it. | `RenderOptions::merge(partial)` in render. |
| API-11 | Low | Verified | yes | `crates/primeval-render/examples/render_svg.rs` hard-codes `photo.jpg`, so `cargo run --example render_svg` fails with NotFound. | Take a path from `std::env::args()`; show progress and cancellation. |

---

## 8. Tests (TEST)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |
| TEST-1 | Medium | Verified | yes | `test/contracts.test.js` checks contracts by regex over source files. It compares `src/index.ts` arrays with `variants()`, which is used nowhere else and can drift from `FromStr`. It greps for `?? 100`-style defaults but misses `\|\|`, destructuring, `cli.ts` and the README. It checks binding parsers by name only. It missed real drift: the `jpeg` alias, `--alpha auto`, NODE-1. | Replace with **runtime, table-driven** tests that run one table of inputs and expected outcomes through the API, the binding and the CLI (omitted = explicit default, boundaries, error class and code). No code generation (section 13). |
| TEST-2 | Medium | Reproduced | yes | The abort test (`test/native.test.js:115-134`) aborts at step 1 of only 32 cheap steps (~62 ms). A 200 ms main-thread stall makes 40/40 runs resolve instead of reject. | Use a large `count` or an already-aborted signal, plus a deterministic late-abort test once NODE-4 is fixed. |
| TEST-3 | Medium | Reproduced | yes | Missing negative tests, each of which corresponds to a bug in this plan: invalid `background` mapping, throwing `onProgress`, thin/1×1/multi-byte/huge inputs, u32 overflow, JPG via the API, concurrency, already-aborted signals, CLI `--background` / `--version` / unknown option / write failure, README examples. | Add them with the fixes (red-green per `AGENTS.md`). |
| TEST-4 | Medium | Verified | partial | Engine tests (112) have gaps: some are weak or circular (`worker.rs:521-548` asserts nothing; a "keeps radius equal" test only checks r ≥ 1; the replay test in `model.rs` is circular; a score test only checks > 0). Missing: tiny images, per-shape score parity, NEON vs scalar parity, seed determinism, PNG vs SVG geometry. | Add `proptest`: rasterizer invariants (in bounds, `x1 ≤ x2`, alpha ≤ 0xFFFF, no duplicate pixels, odd and tiny sizes), fused energy = full recomputation after drawing, the blend bound, `clamp_line` vs `crop_scanlines`, hex colour round-trip, error-grid samples in bounds. |
| TEST-5 | Medium | Verified absent | yes | No fuzzing. RT-2 and RT-3 are exactly what a fuzzer finds in minutes. | `cargo-fuzz` targets for `Color::from_hex` / background parsing and for render on small arbitrary images and options (bounded `count`). |
| TEST-6 | Low | Verified | yes | `test/tooling/packed-install.test.js` is good (real `npm pack`), but its fake platform package (`main: index.js`) does not match the napi-generated package shape, it never runs the tarball's `bin`, it renders only SVG from bytes, and it runs only on linux-x64. `test/cli.test.js` never removes its `mkdtempSync` directories. | Mirror the real platform package shape, run the `bin`, and clean up temp directories. |

---

## 9. Tooling, CI and supply chain (TOOL)

All TOOL items landed in T1. Follow-ups:

- `napi-prebuilds.yml` calls the quality workflow with `uses: ./...` plus `# zizmor: ignore[self-repository]`. Switch to the `$/...` syntax and drop the ignore once actionlint accepts it (1.7.12 does not).
- Dependabot does not bump the actionlint `docker://` digest in the hygiene job; update it by hand.
- Two things only CI can confirm, on the first pull request run: `rustup toolchain install` (no arguments) installs the toolchain and components from `rust-toolchain.toml`, and the pinned actionlint image works as a `docker://` step.
- `gif` and `approx` leave with RM-1 and RM-9.

---

## 10. Documentation and repository content (DOC)

| ID | Severity | Status | `next` | Finding | Fix |
| --- | --- | --- | --- | --- | --- |
| DOC-1 | Medium | Reproduced | partial | README promises that are false today: "`--seed <N>` for deterministic output" (ENG-4); "accepted input formats are JPEG and PNG" (GIF decodes, NODE-7); errors "are mapped to `ValidationError`, `NotFoundError`, and `AbortError`" (NODE-1); an abort "rejects with `AbortError`" (NODE-4); `repeat` described as "extra random mutations to try per step" when it actually adds up to N extra shapes per step (RM-3). | Fix each claim in the same change as its code fix, as `AGENTS.md` requires. |
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
| RM-1 | yes | **GIF output** | 50× the peak memory and about 2× the time of SVG/PNG (Appendix A.4); 256 colours; 2.3 MB files against 27 KB SVG; source of the `u16` panic (RT-4). | Drops the `gif` dependency, `gif_frame_step`, `Model::frames`, `encode_gif`, NeuQuant and most of RT-5. If animation is wanted later, an **animated SVG** (CSS `animation-delay` per shape, in insertion order) is a few KB, vector, and costs no memory. That would be a deliberate new feature. |
| RM-2 | yes | **Path input** (`{ kind: "path" }` in the Node API, `InputSource::Path` in render) | RT-6: unbounded reads, FIFO hangs, file-existence oracle, all IO errors reported as NotFound, UTF-8-only paths. Node's `fs` does this better. | `NotFoundError` disappears; input becomes plain bytes (`approximate(bytes, options)` or `{ input: Uint8Array }`); the CLI uses `fs.readFile` with precise errors. |
| RM-3 | yes | **`repeat`** | It actually adds up to N extra shapes per step (`model.rs:157-171`), so the shape count stops matching `count` and progress `total` is wrong; it is documented incorrectly; it is a niche upstream knob ("mostly good for beziers"); its loop is fragile (ENG-12). | One option fewer and a clean meaning: `count` = number of shapes. |
| RM-4 | yes (decision) | **Alpha channel in the engine**: work in RGB, composite transparent inputs onto the background at decode time, accept only opaque backgrounds (`RGB` / `RRGGBB`) | ENG-16 inconsistency; about 25% of per-pixel work (PERF-4); simpler kernels; transparent output has little value for this product. Validate the trade-off with PERF-0 first. | 3-byte buffers, RGB-only NEON (`vld3_u8`), one background rule across SVG and PNG. |
| RM-5 | yes | **GIF input decoding** (the `gif` feature of `image` in `primeval-render/Cargo.toml`) | Undocumented and untested. | Smaller decode surface. Consider adding **WebP input** (pure-Rust decoder in `image`): it is the format users will most often bring. |
| RM-6 | yes | **Rust-only knobs not exposed to Node**: `prepare()` / `ApproximationRun`, `gif_frame_step`, public `workers` | They break the "layers stay aligned" rule; the binding uses only `approximate`. | `workers` becomes an internal performance knob once ENG-4 makes output independent of it. |
| RM-7 | yes | **CLI extras**: `--format`, the `jpeg` alias, `--progress auto\|plain\|off`, defaulting the output format to the input's | `--format svg -o x.png` writes SVG into a `.png`; three spellings of one thing. | Format comes from the `--output` extension only; the **default output is SVG** (the flagship format); `--quiet` replaces `--progress`; revisit the `_primitive` suffix. |
| RM-8 | yes (decision) | **JPG output** (optional) | Lossy over flat shapes (ringing at edges); 140 KB against 218 KB for PNG at 1024 px, 200 shapes. Keeping it costs little. | If removed: SVG + PNG only. |
| RM-9 | no | **Dead code in core** | Profiling hooks (`profile_quadratic` is always false, `QuadraticProfileStats`, `worker.rs:93-139`); `export::output_paths` (`export.rs:153`, CLI file naming); `util::number_string` (`util.rs:42`); `util::rotate` (tests only); `ShapeKind::variants` and `OutputFormat::variants` (used only by regex tests); `Polygon.convex` (always false, so the convexity check is dead); `Quadratic.width` (never mutated); `WorkerCtx::scratch_vertices`; `parse_alpha_u32`; the unused `_round` parameter in `Shape::mutate`; the `approx` dev-dependency. | Less surface; lets the workspace dead-code lints work once API-1 lands. |

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

Removals before hardening, so no effort goes into code that is about to disappear.

- [ ] RM-1 Remove GIF output
- [ ] RM-2 Remove path input; the CLI reads files itself
- [ ] RM-3 Remove `repeat`
- [ ] RM-5 Remove GIF input decoding (decide on WebP input)
- [ ] RM-6 Remove Rust-only knobs
- [ ] RM-7 CLI simplification (format from extension, SVG default, `--quiet`)
- [ ] RM-8 Decide on JPG
- [ ] RM-9 Delete dead code
- [ ] API-3 **Engine boundary**: engine produces committed shapes; decode, replay and encoders move to render; core drops `image`
- [ ] API-1 Restrict core visibility; `#[non_exhaustive]`
- [ ] API-4 Typed alpha end to end
- [ ] API-9, API-10, API-11 Render facade ergonomics, `merge`, example
- [ ] RM-4 Decide on RGB-only (needs a quick PERF-0 measurement)

### T3: Runtime robustness

- [ ] RT-1 `panic = "unwind"` + `catch_unwind`, mapped to `InternalError`
- [ ] RT-2, RT-3, RT-4 Root-cause fixes plus regression tests at the Rust and Node layers
- [ ] RT-5 `ImageReader` + `Limits`, option caps, drop the full image early, fewer copies (PERF-10)
- [ ] API-2, NODE-6 Typed errors with codes and causes across layers
- [ ] NODE-1, NODE-2, NODE-3, NODE-4 Async-only errors, safe progress, numeric ranges owned by Rust, reliable abort
- [ ] NODE-5 `spawn_blocking`
- [ ] NODE-7 One vocabulary
- [ ] NODE-8, NODE-9, NODE-10 Typings, cancellation handle, loader detail
- [ ] CLI-1 to CLI-4
- [ ] TEST-1, TEST-2, TEST-3, TEST-5, TEST-6

### T4: Portable native builds and release

- [ ] REL-1 Remove `target-cpu=native`; ISA gate on artifacts
- [ ] REL-2 glibc 2.17 baseline; symbol gate
- [ ] REL-5 Per-target smoke tests; Rust tests on macOS arm64
- [ ] REL-6 Release notes, Cargo version bumps, idempotent publish with post-publish verification, retire the `v0.1.1` tag
- [ ] REL-7 Decide on musl
- [ ] API-8 Crate publishability decision

REL-1 is a one-line change and can land at any time. It sits here only because the gate that keeps it fixed belongs to this ticket.

### T5: Engine correctness

Required in any case, because the current engine becomes the reference and baseline for "next".

- [ ] ENG-1 Soundness (assertions, visibility, NEON parity test on arm64 CI)
- [ ] ENG-2 Quadratic duplicates (+ PERF-7)
- [ ] ENG-3 One coordinate convention; PNG-vs-SVG test
- [ ] ENG-4 Determinism independent of thread count (+ PERF-9)
- [ ] ENG-5 to ENG-16
- [ ] TEST-4 Property tests for rasterizers and scoring

### T6: Performance, gated by benchmarks

- [ ] PERF-0 Divan benches and the time+quality script (start this early, in parallel with T2, because RM-4 and every later item need it)
- [ ] PERF-5 Anti-aliased rasterizer interiors (largest measured hotspot)
- [ ] PERF-1 Prefix sums + early exit
- [ ] PERF-4 RGB-only kernels (if RM-4 is accepted)
- [ ] PERF-8 Reduced-resolution random phase (needs quality metrics)
- [ ] PERF-6, PERF-11
- [ ] Deferred until the "next" decision: PERF-2 runtime-dispatched x86 SIMD, PERF-3 NEON accumulator tuning (section 15)

### T7: Documentation

Continuous: each ticket updates the README for the behaviour it changes. This ticket is the final pass.

- [ ] DOC-1 to DOC-4, DOC-6, DOC-7, API-7

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
