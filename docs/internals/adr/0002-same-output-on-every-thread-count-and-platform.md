# 0002. The same seed gives the same output at any thread count and on every target

Status: Accepted

Date: 2026-10-12

## Context

A seeded render must be reproducible: the README's benchmarks, the gallery, the digest pins and users' own placeholders depend on it. The engine runs on rayon pools of any size, natively on aarch64 and x86_64, and in WebAssembly in one or many threads, and the same package serves Node and browsers, so a drawing that changes with the core count or between the native addon and the browser would be a visible bug.

Two sources of divergence were found the hard way:

- **Platform math.** The greedy search computed rotated rectangles' corners and rotated ellipses' rows with the platform's `f64::sin_cos`. Its last bit differs between math libraries, so from 2026-10-07 the `Quality` workflow failed on Linux x86_64: the `RotatedRectangle` and `Any` digests of `seeded_greedy_output_is_pinned` differed from the aarch64 ones (and Rosetta gave a third value). The fix (`651cc41`, 2026-10-10) routed both through the portable `sin_cos_degrees`. Later, the Gaussian move sampler, `rand_distr::StandardNormal`, was found to call the platform's `ln` and `exp` in its tail paths, which differ in the last bit between macOS and the pure Rust `libm` in 7–9% of results, and the PNG writer rotated ellipses with tiny-skia's platform `f32` sine and cosine, which the final stage's export guard compares.
- **`mul_add`.** `f64::mul_add` is correctly rounded everywhere, so it is portable, but it is a software `fma` call in WebAssembly and on x86 without `+fma`: removing it from the quadratic stroke cut quadratics' single-threaded wasm time to 0.51 of what it was.

## Decision

- **Thread independence.** Every random stream derives from the seed and the position of its work, never from a thread: `round_rng(seed, step, round)` for a greedy round, `refine_rng(seed, pass, layer, climb)` for a refit climb ([`rng.rs`](../../../crates/primeval-core/src/rng.rs)). Winners are chosen by energy with ties to the lowest index. Reductions run in a fixed order: the joint optimisation's band sums in band order, the refit's sums in the four-slot order of [0008](0008-the-refit-sums-in-a-fixed-four-slot-order.md). No rule depends on elapsed time.
- **Arithmetic.** The engine uses only `+ − × ÷` and `sqrt`, which IEEE 754 rounds exactly, comparisons, `abs`, `min`, `max`, `floor`, `ceil`, `round` and `trunc`. It never uses `mul_add` and never calls a platform transcendental function. Rotations take their sine and cosine from `sin_cos_degrees` (a Taylor polynomial in [`util.rs`](../../../crates/primeval-core/src/util.rs)) and angles from `atan2_degrees`; the PNG writer rotates ellipses with the same `sin_cos_degrees`. The one transcendental function is `libm::log`, a pure Rust logarithm, in the Gaussian sampler (Marsaglia's polar method).
- **SIMD equals scalar** on every target ([0007](0007-simd-kernels-equal-the-scalar-ones.md)).
- The promise to users stays narrower than the engineering target: the README promises the same output for the same version on the same platform, whatever the core count, and says that native and browser output currently match.

## Consequences

- [`crates/primeval-core/clippy.toml`](../../../crates/primeval-core/clippy.toml) bans `mul_add` and the platform transcendental methods (`disallowed-methods`) outside tests and benches, so the clippy gate enforces the arithmetic list; the `# Arithmetic` section of [`joint.rs`](../../../crates/primeval-core/src/joint.rs) states it.
- Pins that hold on aarch64 and x86_64 alike: `seeded_greedy_output_is_pinned` ([`model.rs`](../../../crates/primeval-core/src/model.rs)), `joint_output_is_pinned` ([`joint.rs`](../../../crates/primeval-core/src/joint.rs)), `sums_are_pinned` ([`refine.rs`](../../../crates/primeval-core/src/refine.rs)), `gaussian_samples_are_pinned` ([`shapes.rs`](../../../crates/primeval-core/src/shapes.rs)) and `raster_output_is_pinned` ([`raster.rs`](../../../crates/primeval-render/src/raster.rs)). Thread-count tests run the same seeded searches, passes and final stages on pools of different sizes: `seeded_output_is_independent_of_the_thread_count`, `seeded_refine_is_independent_of_the_thread_count`, `the_result_does_not_depend_on_the_thread_count` (joint), `every_kind_path_is_identical_across_thread_counts` (pipeline) and `same_seed_svg_is_identical_across_thread_counts` (render). The browser tests compare SVG and PNG bytes from both WebAssembly builds with the native addon's.
- CI runs the Rust suite on `macos-15` (aarch64) and `ubuntu-24.04` (x86_64). Locally, `cargo test --target x86_64-apple-darwin` runs the x86 pins under Rosetta, which catches most but not all libm differences, since Apple's x86 libm is not glibc.
- Costs accepted: a polynomial sine and cosine instead of the platform's; no fused multiply-add even where the hardware has it; any change of a random stream moves every greedy pin, so an otherwise attractive speed-up such as caching the second Gaussian variate of the polar method needs a quality measurement first ([experiments](../experiments.md)).
- A new transcendental function needs a portable implementation, as `libm::log` is, and its own pin.
- Revisiting this means giving up native–browser equality, which the browser tests and the README's "currently match" both rely on.

## Evidence

- `2d0fc87` (2026-10-02): seeded output independent of the thread count.
- `651cc41` (2026-10-10): the greedy search's rotations without platform `sin_cos`.
- `152311a` (2026-10-11): the portable Gaussian sampler; every greedy digest moved, equal on aarch64 and x86_64.
- `b38d7b2` (2026-10-12): the PNG writer's portable rotation; 21 of 40 corpus rows moved in the sixth decimal of a PNG metric, no drawing or guard choice changed.
- `547f550` (2026-10-12): no `mul_add` in the stroke; no pinned digest moved. `6b684db` and `f0401d9` (2026-10-12): the clippy ban on every clippy pass.
