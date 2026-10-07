# Review of the algorithm leap: what to keep, what to retune, what is left

Date: 2026-10-07. Machine: Apple M2 Pro, 10 logical cores, macOS aarch64, dedicated and idle. Companion to `benchmark-report-2026-10-07.md`, which compares Go `primitive`, primeval before the leap (`b9e24b4`) and after it (`16877bb`). This document goes one level down: it takes the leap apart and measures each piece, so that the trade of time for quality can be decided piece by piece rather than as a whole.

## 1. Method

Tool: the engine runner, `cargo run --release -p primeval-render --features lab --example engine`, on its default corpus: *American Gothic*, *Mona Lisa* and three synthetic 512 × 512 images (gradient, shapes, texture), all nine shape kinds, seed 42, checkpoints at 50, 100, 200 and 500 shapes, 10 threads. Quality is `rmse256`, the RMSE of the exported PNG at the working size against the working target, normalised to 0–1. Time is `search_s`, the wall time of the steps and of every pass or stage, decode and encode excluded. Nine runs of the same binary (`main` at `fba7439`) with different flags:

| Run | Flags | What it is |
| --- | --- | --- |
| `greedy-e16` | `--effort 16:1:1 --steps 50,…,1400` | the greedy search alone with the old effort (16 rounds, climb age ×1 for every kind), to 1400 shapes, the curve for the equal-time comparison |
| `end1-e16` | `--refine end:1 --effort 16:1:1` | **the baseline**: greedy plus one refit pass at the end, which is what `approximate` did before the pipeline (`b62fdf8`) |
| `greedy` | – | the greedy search alone with the new per-kind effort |
| `during-only` | `--refine final --joint-scale 0 --final-refits 0` | the new effort and the refit passes during the search, no final stage |
| `final-nodur` | `--refine final --during none` | the new effort and the final stage (B, or refit passes), no passes during the search |
| `final-nojoint-r1` | `--refine final --joint-scale 0 --final-refits 1` | the full pipeline with one refit pass in place of B |
| `final` | `--refine final` | the full pipeline, what `approximate` runs today |
| `final-e16` | `--refine final --effort 16:1:1` | the full pipeline with the old effort |

Every run uses today's code, so the 2 px area-coverage quadratic stroke, the legibility rules and the exact single-rounding blend are in every row, the baseline included. These runs isolate the **search pipeline** (effort, passes during the search, final stage); the three other changes are assessed from the plan's own measurements and from `versus_go` in section 5.

The baseline emulation is faithful: `end1-e16` reproduces the plan's "previous `approximate`" numbers within noise (for example quadratic median 0.19620 / 0.16319 / 0.11824 / 0.04668 against the plan's 0.196203 / 0.163186 / 0.118244 / 0.046677).

## 2. The leap in pieces

| Piece | Where | Size | Role |
| --- | --- | --- | --- |
| Exact single-rounding blend, and the PNG writer's high-precision pipeline | `score.rs`, `primeval-render/src/raster.rs` | small | the engine optimises the composite the export draws; `write_png` 42 → 57 ms |
| Quadratic stroke: 2 px, coverage by pixel area, butt caps, finer flattening | `raster.rs`, `shapes.rs` | +330 lines | the fix of the worst quality defect; rasterization 3× slower |
| Legibility rules: convex polygons, angles above 15°, aspect at most 1:8 | `shapes.rs` | +300 lines | a product decision; costs 1–3 points on polygon and rotated rectangle |
| Refit passes (A1) with step-adapted climbs, cancellable | `refine.rs`, `optimize.rs`, `model.rs` | +1,100 lines | coordinate descent over the committed shapes |
| Per-kind search effort | `model.rs` (`Effort`) | small | 32 rounds for `any` and rotated ellipses; climb age ×2 for quadratics and rotated ellipses |
| Joint gradient optimisation (B) with projections and snaps, and its export guard | `joint.rs`, `joint/{diff,angle,convex,rect}.rs`, `pipeline.rs` | +5,400 lines | Adam over all triangles, polygons and rectangles at once, on a smooth model of the anti-aliased export |
| Pipeline per kind, `Spaced` schedule, lab hooks, engine runner | `pipeline.rs`, `lab.rs`, `examples/engine.rs`, `examples/common` | +2,300 lines | what `approximate` runs around the greedy steps, and the tool that chose it |
| Progress contract and demo "Refining" state | `src/types.ts`, `demo/app.js` | small | shapes already reported can be revised |

