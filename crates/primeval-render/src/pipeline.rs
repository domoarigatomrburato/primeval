//! What [`crate::approximate`] runs around its greedy steps, per shape
//! kind: refit or joint passes during the search, and the final stage.
//!
//! `lab` drives the same functions, so the engine runner cannot drift from
//! [`crate::approximate`]. Every rule depends only on the request and on
//! deterministic scores, never on elapsed time or on the number of threads.

use crate::{ShapeKind, raster};
use primeval_core::{Alpha, Buffer, Drawing, Model, joint};

/// The stages around the greedy steps of one shape kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pipeline {
    /// The passes of the model during the search: refit passes, or joint
    /// passes.
    pub(crate) during: During,
    /// Refit passes of the model after the last step.
    pub(crate) refits: Refits,
    /// After the refit passes, the joint optimisation
    /// ([`joint::optimise`]) with this multiple of its default iteration
    /// count ([`joint::default_iterations`]), or `None` for none.
    pub(crate) joint: Option<u32>,
}

/// When the search runs a pass over the model itself after a step, and
/// which: a refit pass ([`Model::refine`]), or a joint pass. Later steps
/// build on the revised shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum During {
    /// Never.
    #[cfg(any(test, feature = "lab"))]
    Never,
    /// After every step whose number (from 1) is a multiple of this.
    #[cfg(any(test, feature = "lab"))]
    Every(u32),
    /// After step `interval`, then each `max(interval, s / divisor)` steps
    /// after the previous pass, at step `s`: every `interval` steps up to
    /// `interval · divisor`, then at geometrically spaced steps. A search
    /// of `N` steps runs about `divisor · ln(N / (interval · divisor))`
    /// passes after that point, which refit about `divisor · N` layers in
    /// all, so the cost grows linearly with `N`.
    Spaced {
        /// The smallest number of steps between two passes; positive.
        interval: u32,
        /// Positive.
        divisor: u32,
    },
    /// The joint optimisation ([`joint::optimise`]) of every shape at once
    /// in place of the refit pass, on [`During::Spaced`]'s schedule, its
    /// result adopted into the model ([`Model::adopt`]) if `guard` keeps
    /// it. Later steps build on the adopted shapes.
    Joint {
        /// As [`During::Spaced`]'s.
        interval: u32,
        /// As [`During::Spaced`]'s.
        divisor: u32,
        /// The Adam iterations of every pass; positive.
        iterations: u32,
        /// What decides whether a pass's result is kept.
        guard: Guard,
    },
}

/// What keeps the result of a joint pass in the search ([`During::Joint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Guard {
    /// The model's exact canvas, repainted with the adopted shapes, scores
    /// strictly lower ([`Model::adopt`] without `force`), as a refit pass
    /// is checked; otherwise the model is left as it was.
    Canvas,
    /// Lab only: the model's own PNG export at the working size, after
    /// adopting the joint result, is strictly closer to the target than
    /// before ([`exports_closer`]): it also sees the rounding of the
    /// rectangles that adopting converts. Costs a clone of the model and
    /// two exports per pass, on top of the repaint.
    #[cfg(any(test, feature = "lab"))]
    Export,
}

impl During {
    /// Whether a pass runs after step `step` (from 1).
    pub(crate) fn due(self, step: u32) -> bool {
        match self {
            #[cfg(any(test, feature = "lab"))]
            Self::Never => false,
            #[cfg(any(test, feature = "lab"))]
            Self::Every(every) => step.is_multiple_of(every),
            Self::Spaced { interval, divisor } => spaced_due(interval, divisor, step),
            Self::Joint {
                interval, divisor, ..
            } => spaced_due(interval, divisor, step),
        }
    }
}

/// [`During::Spaced`]'s rule: whether a pass runs after step `step`.
fn spaced_due(interval: u32, divisor: u32, step: u32) -> bool {
    let mut next = u64::from(interval);
    let step = u64::from(step);
    while next < step {
        next += u64::from(interval).max(next / u64::from(divisor));
    }
    next == step
}

/// What [`after_step`] ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    /// No pass was due.
    Skipped,
    /// A refit pass.
    Refit,
    /// A joint pass, and whether its result was kept.
    Joint {
        /// Whether the model adopted the joint result.
        kept: bool,
    },
}

/// The refit passes of the final stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refits {
    /// This many passes.
    Passes(u32),
    /// Passes until one lowers the model's score ([`Model::score_f64`]) by
    /// less than `min_gain` ten-thousandths of its score before the pass,
    /// at most `cap` passes. A pass that is not kept lowers it by nothing.
    Until {
        /// In ten-thousandths: 50 is 0.5%.
        min_gain: u32,
        /// The most passes.
        cap: u32,
    },
}

