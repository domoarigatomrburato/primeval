# Tuning

The values the engine uses today because a measurement chose them, with the measurement, its date, and the commit that set the value or recorded the measurement that confirmed it. This document is living: a change to one of these values updates its row in the same change. The decisions these values tune are in [`adr/`](adr/README.md); variants that were measured and not adopted are in [`experiments.md`](experiments.md).

The shape rules (the 15° minimum angle, the aspect cap of 8) are product decisions, not tuning: see [ADR 0005](adr/0005-shapes-read-as-their-kind.md).

## How these numbers are measured

[ADR 0006](adr/0006-how-engine-changes-are-judged.md) states the rule. In short, unless a row says otherwise:

- the engine runner ([`CONTRIBUTING.md`](../../CONTRIBUTING.md#benchmarks); its flags are documented in [`examples/engine.rs`](../../crates/primeval-render/examples/engine.rs)) on its default corpus, *American Gothic*, *Mona Lisa* and three synthetic 512 × 512 images, every shape kind, seed 42, with `approximate`'s pipeline (`--refine final`);
- on an Apple M2 Pro at 10 threads;
- quality is the change of the mean of the 100- and 200-shape median `rmse256` (the export's RMSE at the working size, 0–1); negative is better. "Worst row" is the largest increase on one image, in levels out of 255;
- time is the ratio of the search's wall time (`search_s`, decode and encode excluded) over the same rows;
- "the previous `approximate`" is the engine before the pipeline: 16 search rounds, then one refit pass (`--refine end:1 --effort 16:1:1`), the baseline of 2026-10-07;
- "equal-time ratio" is the median over images of the row's RMSE divided by the RMSE of the plain greedy search (`--effort 16:1:1`) interpolated at the same time; below 1 the row wins at equal time.

The runner's lab flags set each variant without changing the defaults:

| Stage | Flags |
| --- | --- |
| search | `--effort R:C:A` (rounds, candidate and age multiples), `--search seeds=K,candidates=N,age=A` |
| passes during the search | `--during none`, `spaced:K:C`, `joint:K:C:I`, `joint-export:K:C:I` |
| final stage | `--final-refits R` or `until:G:C`, `--joint-scale M`, `--iterations K` |
| joint optimisation | `--joint-warmup W`, `--joint-step X`, `--joint-step-rel F`, `--joint-curved on\|off` |
| refit pass | `--refine-effort R:A` (climbs per layer, climb age) |
| quadratic strokes | `--quadratic-width MIN:MAX`, `--quadratic-cap butt\|round\|square` |
| threads | `RAYON_NUM_THREADS=T` before the command |

To reproduce a row, build the runner at the row's commit and at its parent and compare the two with the same flags, or compare the flags of the row's variants at one commit. Quality columns reproduce to the last digit on the same platform; times move by a few percent.

## Search

Every step runs independent search rounds; each samples random candidates, ranks them, and hill-climbs the best (`RoundEffort::of`).

| Parameter | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| Rounds per step, every kind | 16 | [`model.rs`](../../crates/primeval-core/src/model.rs), `Effort::DEFAULT` | 32 rounds bought `any` 0.8% for 19% more time, and 32 rounds with twice the climb age bought rotated ellipses 2.2% for 90% more. Greedy alone on the paintings: 20 rounds cost 17–24% more time for 0.3–1.0% lower RMSE, 10 rounds save 22–28% and lose 1–2%. | 2026-10-07 | `d85a5b1` |
| Triangle, polygon: candidates, climb age, seeds | 1000, 75, 1 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `RoundEffort::of` (`ShapeKind::Triangle`) | Age 75 against 100: triangles 0.87–0.88 of the time at +0.33%, worst row 0.01; polygons 0.87–0.89 at +0.09%, worst row 0.06. | 2026-10-12 | `c1786c4` |
| Circle | 500, 75, 1 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `RoundEffort::of` (`ShapeKind::Circle`) | Against 1000 candidates and age 100: 0.82–0.85 of the time at +0.09%, worst row 0.05. | 2026-10-12 | `c1786c4` |
| Rotated ellipse | 500, 100, 1 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `RoundEffort::of` (`ShapeKind::RotatedEllipse`) | Against 1000 candidates: 0.82–0.84 of the time at +0.05%, worst row 0.08. | 2026-10-12 | `c1786c4` |
| Quadratic | 500, 200, 2 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `RoundEffort::of` (`ShapeKind::Quadratic`) | 500 candidates against 900: 0.88–0.92 of the time at −2.14%, worst row 0.17. Age 200, twice the others' base: 2.6% for 57% more time (2026-10-07, `d85a5b1`), kept while quadratics had no cheaper lever; not re-measured since their widths became 2–6 px. Two seeds: from the original port, not measured on their own. | 2026-10-12 | `c1786c4` |
| `any`, rectangle, ellipse, rotated rectangle | 1000, 100, 1 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `RoundEffort::of` (`_`) | No cheaper budget passed in the October 2026 budget measurement. 500 candidates with two seeds won rectangles (−0.7%) and rotated rectangles (−1.6%) by the rule on 2026-10-11, but on the paintings rectangles were neutral and rotated rectangles gained about 1%, while synthetic-shapes swung by ±20% between checkpoints, so it was not taken. | 2026-10-12 | `c1786c4` |
| Kinds that rank candidates at half resolution | `any`, circle, ellipse, rotated ellipse, polygon | [`shapes.rs`](../../crates/primeval-core/src/shapes.rs), `ShapeKind::ranks_coarsely` | Search time 16–35% lower for these kinds, quality equal within noise (score +0.11%). Rectangles, rotated rectangles, triangles and quadratics gained almost no time and rank at full resolution. Measured with the earlier quality runner on an Apple M3. | 2026-10-02 | `55c4e11` |
| Coarse candidates rescored at full resolution | 128 | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `COARSE_RESCORED_SEEDS` | Rescoring only the best 4 lost 1.3% at 1000 steps, where shapes are small. | 2026-10-02 | `55c4e11` |

## Passes during the search

`During::Spaced(interval, divisor)`: a pass after step `interval`, then every `max(interval, s / divisor)` steps after the pass at step `s` ([ADR 0004](adr/0004-a-pipeline-per-shape-kind.md)).

| Kind | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| `any` | refit passes, `Spaced(20, 5)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Any`) | With 16 rounds, 6.9% below the previous `approximate` at 1.57 times its time, equal-time ratio 1.01; with 32 rounds and `Spaced(5, 10)` it was 10.8% below at 2.78 times, ratio 1.18. | 2026-10-07 | `d85a5b1` |
| triangle | joint passes of 20 iterations, `Spaced(20, 5)`, kept when the exact canvas scores lower | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Triangle`) | Against refit passes on the same schedule: −1.3% at 0.70 of the time. 10 iterations +1.2% at 0.64; 40 iterations −1.4% at 0.87; `Spaced(10, 10)` with 20 iterations −1.4% at 0.90; no passes +2.6% at 0.55. | 2026-10-08 | `8e77bb2` |
| rectangle, polygon | refit passes, `Spaced(20, 5)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Rectangle`) | Against the previous `approximate`: rectangles −12.4% at 2.13 times (`Spaced(10, 10)`: −13.0% at 2.84 times), polygons −9.5% at 1.37 times (`Spaced(10, 10)`: −9.6% at 1.62 times). | 2026-10-07 | `16877bb` |
| rotated rectangle | refit passes, `Spaced(20, 2)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::RotatedRectangle`) | −11.7% at 1.92 times the previous `approximate`, equal-time ratio 1.00; denser schedules lose: `Spaced(20, 5)` −11.4% at 2.15 times, `Spaced(10, 10)` −10.3% at 2.65 times. Measured with twice the final joint iterations, before they returned to the default; not re-measured since. | 2026-10-07 | `16877bb` |
| ellipse, circle | refit passes, `Spaced(20, 5)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Ellipse`) | With the joint optimisation ending the pipeline, against the previous pipeline (`Spaced(10, 10)` and one final refit pass): ellipses −3.9 / −6.0 / −8.4 / −14.4% and circles −11.3 / −4.0 / −8.5 / −13.1% in the median at 50 / 100 / 200 / 500 shapes. `Spaced(10, 10)` with the joint optimisation gained 1–2 points more at 1.13–1.22 times the time; no passes 1–3 points less at 0.57–0.66 times. The chosen schedule's time was measured on a disturbed machine, at about the previous time. | 2026-10-08 | `53172eb` |
| rotated ellipse | refit passes, `Spaced(10, 10)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::RotatedEllipse`) | With 16 rounds and age 100: −7.0% at 1.56 times the previous `approximate`, equal-time ratio 0.97; `Spaced(20, 5)` −5.7% at 1.30 times, ratio 0.94. | 2026-10-07 | `16877bb`, `d85a5b1` |
| quadratic | refit passes, `Spaced(20, 5)` | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Quadratic`) | At the base climb age: −0.5% at 1.13 times the previous `approximate`; `Spaced(10, 10)` −0.6% at 1.23 times. With age 200 the whole pipeline is −3.1% at 1.81 times. | 2026-10-07 | `16877bb` |

The test `passes_during_the_search_stay_cheap` bounds every schedule: up to 200 steps at most 5 layers per step for the kinds that end with the joint optimisation, 10 for the others.

## Final stage

| Kind | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| `any` | one refit pass, then the joint optimisation with its default iterations, curved outlines moving, quadratics fixed | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Any`) | The joint optimisation of its triangles, polygons and rectangles: 3.2% below the refit pass alone. Moving its ellipses, circles and rotated ellipses too: −0.7 to −1.1% more (the paintings −1.8 to −5.7%) at 1.03–1.06 times. A second final refit pass: −0.2% at 50–200 shapes and −0.6% at 500 at 1.04–1.08 times, below the rule's 0.5%. | 2026-10-11 | `53172eb`, `e649332` |
| triangle, rectangle, polygon | the joint optimisation with its default iterations, no refit pass | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Triangle`) | The joint optimisation alone, without passes during the search: triangles −10.6% at 1.11 times the previous `approximate`, rectangles −9.2% at 1.05, polygons −7.0% at 0.97. Refit passes before it, or more iterations, gained less than 0.5%. | 2026-10-07 | `16877bb` |
| rotated rectangle | the joint optimisation with its default iterations | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::RotatedRectangle`) | Against twice the iterations: 0.77–0.82 of the time at +0.39%, worst row 0.47. Twice the iterations had been 2.2% below no passes during the search with the default iterations, and doubling alone gained 0.4%. | 2026-10-12 | `c1786c4` |
| ellipse, circle | the joint optimisation, curved outlines moving, no refit pass | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Ellipse`) | With the old passes, the joint optimisation in place of the final refit pass matched keeping the pass before it within 0.3 points (ellipses −9.6% against −9.8% at 200 shapes, circles −9.7% against −9.6%), at about the same time. | 2026-10-08 | `53172eb` |
| rotated ellipse | one refit pass | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::RotatedEllipse`) | The joint optimisation with curved outlines gained 2.2–2.9% at 1.09–1.13 times, and its guard kept it on only 1–3 of the 5 images at 200–500 shapes. A second refit pass: −0.2 to −0.6% at 1.02–1.03 times. Passes until one gains less than 1%, 0.5% or 0.2% gained less than 0.5%. | 2026-10-11 | `16877bb`, `e649332` |
| quadratic | refit passes until one gains less than 1% (`min_gain: 100`), at most 4 | [`pipeline.rs`](../../crates/primeval-render/src/pipeline.rs), `pipeline` (`ShapeKind::Quadratic`) | With 16 rounds, 1.8–10.5% below one pass after the greedy search at 50–500 shapes, at about 1.4–1.6 times its evaluations. Until 0.5% with at most 6: −0.4% at 1.03–1.04 times, below the rule. Two fixed passes: +1.8% at 200 shapes. | 2026-10-11 | `16877bb`, `e649332` |

Every joint result of the final stage is kept only if its export is strictly closer to the target ([ADR 0003](adr/0003-optimise-the-composite-the-export-draws.md)).

## Joint optimisation

| Parameter | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| Iterations | 80 up to 50 shapes, linear to 120 at 200 and to 160 at 500, then 160 | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `default_iterations` | The largest counts that keep the optimisation's extra time near 0.5 times the greedy search's at one thread, on the corpus with triangles; quality kept improving up to 150 iterations, so time decides. Measured before the pipeline landed; not re-measured since the warm-up and the curved outlines. | 2026-10-07 | `16877bb` |
| Warm-up | 5 iterations | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `Tuning::default` (`warmup`) | Against none, over all images / on the paintings at 100–500 shapes: `any` −0.1 / −0.5%, triangles −0.2 / −0.7%, rectangles +0.2 / −0.4%, rotated rectangles −1.2 / −3.0%, polygons −0.4 / −0.9%, at 0.98–1.00 of the time. Warm-ups of 10 and 20 gained as much on the paintings but lost on triangles over all images (+1.2% and +0.9%). | 2026-10-08 | `945ccec` |
| First vertex step | 1 px | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `LR_VERTEX` | Smaller first steps lose on triangles, whose large shapes need the full step: 0.5 px +1.3%, 0.25 px +1.7% over all images. | 2026-10-08 | `945ccec` |
| Step relative to the shape's size | none | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `Tuning::default` (`relative_step`) | 1/8 and 1/16 of the root of the area: within ±0.3% on every kind over all images, neutral. | 2026-10-08 | `945ccec` |
| Curved outlines move | on | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `Settings::default` (`curved`) | The ellipse and circle rows of the final stage; `any` −0.7 to −1.1% at 1.03–1.06 times. | 2026-10-08 | `53172eb` |
| Snap of axis-aligned rectangles | 0.5 px (other kinds 0.25 px) | [`joint.rs`](../../crates/primeval-core/src/joint.rs), `RECT_QUANTUM` | Against 0.25 px, with the joint optimisation alone at 50 / 100 / 200 rectangles: 0.5 px saves about 3.5 bytes a rectangle and costs 0.3 / 0.5 / 0.8 points of the greedy search's median RMSE; 1 px saves about 3.8 bytes and costs 0.4 / 0.7 / 1.7. | 2026-10-07 | `16877bb` |
| Coverage cache per band | 32 MiB | [`joint/diff.rs`](../../crates/primeval-core/src/joint/diff.rs), `COVERAGE_BUDGET` | The cache peaks at 2.3 MB on the README originals at the default working size (1000 triangles, 212 × 256), so every layer fits. Output bit-identical; at 500 shapes triangles take 0.91 of their total time, the other joint kinds 0.95–0.99. | 2026-10-12 | `380a529` |

## Refit pass

| Parameter | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| Climbs per layer (`ROUNDS`) | 4 | [`refine.rs`](../../crates/primeval-core/src/refine.rs), `ROUNDS` | First chosen for coarse moves on an Apple M3 (`--refine end:1`, `any`, triangles and rotated ellipses at 100 steps): over `R ∈ {4, 8, 16}` and `AGE ∈ {50, 100, 200}`, `4, 50` was the most efficient point (4.6% gain at 0.15 of the greedy time). Re-checked with the adapted moves on the M2 Pro: 8 or 10 climbs of age 25 changed quality by ±0.9 points with no direction at 1.10–1.50 times the refit time at 10 threads; 8 climbs of age 13 saved 10–35% of it and lost 0.2–2.6 points. | 2026-10-07 | `16877bb`, `646252c` |
| Climb age (`AGE`) | 25 | [`refine.rs`](../../crates/primeval-core/src/refine.rs), `AGE` | Halved from 50 when the moves became step-adapted, to keep the refit time of the coarse `4, 50`: −5.6% and −5.4% at 100 and 200 steps against greedy, where coarse moves gave −4.7% and −3.6%. | 2026-10-07 | `16877bb` |
| Step adaptation | ×2 after a kept move, ×2^(−1/4) after a rejected one, between 1/16 and 1 of the coarse move | [`refine.rs`](../../crates/primeval-core/src/refine.rs), `StepScale` (`SCALE_UP`, `SCALE_DOWN`, `MIN_SCALE`) | The 1/5th success rule gained at least as much as coarse moves on every kind at about the same refit time; alternating coarse and fixed fine moves gained less at every time; a factor of 3 and a smallest `σ` of 2 px gained no more. | 2026-10-07 | `16877bb` |

## Quadratic strokes

| Parameter | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| Width bounds | 2–6 px, chosen per curve by the search | [`shapes.rs`](../../crates/primeval-core/src/shapes.rs), `Quadratic::STROKE_WIDTHS` | Against a fixed 2 px: the median falls by 28 / 60 / 66 / 49% at 50 / 100 / 200 / 500 shapes, the paintings by 52% and 50% at 200, at 1.39 to 1.11 times the time and 3% more SVG bytes. 1.5–4 px: −15 to −54% at 1.08–1.19 times; 1.5–8 px: −40 to −72% at up to 1.52 times; a fixed 3 px: −8 to −44% at 1.10–1.15 times. A 2 px floor gains as much as 1.5 px. | 2026-10-08 | `6803045` |
| Line cap | round | [`worker.rs`](../../crates/primeval-core/src/worker.rs), `WorkerCtx::new` (`quadratic_cap`) | Against butt caps at 2–6 px: −0.2 / +0.3 / −3.0 / −5.0% in the median at 50–500 shapes, the paintings −4.3% and −5.0% at 500, at 0.98–1.00 of the time and the same SVG size. Square caps: −1.0 / +1.4 / −7.1 / −4.1%, the paintings −2.0% and −4.1%. | 2026-10-09 | `7c5a6d9` |

## Runtime and release

| Parameter | Value | Where | Evidence | Date | Commit |
| --- | --- | --- | --- | --- | --- |
| Idle reuse of a browser worker | 10 s, at most one worker per page | [`browser-runtime.ts`](../../src/browser-runtime.ts), `IDLE_MS` | Reuse against a fresh worker, headless Chromium on the M2 Pro: the threaded build takes 0.81–0.82 of the time at 10 shapes (about 50 ms less) and 0.965–0.969 at 100; the single-threaded build 0.99–1.00. The 10 s window itself was not measured. | 2026-10-13 | `c31551f` |
| Registry wait after publishing | 90 checks, 10 s apart (about 15 minutes) | [`release-packages.mjs`](../../scripts/release-packages.mjs), `verifyPublished` (`attempts`, `delayMs`) | For 0.2.2 the Windows package took about 9 minutes to become visible, and the earlier wait of 30 checks failed that release's publish job. | 2026-10-14 | `191a930` |
