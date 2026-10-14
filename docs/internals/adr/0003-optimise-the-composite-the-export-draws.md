# 0003. The engine optimises the composite the export draws, and the export guards the final stage

Status: Accepted

Date: 2026-10-12

## Context

The engine scores candidates on its own canvas; the user gets an SVG, or a PNG drawn by tiny-skia. Any difference between the two is error the search cannot see. Before the algorithm work of October 2026 the engine blended in steps that rounded more than once, and at 500 shapes the PNG export's error was 6.6% above the engine's own score.

The joint optimisation ([`joint.rs`](../../../crates/primeval-core/src/joint.rs)) works on a smooth model of the anti-aliased export, composited in `f32`, and ends by snapping every vertex to a quarter-pixel lattice. Its objective can disagree with the export: on a 24 × 24 stack of finely fitted polygons, its result exported 3.6 times worse than its input, and 1.7 times worse with zero Adam iterations, from the projection, the snap and the colour refit alone. The coverage model was made exact at polygon vertices (`0298d9c`), and the first steps were warmed up (`945ccec`); what remains is the snap and the first steps on small, finely fitted shapes, mostly on hard synthetic edges.

## Decision

- A blend ([`score.rs`](../../../crates/primeval-core/src/score.rs)) is the exact composite of a scanline's colour at its coverage and the shape's alpha, rounded once to 8 bits, in fixed point (`M = 0xFFFF`). The PNG writer ([`raster.rs`](../../../crates/primeval-render/src/raster.rs)) renders the same composite with tiny-skia's high-precision pipeline. Rasterizers cover pixels as the exporter does: a quadratic curve covers each pixel by the share of its area under the stroke, with the same caps and joins.
- The joint optimisation's result is kept only if its PNG export at the working size is strictly closer to the target than the drawing before it (`better_export` in [`pipeline.rs`](../../../crates/primeval-render/src/pipeline.rs)). The comparison is exact integer arithmetic over the PNG writer's raster, on one thread; a tie keeps the input.
- The triangles' joint passes during the search are kept by the cheaper canvas guard instead: the engine's exact canvas, repainted with the adopted shapes, must score strictly lower (`Guard::Canvas`, `Model::adopt`), the refit pass's own rule.

## Consequences

- The engine–export gap at 500 shapes fell from +6.6% to +0.6%, and the refit pass's layer model became exact up to rounding. The cost: `write_png` at 1024 px went from 42 to 57 ms, once per render, and the energy kernels became 7–8% slower on polygon and quadratic lines.
- The joint optimisation can only help the delivered output: a failure of its model costs its time, never quality. On synthetic-shapes the guard still rejects it for polygons at every count, so polygons keep refit passes during the search ([0004](0004-a-pipeline-per-shape-kind.md)).
- The guard renders the drawing twice per final stage. Its raster must stay portable, which is why the PNG writer takes its rotations from `sin_cos_degrees` ([0002](0002-same-output-on-every-thread-count-and-platform.md)): a platform sine could make native and browser keep different drawings in a near tie.
- Tests: `draw_lines_rounds_the_exact_blend_once` (score.rs), `stacked_layers_match_the_composite_rounded_once` (render raster.rs), `the_final_stage_keeps_its_input_when_the_joint_result_exports_worse` and `the_final_stage_keeps_the_joint_result_when_it_exports_better` (pipeline.rs), `a_joint_pass_with_the_canvas_guard_never_raises_the_score`, and `capped_quadratics_export_as_the_engine_covers_them`.
- Revisiting the guard would need a joint model that matches the export exactly, snap included; revisiting the blend would reopen the engine–export gap.

## Evidence

- `16877bb` (2026-10-07): the exact single-rounding blend, the high-precision PNG writer and the guard.
- `152311a` (2026-10-11): the stroke's joins, after which the quadratics' mean engine–export gap fell from 0.079 to 0.052 at 200 shapes and from 0.345 to 0.260 at 500.
- `b38d7b2` (2026-10-12): the portable rotation in the PNG writer.
- [Final stage](../tuning.md#final-stage) in `tuning.md`; [joint passes during the search for other kinds](../experiments.md#2026-10-08-joint-passes-during-the-search-for-the-other-kinds) in `experiments.md`.