/// [`crate::approximate`]'s pipeline for `shape`, chosen with the engine
/// runner on its default corpus by the median RMSE of the export at 100
/// and 200 shapes. Each stage was first extended while the last extension
/// lowered the mean of the two medians by at least 0.5% without raising
/// the mean of the per-image changes; otherwise the cheaper configuration
/// stayed, by a deterministic count of evaluations. The passes during the
/// search of `any`, triangles and ellipses were then thinned to trade
/// some of that quality for time, on the measurements of
/// `docs/algorithm-leap-review-2026-10-07.md`: the mean of the two
/// medians against the previous `approximate` (16 search rounds, then one
/// refit pass) and the time over the same rows, on an Apple M2 Pro.
///
/// - Every kind but triangles runs refit passes during the search, on a
///   [`During::Spaced`] schedule whose cost grows linearly with the shape
///   count. Before any other change they lowered the median RMSE by 0.4%
///   (quadratics) to 7% (`any`, ellipses and rotated ellipses). As first
///   chosen they were 45–70% of the pipeline's time for `any`, triangles,
///   rectangles, ellipses and circles, and the only stage that lost at
///   equal time to the greedy search with more shapes, while the joint
///   optimisation, at 10–15% of the time for most kinds that run it, is
///   the most efficient stage.
/// - Triangles, polygons, rectangles and rotated rectangles end with the
///   joint optimisation of every shape, rotated rectangles with twice its
///   default iterations; refit passes before it, or more iterations for
///   the other three kinds, gained less than 0.5%. Rotated rectangles
///   refit after steps 20 and 40, then each half the step number: with
///   the doubled iterations, 2.2% below no passes during the search and
///   the default iterations; the doubled iterations alone gained 0.4%,
///   and passes every 20 steps up to step 100 cost more and gained less.
/// - Rectangles and polygons refit every 20 steps up to step 100, then
///   each fifth of the step number (`Spaced(20, 5)`). On triangles, with
///   the joint optimisation, that schedule was 12.9% below the previous
///   `approximate` at 2.06 times its time, against 14.8% at 3.22 times
///   with passes every 5 steps up to step 50, then each tenth
///   (`Spaced(5, 10)`), and 10.6% at 1.11 times with no passes: it keeps
///   55% of the passes' gain for 45% of their cost.
/// - Triangles, on the same schedule, run the joint optimisation of every
///   shape with 20 iterations in place of the refit pass, its result kept
///   when the exact canvas scores lower ([`During::Joint`],
///   [`Guard::Canvas`]): 1.3% below the refit passes at 0.70 times the
///   pipeline's time, where no passes during the search are 2.6% above at
///   0.55 times. Both paintings improve at 50, 100 and 200 shapes
///   (American Gothic by 1.4–2.4%, Mona Lisa by 1.0–1.6%), while
///   synthetic-shapes, on whose hard edges the guard rejects most joint
///   results, is 6.1% worse at 200. With 40 iterations, or with
///   `Spaced(10, 10)`, the gain is about the same at 0.87–0.90 times.
///   The other kinds keep the refit passes: polygons because on
///   synthetic-shapes the joint result is rejected and they lose 33–41%
///   at 100–500 shapes, about as with no passes; rectangles and rotated
///   rectangles because rounding the joint result to their integer
///   parameters gives back most of its gain; `any` because the joint
///   optimisation moves only its triangles, polygons and rectangles,
///   while the refit passes also serve its curved layers.
/// - [`ShapeKind::Any`] refits on the rectangles' schedule and ends with
///   one refit pass, then the joint optimisation of its triangles,
///   polygons and rectangles, every other shape fixed in geometry: 3.2%
///   below the refit pass alone. With 16 search rounds the whole pipeline
///   is 6.9% below the previous `approximate` at 1.57 times its time;
///   with 32 rounds and `Spaced(5, 10)` it was 10.8% below at 2.78 times.
///   At equal time and 200 shapes it is now neutral against the previous
///   greedy search with more shapes (a ratio of 1.01), where it was 18%
///   worse.
/// - The joint optimisation's result is kept only if its export is closer
///   to the target than its input ([`better_export`]): on finely fitted
///   polygons it could double the export's error while its own objective
///   reported a gain.
/// - Quadratics end with refit passes until one gains less than 1%, at
///   most four; with 16 search rounds instead of 32 that lowers the
///   median RMSE by 1.8–10.5% at 50–500 shapes against one pass after the
///   greedy search alone, at about 1.4–1.6 times its evaluations.
/// - Ellipses, circles and rotated ellipses end with one refit pass:
///   after the passes during the search, passes until one gained less
///   than 1%, 0.5% or 0.2% gained less than 0.5%.
/// - Ellipses, as circles and rotated ellipses, refit every 10 steps up
///   to step 100, then each tenth of the step number: 5.1% below the
///   previous `approximate` at 2.15 times its time, against 7.2% at 3.11
///   times with `Spaced(5, 20)` and 3.3% at 1.60 times with
///   `Spaced(20, 5)`. The gain is roughly linear in the passes, so this
///   is a choice of time against quality, not a measured optimum.
///
/// Against the first selection, with the model's search effort retuned
/// alongside, this gives back 2–4 points on `any`, triangles, ellipses
/// and rotated ellipses, and cuts their time from 2.8–3.2 to 1.6–2.2
/// times the previous `approximate`'s.
pub(crate) fn pipeline(shape: ShapeKind) -> Pipeline {
    let spaced = |interval, divisor| During::Spaced { interval, divisor };
    let joint = |interval, divisor, iterations| During::Joint {
        interval,
        divisor,
        iterations,
        guard: Guard::Canvas,
    };
    let (none, one) = (Refits::Passes(0), Refits::Passes(1));
    let (during, refits, joint) = match shape {
        ShapeKind::Any => (spaced(20, 5), one, Some(1)),
        ShapeKind::Triangle => (joint(20, 5, 20), none, Some(1)),
        ShapeKind::Rectangle | ShapeKind::Polygon => (spaced(20, 5), none, Some(1)),
        ShapeKind::RotatedRectangle => (spaced(20, 2), none, Some(2)),
        ShapeKind::Ellipse | ShapeKind::Circle | ShapeKind::RotatedEllipse => {
            (spaced(10, 10), one, None)
        }
        ShapeKind::Quadratic => (
            spaced(20, 5),
            Refits::Until {
                min_gain: 100,
                cap: 4,
            },
            None,
        ),
        // `ShapeKind` is non-exhaustive: a kind added later gets the
        // refit passes, which cover every kind.
        _ => (spaced(10, 10), one, None),
    };
    Pipeline {
        during,
        refits,
        joint,
    }
}

