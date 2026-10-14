# 0005. Every shape reads as its kind, in every optimiser

Status: Accepted

Date: 2026-10-09

## Context

`primeval` makes stylised shape versions of a picture, such as a small SVG placeholder of 50 to 200 shapes, not a pixel-faithful copy. A drawing of "triangles" made of slivers, or of "rectangles" that are lines, has a lower error and no longer looks like what the user asked for.

The first pilot of the joint optimisation, in October 2026, produced exactly that: 1–22% of its triangles had an angle under 15°. The maintainer ruled that a triangle that thin no longer reads as a triangle, and on 2026-10-03 extended the rule to every optimiser, the greedy search included: a crossed quadrilateral reads as two triangles, so polygons must be convex; rotated rectangles get an aspect cap, set from the distribution of the greedy search's output.

Quadratic curves raised the same question for their stroke: a pen of one fixed width, 2 px at the working size, fitted the picture 2–3 times worse than any other kind.

## Decision

- A triangle keeps every angle strictly above 15°.
- A polygon is a simple, strictly convex quadrilateral with every angle above 15°.
- A rectangle, axis-aligned or rotated, has its long side at most 8 times its short one (`MAX_ASPECT` in [`shapes.rs`](../../../crates/primeval-core/src/shapes.rs)).
- A quadratic curve is a stroke whose width the search chooses per curve between 2 and 6 working pixels (`Quadratic::STROKE_WIDTHS`), ending in round caps. 2 px is the floor because tiny-skia draws a stroke of at most 1 px as a hairline, thinner than its width on a diagonal, so the PNG would disagree with the SVG, and because 2 px keeps the pixels on the centre line fully covered. 6 px is the ceiling because wider strokes change the curves' look more for diminishing gains.
- The rules hold in every optimiser and on every exported shape. The greedy search undoes and redraws a move that breaks its kind's rule. The joint optimisation projects every shape back onto its rule after each step (Gauss–Newton steps on the angle residuals with a margin of half a degree, exact projections for rectangles and radii) and repairs any shape the final snap made invalid. `Model::adopt` refuses geometry outside the engine's domain.
- The checks compare cross and dot products against `tan 15°` (`is_legible_convex`); they compute no angle, so they stay portable ([0002](0002-same-output-on-every-thread-count-and-platform.md)).

## Consequences

- At a fixed shape count the rules cost polygons 2–3% and rotated rectangles about 1% of median error, and the synthetic texture up to 39%; they are time-neutral. Gains measured with a rule switched off do not count.
- They make the joint optimisation's projections well-posed: dropping them would mean reworking its projections, not just removing a check.
- The quadratics' width range cut their median error by 28–66% at 50–500 shapes against the fixed 2 px, at 1.11–1.39 times the time; round caps gained up to 5% more on the paintings at 500 shapes at no cost. The look changed from one pen to strokes of several widths, ending as a brush would.
- The engine runner's per-kind summary counts the exported shapes that break the rules, and every measurement must report zero. Tests: `triangle_validity_agrees_with_the_acos_rule`, `polygon_validity_accepts_strictly_convex_quads_in_either_orientation` and `the_aspect_cap_is_eight_to_one_either_way` (shapes.rs); `exported_polygons_keep_the_rule`, `exported_rectangles_keep_the_rule` and `exported_curved_shapes_keep_the_rule` (joint.rs); `snapped_triangles_keep_the_rule` (joint/angle.rs), `snapped_quads_keep_the_rule` (joint/convex.rs), `snapped_rectangles_keep_the_rule` (joint/rect.rs).
- A new kind, or an optimiser extended to an existing kind, must answer the same question before it lands: what does a shape of this kind have to look like? Revisiting a rule is a product decision for the maintainer, not a tuning change.

## Evidence

- `16877bb` (2026-10-07): the rules in every optimiser; `6803045` (2026-10-08): stroke widths of 2–6 px; `7c5a6d9` (2026-10-09): round caps.
- [Quadratic strokes](../tuning.md#quadratic-strokes) in `tuning.md`; the other widths and square caps in [`../experiments.md`](../experiments.md).
