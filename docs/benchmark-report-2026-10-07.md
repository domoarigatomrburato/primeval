# Benchmark report: Go primitive, primeval before and after the algorithm leap

Date: 2026-10-07. Machine: Apple M2 Pro, 10 logical cores, macOS aarch64, nothing else heavy running; runs were sequential.

## What was compared

| Label | What it is | Commit |
| --- | --- | --- |
| Go | `fogleman/primitive` v0.0.0-20200504002142-0373c216458b, built with go1.27.1 | – |
| primeval before | `main` after the audit and refactor, before the algorithm leap | `b9e24b4` |
| primeval after | `feat/algorithm-leap`, squash-merged into `main` | `4bab736` (branch tip) |

Tool: `cargo run --release -p primeval-render --example versus_go` (defaults). Both tools get alpha 128, working size 256, output size 1024, PNG output, average background; primeval uses seed 42; 2 images (americangothic, monalisa) × 9 shape kinds × 200 and 1000 steps, 3 runs per tool and configuration, median times. Go is timed as a process, primeval in-process (the Node CLI adds about 0.1 s). Quality is the RGB RMSE on the 0–255 scale of each rendered PNG against the original resized to the output size, computed the same way for both tools; lower is better. Go has no seed flag, so its RMSE is a mean over runs. Go's times in the tables below are the mean of the two sessions (they differ by under 1%).

Micro-benchmarks: Divan, `cargo bench -p primeval-core --features bench --bench core` and `-p primeval-render --features bench --bench render`.

## Summary

- **Quality:** after the leap primeval has a lower RMSE than Go in 36 of 36 configurations. Against Go the geometric-mean RMSE ratio goes from 0.922 to 0.809 at 200 steps and from 0.840 to 0.741 at 1000 steps. Against the previous primeval the RMSE is 2–14% lower for most kinds (the smallest gains are on rectangle, ellipse and circle) and 34–41% lower for quadratic.
- **Speed:** after the leap primeval takes about 2.2× the time of before at equal step count (1.2× to 3.5× depending on the kind), so its speedup over Go falls from 6.2×/6.3× to 2.7×/3.1× (200/1000 steps). It is still faster than Go in every configuration.
- **The change is a trade of time for quality per shape.** At a fixed shape count (the metric that matters for small SVG placeholders) it is clearly better. At a fixed time budget the gain is much smaller, and for some kinds probably nil (see the limits below).

## Totals

Geometric means over the 18 configurations; times are sums of median times.

| | Go | primeval before | primeval after |
| --- | ---: | ---: | ---: |
| Total time, 200 steps | 106 s | 21.4 s | 47.2 s |
| Total time, 1000 steps | 433 s | 76.4 s | 169.1 s |
| Speedup over Go, 200 steps | 1× | 6.20× | 2.68× |
| Speedup over Go, 1000 steps | 1× | 6.31× | 3.06× |
| RMSE ratio to Go, 200 steps | 1.000 | 0.922 | 0.809 |
| RMSE ratio to Go, 1000 steps | 1.000 | 0.840 | 0.741 |
| Configurations with RMSE below Go | – | 17/18, 18/18 | 18/18, 18/18 |

## By shape kind, 200 steps

| Shape | Go time | before | after | after / before | Go RMSE | before | after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| any | 12.2 s | 2.09 s | 7.23 s | 3.46× | 14.89 | 13.63 | 12.04 |
| triangle | 7.1 s | 1.15 s | 3.08 s | 2.68× | 15.74 | 14.07 | 12.33 |
| rectangle | 5.2 s | 0.67 s | 1.39 s | 2.07× | 15.91 | 14.80 | 14.09 |
| ellipse | 11.1 s | 0.92 s | 2.64 s | 2.87× | 14.84 | 14.74 | 13.84 |
| circle | 13.2 s | 0.92 s | 1.89 s | 2.05× | 16.25 | 16.17 | 15.17 |
| rotated-rectangle | 6.8 s | 1.06 s | 2.11 s | 1.99× | 15.53 | 14.02 | 12.92 |
| quadratic | 14.0 s | 3.23 s | 10.99 s | 3.40× | 45.41 | 38.90 | 25.53 |
| rotated-ellipse | 24.2 s | 5.57 s | 11.02 s | 1.98× | 15.22 | 14.06 | 12.47 |
| polygon | 12.5 s | 5.81 s | 6.87 s | 1.18× | 14.72 | 13.18 | 11.64 |

## By shape kind, 1000 steps

