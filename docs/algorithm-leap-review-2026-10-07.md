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

Against today's pipeline this gives back 2–4 points on `any`, triangle, ellipse and rotated ellipse, and cuts their time from 2.8–3.2× to 1.6–2.2× the old `approximate`'s; it leaves rectangle, rotated rectangle and polygon as they are. Measured with `versus_go` after the retune landed (`646252c`, same machine and settings as the benchmark report): total time 37.1 s at 200 steps and 135.7 s at 1000 against 47.2 s and 169.1 s before, so the geometric-mean speedup over Go moves from 2.68× / 3.06× to 3.26× / 3.75×, while the RMSE ratio against Go stays at 0.812 / 0.741 (was 0.809 / 0.741) and primeval remains below Go in 36 of 36 configurations. On the two paintings the retune cost almost no quality: the corpus medians it gave back came mostly from the synthetic images. Quadratic (11 s of the 47) and polygon are then the dominant costs, and neither has a cheap lever left in the pipeline: their time is the greedy search itself. The `any` row deserves one more measurement, since `any`'s passes during the search also serve its curved layers, which B does not move.

## 8. Room for further gains

Measured here, or in the plan, unless marked as a hypothesis.

### Speed

- **The refit pass uses at most 4 of 10 cores. Measured.** `refine_layer` runs `ROUNDS = 4` climbs as rayon tasks, and the layers are strictly sequential (top-down), with a single-threaded replay of up to `√N` layers before each one and a single-threaded fold after it. With `--refine end:4` on the two paintings, the refit time is the same at 4, 8 and 10 threads (triangle at 200 shapes: 0.49 / 0.52 / 0.52 s; rotated ellipse: 0.85 / 0.88 / 0.88 s) while the greedy part scales 1.5–1.7× from 4 to 10 threads, with identical output. In today's pipeline the passes are 45–70% of the time for five kinds, so on this machine half or more of the wall time runs on 4 of 10 cores. **More climbs per layer is not the fix (measured after the retune, with a lab override of the pass's effort).** 8 or 10 climbs of age 25 change the quality by ±0.9 points with no direction, and raise the refit time by 10–50% at 10 threads and by 50–90% at one thread. 8 climbs of age 13, the same evaluations spread wider, cut the refit time by 10–35% at 10 threads but lose 0.2–2.6 points, most on circles and rotated ellipses, whose polish needs the roughly 16 rejected moves that bring a climb to its finest step. That 8 climbs cost more even on 10 cores says the per-layer time is not the climbs: it is the serial work around them, a 196 KB checkpoint copy plus a replay of up to `√N` layers before every layer, a fresh `WorkerCtx` per rayon task per layer, and the fork and join. Removing that overhead is exact (bit-identical output): replay each checkpoint segment once and keep the canvas below each of its layers, as B's `diff.rs` already does per band, and reuse one worker per climb across the layers of a pass. The ambitious options stay on the list for later: refitting layers whose footprints are disjoint in parallel, or a Jacobi-style pass verified on the exact canvas.
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

## 10. Follow-up: B during the search (2026-10-08)

The first item of section 8's list, measured. The engine runner gained a lab-only schedule, `--during joint:K:C:I`: on the `Spaced(K, C)` schedule, in place of a refit pass, the joint optimisation (B) runs on the model's shapes with `I` Adam iterations and its result is adopted into the model (`Model::adopt`, lab only), so later greedy steps build on it. A result is kept only if the exact engine canvas, repainted with the adopted shapes, scores strictly lower, the refit pass's own rule; `joint-export:K:C:I` keeps it if the model's PNG export after adopting is closer to the target instead. Adopting converts B's output back to the engine's shapes: triangles and polygons exactly, as continuous polygons; axis-aligned rectangles rounded from B's half-pixel edges to integer pixel bounds; rotated rectangles to an integer centre, sides and angle; every other kind keeps its geometry and takes B's colours. The final stage of each kind is unchanged.

Runs on the five kinds B moves, default corpus and checkpoints, all at `aa8bfd5` (the retuned pipeline of `d85a5b1`), on the same idle machine:

| run | flags |
| --- | --- |
| `base5` | `--refine final` (today's pipeline: refit passes `Spaced(20, 5)` or `Spaced(20, 2)`, then the final stage) |
| `nodur5` | `--refine final --during none` (no passes during the search) |
| `bdur-20-5-i10` / `i20` / `i40` | `--refine final --during joint:20:5:I` |
| `bdur-10-10-i20` | `--refine final --during joint:10:10:20` |
| `bdur-20-5-i20-x` | `--refine final --during joint-export:20:5:20` |

Mean of the 100- and 200-shape median rmse256 against `base5`, and the time ratio over the same rows:

| kind | `nodur5` | `joint:20:5:10` | `joint:20:5:20` | `joint:20:5:40` | `joint:10:10:20` | `joint-export:20:5:20` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| any | +1.4% · 0.73× | +1.4% · 0.74× | +0.9% · 0.76× | +0.5% · 0.85× | +1.4% · 0.83× | +0.9% · 0.78× |
| triangle | +2.6% · 0.55× | +1.2% · 0.64× | **−1.3% · 0.70×** | −1.4% · 0.87× | −1.4% · 0.90× | −1.3% · 0.74× |
| rectangle | +3.7% · 0.49× | +3.0% · 0.58× | +2.5% · 0.65× | +1.8% · 0.79× | +2.3% · 0.83× | +2.5% · 0.67× |
| rotated-rectangle | +1.8% · 0.78× | +1.7% · 0.88× | +0.9% · 0.97× | +1.0% · 1.11× | +1.1% · 1.14× | +1.6% · 0.98× |
| polygon | +2.8% · 0.71× | +0.8% · 0.75× | −0.2% · 0.76× | −0.0% · 0.84× | −0.5% · 0.84× | −0.2% · 0.78× |

The `adopted` column of the runner (passes kept over passes run, up to the checkpoint) explains the kinds:

- **Triangles and polygons adopt almost every pass** (8/8 at 200 shapes on every image but synthetic-shapes, 1–3/8 there; 12–13/13 at 500). The conversion is exact, so B's gain carries over to the canvas.
- **Rectangles adopt 1/8 and 3/8 on the two paintings** (7–8/8 on the gradient and the texture), **rotated rectangles 2–4/8 on the paintings and 0/8 on the texture.** Rounding B's half-pixel edges, or its angle to the degree, gives back most of what B gained, and the exact canvas then rejects the result. The export guard keeps the same counts on rectangles, so the rounding, not the guard, is the limit: these kinds would need a continuous representation in the engine before B can run during their search.
- **`any` adopts 8/8 on the paintings**, but B moves only its triangles, polygons and rectangles, while the refit passes it replaces also serve the curved layers.

Per image and shape count, against `base5`, for triangles with `joint:20:5:20`: both paintings improve at 50, 100 and 200 shapes (American Gothic −2.1 / −1.4 / −2.4%, Mona Lisa −1.1 / −1.0 / −1.6%) and are within ±1.4% at 500; the gradient and the texture are within ±1.3%; synthetic-shapes, the hard-edged image on which the guard rejects B (section 6), is +6.1% at 200 and +3.5% at 500 on a median rmse256 of 0.0065. With 40 iterations the paintings gain about the same for 0.87× the time, and `joint:10:10:20` gains the most on the paintings (−3.8% and −2.2% at 200) at 0.90×, with +10% on synthetic-shapes: 20 iterations at `Spaced(20, 5)` is the knee. At equal time against the greedy curve of section 4, triangles at 200 shapes move from a ratio of 1.019 (`base5`) to 0.936; `nodur5`, with no passes at all, stays the most time-efficient at 0.898, as every stage during the search costs time, but the product's use case is a fixed shape count.

For polygons the mean hides a split: the paintings are within ±1.8% and the gradient improves, while synthetic-shapes regresses by 33–41% at 100–500 shapes, exactly as `nodur5` does (+48–53%). On flat hard-edged shapes the refit passes are what polygons need, and B, rejected by the guard there (the near-collinear coverage of section 8), cannot replace them. For `any`, the paintings lose 2.2% (Mona Lisa, 200) and 2.6% (American Gothic, 500).

Verdicts:

- **Triangles: B during the search replaces the refit passes** in `approximate`'s pipeline: `Spaced(20, 5)`, 20 iterations, the canvas guard; 1.3% below the retuned pipeline at 0.70× its time, and every step of the search stays independent of the thread count (the adoption repaints sequentially and B's result is thread-independent; its conversion is an exact copy, so native and WebAssembly agree).
- **Polygons: deferred** until B's near-collinear coverage is fixed (section 8's third item), which should let B keep what the refit passes do on hard edges; then the same schedule is worth re-measuring, since it already saves 24% of the time on the paintings.
- **Rectangles, rotated rectangles, `any`: negative.** The refit passes stay.