## 3. What each stage buys and costs

Mean of the 100- and 200-shape median `rmse256` against the baseline, and the time ratio over the same rows (sum of `search_s` at 100 and 200 shapes over the five images). Lower is better in both columns.

| Kind | greedy, new effort | passes during only | final stage only | full, A1 instead of B | **full pipeline** | full, old effort |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| any | +7.9% · 1.36× | −8.0% · 2.58× | −7.1% · 1.62× | −8.8% · 2.65× | **−10.8% · 2.78×** | −10.0% · 2.34× |
| triangle | +7.6% · 0.78× | −6.4% · 2.89× | −10.6% · 1.11× | −7.3% · 2.98× | **−14.8% · 3.22×** | −14.8% · 3.23× |
| rectangle | +6.2% · 0.76× | −2.7% · 1.83× | −9.2% · 1.05× | −3.7% · 1.98× | **−12.4% · 2.13×** | −12.4% · 2.14× |
| ellipse | +3.9% · 0.85× | −7.1% · 3.07× | 0.0% · 0.99× | −7.2% · 3.10× | **−7.2% · 3.11×** | −7.2% · 3.11× |
| circle | +4.2% · 0.88× | −5.2% · 2.08× | 0.0% · 0.99× | −5.7% · 2.14× | **−5.7% · 2.14×** | −5.7% · 2.15× |
| rotated-rectangle | +6.6% · 0.85× | −1.1% · 1.31× | −10.2% · 1.51× | −3.2% · 1.41× | **−11.7% · 1.95×** | −11.7% · 1.96× |
| quadratic | +0.6% · 1.66× | −2.3% · 1.78× | −2.1% · 1.71× | −2.9% · 1.80× | **−3.1% · 1.81×** | −0.5% · 1.15× |
| rotated-ellipse | +6.9% · 2.48× | −8.6% · 3.02× | −2.0% · 2.49× | −9.2% · 3.09× | **−9.2% · 3.03×** | −7.0% · 1.59× |
| polygon | +11.9% · 0.90× | −5.1% · 1.30× | −7.0% · 0.97× | −6.9% · 1.35× | **−9.5% · 1.37×** | −9.5% · 1.39× |

The greedy column is positive because the baseline includes one refit pass: it shows what that single pass was worth (4–12%, for 10–25% of the time). The "final stage only" column for ellipse and circle is exactly the baseline, because their final stage *is* one refit pass.

Where the time goes at 200 shapes, over the five images:

| Kind | greedy | passes during | final stage | total | during share | final-stage share | total with old effort |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| any | 11.3 s | 10.6 s | 2.5 s | 23.5 s | 45% | 10% | 19.7 s |
| triangle | 2.7 s | 7.3 s | 1.3 s | 11.2 s | 65% | 11% | 11.2 s |
| rectangle | 1.8 s | 2.6 s | 0.8 s | 5.2 s | 51% | 15% | 5.2 s |
| ellipse | 2.4 s | 6.4 s | 0.4 s | 9.2 s | 70% | 5% | 9.2 s |
| circle | 2.4 s | 3.5 s | 0.4 s | 6.2 s | 56% | 6% | 6.2 s |
| rotated-rectangle | 2.9 s | 1.4 s | 2.3 s | 6.5 s | 21% | 35% | 6.6 s |
| quadratic | 31.4 s | 2.3 s | 2.1 s | 35.6 s | 7% | 6% | 22.8 s |
| rotated-ellipse | 27.7 s | 7.9 s | 1.0 s | 35.2 s | 23% | 3% | 18.8 s |
| polygon | 13.9 s | 7.0 s | 1.7 s | 22.0 s | 32% | 8% | 22.4 s |

Readings:

