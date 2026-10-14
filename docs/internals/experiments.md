# Experiments

Experiments that were measured and not adopted, or parked, with their numbers, oldest first. What was adopted is in [`tuning.md`](tuning.md) and [`adr/`](adr/README.md); this file keeps what was tried around it, so that nobody repeats a measurement without knowing its result.

The log is append-only. An entry is never edited after it is written; an experiment that is retried gets a new entry, which links the earlier one. Unless an entry says otherwise, the measurements follow [ADR 0006](adr/0006-how-engine-changes-are-judged.md) and the conventions at the top of [`tuning.md`](tuning.md#how-these-numbers-are-measured): the engine runner's default corpus, seed 42, an Apple M2 Pro at 10 threads, quality as the change of the mean of the 100- and 200-shape median RMSEs of the export.

Each entry gives the date, what was tried, how it was measured, the numbers, the verdict (dropped or parked), and what would make it worth retrying.

## 2026-10-07: Doubled search effort for `any` and rotated ellipses

- **Tried.** 32 search rounds per step for `any` and rotated ellipses, and twice the climb age for rotated ellipses, as the pipeline first shipped them (`16877bb`). Polygons had already returned from 32 to 16 rounds while the pipeline was chosen.
- **Measured.** The full pipeline with this effort against the same pipeline at 16 rounds and the base age.
- **Numbers.** `any`: 0.8% for 19% more time (−10.8% at 2.78 times the previous `approximate`, against −10.0% at 2.34 times). Rotated ellipses: 2.2% for 90% more time (−9.2% at 3.03 times, against −7.0% at 1.59 times); at equal time and 100–200 shapes the base effort was better than the plain greedy search (ratios 0.96–0.98), the doubled one worse (1.19–1.25).
- **Verdict.** Dropped in `d85a5b1`. The quadratics' doubled age stayed: 2.6% for 57% more time, with no cheaper lever at the time.
- **Retry if** quality at a fixed count were again worth any time; more seeds per round give a better return ([2026-10-11](#2026-10-11-more-seeds-per-search-round)).

## 2026-10-07: Dense refit schedules during the search

- **Tried.** The schedules the pipeline first shipped: `Spaced(5, 10)` for `any` and triangles (25 passes and 9.8 layer refits per step up to 200 shapes) and `Spaced(5, 20)` for ellipses (35 passes, 16 layers per step), against `Spaced(20, 5)` (8 passes, 3.7 layers per step) and `Spaced(10, 10)` (17 passes, 7.9 layers per step) for every kind, at the base search effort.
- **Numbers.** Against the previous `approximate`: triangles −14.8% at 3.21 times with `Spaced(5, 10)`, −12.9% at 2.06 times with `Spaced(20, 5)`, which keeps 55% of the passes' gain for 45% of their cost; `any` −10.0% at 2.30 times against −6.9% at 1.57 times; ellipses −7.2% at 3.05 times with `Spaced(5, 20)`, −5.1% at 2.15 times with `Spaced(10, 10)`. Equal-time ratios at 200 shapes: 1.07–1.30 for the dense schedules, 1.01–1.19 for the thin ones. The one row worse than the baseline by more than 2% with the dense schedule: ellipses on synthetic-shapes at 500 shapes, +19.5%, from passes that lower the engine's binary-coverage score while raising the anti-aliased export's error.
- **Verdict.** Dropped in `d85a5b1`; the last 2–3 points of the dense schedules cost 0.7–1.2 times the baseline's whole time. Triangles later moved to joint passes (`8e77bb2`), ellipses to `Spaced(20, 5)` with the joint optimisation (`53172eb`).
- **Retry if** time at a fixed shape count stopped mattering.

## 2026-10-07: The previous engine as a fast preset

- **Tried.** Considered, not built: keeping the engine before the pipeline (16 rounds, then one refit pass) as a public fast preset.
- **Numbers.** The joint optimisation as the final stage alone takes 0.97–1.11 times the old `approximate`'s time for triangles, rectangles and polygons and is neutral or better at equal time; for the other kinds the old `approximate` is what the pipeline runs without its passes during the search.
- **Verdict.** Dropped: not needed, and it would add a public option, which `AGENTS.md` rules out unless consistency requires it.
- **Retry if** render time at any shape count became a stated use case.

## 2026-10-07: More climbs per refit layer

- **Tried.** A lab override of the refit pass's effort (`646252c`, `--refine-effort`): 8 or 10 climbs of age 25, and 8 climbs of age 13, the same evaluations spread wider, against 4 climbs of age 25.
- **Numbers.** 8 or 10 climbs of age 25 changed quality by ±0.9 points with no direction, and raised the refit time by 10–50% at 10 threads and 50–90% at one. 8 climbs of age 13 cut the refit time by 10–35% at 10 threads and lost 0.2–2.6 points, most on circles and rotated ellipses, whose polish needs the roughly 16 rejected moves that bring a climb to its finest step.
- **Verdict.** Dropped. That more climbs cost more even on 10 cores showed the per-layer time was serial work around the climbs ([2026-10-10](#2026-10-10-speculative-refit-layers)).
- **Retry if** the climbs' imbalance, not their number, stopped being the limit.

## 2026-10-07: Search rounds and thread waves

- **Tried.** The greedy search alone on the two paintings with 10, 16 and 20 rounds per step (`any`, triangles, rotated ellipses), at 10 threads: 16 rounds on 10 threads are two waves, the second with 6 idle threads.
- **Numbers.** Time per round at 10 / 16 / 20 rounds: 1.25 / 1.00 / 0.93 (triangles), 1.16 / 1.00 / 0.91 (`any`), 1.17 / 1.00 / 0.98 (rotated ellipses). 20 rounds, two full waves, cost 17–24% more time than 16 for 25% more work and 0.3–1.0% lower RMSE; 10 rounds save 22–28% and lose 1–2%. Work stealing and the uneven climb lengths already absorb most of the wave effect, so the gain left is at most 10–15%.
- **Verdict.** Parked. The rounds cannot follow the thread count without breaking the seed contract ([ADR 0002](adr/0002-same-output-on-every-thread-count-and-platform.md)).
- **Retry if** someone splits each round's random phase into two tasks, which fills the second wave without changing any result.

## 2026-10-07: Cheaper ends in the quadratic stroke rasterizer

- **Tried.** Not built. The stroke rasterizer covers a pixel with two trapezoid primitives and, near the ends, a distance with a square root; at 2 px it took 284 µs a shape against 90 µs for the earlier 1 px stroke. The inner band of a long segment is already special-cased; the ends could be clipped by a cheaper test first.
- **Verdict.** Parked; still open when the speed work closed on 2026-10-13.
- **Retry if** quadratic time matters again.

## 2026-10-07: Smaller bands in the joint optimisation

- **Tried.** Not built. The joint passes run one rayon task per band of 16 rows: 16 tasks at 256 px on 10 threads, and bands with more layers take longer. Bands of 8 rows might balance better, at the cost of more checkpoints.
- **Verdict.** Parked, worth one measurement; still open when the speed work closed on 2026-10-13.
- **Retry if** the joint optimisation's share of the time grows.

## 2026-10-08: Joint passes during the search for the other kinds

- **Tried.** The lab schedule `--during joint:K:C:I` (`04d1406`): on the `Spaced(K, C)` schedule, the joint optimisation with `I` iterations in place of a refit pass, its result adopted into the model (`Model::adopt`) when the exact canvas scores lower; `joint-export:K:C:I` kept it when the PNG export was closer instead. Converting back: triangles and polygons exactly; rectangles rounded to integer bounds; rotated rectangles to an integer centre, sides and angle; other kinds keep their geometry and take the colours.
- **Measured.** At `aa8bfd5` on `any`, triangles, rectangles, rotated rectangles and polygons, against `--refine final`; 104–135 s per run.
- **Numbers.** With `joint:20:5:20`, against the refit passes: triangles −1.3% at 0.70 of the time (adopted, see [`tuning.md`](tuning.md#passes-during-the-search)); `any` +0.9% at 0.76; rectangles +2.5% at 0.65; rotated rectangles +0.9% at 0.97; polygons −0.2% at 0.76, but +33–41% on synthetic-shapes at 100–500 shapes. Rectangles adopted 1 and 3 of 8 passes on the paintings, rotated rectangles 2–4 of 8 and none on the texture: rounding gives back most of the gain; the export guard kept the same counts on rectangles. `any`'s paintings lost 2.2% (Mona Lisa, 200 shapes) and 2.6% (American Gothic, 500). 10 or 40 iterations and `Spaced(10, 10)` did not change the verdict.
- **Verdict.** Dropped for rectangles, rotated rectangles and `any`; deferred for polygons until the joint coverage at vertices was exact.
- **Retry if** rectangles got a continuous representation in the engine, or the joint optimisation moved quadratics (for `any`).

## 2026-10-08: Polygons with joint passes, after the exact vertex coverage

- **Tried.** The polygon run of [the previous entry](#2026-10-08-joint-passes-during-the-search-for-the-other-kinds) again after `0298d9c`, which covers a pixel cut by two or more edges by the polygon's exact area.
- **Numbers.** The paintings gained 0.4–1.9% over the same schedule with the old coverage, and the regression on synthetic-shapes stayed. On the guard test's 24 × 24 stack the joint result exported 3.6 times worse than its input with the exact coverage (4.0 with the product of half-planes), and 1.7 times worse with zero Adam iterations: the projection, the snap to the quarter-pixel lattice and the colour refit alone, then the 1 px first steps.
- **Verdict.** Polygons stay on refit passes. The exact coverage itself was kept: neutral to −2.2% at 500 shapes, 0.92–1.04 of the time.
- **Retry if** the snap and the first steps stop wrecking small, finely fitted shapes on hard edges.

## 2026-10-08: Other first steps for the joint optimisation

- **Tried.** Lab knobs on `joint::Tuning` (`945ccec`): warm-ups of 5, 10 and 20 iterations, first vertex steps of 0.5 and 0.25 px, and a step relative to each shape's size, `min(step, f · √area)` with `f` of 1/8 and 1/16.
- **Numbers.** On the guard test's stack the ratio of the export error to the input's fell from 3.61 to 0.79 with a warm-up of 10, 0.40 with a first step of 0.25 px and 3.16 with a relative step of 1/8. On the corpus, over all images / on the paintings at 100–500 shapes, triangles: warm-up 10 +1.2 / −0.4%, warm-up 20 +0.9 / 0.0%, step 0.5 px +1.3 / +0.7%, step 0.25 px +1.7 / +1.5%, relative 1/8 +0.1 / −0.4%, relative 1/16 −0.2 / +0.6%; every variant within 1.00–1.06 of the time.
- **Verdict.** A warm-up of 5 was adopted ([`tuning.md`](tuning.md#joint-optimisation)); the others were dropped, and the knobs stay for the lab. The guard still rejects polygons on synthetic-shapes at 100–500 shapes.
- **Retry if** a snap that checks each vertex against the loss is built ([its entry](#2026-10-08-a-snap-that-checks-each-vertex-against-the-loss)).

## 2026-10-08: Polygons and rotated rectangles with joint passes, with the warm-up

- **Tried.** `--during joint:20:5:20 --joint-warmup 5` for polygons and rotated rectangles, against their refit schedules with the same warm-up.
- **Numbers.** Polygons: the paintings within −1.4% to +1.9%, synthetic-shapes 11–34% worse, at 0.86–0.93 of the time; 7–10% better on synthetic-shapes than joint passes without the warm-up. Rotated rectangles: 1–3% worse on the paintings at 1.07–1.20 times.
- **Verdict.** Dropped; both kinds stay on refit passes.
- **Retry if** the snap on hard edges is fixed.

## 2026-10-08: A snap that checks each vertex against the loss

- **Tried.** Not built. The joint optimisation's remaining rejections on hard edges come from the snap to the quarter-pixel lattice on finely fitted small shapes, which neither a smaller first step nor a warm-up undoes.
- **Verdict.** Parked: a small runner experiment, a snap that keeps each vertex's rounding only if it does not raise the loss.
- **Retry if** polygons or rotated rectangles are to run joint passes during the search.

## 2026-10-08: Rotated ellipses in the joint optimisation

- **Tried.** The joint optimisation moving rotated ellipses (`--joint-curved`, `53172eb`), with their coverage taken as locally straight in each pixel, after their refit passes with and without the final refit pass, and alone.
- **Numbers.** With the passes and the refit pass: −2.4 / −2.8 / −2.4 / −2.4% at 50 / 100 / 200 / 500 shapes, at 1.10–1.12 times; the guard kept the joint result on 3 of 5 images at 200 shapes and 1 of 5 at 500. Alone: +1.1 / −1.7 / +0.8 / +1.5% at 0.68–0.73 times. Their greedy search makes thin ellipses, radii from 1 px, where the straight boundary gives pixels just outside the curve a coverage of up to 0.22 below 2 px of minor radius, so the objective disagrees with the export.
- **Verdict.** Parked: rotated ellipses keep one final refit pass. Open in the [audit plan](plans/2026-10-audit-and-refactor-plan.md) ("Joint optimisation for rotated ellipses").
- **Retry if** a coverage exact at high curvature exists.

## 2026-10-08: Other schedules for ellipses and circles with the joint optimisation

- **Tried.** With the joint optimisation ending the pipeline: the old passes `Spaced(10, 10)` with and without the final refit pass before it, and no passes at all; the adopted `Spaced(20, 5)` is in [`tuning.md`](tuning.md#passes-during-the-search).
- **Numbers.** Against the old pipeline, at 50 / 100 / 200 / 500 shapes: ellipses −3.9 / −6.7 / −9.8 / −15.2% at 1.17–1.20 times with the refit pass, −3.8 / −6.4 / −9.6 / −15.1% at 1.13–1.20 without it, −2.5 / −4.1 / −7.0 / −13.3% at 0.57–0.64 with no passes; circles −12.7 / −6.5 / −9.6 / −14.0% at 1.14–1.19, −12.7 / −6.6 / −9.7 / −14.0% at 1.15–1.22, −8.7 / −3.2 / −5.3 / −12.0% at 0.56–0.66. With no passes the texture lost 2–6% at 100–200 shapes.
- **Verdict.** Not adopted: `Spaced(20, 5)` sits between them at about the old time.
- **Retry if** the trade of time for quality changes in either direction.

## 2026-10-08: Other stroke widths for quadratic curves

- **Tried.** The lab hook `--quadratic-width MIN:MAX` (`6803045`): a fixed 3 px, and ranges 1.5–4, 1.5–6, 1.5–8 and 2–6 px, against the fixed 2 px.
- **Numbers.** Median at 50 / 100 / 200 / 500 shapes: 3 px −7.9 / −14.2 / −43.8 / −26.3% at 1.10–1.15 times; 1.5–4 px −15.3 / −26.8 / −54.1 / −46.6% at 1.08–1.19; 1.5–6 px −28.3 / −59.8 / −68.0 / −49.3% at 1.10–1.38; 1.5–8 px −40.0 / −64.6 / −72.4 / −50.4% at 1.14–1.52. 2–6 px gains as much as 1.5–6 px. `any`, whose quadratics are one shape in eight, moved by ±1.6% with any range.
- **Verdict.** 2–6 px adopted ([`tuning.md`](tuning.md#quadratic-strokes)); 8 px would gain 5–6 more points on the paintings at 200 shapes for up to 1.52 times the time and change the look more.
- **Retry if** the maintainer wants broader strokes.

## 2026-10-08: The quadratics' doubled climb age, not re-measured

- **Tried.** Not re-measured. The doubled age (now 200) was kept on 2026-10-07 for 2.6% at 57% more time because quadratics had no cheaper lever; since then their width range cut their error by half or more.
- **Verdict.** Parked: a measurement of age 100 against 200 with the 2–6 px strokes and round caps.
- **Retry if** quadratic render time matters: they are the slowest kind (4.9 s for 100 shapes on a painting at 10 threads, against 1.2 s for `any`, on 2026-10-12).

## 2026-10-09: Square caps on quadratic curves

- **Tried.** The lab hook `--quadratic-cap` (`7c5a6d9`): butt, round and square caps, at 2 px and at 2–6 px.
- **Numbers.** Against butt caps at 2–6 px, median at 50 / 100 / 200 / 500 shapes: round −0.2 / +0.3 / −3.0 / −5.0%, the paintings −4.3 / −5.0% at 500; square −1.0 / +1.4 / −7.1 / −4.1%, the paintings −2.0 / −4.1%, the medians driven by the synthetic gradient. At 2 px: round −1.0% and −1.5% at 200 and 500, square −0.6% and −1.0%. Time and SVG size within 1–3%.
- **Verdict.** Round caps adopted; square caps dropped: less on the paintings, and they lengthen wide strokes into blocks.
- **Retry.** Not on quality alone; the look decides.

## 2026-10-09: A stroke width that varies along one curve (4c)

- **Tried.** Not built. A width that changes along a quadratic curve cannot be an SVG stroke: the curve would be exported as a filled outline, about three times the bytes.
- **Verdict.** Parked by the maintainer. Open in the [audit plan](plans/2026-10-audit-and-refactor-plan.md).
- **Retry if** the bytes become acceptable for the look it gives.

## 2026-10-10: Preparing the next refit run on idle cores

- **Tried.** With the change that replays each checkpoint segment of a refit pass once (`0ead392`, kept: the refit time fell by 3.4% at 10 threads, 6.3% at 4 and 2.1% at 1, output identical), also preparing the next run's canvases on the idle cores.
- **Numbers.** No measurable gain.
- **Verdict.** Dropped. Instrumented, the serial part of a layer is the wait for the longest of its four climbs: their evaluations fill 64–72% of four times the longest (circles 90%), so four threads give about 2.6 times on the climbs.
- **Retry.** Not on its own; see the next entry.

## 2026-10-10: Speculative refit layers

- **Tried.** Refitting a layer against an `Above` that lacks the decision of the layer above it gives the sequential result whenever the pixels its candidates covered miss that layer's decided footprint. Measured first: the condition held for 81% of layers at 200 shapes and 86% at 500 for the layer above. Built as pairs (`rayon::join` of layers `i` and `i − 1`, `i − 1` accepted if its candidates missed `i`'s footprint, else redone), and as a sliding window with two layers always in flight.
- **Numbers.** Every row bit-identical under pools of 1, 2, 4 and 10 threads. Pairs: 38–41% of layers speculated, 80–85% of those accepted, 8–11% of the climbs' evaluations discarded; the refit time fell by 7.2% at 200 shapes and 9.8% at 500 at 10 threads, rose by 1–5% at 4 threads and 17% at 1. On the whole search: 0.4% at 200 shapes, 1.0% at 500. The window gained nothing over the pairs.
- **Verdict.** Dropped: 1,300 lines, a second code path chosen by the thread count and bitmaps in the climbs for about 1% of the time. The patches are kept outside the repository by the maintainer. A Jacobi-style pass verified on the exact canvas, which would change results, was not tried.
- **Retry if** the refit's share of the time grows a lot; fewer evaluations per layer would move it more, and belong with quality measurements.

## 2026-10-11: Round joins at every stroke vertex

- **Tried.** While fixing the quadratic stroke's coverage at joins (`152311a`), round joins at every vertex of the flattened curve.
- **Numbers.** Exact, but 1.6 times the quadratic search. The adopted scheme, round joins only for sharp turns, has a worst pixel error of 0.15 at 1.03–1.05 times.
- **Verdict.** Dropped.
- **Retry if** the adopted scheme's error bound (0.2 per pixel, 2.5% overall, pinned by a test) became too loose; not for speed.

## 2026-10-11: A parallel export guard

- **Tried.** Considered in the review of the merged change: rendering the input and the joint result of the export guard with `rayon::join`.
- **Verdict.** Not taken: it needs `rayon` as a dependency of `primeval-render` for under 1% of the time.
- **Retry if** the guard's share of the time grows, for example at large working sizes.

## 2026-10-11: A second final refit pass

- **Tried.** `--final-refits 2` for `any` and rotated ellipses, and `until:50:6` or two fixed passes for quadratics, against the final stage's refit passes.
- **Numbers.** `any`: −0.2% at 50, 100 and 200 shapes and −0.6% at 500 (per image −0.5 / +0.3 / −0.9 / +0.4%, synthetic-shapes 1.9% worse at 100 and 2.7% at 500), at 1.04–1.08 times. Rotated ellipses: −0.2 to −0.3% at 50–200 and −0.6% at 500 at 1.02–1.03 times. Quadratics: until 0.5% with at most six passes −0.4% at 100–500 at 1.03–1.04 times; two fixed passes +1.8% at 200 against the rule. Times from a busy machine, indicative.
- **Verdict.** Dropped: every change is below the rule's 0.5%. The 1–3% measured before the pipeline was against the greedy search alone; the passes during the search now take most of it.
- **Retry if** the passes during the search are thinned again.

## 2026-10-11: The step-adapted climb in the greedy search

- **Tried.** The plan's "A2": the refit's step-adapted climb (a (1+1) evolution strategy with the 1/5th success rule) in place of the greedy search's coarse moves, at climb ages 100, 50 and 25 (lab hook `Model::set_search`, `dc8b5a3`).
- **Numbers.** Over all 45 rows: +0.2% at age 100, −0.7% at 50, −0.6% at 25, with the per-image mean worse at every age (+1.0%, +1.3%, +2.0%), at 1.06, 0.90 and 0.84 times the time. The rule's winners, circles and ellipses at age 50, were synthetic-shapes alone for circles, and the gradient 2.7% and 7.4% worse at 200 and 500 for ellipses.
- **Verdict.** Dropped, and the `climb` key removed from the hook: from a random candidate the coarse moves find the place, and the polish is what the refit passes and the joint optimisation do anyway.
- **Retry.** Not as is.

## 2026-10-11: More seeds per search round

- **Tried.** Climbing the best 2 or 4 random candidates of each round instead of 1 (`--search seeds=K`), also with 250 candidates and 4 seeds.
- **Numbers.** 2 seeds: −1.1% over all rows (per image −0.4%) at 1.10–1.12 times. 4 seeds: −0.7% (per image −1.1%) at 1.48–1.58 times, with `any` −2.4%, quadratics −2.1%, rotated ellipses −2.4% and rotated rectangles −1.0%, the paintings 1–3% better at 100–500 shapes. 250 candidates and 4 seeds: −1.5% (per image −1.1%) at 1.34–1.48 times.
- **Verdict.** Dropped. On 2026-10-12 the maintainer ruled out four seeds: about 2% of RMSE, invisible in a stylised drawing, for 1.3–1.6 times the time ([ADR 0006](adr/0006-how-engine-changes-are-judged.md)). The lab knob stays.
- **Retry if** quality at a fixed count became worth that time again.

## 2026-10-11: 500 random candidates for every kind

- **Tried.** `--search candidates=500` for every kind, alone and with two seeds.
- **Numbers.** Alone: 8–11% less time at quality within 0.5% on the medians, slightly worse (+0.1% per image, +0.5% on the all-rows median at 500 shapes). With two seeds: −0.4% (per image −0.5%) at about the same time.
- **Verdict.** Not adopted for every kind under the quality-first rule of the day; it became the speed review's lead, and 500 candidates were adopted for circles, rotated ellipses and quadratics on 2026-10-12 ([`tuning.md`](tuning.md#search)).
- **Retry if** for the other kinds, with the worst-row criterion of [ADR 0006](adr/0006-how-engine-changes-are-judged.md).

## 2026-10-12: Summing full-coverage lines before weighting them in the colour fit

- **Tried.** One of the speed review's exact proposals: in the colour fit, sum the lines of full coverage first and weight the partial ones after.
- **Numbers.** Output identical. Nothing measurable on circles and triangles; its constant cost per fit made quadratics 8% and rotated ellipses 3% slower; with that cost skipped it only matched the version without it.
- **Verdict.** Dropped.
- **Retry.** Not as is.

## 2026-10-12: Tail handling of the simd128 kernels

- **Tried.** Quadratics took 1.02 of their time with the simd128 kernels (`135df40`) in single-threaded wasm. Masked tails from 3, 8 or 12 pixels or never, and short lines sent to the scalar kernels.
- **Numbers.** No threshold removed the quadratics' 2%; sending short lines to the scalar kernels cost the other kinds 1–1.5%.
- **Verdict.** Dropped. The 2% most likely comes from the SIMD flag's effect on code outside the kernels, which the speed review had seen on its own.
- **Retry if** a profile pins the 2% on a kernel.

## 2026-10-12: An SSE4.1 kernel path

- **Tried.** Not built, with the AVX2 `line_after` (`559363a`).
- **Verdict.** Dropped: almost every x86_64 CPU since 2013 has AVX2, and CPUs without it run the scalar kernels with the same output.
- **Retry if** a common deployment target lacks AVX2.

## 2026-10-12: Caching the second Gaussian variate

- **Tried.** Not built. Marsaglia's polar method draws two normal variates per accepted pair and the sampler uses one; keeping the second would halve the logarithms.
- **Verdict.** Parked: it changes every random stream, so every seeded drawing and pin, and needs a quality measurement first.
- **Retry if** the sampler shows up in a profile.

## 2026-10-13: Short AVX2 tails in half a block

- **Tried.** `a75b641`: a tail of 3 to 16 pixels takes a masked 48-byte half block instead of a full 96-byte one.
- **Numbers.** Output identical. On GitHub's x86 runners against its parent: 0.997 at one thread (AMD EPYC 9V74), 1.001 on all cores (Intel Xeon Platinum 8573C), within the noise.
- **Verdict.** Dropped and reverted in `8cb4bc8`: a second tail path and stack spills in the full-block loop for nothing.
- **Retry.** Not as is.

## 2026-10-13: The four-slot order in portable Rust

- **Tried.** The refit sums' four-slot order ([ADR 0008](adr/0008-the-refit-sums-in-a-fixed-four-slot-order.md)) written as portable Rust, for the compiler to vectorise.
- **Numbers.** It did not vectorise.
- **Verdict.** Dropped: `refine/sums.rs` implements the order with NEON, SSE2 and simd128 behind one generic loop (`b155074`), then reduced from 1917 to 1000 lines without losing speed.
- **Retry if** a compiler release vectorises the portable loop; the parity tests would tell.

## 2026-10-13: Rejecting refit candidates hidden by the layers above

- **Tried.** Counted, not built: refit candidates entirely hidden by the layers above, which could be rejected before their sums.
- **Numbers.** 376 of 13.3 million refit evaluations.
- **Verdict.** Dropped.
- **Retry.** Not at these shape counts.

## 2026-10-13: Pixel runs in the joint optimisation

- **Tried.** The joint passes evaluate each layer's coverage, and in the gradient pass its derivative, at every pixel of its bounding box. For triangles about half of those pixels lie outside the shape and 44% inside, where the edge distances only confirm a coverage of 0 or 1 and a zero gradient. The change found, per row, the runs where the distances prove that outcome, by binary search on the same monotone expressions, and skipped the edge evaluation there: about 90% of polygon pixels, and the inner disk of curved layers.
- **Numbers.** Output identical, and slower. At one thread over three interleaved pairs, the refine time rose by 7–9% for circles and ellipses, 2–7% for triangles and 0–2.5% for polygons, against a base noise of about 1%; at 10 threads only triangles and rectangles looked faster, within the noise.
- **Verdict.** Dropped: each pixel's edge test was already cheap, and the per-row searches and the extra branch cost more than they saved. The patch is kept outside the repository by the maintainer.
- **Retry.** Not as is.

## 2026-10-13: Prepared geometry and projection derivatives in the joint optimisation

- **Tried.** Not built: reusing `Prepared` between the colour fit and the gradient, and skipping the projection's derivatives for layers already valid.
- **Verdict.** Parked: both are constant work per layer.
- **Retry if** a profile at high shape counts shows the per-layer work.