/// Runs the pass that `during` schedules after step `step` (from 1), if
/// any, and returns what ran. Returns `None` once `cancelled` returns true,
/// with the model as it was before the pass.
pub(crate) fn after_step(
    model: &mut Model,
    during: During,
    step: u32,
    alpha: Alpha,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Pass> {
    if !during.due(step) {
        return Some(Pass::Skipped);
    }
    if let During::Joint {
        iterations, guard, ..
    } = during
    {
        let kept = joint_pass(model, iterations, guard, alpha, cancelled)?;
        return Some(Pass::Joint { kept });
    }
    model.refine_unless(alpha, &mut cancelled)?;
    Some(Pass::Refit)
}

/// A joint pass of `iterations` iterations on `model`, its result adopted
/// if `guard` keeps it; returns whether it was kept. With
/// [`Guard::Canvas`] the model's exact canvas decides ([`Model::adopt`]
/// without `force`); with the lab's `Guard::Export` the result is adopted
/// by force into the model, whose own export after adopting must then be
/// strictly closer to the target than its export before
/// ([`exports_closer`]), or a clone taken before restores it. A result the model cannot adopt
/// ([`Model::adopt`] returns `None`) is not kept, and a pass not kept
/// leaves the model as it was. `cancelled` is polled as [`joint::optimise`]
/// polls it, then once more before the guard; once it returns true,
/// `None`, with the model unchanged.
fn joint_pass(
    model: &mut Model,
    iterations: u32,
    guard: Guard,
    alpha: Alpha,
    mut cancelled: impl FnMut() -> bool,
) -> Option<bool> {
    let mut settings = joint::Settings::default();
    settings.iterations = Some(iterations);
    let optimised = joint::optimise(model, alpha, settings, &mut cancelled)?;
    if cancelled() {
        return None;
    }
    let kept = match guard {
        Guard::Canvas => model.adopt(&optimised, false) == Some(true),
        #[cfg(any(test, feature = "lab"))]
        Guard::Export => {
            let before = model.drawing();
            let backup = model.clone();
            let closer = model.adopt(&optimised, true) == Some(true)
                && exports_closer(backup.target(), &before, &model.drawing());
            if !closer {
                *model = backup;
            }
            closer
        }
    };
    Some(kept)
}

/// The final stage of `pipeline` on `model`, after its last step: the
/// refit passes, which change `model`, then the joint optimisation, which
/// does not, with `iterations`, if set, in place of its scaled default.
/// The joint result is kept only if it exports closer to the target than
/// the refitted model's drawing ([`better_export`]). Returns the drawing to
/// encode, or `None` once `cancelled` returns true.
pub(crate) fn final_stage(
    model: &mut Model,
    pipeline: Pipeline,
    alpha: Alpha,
    iterations: Option<u32>,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Drawing> {
    match pipeline.refits {
        Refits::Passes(passes) => {
            for _ in 0..passes {
                model.refine_unless(alpha, &mut cancelled)?;
            }
        }
        Refits::Until { min_gain, cap } => {
            for _ in 0..cap {
                let before = model.score_f64();
                model.refine_unless(alpha, &mut cancelled)?;
                let gain = before - model.score_f64();
                if gain * 10_000.0 < before * f64::from(min_gain) {
                    break;
                }
            }
        }
    }
    let Some(scale) = pipeline.joint else {
        return Some(model.drawing());
    };
    let mut settings = joint::Settings::default();
    settings.iterations = Some(iterations.unwrap_or_else(|| {
        joint::default_iterations(model.drawing().shapes.len()).saturating_mul(scale)
    }));
    let optimised = joint::optimise(model, alpha, settings, cancelled)?;
    Some(better_export(model.target(), model.drawing(), optimised))
}

/// `joint` if its PNG export at the working size is strictly closer to
/// `target` than `input`'s ([`raster::squared_error`]), otherwise `input`:
/// a tie, or a drawing that cannot be rendered, keeps `input`.
///
/// The joint optimisation lowers its own objective, a smooth model of the
/// anti-aliased export, but that model can disagree with the export: the
/// product of half-planes it takes as a polygon's coverage squares where
/// two near-collinear edges cross the same pixels, and the snap to its
/// lattice moves every vertex after the last iteration. On finely fitted
/// stacks the export can then be much worse than the joint optimisation's
/// input while its objective reports a gain, so the guard measures the
/// export itself.
///
/// The comparison is exact integer arithmetic over the PNG writer's
/// raster, in one thread, so it does not depend on the number of threads.
/// That raster (tiny-skia's high-precision pipeline) computes in `f32`
/// with `+ − × ÷` and `sqrt`, converts to fixed point with `as`, and
/// rounds to nearest even, alike on NEON, SSE2 and WebAssembly's scalar
/// path, so native and WebAssembly builds agree; the one platform
/// function it calls is the `sin` and `cos` of a rotated ellipse's angle,
/// which only `any` can draw and which the export itself depends on too.
fn better_export(target: &Buffer, input: Drawing, joint: Drawing) -> Drawing {
    if exports_closer(target, &input, &joint) {
        joint
    } else {
        input
    }
}

/// Whether `candidate`'s PNG export at the working size is strictly closer
/// to `target` than `input`'s ([`raster::squared_error`]): [`better_export`]'s
/// rule. A drawing that cannot be rendered is as far as can be, so a tie,
/// or a `candidate` that cannot be rendered, is not closer.
fn exports_closer(target: &Buffer, input: &Drawing, candidate: &Drawing) -> bool {
    let error = |drawing: &Drawing| raster::squared_error(drawing, target).unwrap_or(u64::MAX);
    error(candidate) < error(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primeval_core::{Color, ModelOptions};

    /// A 48 × 40 model of `shape` after `steps` greedy steps.
    fn model(shape: ShapeKind, steps: u32) -> Model {
        sized_model(shape, steps, 48, 40)
    }

    /// A `width` × `height` model of `shape` after `steps` greedy steps.
    fn sized_model(shape: ShapeKind, steps: u32, width: u32, height: u32) -> Model {
        let pixels = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                let ring = (x as i32 - 20).pow(2) + (y as i32 - 18).pow(2) < 150;
                [
                    (x * 5) as u8,
                    if ring { 230 } else { (y * 6) as u8 },
                    ((x + 2 * y) % 30 * 8) as u8,
                ]
            })
            .collect();
        let target = Buffer::from_rgb(width, height, pixels).expect("target");
        let mut options = ModelOptions::default();
        options.seed = Some(5);
        let mut model = Model::new(target, Color::new(128, 128, 128, 255), options);
        for _ in 0..steps {
            model.step(shape, Alpha::Auto);
        }
        model
    }

    /// The pixels of a `side` × `side` point-sampled version of the engine
    /// runner's `synthetic-shapes` image: flat shapes with hard edges.
    fn hard_shapes(side: u32) -> Vec<u8> {
        (0..side * side)
            .flat_map(|i| {
                let (u, v) = (
                    f64::from(i % side) / f64::from(side),
                    f64::from(i / side) / f64::from(side),
                );
                if (u - 0.3).powi(2) + (v - 0.3).powi(2) < 0.04 {
                    [200, 30, 40]
                } else if (0.55..0.9).contains(&u) && (0.15..0.45).contains(&v) {
                    [30, 90, 200]
                } else if v > 0.55 && v < 0.95 && (u - 0.5).abs() < (v - 0.55) {
                    [240, 200, 30]
                } else if (0.05..0.25).contains(&u) && v > 0.6 {
                    [20, 20, 20]
                } else {
                    [235, 235, 225]
                }
            })
            .collect()
    }

    /// A 24 × 24 model of [`hard_shapes`] after `steps` greedy steps of
    /// `shape` with `seed`, and a refit pass after every fifth.
    fn hard_shapes_model(shape: ShapeKind, seed: u64, steps: u32) -> Model {
        let target = Buffer::from_rgb(24, 24, hard_shapes(24)).expect("target");
        let mut options = ModelOptions::default();
        options.seed = Some(seed);
        let mut model = Model::new(target, Color::new(128, 128, 128, 255), options);
        for step in 1..=steps {
            model.step(shape, Alpha::Auto);
            after_step(&mut model, During::Every(5), step, Alpha::Auto, || false)
                .expect("not cancelled");
        }
        model
    }

    /// The sum of the squared differences between `drawing`, rendered by
    /// the PNG writer at its own size, and `target`.
    fn png_error(drawing: &Drawing, target: &[u8]) -> u64 {
        crate::raster::render_rgb(drawing, drawing.width, drawing.height)
            .expect("raster")
            .iter()
            .zip(target)
            .map(|(&a, &b)| u64::from(a.abs_diff(b)).pow(2))
            .sum()
    }

    /// The joint optimisation's own result for `model`, at `scale` times
    /// its default iterations, as the final stage runs it.
    fn joint_result(model: &Model, scale: u32) -> Drawing {
        let mut settings = joint::Settings::default();
        settings.iterations = Some(joint::default_iterations(model.drawing().shapes.len()) * scale);
        joint::optimise(model, Alpha::Auto, settings, || false).expect("not cancelled")
    }

    /// Every shape kind, `any` included.
    const KINDS: [ShapeKind; 9] = [
        ShapeKind::Any,
        ShapeKind::Triangle,
        ShapeKind::Rectangle,
        ShapeKind::Ellipse,
        ShapeKind::Circle,
        ShapeKind::RotatedRectangle,
        ShapeKind::Quadratic,
        ShapeKind::RotatedEllipse,
        ShapeKind::Polygon,
    ];

    fn stages(refits: Refits, joint: Option<u32>) -> Pipeline {
        Pipeline {
            during: During::Never,
            refits,
            joint,
        }
    }

    fn due_steps(during: During, last: u32) -> Vec<u32> {
        (1..=last).filter(|&step| during.due(step)).collect()
    }

    #[test]
    fn every_runs_a_pass_after_each_multiple_of_its_interval() {
        assert_eq!(due_steps(During::Every(20), 70), [20, 40, 60]);
        assert_eq!(due_steps(During::Never, 70), [] as [u32; 0]);
    }

    /// Every 10 steps up to 50, then each step / 5 steps after the
    /// previous pass, rounded down.
    #[test]
    fn spaced_passes_follow_the_interval_then_spread_out() {
        let spaced = During::Spaced {
            interval: 10,
            divisor: 5,
        };
        assert_eq!(
            due_steps(spaced, 220),
            [10, 20, 30, 40, 50, 60, 72, 86, 103, 123, 147, 176, 211]
        );
    }

    /// The passes of a spaced schedule refit about `divisor` layers per
    /// step, at most `(divisor + 1) · N + interval · divisor · (divisor +
    /// 1) / 2` in all up to step `N`, so their cost grows linearly with
    /// the shape count; every 10 steps would refit `N² / 20`.
    #[test]
    fn spaced_passes_refit_linearly_many_layers() {
        for (interval, divisor) in [(10, 5), (10, 10), (5, 20), (20, 2), (20, 5)] {
            let spaced = During::Spaced { interval, divisor };
            for last in [500, 2000, 100_000] {
                let layers: u64 = due_steps(spaced, last).iter().map(|&s| u64::from(s)).sum();
                let bound = u64::from(divisor + 1) * u64::from(last)
                    + u64::from(interval * divisor * (divisor + 1) / 2);
                assert!(layers <= bound, "{interval}:{divisor} to {last}: {layers}");
            }
        }
    }

    /// The passes during the search are the pipeline's most expensive
    /// stage and the only one that loses at equal time
    /// (`docs/algorithm-leap-review-2026-10-07.md`): up to 200 steps they
    /// refit, or jointly optimise, at most 5 layers per step for the kinds
    /// that end with the joint optimisation, which gains more for less,
    /// and at most 10 for the others.
    #[test]
    fn passes_during_the_search_stay_cheap() {
        for shape in KINDS {
            let stages = pipeline(shape);
            let layers: u32 = due_steps(stages.during, 200).iter().sum();
            let most = if stages.joint.is_some() { 5 } else { 10 };
            assert!(layers <= most * 200, "{shape:?}: {layers} layers");
        }
    }

    /// Every kind's path through the pipeline, its greedy steps at its
    /// search effort, a refit pass in the search and its final stage, gives
    /// the same drawing at 1, 2, 4 and 8 threads. The schedule's steps
    /// depend only on the step number (above), and the lab identity test
    /// runs the whole of `approximate`'s path.
    #[test]
    fn every_kind_path_is_identical_across_thread_counts() {
        for shape in KINDS {
            let stages = pipeline(shape);
            let on_threads = |threads| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .expect("test thread pool");
                pool.install(|| {
                    let mut model = sized_model(shape, 3, 24, 18);
                    after_step(&mut model, During::Every(3), 3, Alpha::Auto, || false)
                        .expect("not cancelled");
                    final_stage(&mut model, stages, Alpha::Auto, None, || false)
                        .expect("not cancelled")
                })
            };
            let reference = on_threads(1);
            for threads in [2, 4, 8] {
                assert!(
                    on_threads(threads) == reference,
                    "{shape:?}: {threads} threads changed the drawing"
                );
            }
        }
    }

    /// On a finely fitted stack of polygons the joint optimisation's
    /// coverage model disagrees with the exported image, and its result
    /// is far worse once rendered; the final stage then returns its input,
    /// the drawing after the refit passes.
    #[test]
    fn the_final_stage_keeps_its_input_when_the_joint_result_exports_worse() {
        let target = hard_shapes(24);
        let model = hard_shapes_model(ShapeKind::Polygon, 2, 25);
        let input = model.drawing();
        let joint = joint_result(&model, 1);
        assert!(
            png_error(&joint, &target) > 2 * png_error(&input, &target),
            "the joint optimisation no longer worsens this case"
        );
        let kept = final_stage(
            &mut model.clone(),
            stages(Refits::Passes(0), Some(1)),
            Alpha::Auto,
            None,
            || false,
        );
        assert_eq!(kept, Some(input));
    }

    /// Where the joint result exports closer to the target, the final
    /// stage returns it.
    #[test]
    fn the_final_stage_keeps_the_joint_result_when_it_exports_better() {
        let target = hard_shapes(24);
        let model = hard_shapes_model(ShapeKind::Triangle, 2, 20);
        let input = model.drawing();
        let joint = joint_result(&model, 1);
        assert!(
            png_error(&joint, &target) < png_error(&input, &target),
            "the joint optimisation no longer improves this case"
        );
        let kept = final_stage(
            &mut model.clone(),
            stages(Refits::Passes(0), Some(1)),
            Alpha::Auto,
            None,
            || false,
        );
        assert_eq!(kept, Some(joint));
    }

    /// Two drawings that export to the same pixels tie, and a tie keeps
    /// the input; the joint result must be strictly closer.
    #[test]
    fn a_tie_keeps_the_input() {
        let target = Buffer::from_rgb(4, 4, vec![90; 48]).expect("target");
        let drawing = |x| Drawing {
            width: 4,
            height: 4,
            background: Color::new(90, 90, 90, 255),
            shapes: vec![primeval_core::DrawnShape {
                geometry: primeval_core::Geometry::Rect {
                    x,
                    y: 0.0,
                    width: 2.0,
                    height: 2.0,
                },
                color: Color::new(255, 0, 0, 0),
            }],
        };
        let (input, joint) = (drawing(0.0), drawing(2.0));
        assert_eq!(better_export(&target, input.clone(), joint.clone()), input);
        assert_eq!(better_export(&target, joint.clone(), input), joint);
    }

    /// `Until` runs passes until the first whose relative gain in the
    /// model's score is below the threshold, that pass included, or the
    /// cap: the same drawing as that many plain passes.
    #[test]
    fn refits_until_stop_at_the_first_pass_below_the_gain() {
        let greedy = model(ShapeKind::Ellipse, 12);
        let mut plain = greedy.clone();
        let mut scores = vec![plain.score_f64()];
        for _ in 0..6 {
            plain.refine(Alpha::Auto);
            scores.push(plain.score_f64());
        }
        let mut counts = Vec::new();
        for min_gain in [0, 20, 50, 100, 400, 10_000] {
            let expected = (1..=6)
                .find(|&p| {
                    (scores[p - 1] - scores[p]) * 10_000.0 < scores[p - 1] * f64::from(min_gain)
                })
                .unwrap_or(6);
            counts.push(expected);
            let until = final_stage(
                &mut greedy.clone(),
                stages(Refits::Until { min_gain, cap: 6 }, None),
                Alpha::Auto,
                None,
                || false,
            );
            let passes = final_stage(
                &mut greedy.clone(),
                stages(Refits::Passes(expected as u32), None),
                Alpha::Auto,
                None,
                || false,
            );
            assert_eq!(until, passes, "min_gain {min_gain}");
        }
        counts.dedup();
        assert!(counts.len() >= 3, "the thresholds stop alike: {counts:?}");
        assert_eq!(counts.first(), Some(&6));
        assert_eq!(counts.last(), Some(&1));
    }

    /// The stop rule reads only deterministic scores: the stage gives the
    /// same drawing whatever the number of threads.
    #[test]
    fn refits_until_do_not_depend_on_the_thread_count() {
        let greedy = model(ShapeKind::RotatedEllipse, 12);
        let on_threads = |threads| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("test thread pool");
            pool.install(|| {
                final_stage(
                    &mut greedy.clone(),
                    stages(
                        Refits::Until {
                            min_gain: 50,
                            cap: 16,
                        },
                        None,
                    ),
                    Alpha::Auto,
                    None,
                    || false,
                )
            })
        };
        let reference = on_threads(1);
        assert!(reference.is_some());
        for threads in [2, 4, 8] {
            assert_eq!(on_threads(threads), reference, "{threads} threads");
        }
    }

    /// A refit pass in the search polls `cancelled` before each layer;
    /// once it returns true the pass stops and leaves the model as it
    /// was, which `approximate` turns into
    /// [`crate::ApproximateError::Aborted`]. A step with no pass due
    /// polls nothing.
    #[test]
    fn cancellation_during_a_refit_in_the_search_stops_it() {
        let greedy = model(ShapeKind::Ellipse, 10);
        let during = During::Every(10);
        let mut polls = 0;
        let mut refitted = greedy.clone();
        let finished = after_step(&mut refitted, during, 10, Alpha::Auto, || {
            polls += 1;
            false
        });
        assert_eq!(finished, Some(Pass::Refit));
        assert_ne!(
            refitted.drawing(),
            greedy.drawing(),
            "the pass changed nothing"
        );
        assert!(polls > 10, "{polls} polls");
        for cancel_at in [1, polls / 2, polls] {
            let mut model = greedy.clone();
            let mut count = 0;
            let stopped = after_step(&mut model, during, 10, Alpha::Auto, || {
                count += 1;
                count >= cancel_at
            });
            assert_eq!(stopped, None, "cancelled at poll {cancel_at}");
            assert_eq!(model.drawing(), greedy.drawing());
        }
        let mut model = greedy.clone();
        assert_eq!(
            after_step(&mut model, during, 9, Alpha::Auto, || panic!("polled")),
            Some(Pass::Skipped)
        );
    }

    /// Every loop of the final stage polls `cancelled`: the fixed refit
    /// passes, the passes until the gain falls, and the joint
    /// optimisation after them.
    #[test]
    fn cancellation_stops_every_loop_of_the_final_stage() {
        for (shape, pipeline) in [
            (ShapeKind::Ellipse, stages(Refits::Passes(3), None)),
            (
                ShapeKind::Ellipse,
                stages(
                    Refits::Until {
                        min_gain: 0,
                        cap: 3,
                    },
                    None,
                ),
            ),
            (ShapeKind::Triangle, stages(Refits::Passes(2), Some(1))),
            (
                ShapeKind::Triangle,
                stages(
                    Refits::Until {
                        min_gain: 0,
                        cap: 2,
                    },
                    Some(2),
                ),
            ),
        ] {
            let greedy = model(shape, 6);
            let mut polls = 0;
            let finished = final_stage(&mut greedy.clone(), pipeline, Alpha::Auto, None, || {
                polls += 1;
                false
            });
            assert!(finished.is_some());
            for cancel_at in [1, polls / 3, 2 * polls / 3, polls] {
                let mut count = 0;
                let stopped = final_stage(&mut greedy.clone(), pipeline, Alpha::Auto, None, || {
                    count += 1;
                    count >= cancel_at
                });
                assert_eq!(stopped, None, "{pipeline:?}: cancelled at poll {cancel_at}");
            }
        }
    }

    /// A joint pass in the search, every `interval` steps up to
    /// `interval · divisor`, then spaced, of `iterations` iterations.
    fn joint(interval: u32, divisor: u32, iterations: u32, guard: Guard) -> During {
        During::Joint {
            interval,
            divisor,
            iterations,
            guard,
        }
    }

    /// The kinds the joint optimisation moves, `any` included.
    const JOINT_KINDS: [ShapeKind; 5] = [
        ShapeKind::Any,
        ShapeKind::Triangle,
        ShapeKind::Rectangle,
        ShapeKind::RotatedRectangle,
        ShapeKind::Polygon,
    ];

    #[test]
    fn joint_passes_are_due_at_the_spaced_steps() {
        for (interval, divisor) in [(10, 5), (20, 5), (5, 2), (20, 2), (3, 1)] {
            let spaced = due_steps(During::Spaced { interval, divisor }, 600);
            for guard in [Guard::Canvas, Guard::Export] {
                let joint = joint(interval, divisor, 7, guard);
                assert_eq!(due_steps(joint, 600), spaced, "{interval}:{divisor}");
            }
        }
    }

    /// Triangles run the joint optimisation during the search, kept by the
    /// exact canvas; every other kind runs refit passes, so a change to
    /// another kind's schedule is deliberate.
    #[test]
    fn only_triangles_run_joint_passes_during_the_search() {
        for shape in KINDS {
            let during = pipeline(shape).during;
            if shape == ShapeKind::Triangle {
                assert_eq!(during, joint(20, 5, 20, Guard::Canvas));
            } else {
                assert!(
                    matches!(during, During::Spaced { .. }),
                    "{shape:?}: {during:?}"
                );
            }
        }
    }

    /// A model of `shape` after `steps` greedy steps, each followed by the
    /// pass `during` schedules, and what each pass did.
    fn joint_model(shape: ShapeKind, during: During, steps: u32) -> (Model, Vec<Pass>) {
        let mut model = model(shape, 0);
        let passes = (1..=steps)
            .map(|step| {
                model.step(shape, Alpha::Auto);
                after_step(&mut model, during, step, Alpha::Auto, || false).expect("not cancelled")
            })
            .collect();
        (model, passes)
    }

    /// With the canvas guard a joint pass is kept only if the model's
    /// exact canvas scores strictly lower, and otherwise leaves the model
    /// as it was.
    #[test]
    fn a_joint_pass_with_the_canvas_guard_never_raises_the_score() {
        let during = joint(3, 2, 10, Guard::Canvas);
        let mut kept = 0;
        for shape in JOINT_KINDS {
            let mut model = model(shape, 0);
            for step in 1..=12 {
                model.step(shape, Alpha::Auto);
                let before = model.clone();
                let pass = after_step(&mut model, during, step, Alpha::Auto, || false)
                    .expect("not cancelled");
                let context = format!("{shape:?}, step {step}");
                match pass {
                    Pass::Joint { kept: true } => {
                        kept += 1;
                        assert!(model.score_f64() < before.score_f64(), "{context}");
                    }
                    Pass::Joint { kept: false } => {
                        assert_eq!(model.drawing(), before.drawing(), "{context}");
                        assert_eq!(model.score_f64(), before.score_f64(), "{context}");
                    }
                    Pass::Skipped => assert!(!during.due(step), "{context}"),
                    Pass::Refit => panic!("{context}: a refit pass"),
                }
            }
        }
        assert!(kept >= 10, "only {kept} passes kept");
    }

    /// With the export guard a joint pass is kept only if the model's own
    /// PNG export at the working size, after adopting, is strictly closer
    /// to the target, so it never moves further from the target, also for
    /// the rectangles that adopting rounds; otherwise the model is left as
    /// it was.
    #[test]
    fn a_joint_pass_with_the_export_guard_never_raises_the_export_error() {
        let during = joint(3, 2, 10, Guard::Export);
        let mut kept = 0;
        for shape in JOINT_KINDS {
            let mut model = model(shape, 0);
            let error = |model: &Model| {
                raster::squared_error(&model.drawing(), model.target()).expect("raster")
            };
            for step in 1..=12 {
                model.step(shape, Alpha::Auto);
                let before = model.clone();
                let pass = after_step(&mut model, during, step, Alpha::Auto, || false)
                    .expect("not cancelled");
                let context = format!("{shape:?}, step {step}");
                match pass {
                    Pass::Joint { kept: true } => {
                        kept += 1;
                        assert!(error(&model) < error(&before), "{context}");
                    }
                    Pass::Joint { kept: false } => {
                        assert_eq!(model.drawing(), before.drawing(), "{context}");
                        assert_eq!(model.score_f64(), before.score_f64(), "{context}");
                    }
                    Pass::Skipped => assert!(!during.due(step), "{context}"),
                    Pass::Refit => panic!("{context}: a refit pass"),
                }
            }
        }
        // 15 of the 20 passes are kept.
        assert!(kept >= 10, "only {kept} passes kept");
    }

    /// A joint pass in the search polls `cancelled` before every iteration,
    /// before the snap and once more before adopting; a cancel at any of
    /// those polls stops it with the model as it was.
    #[test]
    fn cancellation_during_a_joint_pass_in_the_search_stops_it() {
        for guard in [Guard::Canvas, Guard::Export] {
            let greedy = model(ShapeKind::Triangle, 10);
            let during = joint(10, 1, 5, guard);
            let mut polls = 0;
            let mut adopted = greedy.clone();
            let finished = after_step(&mut adopted, during, 10, Alpha::Auto, || {
                polls += 1;
                false
            });
            assert_eq!(finished, Some(Pass::Joint { kept: true }), "{guard:?}");
            assert_ne!(adopted.drawing(), greedy.drawing(), "{guard:?}");
            // Five iterations, the snap and the adoption.
            assert_eq!(polls, 7, "{guard:?}");
            for cancel_at in 1..=polls {
                let mut model = greedy.clone();
                let mut count = 0;
                let stopped = after_step(&mut model, during, 10, Alpha::Auto, || {
                    count += 1;
                    count >= cancel_at
                });
                assert_eq!(stopped, None, "{guard:?}: cancelled at poll {cancel_at}");
                assert_eq!(model.drawing(), greedy.drawing(), "{guard:?}");
                assert_eq!(model.score_f64(), greedy.score_f64(), "{guard:?}");
            }
        }
    }

    /// The search with joint passes gives the same drawing at 1, 2, 4 and
    /// 8 threads, with the canvas guard for every kind the joint
    /// optimisation moves and with the export guard for two of them; the
    /// final stage after it does too
    /// (`every_kind_path_is_identical_across_thread_counts`).
    #[test]
    fn joint_passes_are_identical_across_thread_counts() {
        let export = [ShapeKind::Triangle, ShapeKind::Any].map(|shape| (shape, Guard::Export));
        let mut kept = 0;
        for (shape, guard) in JOINT_KINDS
            .map(|shape| (shape, Guard::Canvas))
            .into_iter()
            .chain(export)
        {
            let on_threads = |threads| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .expect("test thread pool");
                pool.install(|| {
                    let mut model = sized_model(shape, 0, 24, 18);
                    let mut passes = Vec::new();
                    for step in 1..=4 {
                        model.step(shape, Alpha::Auto);
                        // Passes after steps 1, 2 and 4.
                        let during = joint(1, 1, 6, guard);
                        passes.push(
                            after_step(&mut model, during, step, Alpha::Auto, || false)
                                .expect("not cancelled"),
                        );
                    }
                    (model.drawing(), passes)
                })
            };
            let reference = on_threads(1);
            kept += reference
                .1
                .iter()
                .filter(|&&pass| pass == Pass::Joint { kept: true })
                .count();
            for threads in [2, 4, 8] {
                assert!(
                    on_threads(threads) == reference,
                    "{shape:?} {guard:?}: {threads} threads changed the drawing"
                );
            }
        }
        assert!(kept >= 7, "only {kept} passes kept");
    }

    /// Greedy steps build on an adopted joint result, whose triangles are
    /// now polygons: every step still lowers the score or keeps it.
    #[test]
    fn greedy_steps_continue_after_an_adopted_joint_pass() {
        for shape in [ShapeKind::Triangle, ShapeKind::Any] {
            let (mut model, passes) = joint_model(shape, joint(5, 1, 10, Guard::Canvas), 5);
            assert_eq!(
                passes.last(),
                Some(&Pass::Joint { kept: true }),
                "{shape:?}"
            );
            for _ in 0..5 {
                let before = model.score_f64();
                model.step(shape, Alpha::Auto);
                assert!(model.score_f64() <= before, "{shape:?}");
            }
            assert_eq!(model.drawing().shapes.len(), 10, "{shape:?}");
        }
    }
}