- **B is the efficient part.** For triangles, rectangles and polygons the final stage alone takes 0.97–1.11× the baseline's time and delivers 7–11 points; with the same passes during the search, B beats one refit pass by 3.4–17 points (triangle), 6–11 (rectangle), 6–12 (rotated rectangle) and 2.5–3.7 (polygon) at a few percent more time. It is 10–15% of the pipeline's time except for rotated rectangles (35%, the doubled iterations).
- **The passes during the search are the cost.** They are 45–70% of the time for `any`, triangle, rectangle, ellipse and circle. On triangles they add 4.2 points over the final stage alone for 2.9× its time; on rectangles 3.2 points for 2×. For ellipses and circles they are the only lever (5–7%) and they cost 2–3× the time. The one place they are cheap is rotated ellipses: 6.6 points for 22% more time over the new-effort greedy, because that greedy is itself slow.
- **The per-kind effort is mostly waste.** 32 rounds on `any` buy 0.8 points for 19% more time. 32 rounds and climb age ×2 on rotated ellipses buy 2.2 points for 90% more time (3.03× against 1.59×). Climb age ×2 on quadratics buys 2.6 points for 57% more time (1.81× against 1.15×), the only one of the three with a defensible return, and only because quadratics have no better lever yet.

### Cheaper schedules for the passes during the search

Two more runs, both at the old effort, replace every kind's schedule with one `Spaced` rule (the final stage of each kind unchanged): `spaced:20:5` (a pass after step 20, then each `max(20, s / 5)` steps: 8 passes and about 3.7 layer refits per step up to 200 shapes) and `spaced:10:10` (17 passes, 7.9 layers per step). Today's schedules are `Spaced(5, 10)` for `any` and triangle (25 passes, 9.8 layers per step), `Spaced(5, 20)` for ellipse (35 passes, 16 layers per step), `Spaced(10, 10)` for circle and rotated ellipse, `Spaced(20, 5)` for rectangle, polygon and quadratic, `Spaced(20, 2)` for rotated rectangle. Same columns as above; the equal-time ratio is from section 4's method at 200 shapes.

| Kind | `spaced:20:5`, old effort | `spaced:10:10`, old effort | today's schedule, old effort | today's pipeline (new effort) | equal-time ratio at 200: 20:5 / 10:10 / today |
| --- | ---: | ---: | ---: | ---: | ---: |
| any | −6.9% · 1.57× | −7.4% · 1.92× | −10.0% · 2.30× | −10.8% · 2.78× | 1.01 / 1.07 / 1.18 |
| triangle | −12.9% · 2.06× | −13.7% · 2.64× | −14.8% · 3.21× | −14.8% · 3.22× | 1.01 / 1.07 / 1.07 |
| rectangle | −12.4% · 2.14× | −13.0% · 2.84× | = 20:5 | −12.4% · 2.13× | 1.11 / 1.22 / 1.10 |
| ellipse | −3.3% · 1.60× | −5.1% · 2.15× | −7.2% · 3.05× | −7.2% · 3.11× | 1.10 / 1.19 / 1.30 |
| circle | −3.3% · 1.60× | = 10:10 | −5.7% · 2.11× | −5.7% · 2.14× | 1.11 / 1.17 / 1.15 |
| rotated-rectangle | −11.4% · 2.15× | −10.3% · 2.65× | −11.7% · 1.92× | −11.7% · 1.95× | 1.06 / 1.16 / 1.00 |
| quadratic | = 20:5 | −0.6% · 1.23× | −0.5% · 1.13× | −3.1% · 1.81× | 1.13 / 1.19 / 1.60 |
| rotated-ellipse | −5.7% · 1.30× | = 10:10 | −7.0% · 1.56× | −9.2% · 3.03× | 0.94 / 0.97 / 1.19 |
| polygon | = 20:5 | −9.6% · 1.62× | = 20:5 | −9.5% · 1.37× | 0.93 / 0.96 / 0.90 |

Readings:

- **For triangle, `Spaced(20, 5)` keeps 55% of the passes' gain (2.3 of 4.2 points over the final stage alone) for 45% of their cost**, and for `any` it keeps 69% of the pipeline's gain (6.9 of 10.0 points) in 68% of its time. Both become neutral at equal time (1.01). The last 2–3 points of the dense schedule cost 0.7–1.2× the baseline's whole time.
- **Rotated rectangles, rectangles and polygons are already on their best schedule**: a denser one gains nothing or loses (rotated rectangles lose 0.3–1.4 points with more passes; B does the work there).
- **Ellipses and circles have no cheap lever.** Their gain is roughly linear in the passes: 3.3 points for 0.6× the baseline time, 5–7 points for 1.1–2×. The choice is a product one; `Spaced(20, 5)` or `(10, 10)` would put them at 1.6–2.2× instead of 2.1–3.1×.
- **Regressions over 2%**: two rows for each cheap schedule, all on synthetic-shapes (`any` at 100 +16.6% with 20:5; ellipse at 500 +9% with both; polygon at 500 +4.6% with 10:10), against one for today's schedule (ellipse at 500, +19.5%). Medians are unaffected; this image is the known outlier.

## 4. Fixed shape count against fixed time

The plan chose quality at a fixed shape count (user decision of 2026-10-03). The other view matters for anyone who feels the time: for each row, the old greedy curve (`greedy-e16`, to 1400 shapes) is interpolated at the same `search_s`, and the table gives the median over images of `rmse256(run) / rmse256(old greedy at equal time)`, the equivalent old shape count, and the ratio of SVG sizes. Below 1 the pipeline wins at equal time. `>` marks rows whose time is past the curve's last checkpoint, where the curve is clamped and the ratio flatters the run.

| Kind | shapes | full pipeline | ≈ old shapes | SVG ratio | final stage only | full, old effort |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| any | 100 | 1.246 | 364 | 0.26 | 1.063 | 1.164 |
| any | 200 | 1.179 | 741 | 0.27 | 1.030 | 1.107 |
| triangle | 100 | 1.199 | 499 | 0.19 | 0.961 | 1.197 |
| triangle | 200 | 1.074> | 970 | 0.20 | 0.887 | 1.074> |
| rectangle | 100 | 1.134 | 255 | 0.41 | 0.971 | 1.132 |
| rectangle | 200 | 1.098 | 460 | 0.45 | 0.995 | 1.095 |
| ellipse | 100 | 1.290 | 339 | 0.30 | 1.011 | 1.286 |
| ellipse | 200 | 1.302> | 701 | 0.29 | 1.002 | 1.302> |
| circle | 100 | 1.159 | 258 | 0.39 | 1.008 | 1.159 |
| circle | 200 | 1.154 | 550 | 0.37 | 1.002 | 1.157 |
| rotated-rectangle | 100 | 1.075 | 221 | 0.44 | 1.022 | 1.072 |
| rotated-rectangle | 200 | 1.003 | 430 | 0.47 | 0.987 | 1.002 |
| quadratic | 100 | 1.325 | 195 | 0.50 | 1.310 | 1.043 |
| quadratic | 200 | 1.595 | 439 | 0.45 | 1.511 | 1.082 |
| rotated-ellipse | 100 | 1.250 | 364 | 0.27 | 1.284 | 0.979 |
| rotated-ellipse | 200 | 1.191 | 672 | 0.29 | 1.244 | 0.955 |
| polygon | 100 | 0.953 | 158 | 0.56 | 0.877 | 0.951 |
| polygon | 200 | 0.897 | 331 | 0.60 | 0.829 | 0.935 |

At 500 shapes the full pipeline is at or below 1.0 for triangle (0.91), rectangle (1.00), rotated rectangle (0.95) and polygon (0.84), and still 1.09–1.42 for the others.

Readings:

- **At equal time the full pipeline loses to the old greedy on most kinds at 50–200 shapes**, by 10–30%, and by 30–60% on quadratics. Its output is 2–5× smaller, which is the point for placeholders, but a user who fixes the time and does not care about bytes is worse off.
- **The final stage alone is neutral or better at equal time** for triangle, rectangle, rotated rectangle and polygon, and neutral for ellipse and circle (where it is the old one pass). It is the part of the leap that pays for itself under both metrics.
- **The passes during the search are what loses at equal time**, and for quadratic and rotated ellipse the effort is: with the old effort the pipeline is neutral at equal time on quadratics (1.04–1.08) and *better* on rotated ellipses (0.96–0.98) while still 7–9% better than the baseline at a fixed count.