Reproduction, with the runner at or after this change:

```bash
E=target/release/examples/engine
K=any,triangle,rectangle,rotated-rectangle,polygon
$E --shapes $K --refine final > base5.md
$E --shapes $K --refine final --during none > nodur5.md
for i in 10 20 40; do $E --shapes $K --refine final --during joint:20:5:$i > bdur-20-5-i$i.md; done
$E --shapes $K --refine final --during joint:10:10:20 > bdur-10-10-i20.md
$E --shapes $K --refine final --during joint-export:20:5:20 > bdur-20-5-i20-x.md
```

Each run took 104–135 s.

Triangles moved to this schedule in `approximate` at `8e77bb2` (`During::Joint`, `Guard::Canvas`, `Model::adopt`; the rotated-rectangle conversion and the export guard stay lab-only). Regenerated on the same machine with the README's runners: the quality runner's triangle rows at 200 steps went from 1.21 s to 0.89 s (American Gothic) and from 0.95 s to 0.81 s (Mona Lisa), with the export's RMSE 0.7% and 0.2% lower; `versus_go` (alpha 128, not auto) has triangles at 3.96× Go's speed at 200 steps (was 3.4×) and 4.01× at 1000 (was 3.6×), with the RMSE against the resized original 12.30 (was 12.32) and 9.53 (was 9.45): at 1000 shapes with a fixed alpha the quality is 0.8% worse for 10% less time, a regime the engine runner, which stops at 500 shapes with alpha auto, did not cover. The geometric-mean speedup over Go moves from 3.26× / 3.75× to 3.30× / 3.78×, the RMSE ratio stays at 0.811 / 0.742, and every other kind's row is unchanged to the last digit, as the pipeline change is confined to triangles.

## 11. Follow-up: exact pixel coverage at B's vertices (2026-10-08)

The third item of section 8's list, done before the second at the user's choice, since item 1 left polygons waiting on it. B's forward model (`joint/diff.rs`) covered a pixel by the product of its edges' box-filtered half-planes, which is the exact area only where one edge cuts the pixel; near a vertex it gave about half the true area at an angle near 180° and far more than it at an acute one (+45% of a 2 px triangle's area). The model now takes the exact area of the polygon inside the pixel wherever two or more edges cut it, by clipping the pixel square to the cutting edges, with the exact gradient from the first variation of the area; single-edge pixels, the vast majority, keep their formula and their bits. Axis-aligned rectangles were exact already and are bit-identical. The clip runs on 1–6% of the edge pixels.

Measured on the same corpus with the previous binary (`b1f82a4`) against the new one, `--refine final`, five kinds, and with `--during joint:20:5:20` for the four kinds that did not move to it in section 10. Rectangles are identical to the last digit. Change of the median rmse256 and time ratio at 50 / 100 / 200 / 500 shapes:

| kind | Δ median rmse256 | time |
| --- | --- | --- |
| any | −0.1 / −0.1 / −0.3 / −0.7% | 0.98–1.00× |
| triangle | +0.6 / 0.0 / −0.3 / −0.2% | 1.02–1.04× |
| rotated-rectangle | 0.0 / −0.1 / +0.3 / +0.7% | 0.92–0.96× |
| polygon | +0.1 / +0.3 / −0.6 / −2.2% | 0.98–0.99× |

On the paintings polygons gain 0.5–2.2% from 200 shapes and `any` is within ±1%; the gains of `any` on synthetic-shapes (−2 to −8%) and the triangle losses there (+10% and +5% at 50 and 100 shapes, on a median rmse256 of 0.007–0.010, −3% and −1% from 200) are the hard-edged image moving with the model. The runner's new `final_joint` column says what the guard did: with the exact model it keeps B's result on every painting row and rejects it only on synthetic-shapes for polygons (every count) and rectangles (500), and once on synthetic-texture for `any` (500).

**What the guard still rejects is not the coverage.** On the 24 × 24 hard-edged stack of the guard's own test, B's result exports 3.6 times worse than its input with the exact model (4.0 with the product), and already 1.7 times worse with zero Adam iterations, that is after the projection, the snap to the quarter-pixel lattice and the colour refit alone; the first Adam iteration, a 1 px step, is worse still before the later ones recover. The rejections on synthetic-shapes come from the snap and the first steps on finely fitted small shapes, not from the model's disagreement with the export. Consistently, polygons with B during the search (`bdur4-new` against `bdur4-old`) gain 0.4–1.9% on the paintings but keep the synthetic-shapes regression of section 10 (+3 to +4% against the refit passes' schedule at 200 and 500, after the +33–41% that schedule already loses there), so **polygons stay on the refit passes**. A snap that checks each vertex against the loss, or a first step scaled to the shape's size, is the next lever for B on hard edges; it is a small runner experiment.