| Shape | Go time | before | after | after / before | Go RMSE | before | after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| any | 48.4 s | 8.44 s | 24.01 s | 2.84× | 12.66 | 10.44 | 9.72 |
| triangle | 25.4 s | 4.87 s | 9.19 s | 1.89× | 13.64 | 10.99 | 9.49 |
| rectangle | 20.5 s | 3.07 s | 4.96 s | 1.62× | 13.54 | 11.30 | 11.02 |
| ellipse | 45.0 s | 3.81 s | 9.00 s | 2.36× | 11.55 | 11.34 | 10.88 |
| circle | 54.7 s | 3.76 s | 6.53 s | 1.74× | 12.46 | 12.28 | 11.73 |
| rotated-rectangle | 26.4 s | 4.59 s | 7.25 s | 1.58× | 12.88 | 10.73 | 10.08 |
| quadratic | 58.1 s | 12.05 s | 39.20 s | 3.25× | 26.41 | 18.81 | 11.18 |
| rotated-ellipse | 106.8 s | 16.45 s | 42.89 s | 2.61× | 12.91 | 10.48 | 9.55 |
| polygon | 47.6 s | 19.38 s | 26.06 s | 1.34× | 12.93 | 10.45 | 9.40 |

Speedup over Go after the leap, 200 / 1000 steps: circle 7.0× / 8.4×, ellipse 4.2× / 5.0×, rectangle 3.7× / 4.1×, rotated-rectangle 3.2× / 3.7×, triangle 2.3× / 2.8×, rotated-ellipse 2.2× / 2.5×, any 1.7× / 2.0×, polygon 1.8× / 1.8×, quadratic 1.3× / 1.5×.

SVG size is similar or smaller than before (for example polygon at 1000 steps 112 KB before, 97 KB after; quadratic 124 KB in both).

## Micro-benchmarks (Divan, medians)

`Model::step`, one step on the benchmark image:

| Shape | before | after |
| --- | ---: | ---: |
| rectangle | 30.9 ms | 25.3 ms |
| circle | 41.1 ms | 37.4 ms |
| ellipse | 47.3 ms | 39.6 ms |
| rotated-rectangle | 66.5 ms | 65.8 ms |
| triangle | 82.7 ms | 71.2 ms |
| any | 71.6 ms | 130.6 ms |
| quadratic | 84.6 ms | 243.8 ms |
| polygon | 301 ms | 276 ms |
| rotated-ellipse | 165 ms | 536 ms |

Single-shape rasterization: quadratic 90 µs → 284 µs, polygon 648 µs → 576 µs; the other kinds are unchanged within noise. `write_svg` 297 µs → 292 µs; `write_png` (1024 px) 41.9 ms → 56.9 ms, which was not investigated.

The micro-benchmarks show where the time went: the cost of the new pipeline is concentrated in `any`, quadratic and rotated-ellipse. Rectangle, circle, ellipse, triangle and polygon steps are the same speed or slightly faster.

## Recommendation

Keep the new algorithm.

- It removes the worst quality defect of the previous engine (quadratic) and improves every other kind at equal shape count, which is what matters for small SVG placeholders with 50–200 shapes.
- It still beats Go on both time (2.7–3.1×) and quality (36/36 configurations) on this machine.
- The price is about 2.2× the time of the previous engine. The previous engine is a legitimate fast preset; the plan in `docs/plans/` already suggests keeping it, but `AGENTS.md` discourages new public render options, so that is a decision to take explicitly.

## Limits of this report and open items

- **Fixed-shape-count versus fixed-time-budget.** The numbers above compare equal step counts. A back-of-the-envelope interpolation for triangle suggests that the previous engine with about 2.7× more steps (same time as the new one at 200 steps) reaches about the same RMSE, with a much larger SVG. This was not measured directly; if time per render is what users feel, a direct equal-time comparison should be run.
- **Quality is RMSE only.** No SSIM, and no visual side-by-side was produced. The `quality` runner (`cargo run --release -p primeval-render --example quality`) was run on the previous engine only; its output was not compared with the new one.
- **One machine, one ISA.** These are Apple M2 Pro (NEON) numbers. The plan's own x86 re-baseline (Xeon VM, no SIMD) reports polygon and quadratic slower than Go after the pipeline (about 0.8× and 0.4–0.5×). That was not measured here and should be weighed before a release.
- **Known worse cases from the plan.** Ellipse at 500 steps on one synthetic image gets 19.5% worse; the plan records this as a known property of the refit passes. The guard that rejects joint optimisation steps which worsen the export is not a general fix.
- **Pre-merge items from the plan are still open.** A full review of the squashed 11.8k-line change, regenerated gallery, README comparison images and versus-Go numbers (`CONTRIBUTING.md`), and the manual check of the demo's "Refining" state during a long single-threaded pass. The README's Benchmarks section is therefore stale.
- **Go variability.** Go has no seed flag; its RMSE is a mean of three runs and its times vary by under 1% between the two sessions.
- **`write_png` slowdown** (41.9 → 56.9 ms) is unexplained and may be noise or a real change.

## Reproduction

```bash
cargo run --release -p primeval-render --example versus_go > versus-go.md   # about 25 to 45 minutes
cargo bench -p primeval-core --features bench --bench core
cargo bench -p primeval-render --features bench --bench render
```

Before-leap numbers come from commit `b9e24b4`, after-leap numbers from `4bab736` (the tip of the squashed branch).