## 5. The pieces outside the search pipeline

- **Exact blend and high-precision PNG.** Keep. It closed the engine–export gap at 500 shapes from +6.6% to +0.6% and made the refit's layer model exact up to rounding. The price is `write_png` at 1024 px, 42 → 57 ms (once per render), and 7–8% slower energy kernels on polygon and quadratic lines.
- **2 px quadratic stroke.** Keep. It is most of the quadratic gain in `versus_go` (RMSE against Go 0.86 → 0.56 at 200 steps, 0.71 → 0.42 at 1000). It also changes the look (strokes twice as wide at the working size) and triples the rasterization cost, which with the age ×2 is why quadratic steps went from 85 to 244 ms. A 3 px stroke fits better still per the plan; that is a look decision.
- **Legibility rules.** A product decision, not a quality one: they cost polygon 2–3 points and rotated rectangle about 1 point at a fixed count, and the synthetic texture up to 39%. Time-neutral. They are also what makes B's projections well-posed, so dropping them would mean reworking B.
- **Progress contract and "Refining".** A consequence of any revision of already-reported shapes; nothing to decide separately.

## 6. Regressions

Over the 180 rows of the corpus, the full pipeline is worse than the baseline by more than 2% in exactly one: synthetic-shapes, ellipse, 500 shapes, +19.5%. It comes from the passes during the search (the final stage alone is at 0.0% there) and is the known property recorded in the plan: a pass is kept when it lowers the engine's binary-coverage score, which on hard synthetic edges can raise the anti-aliased export's error. Mean SSIM at 128 px rises for every kind (+0.012 to +0.029). Zero rule violations in every run.

## 7. Verdicts

1. **Keep B, as is.** It is the quality lever that costs least, it holds under both metrics, and its guard contains its one known failure mode. The open fix (exact polygon coverage where two near-collinear edges share a pixel) would let it gain on the synthetic shapes where the guard now rejects it.
2. **Keep the refit pass module, retune its use.** One pass at the end was already in the baseline. The passes during the search should be rare: they are the most expensive part of the pipeline and the only part that loses at equal time. Measured: `Spaced(20, 5)` for `any` and triangle keeps most of the gain at a third of the cost; ellipse and circle are a product choice between 3 and 7 points at 0.6× to 2× the baseline time.
3. **Drop the extra effort for `any` and rotated ellipses** (back to 16 rounds, age ×1): 0.8 and 2.2 points for 19% and 90% more time. **Reconsider the age ×2 for quadratics**: 2.6 points for 57% more time, kept only while quadratics have no cheaper lever.
4. **Keep the exact blend, the 2 px stroke and the rules** (section 5).
5. **The previous engine as a fast preset** is not needed if the schedule is retuned: the final stage alone is within 1.0–1.1× of the old `approximate`'s time for the kinds B covers, and the old `approximate` is what the pipeline with no passes during the search and one final pass *is* for the others.

### A retuned pipeline, from measured rows

Each kind's row in a run depends only on that kind's settings, so rows from different runs can be combined. One combination that keeps B everywhere, drops the extra effort and thins the passes during the search; every cell is a measured row (mean of the 100/200 medians against the old `approximate`, time ratio, equal-time ratio at 200 shapes):

