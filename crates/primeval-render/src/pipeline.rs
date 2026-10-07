//! What [`crate::approximate`] runs around its greedy steps, per shape
//! kind: refit passes during the search, and the final stage.
//!
//! `lab` drives the same functions, so the engine runner cannot drift from
//! [`crate::approximate`]. Every rule depends only on the request and on
//! deterministic scores, never on elapsed time or on the number of threads.

use crate::{ShapeKind, raster};
use primeval_core::{Alpha, Buffer, Drawing, Model, joint};

/// The stages around the greedy steps of one shape kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pipeline {
    /// Refit passes of the model during the search.
    pub(crate) during: During,
    /// Refit passes of the model after the last step.
    pub(crate) refits: Refits,
    /// After the refit passes, the joint optimisation
    /// ([`joint::optimise`]) with this multiple of its default iteration
    /// count ([`joint::default_iterations`]), or `None` for none.
    pub(crate) joint: Option<u32>,
}

/// When the search runs a refit pass ([`Model::refine`]) of the model
/// itself, after a step. Later steps build on the refitted shapes.
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
}

impl During {
    /// Whether a pass runs after step `step` (from 1).
    pub(crate) fn due(self, step: u32) -> bool {
        match self {
            #[cfg(any(test, feature = "lab"))]
            Self::Never => false,
            #[cfg(any(test, feature = "lab"))]
            Self::Every(every) => step.is_multiple_of(every),
            Self::Spaced { interval, divisor } => {
                let mut next = u64::from(interval);
                let step = u64::from(step);
                while next < step {
                    next += u64::from(interval).max(next / u64::from(divisor));
                }
                next == step
            }
        }
    }
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
/// - Every kind runs refit passes during the search, on a [`During::Spaced`]
///   schedule whose cost grows linearly with the shape count. Before any
///   other change they lowered the median RMSE by 0.4% (quadratics) to 7%
///   (`any`, ellipses and rotated ellipses). As first chosen they were
///   45–70% of the pipeline's time for `any`, triangles, rectangles,
///   ellipses and circles, and the only stage that lost at equal time to
///   the greedy search with more shapes, while the joint optimisation, at
///   10–15% of the time for most kinds that run it, is the most efficient
///   stage.
/// - Triangles, polygons, rectangles and rotated rectangles end with the
///   joint optimisation of every shape, rotated rectangles with twice its
///   default iterations; refit passes before it, or more iterations for
///   the other three kinds, gained less than 0.5%. Rotated rectangles
///   refit after steps 20 and 40, then each half the step number: with
///   the doubled iterations, 2.2% below no passes during the search and
///   the default iterations; the doubled iterations alone gained 0.4%,
///   and passes every 20 steps up to step 100 cost more and gained less.
/// - Triangles, as rectangles and polygons, refit every 20 steps up to
///   step 100, then each fifth of the step number: with the joint
///   optimisation, 12.9% below the previous `approximate` at 2.06 times
///   its time, against 14.8% at 3.22 times with passes every 5 steps up
///   to step 50, then each tenth (`Spaced(5, 10)`). The joint
///   optimisation alone, with no passes during the search, is 10.6% below
///   at 1.11 times, so the passes are what cost: `Spaced(20, 5)` keeps
///   55% of their gain for 45% of their cost.
/// - [`ShapeKind::Any`] refits on the same schedule as triangles and ends
///   with one refit pass, then the joint optimisation of its triangles,
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
    let (none, one) = (Refits::Passes(0), Refits::Passes(1));
    let (during, refits, joint) = match shape {
        ShapeKind::Any => (spaced(20, 5), one, Some(1)),
        ShapeKind::Triangle | ShapeKind::Rectangle | ShapeKind::Polygon => {
            (spaced(20, 5), none, Some(1))
        }
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

/// Runs the refit pass that `during` schedules after step `step` (from 1),
/// if any. Returns `None` once `cancelled` returns true, with the model as
/// it was before the pass.
pub(crate) fn after_step(
    model: &mut Model,
    during: During,
    step: u32,
    alpha: Alpha,
    cancelled: impl FnMut() -> bool,
) -> Option<()> {
    if during.due(step) {
        model.refine_unless(alpha, cancelled)?;
    }
    Some(())
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
    let error = |drawing: &Drawing| raster::squared_error(drawing, target).unwrap_or(u64::MAX);
    if error(&joint) < error(&input) {
        joint
    } else {
        input
    }
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
    /// refit at most 5 layers per step for the kinds that end with the
    /// joint optimisation, which gains more for less, and at most 10 for
    /// the others.
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
        assert_eq!(finished, Some(()));
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
            Some(())
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
}
