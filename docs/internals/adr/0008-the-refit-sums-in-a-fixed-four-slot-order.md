# 0008. The refit pass sums its pixels in a fixed four-slot order

Status: Accepted

Date: 2026-10-13

## Context

Every move of a refit climb scores its candidate through `Layer::sums` ([`refine.rs`](../../../crates/primeval-core/src/refine.rs)): one pass over the covered pixels that sums, in `f64`, `Σ g²`, `Σ g · q` per channel, `Σ q²` and `Σ r²`, from which the best colour and the energy follow. The refit passes took 19–44% of the time of quadratics, polygons, rotated ellipses and `any` when measured on 2026-10-10, and this loop runs for every move of every climb.

The sums were one serial chain of dependent floating-point additions. Floating-point addition is not associative, so the result depends on the order: a vectorised loop that adds four pixels at a time in any other order would change the sums' last bits, then occasional decisions of the climbs, then the drawing, and differently on each target. Bit-identical output across aarch64, x86_64 and wasm32 ([0002](0002-same-output-on-every-thread-count-and-platform.md)) rules that out. Exact slicing of the loop, which kept every addition in its serial order, saved 1–11% of the time per kind (4.7% over all kinds at 10 threads); SIMD needed a different order.

## Decision

The order of the additions is part of the engine's contract, the same on every target, and was approved as a change of output:

- After clipping, pixel `j` of a line goes to slot `j mod 4`, counting hidden pixels, and each line restarts at slot 0.
- Six planes of four `f64` slots, initially `+0.0`, persist across the lines of a layer.
- The per-pixel `f32` formulas keep their operand order; after the exact conversion to `f64`, each slot adds `g · g`, `g · q` per channel, `(qR² + qG²) + qB²` and `(rR² + rG²) + rB²`.
- Hidden pixels and missing tail slots add `+0.0`, which is exact, since a slot never holds `−0.0`.
- Every plane folds as `(((+0.0 + s0) + s1) + s2) + s3`.

[`refine/sums.rs`](../../../crates/primeval-core/src/refine/sums.rs) implements the order with NEON, baseline SSE2 (no run-time detection) and simd128, behind one generic loop over a small per-target trait of primitives; other targets use the scalar slot code. The doc comment of `Sums` states the order.

## Consequences

- With SIMD, the refit-heavy kinds gained most: rectangles, circles and ellipses took 0.86–0.90 of their time on aarch64 at 10 threads and 0.85–0.90 on x86_64; over all kinds 0.958 (aarch64, 10 threads), 0.975 (one thread) and 0.969 (x86_64). Quadratics and polygons gained least, since 83% and 79% of their lines are one pixel long.
- The order changed 10 of the 90 corpus rows, all on synthetic-shapes, by at most 0.27 levels out of 255 in either direction; the paintings are unchanged, and aarch64 and x86_64 agree on every row.
- `sums_are_pinned` hashes the bits of `Layer::sums` over the parity cases and must give the same three constants on aarch64 (NEON), x86_64 (SSE2) and wasm32 (simd128): no model pin runs the refit, so without it a change of the fold order would pass every test. `simd_sums_match_scalar_sums` and `groups_match_scalar_slots_for_real_blends` compare every target with the scalar reference bit for bit; `node scripts/build-wasm.mjs test-kernels` runs them for simd128.
- Any change to the order, the slot count or the per-pixel formulas is a change of output: it moves `sums_are_pinned` and needs a quality measurement under [0006](0006-how-engine-changes-are-judged.md) and a new record superseding this one.

## Evidence

- `b3b9d4a` (2026-10-13): exact slicing; `b155074` (2026-10-13): the four-slot order with SIMD; `4f8dc60` (2026-10-13): `sums_are_pinned`.
- The portable version of the order that did not vectorise is in [`../experiments.md`](../experiments.md#2026-10-13-the-four-slot-order-in-portable-rust).