| Kind | Today | Measured | Today's result | Retuned result |
| --- | --- | --- | ---: | ---: |
| any | 32 rounds, `Spaced(5, 10)`, 1 pass, B | 16 rounds, `Spaced(20, 5)`, 1 pass, B | −10.8% · 2.78× · 1.18 | −6.9% · 1.57× · 1.01 |
| triangle | `Spaced(5, 10)`, B | `Spaced(20, 5)`, B | −14.8% · 3.22× · 1.07 | −12.9% · 2.06× · 1.01 |
| rectangle | `Spaced(20, 5)`, B | unchanged | −12.4% · 2.13× · 1.10 | same |
| ellipse | `Spaced(5, 20)`, 1 pass | `Spaced(10, 10)`, 1 pass | −7.2% · 3.11× · 1.30 | −5.1% · 2.15× · 1.19 |
| circle | `Spaced(10, 10)`, 1 pass | unchanged (or `Spaced(20, 5)`: −3.3% · 1.60×) | −5.7% · 2.14× · 1.15 | same |
| rotated-rectangle | `Spaced(20, 2)`, B ×2 | unchanged | −11.7% · 1.95× · 1.00 | same |
| quadratic | age ×2, `Spaced(20, 5)`, passes until < 1% | unchanged, or age ×1 (−0.5% · 1.13× · 1.10) | −3.1% · 1.81× · 1.60 | a look-and-time choice |
| rotated-ellipse | 32 rounds, age ×2, `Spaced(10, 10)`, 1 pass | 16 rounds, age ×1, same schedule | −9.2% · 3.03× · 1.19 | −7.0% · 1.56× · 0.97 |
| polygon | `Spaced(20, 5)`, B | unchanged | −9.5% · 1.37× · 0.90 | same |

Against today's pipeline this gives back 2–4 points on `any`, triangle, ellipse and rotated ellipse, and cuts their time from 2.8–3.2× to 1.6–2.2× the old `approximate`'s; it leaves rectangle, rotated rectangle and polygon as they are. Applied to the benchmark report's `versus_go` totals (an estimate from the ratios above, not a measurement), the four kinds' time falls by about 10 s of 47 s at 200 steps and 37 s of 169 s at 1000, so the speedup over Go moves from 2.7× / 3.1× to roughly 2.9× / 3.3×. Quadratic (11 s of the 47) and polygon are then the dominant costs, and neither has a cheap lever left in the pipeline: their time is the greedy search itself. The `any` row deserves one more measurement, since `any`'s passes during the search also serve its curved layers, which B does not move.

## 8. Room for further gains

Measured here, or in the plan, unless marked as a hypothesis.

### Speed

- **The refit pass uses at most 4 of 10 cores. Measured.** `refine_layer` runs `ROUNDS = 4` climbs as rayon tasks, and the layers are strictly sequential (top-down), with a single-threaded replay of up to `√N` layers before each one and a single-threaded fold after it. With `--refine end:4` on the two paintings, the refit time is the same at 4, 8 and 10 threads (triangle at 200 shapes: 0.49 / 0.52 / 0.52 s; rotated ellipse: 0.85 / 0.88 / 0.88 s) while the greedy part scales 1.5–1.7× from 4 to 10 threads, with identical output. In today's pipeline the passes are 45–70% of the time for five kinds, so on this machine half or more of the wall time runs on 4 of 10 cores. Fixes, in order of ambition: 8 or 10 climbs of a shorter age (same evaluations, more parallelism; quality to measure, since the climbs would be shallower); refitting layers whose bounding boxes are disjoint in parallel, since they do not interact in the layer model; a Jacobi-style pass that refits every layer against the old stack at once and verifies on the exact canvas. Any of them keeps the seed contract if the climbs' streams stay per layer and per climb.
- **16 search rounds on 10 threads is two waves, the second with 6 idle threads. Measured, modest.** Greedy alone on the paintings at 10 / 16 / 20 rounds: the time per round is 1.25× / 1.00× / 0.93× (triangle), 1.16× / 1.00× / 0.91× (`any`), 1.17× / 1.00× / 0.98× (rotated ellipse), so 20 rounds, two full waves on 10 cores, cost 17–24% more time than 16 for 25% more work and 0.3–1.0% lower RMSE; 10 rounds save 22–28% of the time and lose 1–2%. Work stealing and the uneven climb lengths already absorb most of the wave effect, so the gain is at most 10–15%. The rounds cannot follow the thread count without breaking the seed contract; a cheaper fix is splitting each round's random phase in two tasks.
- **x86 has no SIMD path.** Every scoring kernel is scalar there; the plan's x86 re-baseline has polygon at 0.83× and quadratic at 0.42–0.50× Go's speed on the pipeline. An SSE2/AVX2 twin of the NEON kernels, dispatched at runtime, is the one change that would move the server numbers, and it was deferred by the plan only until the "next engine" decision, which is now taken.
- **The quadratic rasterizer** is 284 µs a shape against 90 µs before: per pixel it computes two trapezoid primitives, two `copysign` and, near the ends, a `hypot`. The inner band of a long segment (the common case) only needs `band(half_width, |across|)`, which is already special-cased; the ends could be clipped by a cheaper test than `hypot`.
- **B's bands.** 16 bands of 16 rows at 256 px give 16 tasks on 10 threads, and bands with more layers take longer. Smaller bands cost more checkpoints; 8-row bands are worth one measurement.

