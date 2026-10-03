//! What [`crate::approximate`] runs around its greedy steps, per shape
//! kind: refit passes during the search, and the final stage.
//!
//! `lab` drives the same functions, so the engine runner cannot drift from
//! [`crate::approximate`]. Every rule depends only on the request and on
//! deterministic scores, never on elapsed time or on the number of threads.

use crate::ShapeKind;
use primeval_core::{Alpha, Drawing, Model, joint};

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
    #[cfg(any(test, feature = "lab"))]
    Until {
        /// In ten-thousandths: 50 is 0.5%.
        min_gain: u32,
        /// The most passes.
        cap: u32,
    },
}

/// [`crate::approximate`]'s pipeline for `shape`, chosen with the engine
/// runner on its default corpus by the median RMSE of the export at 100
/// and 200 shapes: each stage was extended while the last extension
/// lowered the mean of the two medians by at least 0.5%.
///
/// - Every kind runs refit passes during the search, on a [`During::Spaced`]
///   schedule whose cost grows linearly with the shape count. Before any
///   other change they lowered the median RMSE by 0.4% (quadratics) to 7%
///   (`any`, ellipses and rotated ellipses).
/// - Triangles, polygons, rectangles and rotated rectangles end with the
///   joint optimisation of every shape, rotated rectangles with twice its
///   default iterations; refit passes before it, or more iterations for
///   the other three kinds, gained less than 0.5%.
/// - [`ShapeKind::Any`] ends with one refit pass, then the joint
///   optimisation of its triangles, polygons and rectangles, every other
///   shape fixed in geometry: 3.2% below the refit pass alone.
/// - Ellipses, circles, rotated ellipses and quadratics end with one refit
///   pass: after the passes during the search, passes until one gained
///   less than 1%, 0.5% or 0.2% gained less than 0.5%.
///
/// With the model's search effort per kind, the whole pipeline lowers the
/// median RMSE by 1–11% against the greedy search followed by the final
/// stage alone.
pub(crate) fn pipeline(shape: ShapeKind) -> Pipeline {
    let spaced = |interval, divisor| During::Spaced { interval, divisor };
    let (during, refits, joint) = match shape {
        ShapeKind::Any => (spaced(5, 10), 1, Some(1)),
        ShapeKind::Triangle => (spaced(5, 10), 0, Some(1)),
        ShapeKind::Rectangle | ShapeKind::Polygon => (spaced(20, 5), 0, Some(1)),
        ShapeKind::RotatedRectangle => (spaced(20, 5), 0, Some(2)),
        ShapeKind::Ellipse => (spaced(5, 20), 1, None),
        ShapeKind::Circle | ShapeKind::RotatedEllipse => (spaced(10, 10), 1, None),
        ShapeKind::Quadratic => (spaced(20, 5), 1, None),
        // `ShapeKind` is non-exhaustive: a kind added later gets the
        // refit passes, which cover every kind.
        _ => (spaced(10, 10), 1, None),
    };
    Pipeline {
        during,
        refits: Refits::Passes(refits),
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
/// Returns the drawing to encode, or `None` once `cancelled` returns true.
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
        #[cfg(any(test, feature = "lab"))]
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
    joint::optimise(model, alpha, settings, cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primeval_core::{Buffer, Color, ModelOptions};

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
        for (interval, divisor) in [(10, 5), (10, 10), (5, 20)] {
            let spaced = During::Spaced { interval, divisor };
            for last in [500, 2000, 100_000] {
                let layers: u64 = due_steps(spaced, last).iter().map(|&s| u64::from(s)).sum();
                let bound = u64::from(divisor + 1) * u64::from(last)
                    + u64::from(interval * divisor * (divisor + 1) / 2);
                assert!(layers <= bound, "{interval}:{divisor} to {last}: {layers}");
            }
        }
    }

    /// Every kind's path through the pipeline, its greedy steps at its
    /// search effort, a refit pass in the search and its final stage, gives
    /// the same drawing at 1, 2, 4 and 8 threads. The schedule's steps
    /// depend only on the step number (above), and the lab identity test
    /// runs the whole of `approximate`'s path.
    #[test]
    fn every_kind_path_is_identical_across_thread_counts() {
        for shape in [
            ShapeKind::Any,
            ShapeKind::Triangle,
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Circle,
            ShapeKind::RotatedRectangle,
            ShapeKind::Quadratic,
            ShapeKind::RotatedEllipse,
            ShapeKind::Polygon,
        ] {
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
