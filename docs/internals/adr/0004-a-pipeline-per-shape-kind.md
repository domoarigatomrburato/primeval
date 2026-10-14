# 0004. A pipeline per shape kind: passes during the search, then a final stage

Status: Accepted

Date: 2026-10-12

## Context

The greedy search adds one shape per step and never revisits it. Two ways of revisiting committed shapes exist:

- the **refit pass** ([`refine.rs`](../../../crates/primeval-core/src/refine.rs)): coordinate descent, each shape re-optimised at its own layer with the others fixed, on the engine's binary or anti-aliased canvas, for every kind;
- the **joint optimisation** ([`joint.rs`](../../../crates/primeval-core/src/joint.rs)): Adam over every shape at once on a smooth model of the export, for the kinds whose outline it can differentiate (triangles, polygons, rectangles, rotated rectangles, ellipses, circles, rotated ellipses; quadratics keep their geometry).

They can run during the search, so that later steps build on the revised shapes, or once after the last step. Measured on the engine runner's corpus on 2026-10-07 ([0006](0006-how-engine-changes-are-judged.md)), against the previous `approximate` (greedy, then one refit pass):

- the joint optimisation as the final stage alone delivered 7–11% for triangles, rectangles and polygons at 0.97–1.11 times the time (rotated rectangles 10.2% at 1.51 times, with twice the iterations), and for those four kinds it was neutral or better at equal time against the greedy search with more shapes;
- the refit passes during the search, as first scheduled, took 45–70% of the pipeline's time for `any`, triangles, rectangles, ellipses and circles, and were the only stage that lost at equal time; for ellipses and circles, before the joint optimisation could move them, they were the only lever.

The product's use case is quality at a fixed shape count, a small SVG placeholder, so a stage that loses at equal time can still be worth running, as long as its cost stays bounded.

## Decision

`pipeline` in [`crates/primeval-render/src/pipeline.rs`](../../../crates/primeval-render/src/pipeline.rs) chooses, per kind, what runs around the greedy steps:

| Kind | During the search | Final stage |
| --- | --- | --- |
| `any` | refit passes | one refit pass, then the joint optimisation of every shape but the quadratics |
| triangle | joint passes, adopted into the model when the exact canvas scores lower | joint optimisation |
| rectangle, polygon, rotated rectangle | refit passes | joint optimisation |
| ellipse, circle | refit passes | joint optimisation, curved outlines moving |
| rotated ellipse | refit passes | one refit pass |
| quadratic | refit passes | refit passes until one gains less than 1%, at most four |

- The passes during the search follow a `During::Spaced { interval, divisor }` schedule: after step `interval`, then every `max(interval, s / divisor)` steps, so they refit about `divisor` layers per step and their cost grows linearly with the shape count.
- Every joint result in the final stage passes the export guard ([0003](0003-optimise-the-composite-the-export-draws.md)).
- Every rule depends only on the request and on deterministic scores, never on elapsed time or the thread count.

The schedules, iterations and thresholds are in [`../tuning.md`](../tuning.md).

## Why each kind ends where it does

- **Triangles** run the joint optimisation during the search too: converting its result back into the engine's shapes is exact for triangles, so the gain carries into later steps (1.3% below refit passes at 0.70 times the time).
- **Rectangles and rotated rectangles** keep refit passes during the search: rounding the joint result to their integer parameters gives back most of its gain, and the exact canvas rejects it (1–4 of 8 passes kept on the paintings).
- **Polygons** keep refit passes during the search: on hard synthetic edges the joint result is rejected, and with joint passes polygons lost 33–41% on synthetic-shapes at 100–500 shapes, still 11–34% with the exact vertex coverage and the warm-up.
- **`any`** keeps refit passes: the joint optimisation does not move its quadratics, and the refit passes serve every layer.
- **Ellipses and circles** end with the joint optimisation since its outlines gained curved boundaries, locally straight in each pixel: 8.4–14.4% lower median error at 200–500 shapes than their previous pipeline. Against a 64 × 64 supersampled reference that coverage errs on boundary pixels by 0.047 on average below 2 px of radius, 0.018 from 2 to 8 px and 0.0044 above, and the total area is within 0.63% from 4 px.
- **Rotated ellipses** end with one refit pass: the joint optimisation gained them only 2–3%, and the guard rejected it on most images, on thin ellipses (radii from 1 px), where the locally straight coverage errs most.
- **Quadratics** have no joint stage: their stroke is not in the joint model, which only refits their colour and opacity.

## Consequences

- Test `passes_during_the_search_stay_cheap` bounds the schedules: up to 200 steps, at most 5 layers per step for the kinds that end with the joint optimisation, 10 for the others. `spaced_passes_refit_linearly_many_layers` holds the linear bound, and `every_kind_path_is_identical_across_thread_counts` and `joint_passes_are_identical_across_thread_counts` the determinism.
- The engine runner's `--refine final` drives the same functions as `approximate` through the `lab` feature, so measurements cannot drift from production.
- At equal time the full pipeline lost to the old greedy search with more shapes on most kinds when it was chosen; its output was 2–5 times smaller. Users who fix time rather than shape count are not the target.
- Revisiting a kind's ending: rotated ellipses need a coverage exact at high curvature ([audit plan](../plans/2026-10-audit-and-refactor-plan.md)); rectangles would need a continuous representation in the engine before joint passes during their search; polygons need a snap that does not undo fine fits on hard edges. Large canvases may need the scale guard of the same plan.

## Evidence

- `16877bb` (2026-10-07): the pipeline; `d85a5b1` (2026-10-07): the retune to thinner schedules; `8e77bb2` (2026-10-08): triangles' joint passes; `53172eb` (2026-10-08): curved outlines; `c1786c4` (2026-10-12): rotated rectangles back to the default joint iterations.
- [Passes during the search](../tuning.md#passes-during-the-search) and [final stage](../tuning.md#final-stage) in `tuning.md`; the dropped schedules and stages in [`../experiments.md`](../experiments.md).