### Quality

- **Anti-aliased refit for the kinds B does not cover.** The refit pass optimises the binary engine canvas; the ellipse regression and the small gains of circles and ellipses both follow from that. A smooth coverage for ellipses (an exact-area ellipse edge is harder than a half-plane, but an approximation on the boundary pixels would already beat binary) would also let B cover ellipses, circles and rotated ellipses, which is the plan's open experiment 7.
- **B's near-collinear coverage.** The product of half-planes double-counts where two edges cross the same pixel at a near-180° vertex. An exact convex-polygon pixel area, or a penalty on near-180° angles, would let B win on the synthetic shapes instead of being rejected by the guard.
- **A2 was never tried.** The plan's recommendation was A1 *and* A2 (an evolution strategy per shape in place of "1000 random candidates and a hill climb"). Only A1 landed. The greedy step still spends 75–80% of its evaluations on random proposals of which one survives. A (1+λ)-ES seeded from the error grid, or simply the step-adapted climb from the best few random candidates (the refit's `climb` already exists), could cut the greedy time or raise its quality at a fixed count, and it compounds with everything above because B starts from the greedy result.
- **Second final refit pass** for the kinds without B: the plan measured 1–3% more for about 10% more time. Cheap, and much better value than the passes during the search.
- **Quadratics.** They are now the slowest kind against Go (1.3–1.5× on this machine, slower than Go on x86) and still 2–3× worse in RMSE than every other kind. A stroke width per shape (part of the state, bounded, exported as `stroke-width`) is the natural next step and needs no public option; it changes the look less than a global 3 px.

### Product

- **Expose nothing new, but pick the default by the use case.** The repository's stated use case is small SVG placeholders at a fixed count. If that stays the target, the retuned pipeline (B kept, rare passes during the search, old effort) keeps most of the quality at roughly 1.2–1.6× the old time instead of 2–3×. If render time at any count also matters, the final stage alone is the configuration that never loses.

## 9. Reproduction

```bash
cargo build --release -p primeval-render --features lab --example engine
E=target/release/examples/engine
$E --effort 16:1:1 --steps 50,100,200,500,1000,1400 > greedy-e16.md
$E --refine end:1 --effort 16:1:1 > end1-e16.md
$E > greedy.md
$E --refine final --joint-scale 0 --final-refits 0 > during-only.md
$E --refine final --during none > final-nodur.md
$E --refine final --joint-scale 0 --final-refits 1 > final-nojoint-r1.md
$E --refine final > final.md
$E --refine final --effort 16:1:1 > final-e16.md
$E --refine final --effort 16:1:1 --during spaced:20:5 > cheap-s20-5.md
$E --refine final --effort 16:1:1 --during spaced:10:10 > cheap-s10-10.md
for t in 4 8 10; do RAYON_NUM_THREADS=$t $E --no-synthetic --shapes triangle,rotated-ellipse --steps 100,200 --refine end:4 --effort 16:1:1 > threads-$t.md; done
for r in 10 16 20; do $E --no-synthetic --shapes triangle,any,rotated-ellipse --steps 100,200 --effort $r:1:1 > rounds$r.md; done
```

Each corpus run takes 3–6 minutes on the M2 Pro; the thread and round runs take seconds. The baseline and the two reference runs were run twice, before and after a reboot: the quality columns are identical (seeded output does not depend on timing) and the time ratios agree within 4%. The comparison scripts (`parse.py`, `ladder.py`, `equal_time.py`) read the runner's Markdown tables; they are not part of the repository. The raw tables are kept in `target/abl/` (gitignored).