Verdict: the exact coverage stays. It is the correct model, costs nothing, gains a little at high counts, and the tests now hold the model to the exact pixel area within `1e-9` at every size instead of the product's 5% bounds.

Reproduction: `target/abl/run_exact.sh` (the previous binary kept as `target/abl/engine-b1f82a4`), `analyse_exact.py`.

## 12. Follow-up: B's first steps (2026-10-08)

Section 11 traced the guard's remaining rejections to the start of the optimisation: Adam's first update is a full step of `LR_VERTEX`, 1 px, on every coordinate whatever the gradient (the bias correction makes the first update `±1`), which wrecks a small, well-placed shape before the decayed iterations recover. Three lab knobs on `joint::Settings` (`Tuning`: a warm-up of the step factor, `min(1, (t + 1) / (warmup + 1))` times the decay; the first vertex step in px; a step relative to each shape's size, `min(step, f · √area)`) and the runner flags `--joint-warmup`, `--joint-step`, `--joint-step-rel` measured them, with the defaults bit-identical to before. On the guard test's 24 × 24 polygon stack the ratio of B's export error to its input's falls from 3.61 (default) to 0.79 with a warm-up of 10, 0.40 with a first step of 0.25 px and 3.16 with a relative step of 1/8.

On the corpus, against `exact5` (same code, default tuning), `--refine final`, five kinds. Columns: mean of the 100/200 medians over all images, mean over the two paintings at 100–500 shapes, time ratio:

| kind | warm-up 5 | warm-up 10 | warm-up 20 | step 0.5 px | step 0.25 px | relative 1/8 | relative 1/16 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| any | −0.1 / −0.5% · 1.00× | −0.2 / −0.7% · 1.01× | −0.1 / −0.6% · 1.02× | 0.0 / −0.3% · 1.02× | +0.2 / −0.4% · 1.02× | 0.0 / −0.1% · 1.00× | +0.1 / −0.1% · 1.01× |
| triangle | −0.2 / −0.7% · 0.98× | +1.2 / −0.4% · 1.01× | +0.9 / 0.0% · 1.04× | +1.3 / +0.7% · 1.01× | +1.7 / +1.5% · 1.03× | +0.1 / −0.4% · 1.00× | −0.2 / +0.6% · 1.01× |
| rectangle | +0.2 / −0.4% · 1.00× | +0.3 / −0.4% · 1.04× | +0.5 / −0.3% · 1.02× | +0.6 / −0.2% · 1.01× | +0.8 / +0.2% · 1.02× | −0.1 / 0.0% · 1.00× | +0.1 / +0.1% · 1.01× |
| rotated-rectangle | −1.2 / −3.0% · 1.00× | −1.0 / −2.5% · 1.06× | −1.4 / −2.7% · 1.05× | −0.5 / −1.7% · 1.01× | −0.1 / −1.4% · 1.02× | −0.3 / −0.5% · 1.02× | −0.3 / −0.7% · 1.01× |
| polygon | −0.4 / −0.9% · 1.00× | −0.3 / −1.0% · 1.03× | −0.2 / −0.8% · 1.02× | −0.1 / −0.4% · 1.02× | +0.2 / +0.1% · 1.02× | −0.1 / −0.1% · 1.00× | 0.0 / −0.1% · 1.02× |

A warm-up of five iterations is the one tuning that never loses: on the paintings it gains on every kind, most on rotated rectangles (−4.2% and −2.4% at 200 shapes, −4.8% and −4.6% at 500) and on triangles at 500 (−2.7% and −1.2%), at no cost in time, and it cuts the hard-edged image's error by 20–37% for triangles and rotated rectangles at 50–100 shapes. Longer warm-ups gain the same on the paintings but lose on triangles over the whole corpus; a smaller absolute first step loses on triangles, whose large shapes need the full step; a relative step is neutral. **Warm-up 5 is now the default of every joint optimisation**, final stage and passes during the search alike (the lab knobs stay for later experiments). The guard still rejects B's result for polygons on synthetic-shapes at 100–500 shapes (and for rectangles at 500); with warm-up 20 only at 100 and 500. What remains there is the snap to the quarter-pixel lattice on finely fitted small shapes, which a smaller first step cannot undo.

**Polygons with B during the search, re-measured with the warm-up** (`bdur4-wu5` against the refit schedule with the same warm-up): the paintings are within −1.4% to +1.9% and synthetic-shapes still loses 11–34% at 0.86–0.93× the time; rotated rectangles lose 1–3% on the paintings at 1.07–1.20×. Both kinds stay on the refit passes. Against B during the search without the warm-up, polygons gain 7–10% on synthetic-shapes and 1–3% on American Gothic at 200–500, so the warm-up helps the in-search passes too; it is not enough on hard edges.

Reproduction: `target/abl/run_steps.sh` and `analyse_steps.py`, then
`engine --shapes any,rectangle,rotated-rectangle,polygon --refine final --during joint:20:5:20 --joint-warmup 5`.

## 13. Follow-up: B moves ellipses, circles and rotated ellipses (2026-10-08)

The second item of section 8's list. B's forward model gained three outlines, a circle `(c, r)`, an axis-aligned ellipse `(c, rx, ry)` and a rotated ellipse `(c, a, b)` with a semi-axis vector `a` in place of an angle, so that no trigonometry enters the loop (as the rotated rectangle's `c, u, h`). A pixel's coverage takes the boundary as locally straight: in the ellipse's normalised frame, with `r = |q|`, the signed distance is `d = (1 − r) / |∇r|` and the coverage is the edge CDF of `d` with the filter square projected on the normal; the gradient is the exact derivative of that model (central differences agree to 3e-7). Against a 64 × 64 supersampled reference the mean error on boundary pixels is 0.047 below 2 px of radius, 0.018 from 2 to 8 px and 0.0044 above, the total area within 0.63% from 4 px; the one systematic error is a small coverage on pixels just outside a thin ellipse, up to 0.22 below 2 px of minor radius, where the straight half-plane reaches past the curve. Projection keeps every radius at least 1 px, the snap puts centre and radii on the quarter-pixel lattice, a rotated ellipse's exported rotation comes from a trig-free `atan2` of the snapped axis vector, and each kind keeps its contract (a circle stays a circle). A switch, `joint::Settings::curved` and the runner's `--joint-curved`, is off by default, so these runs compare against bit-identical baselines.

Runs on `ellipse,circle,rotated-ellipse,any`, against `--refine final` (today's pipelines: refit passes `Spaced(10, 10)` and one final pass for the three curved kinds; `any`'s B with its curved layers fixed). Change of the median rmse256 at 50 / 100 / 200 / 500 shapes, time ratio over those rows, and the guard's choice at 200 and 500:

| kind | `--joint-curved` with… | Δ median | time | guard kept |
| --- | --- | --- | ---: | --- |
| any | its pipeline (B moves the curved layers) | −1.1 / −0.7 / −0.8 / −1.0% | 1.03–1.06× | 4/5 |
| ellipse | passes, one refit pass, then B | −3.9 / −6.7 / −9.8 / −15.2% | 1.17–1.20× | 5/5, 4/5 |
| ellipse | passes, then B in place of the refit pass | −3.8 / −6.4 / −9.6 / −15.1% | 1.13–1.20× | 5/5, 4/5 |
| ellipse | B alone, no passes | −2.5 / −4.1 / −7.0 / −13.3% | 0.57–0.64× | 5/5 |
| circle | passes, one refit pass, then B | −12.7 / −6.5 / −9.6 / −14.0% | 1.14–1.19× | 5/5 |
| circle | passes, then B in place of the refit pass | −12.7 / −6.6 / −9.7 / −14.0% | 1.15–1.22× | 5/5 |
| circle | B alone, no passes | −8.7 / −3.2 / −5.3 / −12.0% | 0.56–0.66× | 5/5 |
| rotated-ellipse | passes, one refit pass, then B | −2.4 / −2.8 / −2.4 / −2.4% | 1.10–1.12× | 3/5, 1/5 |
| rotated-ellipse | passes, then B in place of the refit pass | −2.2 / −2.9 / −2.3 / −2.2% | 1.09–1.13× | 3/5, 1/5 |
| rotated-ellipse | B alone, no passes | +1.1 / −1.7 / +0.8 / +1.5% | 0.68–0.73× | 4/5, 3/5 |

On the paintings ellipses and circles gain far more than the medians say: American Gothic −13.6% (ellipse) and −7.3% (circle) at 200 shapes, −21.8% and −14.9% at 500, Mona Lisa −9.8% / −9.6% and −15.2% / −14.0%; synthetic-shapes −37 to −57%; the texture gains 3–7% with the passes and loses 2–6% at 100–200 shapes with B alone. `any`'s paintings gain 1.8–5.7% (American Gothic) and 0.7–1.1% (Mona Lisa), synthetic-shapes 22–58%, for 3–6% more time. Rotated ellipses gain 2–3% at 10% more time, and the guard rejects B on two to four of the five images from 100 shapes: their greedy search makes thin ellipses (radii from 1 px, continuous), where the local-straight model's halo is largest and its objective disagrees with the export. A coverage exact on high curvature is the follow-up for them.

The schedule for ellipses and circles: with the passes thinned to `Spaced(20, 5)`, the triangles' schedule, and B in place of the final refit pass, the medians are −3.9 / −6.0 / −8.4 / −14.4% (ellipse) and −11.3 / −4.0 / −8.5 / −13.1% (circle), every painting row better (American Gothic −11.7% and −20.4% with ellipses at 200 and 500 shapes, circles −7.0% and −14.6%), the texture within +1.9 / −3.0%: one to two points less than the full `Spaced(10, 10)` passes with B, one to three more than B alone, at about today's time (this run's timing was disturbed by a parallel build; the regeneration after the change measures it).

Verdicts: **`any` adopts B for its curved layers; ellipses and circles move to `Spaced(20, 5)`, no final refit pass, then B**; **rotated ellipses keep their pipeline** until the thin-ellipse coverage improves. The switch stays for the lab (`--joint-curved off`), on by default.

Reproduction: `target/abl/run_curved.sh`, then `engine --shapes ellipse,circle --refine final --joint-curved --joint-scale 1 --final-refits 0 --during spaced:20:5`.

## 14. Follow-up: a stroke width per quadratic curve (2026-10-08)

The fourth item of section 8's list. A quadratic curve already carried its stroke width per shape (`Quadratic::width`, rasterised by area coverage and exported as the path's `stroke-width`), fixed at 2 px by the search; the engine's workers now carry bounds for it, the search draws a random curve's width uniformly within them and moves it like a coordinate (a fourth move, σ 1 px, scaled in the refit climbs), and a lab hook with the runner flag `--quadratic-width MIN:MAX` sets them. With the default, a fixed 2 px, the random streams and the greedy digests are unchanged, so the baseline is bit-identical.

Runs on `quadratic,any`, `--refine final`, against the fixed 2 px. Change of the median rmse256 at 50 / 100 / 200 / 500 shapes, the two paintings at 200, the time and the SVG size ratios:

| bounds | quadratic, Δ median | paintings at 200 | time | SVG |
| --- | --- | --- | ---: | ---: |
| 3 px, fixed | −7.9 / −14.2 / −43.8 / −26.3% | −26.7 / −27.1% | 1.10–1.15× | 1.01–1.03× |
| 1.5–4 px | −15.3 / −26.8 / −54.1 / −46.6% | −40.1 / −40.3% | 1.08–1.19× | 0.99–1.02× |
| 1.5–6 px | −28.3 / −59.8 / −68.0 / −49.3% | −52.4 / −49.9% | 1.10–1.38× | 1.03–1.04× |
| 1.5–8 px | −40.0 / −64.6 / −72.4 / −50.4% | −56.7 / −52.6% | 1.14–1.52× | 1.04× |

Every image gains at every count with every range; the texture least (−6 to −28%), the hard-edged synthetic image most (−44 to −93%). The width is the lever the review expected (section 8, "Quadratics"): quadratics were 2–3 times worse than every other kind, and a range of widths halves their error on the paintings at 100–200 shapes. Wider bounds gain with diminishing returns and cost time, since a wider stroke covers more pixels per candidate: 1.5–6 px takes 10–38% more time than the fixed 2 px, 1.5–8 px up to 52%. `any`, whose quadratics are one shape in eight, moves by ±1.6% with any range.

The minimum: the same range from 2 px (−28.3 / −59.6 / −66.2 / −48.7%, paintings −52.3 / −49.6% at 200, 1.39–1.11× the time) gains as much as from 1.5 px and keeps the stroke's centre line fully covered, the property the rasteriser's tests hold. **Quadratic curves now choose their width between 2 and 6 px**: the look changes, from a pen of one width to strokes of several, which is the trade the plan accepted for this item; 8 px would gain 5–6 more points on the paintings at 200 shapes for up to 1.52× the time.

Reproduction: `target/abl/run_qwidth.sh`, `analyse_qwidth.py`, then `engine --shapes quadratic,any --refine final --quadratic-width 2:6`.

## 15. Follow-up: round caps on quadratic curves (2026-10-09)

Experiment 4b of section 14's item. A quadratic curve's stroke ended in butt caps, cut flat across the curve at its end points, in the engine's coverage, in the SVG (the default of `stroke-linecap`) and in the PNG. The engine's workers now carry the cap (`LineCap`, butt, round or square), the rasteriser covers a pixel past a round end by its share within half the width of the end point (the band across the pixel centre's direction from that point, so the disc is locally straight, as the curve's body is) and lengthens the end segments by half the width for a square one, the SVG writer puts `stroke-linecap` once on the root element, which every path inherits, and the export raster strokes with the same cap. Against a stroke supersampled 16 × 16 over seven curves 2–6 px wide, the worst pixel near an end differs by 0.12 with round caps, 0.14 with square ones and 0.19 with butt ones, the sum over the stroke by 1.6%, 1.7% and 1.8% of the reference's; against the export's coverage, round and square caps agree at least as well as butt. A lab hook and the runner flag `--quadratic-cap` set the cap; the butt runs reproduce section 14's bit for bit.

Runs on `quadratic,any`, `--refine final`, at the fixed 2 px and at the 2–6 px of section 14, against butt caps at the same width. Change of the median rmse256 at 50 / 100 / 200 / 500 shapes, the two paintings at 500, the time and the SVG size ratios:

| cap | width | quadratic, Δ median | paintings at 500 | time | SVG |
| --- | --- | --- | --- | ---: | ---: |
| round | 2 px | +0.4 / −0.1 / −1.0 / −1.5% | −1.5 / −2.3% | 0.99–1.00× | 1.00× |
| square | 2 px | +0.3 / −0.1 / −0.6 / −1.0% | −1.0 / −1.4% | 1.01–1.02× | 1.00–1.01× |
| round | 2–6 px | −0.2 / +0.3 / −3.0 / −5.0% | −4.3 / −5.0% | 0.98–1.00× | 1.00× |
| square | 2–6 px | −1.0 / +1.4 / −7.1 / −4.1% | −2.0 / −4.1% | 1.00–1.03× | 1.00–1.01× |

The caps are a small lever next to the width: they gain where strokes are wide and many, up to 5% on the paintings at 500 shapes, and cost nothing, since a cap covers a few pixels per candidate and the SVG carries one attribute. Square caps gain as much as round ones on the medians, driven by the synthetic gradient, less on the paintings, and lengthen wide strokes into blocks; round ones end them as a brush would. `any`, whose quadratics are one shape in eight, moves within the noise of its hard-edged synthetic image (±28% there, ±3% elsewhere). **Quadratic curves now end in round caps**, with the 2–6 px width of section 14: the root `<svg>` element of every document carries `stroke-linecap="round"`, whatever the shape kind, so that a live preview wrapped in the root of any render shows the curves as the output does.

Reproduction: `target/abl/run_caps.sh`, `analyse_caps.py`, `montage.mjs` (both paintings, three caps, two widths, side by side with a 3× crop).
