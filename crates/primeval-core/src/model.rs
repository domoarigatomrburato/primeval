use crate::alpha::Alpha;
use crate::coarse::Coarse;
use crate::drawing::{Drawing, DrawnShape};
use crate::error_grid::ErrorGrid;
use crate::score;
use crate::shapes::{Shape, ShapeKind};
use crate::state::State;
use crate::worker::{SearchRound, WorkerCtx};
use crate::{Buffer, Color};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

/// A shape the model has painted, with the colour it was painted in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CommittedShape {
    pub(crate) shape: Shape,
    pub(crate) color: Color,
}

/// How hard [`Model::step`] searches: its independent search rounds, of
/// which the best is painted, and the multiples of each round's random
/// candidates and climb age ([`Model::search_params`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Effort {
    rounds: u64,
    candidates: usize,
    age: usize,
}

impl Effort {
    /// The effort for `kind`: 16 rounds, or 32 for [`ShapeKind::Any`],
    /// polygons, quadratics and rotated ellipses, whose quadratics and
    /// rotated ellipses also climb twice as long.
    ///
    /// Chosen on the engine runner's corpus, with the passes and final
    /// stage `primeval-render` runs around the search, by the median RMSE
    /// of the export at 100 and 200 shapes: twice the rounds lowered it by
    /// 0.7–3.6% for those kinds, and twice the age by 0.5–1.0% more for
    /// quadratics and rotated ellipses. Neither gained 0.5% for the other
    /// kinds, nor did four times either.
    const fn of(kind: ShapeKind) -> Self {
        let (rounds, age) = match kind {
            ShapeKind::Any | ShapeKind::Polygon => (32, 1),
            ShapeKind::Quadratic | ShapeKind::RotatedEllipse => (32, 2),
            ShapeKind::Triangle
            | ShapeKind::Rectangle
            | ShapeKind::Ellipse
            | ShapeKind::Circle
            | ShapeKind::RotatedRectangle => (16, 1),
        };
        Self {
            rounds,
            candidates: 1,
            age,
        }
    }
}

/// Search settings for a [`Model`].
///
/// Construct with [`ModelOptions::default`] and set the fields you need.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct ModelOptions {
    /// Deterministic RNG seed. `None` seeds from the platform's entropy source.
    ///
    /// The same seed gives the same output for the same version on the same
    /// platform, whatever the number of threads.
    pub seed: Option<u64>,
    /// Columns of the error grid that biases sampling; `0` is treated as `1`.
    pub grid_cols: u32,
    /// Rows of the error grid that biases sampling; `0` is treated as `1`.
    pub grid_rows: u32,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            seed: None,
            grid_cols: 16,
            grid_rows: 16,
        }
    }
}

/// The search: paints shapes one [`step`](Model::step) at a time to
/// approximate a target image, and can [`refine`](Model::refine) the shapes
/// it painted.
///
/// A clone is an independent copy of the search, which continues exactly as
/// the original would.
#[derive(Clone)]
pub struct Model {
    background: Color,
    target: Buffer,
    current: Buffer,
    /// Raw squared difference between `current` and `target`.
    score: u64,
    history: Vec<CommittedShape>,
    error_grid: ErrorGrid,
    /// The half-resolution copy the random phase scores against, for
    /// targets of at least [`crate::coarse::MIN_SIDE`] on both sides.
    coarse: Option<Coarse>,
    seed: u64,
    /// Refit passes run so far, the pass index of the next
    /// [`Model::refine`]'s random streams.
    passes: u64,
    /// Scratch for rasterizing the shape that [`Model::add`] paints.
    scratch: WorkerCtx<ChaCha8Rng>,
    /// The search effort of [`Model::step`] for every kind, set only by
    /// the lab hook; otherwise each kind's [`Effort::of`].
    effort: Option<Effort>,
    /// Whether the search may stop evaluations early; see
    /// `WorkerCtx::pruning`.
    #[cfg(test)]
    pruning: bool,
    /// Whether every refit pass is reverted, as if it had not improved.
    #[cfg(test)]
    reject_passes: bool,
}

impl Model {
    fn search_params(kind: ShapeKind) -> (usize, usize) {
        match kind {
            ShapeKind::Quadratic => (900, 100),
            _ => (1000, 100),
        }
    }

    /// Starts a search for `target` from a canvas filled with `background`.
    ///
    /// `target` is at least 2 x 2 pixels: [`Buffer::from_rgb`] rejects
    /// anything smaller, so a model for an empty or one-pixel-wide canvas
    /// cannot be created. The canvas is opaque RGB, so `background` should
    /// be opaque: its alpha is ignored.
    #[must_use]
    pub fn new(target: Buffer, background: Color, options: ModelOptions) -> Self {
        let target_width = target.width();
        let target_height = target.height();
        debug_assert!(
            target_width >= 2 && target_height >= 2,
            "the engine needs a target of at least 2 x 2 pixels"
        );
        let current = Buffer::new_from_color(target_width, target_height, background);
        let score = score::difference_full_raw(&target, &current);
        let coarse = Coarse::new(&target, &current);
        let seed = options.seed.unwrap_or_else(crate::util::entropy_seed);
        let scratch = WorkerCtx::new(
            target_width as i32,
            target_height as i32,
            crate::rng::round_rng(seed, 0, 0),
        );

        Self {
            background,
            target,
            current,
            score,
            history: Vec::new(),
            error_grid: ErrorGrid::new(
                target_width,
                target_height,
                options.grid_cols,
                options.grid_rows,
            ),
            coarse,
            seed,
            passes: 0,
            scratch,
            effort: None,
            #[cfg(test)]
            pruning: true,
            #[cfg(test)]
            reject_passes: false,
        }
    }

    /// Searches for the best next shape of `kind` and paints it.
    ///
    /// Every step runs 16 independent search rounds, 32 for
    /// [`ShapeKind::Any`], polygons, quadratics and rotated ellipses, as
    /// rayon tasks in the current pool: the global pool, unless the caller
    /// runs `step` inside [`rayon::ThreadPool::install`]. A step of
    /// [`ShapeKind::Quadratic`] or [`ShapeKind::RotatedEllipse`] also
    /// climbs twice as long. Each round draws from its own random
    /// stream, derived from the seed, the step index and the round index,
    /// and the best round wins, ties going to the lowest round index, so the
    /// result does not depend on the number of threads or on scheduling.
    ///
    /// Returns the number of candidate evaluations the search made.
    pub fn step(&mut self, kind: ShapeKind, alpha: Alpha) -> u64 {
        self.error_grid.compute(&self.target, &self.current);
        if let Some(coarse) = &mut self.coarse {
            coarse.prepare();
        }

        let coarse_round = self.coarse.as_ref().map(Coarse::round);
        let round = SearchRound {
            target: &self.target,
            current: &self.current,
            error_grid: &self.error_grid,
            score: self.score,
            coarse: coarse_round.as_ref(),
        };
        let (width, height) = (self.target.width() as i32, self.target.height() as i32);
        let seed = self.seed;
        // Each step commits exactly one shape, so this is the step index.
        let step = self.history.len() as u64;
        let (candidate_count, hill_climb_age) = Self::search_params(kind);
        let Effort {
            rounds,
            candidates,
            age,
        } = self.effort.unwrap_or(Effort::of(kind));
        let (candidate_count, hill_climb_age) =
            (candidate_count * candidates, hill_climb_age * age);
        #[cfg(test)]
        let pruning = self.pruning;
        let results: Vec<(State, u64)> = (0..rounds)
            .into_par_iter()
            .map_init(
                || WorkerCtx::new(width, height, crate::rng::round_rng(seed, step, 0)),
                |worker, index| {
                    worker.rng = crate::rng::round_rng(seed, step, index);
                    #[cfg(test)]
                    {
                        worker.pruning = pruning;
                    }
                    let evaluations_before = worker.evaluations;
                    let state =
                        worker.search_round(&round, kind, alpha, candidate_count, hill_climb_age);
                    (state, worker.evaluations - evaluations_before)
                },
            )
            .collect();

        let evaluations = results.iter().map(|(_, evaluations)| evaluations).sum();
        // `collect` keeps round order and `min_by_key` returns the first of
        // equal minima, so ties go to the lowest round index.
        let (best, _) = results
            .into_iter()
            .min_by_key(|(state, _)| state.cached_energy.unwrap_or(u64::MAX))
            .expect("a step always runs at least one round");

        self.add(best.shape, best.alpha);
        evaluations
    }

    /// Lab only: makes every later [`Model::step`] run `rounds` search
    /// rounds instead of its kind's 16 or 32, each sampling `candidates` times as many random
    /// candidates and climbing until `age` times as many moves in a row are
    /// not kept. All three are at least 1. Not part of the supported API.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub fn set_search_effort(&mut self, rounds: u64, candidates: usize, age: usize) {
        assert!(
            rounds > 0 && candidates > 0 && age > 0,
            "effort must be positive"
        );
        self.effort = Some(Effort {
            rounds,
            candidates,
            age,
        });
    }

    /// Runs one refit pass: re-optimises every committed shape at its own
    /// layer, with the others fixed, from the top layer down.
    ///
    /// At each layer the committed shape is the bar, in its current colour,
    /// and independent hill climbs from it search for a better shape,
    /// alpha (when `alpha` is [`Alpha::Auto`]) and colour against a model of
    /// the layers below and above, with moves that start at the greedy
    /// search's size and shrink to one- and two-pixel moves as fewer are
    /// kept; see `refine.rs`. With
    /// [`Alpha::Fixed`] every shape keeps its alpha. The climbs run as rayon
    /// tasks in the current pool, each on a random stream derived from the
    /// seed, the pass index, the layer and the climb, so the result does not
    /// depend on the number of threads or on scheduling, and every pass
    /// draws fresh streams.
    ///
    /// The pass is then checked on the exact canvas and kept only if it
    /// lowers the score; otherwise the model is left as it was. The number
    /// of shapes never changes. A model without shapes is left unchanged.
    ///
    /// Returns the number of candidate evaluations the pass made.
    pub fn refine(&mut self, alpha: Alpha) -> u64 {
        self.refine_unless(alpha, || false)
            .expect("a pass that is never cancelled finishes")
    }

    /// [`Model::refine`], cancellable: `cancelled` is polled before each
    /// layer of the pass and before the exact re-render that verifies it.
    ///
    /// Once `cancelled` returns true the pass stops and returns `None`, and
    /// the model is left exactly as it was before the pass, including the
    /// pass index, so the next pass draws the streams this one would have.
    /// Otherwise the pass runs and ends exactly as [`Model::refine`] does,
    /// and returns its number of candidate evaluations.
    #[must_use]
    pub fn refine_unless(
        &mut self,
        alpha: Alpha,
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<u64> {
        if self.history.is_empty() {
            return Some(0);
        }
        let streams = crate::refine::Streams {
            seed: self.seed,
            pass: self.passes,
        };
        let (refitted, evaluations) = crate::refine::pass(
            &self.target,
            self.background,
            &self.history,
            alpha,
            streams,
            &mut self.scratch,
            &mut cancelled,
        )?;
        let Some(history) = refitted else {
            self.passes += 1;
            return Some(evaluations);
        };
        if cancelled() {
            return None;
        }
        self.passes += 1;

        // The model only predicts; the exact canvas decides.
        let (width, height) = (self.target.width(), self.target.height());
        let current =
            crate::refine::render(width, height, self.background, &history, &mut self.scratch);
        let score = score::difference_full_raw(&self.target, &current);
        #[cfg(test)]
        let score = if self.reject_passes { u64::MAX } else { score };
        if score < self.score {
            self.history = history;
            self.current = current;
            self.score = score;
            if let Some(coarse) = &mut self.coarse {
                coarse.sync(&self.current);
            }
        }
        Some(evaluations)
    }

    /// Paints `shape` at `alpha`, which must be `1..=255`.
    fn add(&mut self, shape: Shape, alpha: u8) {
        debug_assert!(alpha > 0, "alpha must be non-zero");
        let lines = shape.rasterize(&mut self.scratch);
        let fit = score::fit(&self.target, &self.current, None, lines, i32::from(alpha));
        let score = score::energy(&self.target, &self.current, lines, fit, self.score);
        score::draw_lines(&mut self.current, fit.color, lines);
        self.score = score;
        if let Some(coarse) = &mut self.coarse {
            coarse.sync(&self.current);
        }
        self.history.push(CommittedShape {
            shape,
            color: fit.color,
        });
    }

    /// Normalized difference between the canvas and the target: the RMSE
    /// over the RGB channels divided by 255, so `0.0` is a perfect match and
    /// `1.0` is black against white.
    #[must_use]
    pub fn score_f64(&self) -> f64 {
        score::raw_score_to_normalized(self.score, self.current.width(), self.current.height())
    }

    /// The most recently committed shape, as the last shape of
    /// [`Model::drawing`] would be, or `None` before the first step. Unlike
    /// [`Model::drawing`], it converts only that one shape.
    #[must_use]
    pub fn last_shape(&self) -> Option<DrawnShape> {
        self.history.last().map(|committed| DrawnShape {
            geometry: committed.shape.geometry(),
            color: committed.color,
        })
    }

    /// The target, the background and the committed shapes, in paint
    /// order: what [`crate::joint::optimise`] optimises.
    pub(crate) fn joint_parts(&self) -> (&Buffer, Color, &[CommittedShape]) {
        (&self.target, self.background, &self.history)
    }

    /// The committed shapes in paint order, as engine-independent geometry
    /// on the working-resolution canvas.
    #[must_use]
    pub fn drawing(&self) -> Drawing {
        Drawing {
            width: self.target.width(),
            height: self.target.height(),
            background: self.background,
            shapes: self
                .history
                .iter()
                .map(|committed| DrawnShape {
                    geometry: committed.shape.geometry(),
                    color: committed.color,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score;
    use crate::shapes::{Rectangle, Shape};
    use crate::test_util::fixed_alpha;

    #[test]
    fn search_params_keeps_quadratic_budget_near_default() {
        assert_eq!(Model::search_params(ShapeKind::Quadratic), (900, 100));
        assert_eq!(Model::search_params(ShapeKind::Circle), (1000, 100));
    }

    #[test]
    fn add_score_matches_full_recomputation() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(target, Color::new(0, 0, 0, 255), ModelOptions::default());

        model.add(
            Shape::Rectangle(Rectangle {
                x1: 1,
                y1: 2,
                x2: 5,
                y2: 6,
            }),
            180,
        );

        assert_eq!(
            model.score,
            score::difference_full_raw(&model.target, &model.current)
        );
    }

    #[test]
    fn add_score_matches_full_recomputation_for_every_kind() {
        use crate::test_util::make_test_round;
        use rand::{RngExt, SeedableRng};

        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0xadd);
        for (index, &kind) in ShapeKind::all_kinds().iter().enumerate() {
            for (width, height) in [(2, 2), (23, 17), (40, 9)] {
                let mut pixels = vec![0_u8; (width * height * 3) as usize];
                rng.fill(&mut pixels[..]);
                let target = Buffer::from_rgb(width, height, pixels).expect("valid length");
                let background = Color::new(rng.random(), rng.random(), rng.random(), 255);
                let mut model = Model::new(target, background, ModelOptions::default());
                let (mut worker, round) = make_test_round(width, height, index as u64);

                for _ in 0..20 {
                    let shape = Shape::random(kind, &mut worker, &round);
                    model.add(shape, rng.random_range(1..=255));
                    assert_eq!(
                        model.score,
                        score::difference_full_raw(&model.target, &model.current),
                        "{kind:?} on {width}x{height}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_step_runs_all_search_rounds() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            ModelOptions {
                seed: Some(7),
                ..ModelOptions::default()
            },
        );
        let (candidates, age) = Model::search_params(ShapeKind::Triangle);
        // Each round samples every candidate and then hill-climbs for at
        // least `age` evaluations.
        let minimum = Effort::of(ShapeKind::Triangle).rounds * (candidates + age) as u64;

        for step in 0..2 {
            let evaluations = model.step(ShapeKind::Triangle, fixed_alpha(128));
            assert!(evaluations >= minimum, "step {step}: {evaluations}");
        }
    }

    #[test]
    fn every_shape_kind_steps_on_tiny_targets() {
        let mut kinds = vec![ShapeKind::Any];
        kinds.extend_from_slice(ShapeKind::all_kinds());
        for (width, height) in [(2, 2), (2, 9), (9, 2)] {
            for &kind in &kinds {
                let target = Buffer::new_from_color(width, height, Color::new(200, 40, 90, 255));
                let mut model = Model::new(
                    target,
                    Color::new(0, 0, 0, 255),
                    ModelOptions {
                        seed: Some(11),
                        ..ModelOptions::default()
                    },
                );
                for _ in 0..3 {
                    model.step(kind, Alpha::Auto);
                }

                let drawing = model.drawing();
                assert_eq!(
                    (drawing.width, drawing.height, drawing.shapes.len()),
                    (width, height, 3),
                    "{kind:?} on {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn last_shape_is_the_last_shape_of_the_drawing() {
        let target = Buffer::new_from_color(8, 8, Color::new(200, 40, 90, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            ModelOptions {
                seed: Some(5),
                ..ModelOptions::default()
            },
        );
        assert_eq!(model.last_shape(), None);

        for step in 1..=3 {
            model.step(ShapeKind::Any, Alpha::Auto);
            let drawing = model.drawing();
            assert_eq!(drawing.shapes.len(), step);
            assert_eq!(model.last_shape().as_ref(), drawing.shapes.last());
        }
    }

    #[test]
    fn new_clamps_zero_grid_dimensions() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            ModelOptions {
                seed: Some(7),
                grid_cols: 0,
                grid_rows: 0,
                ..ModelOptions::default()
            },
        );

        let evaluations = model.step(ShapeKind::Triangle, fixed_alpha(128));

        assert!(evaluations > 0);
    }

    /// The small target of the seeded tests, below the coarse minimum.
    const SMALL: (u32, u32) = (16, 12);
    /// The smallest seeded target that gets a coarse random phase.
    const COARSE: (u32, u32) = (40, crate::coarse::MIN_SIDE);

    /// Runs `steps` seeded `Any` steps on a 16 x 12 noise target inside a
    /// dedicated rayon pool of `threads` threads.
    fn seeded_drawing(seed: u64, threads: usize, steps: usize) -> Drawing {
        seeded_drawing_of(seed, threads, steps, ShapeKind::Any, true, SMALL)
    }

    /// [`seeded_drawing`] for any `kind` and noise target size, with or
    /// without the early exit.
    fn seeded_drawing_of(
        seed: u64,
        threads: usize,
        steps: usize,
        kind: ShapeKind,
        pruning: bool,
        (width, height): (u32, u32),
    ) -> Drawing {
        use rand::{RngExt, SeedableRng};

        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x5eed);
        let mut pixels = vec![0_u8; (width * height * 3) as usize];
        rng.fill(&mut pixels[..]);
        let target = Buffer::from_rgb(width, height, pixels).expect("valid length");
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("test thread pool");
        pool.install(|| {
            let mut model = Model::new(
                target,
                Color::new(0, 0, 0, 255),
                ModelOptions {
                    seed: Some(seed),
                    ..ModelOptions::default()
                },
            );
            model.pruning = pruning;
            for _ in 0..steps {
                model.step(kind, Alpha::Auto);
            }
            model.drawing()
        })
    }

    #[test]
    fn seeded_output_is_independent_of_the_thread_count() {
        let reference = seeded_drawing(42, 1, 4);
        for threads in [2, 3, 8] {
            assert_eq!(
                seeded_drawing(42, threads, 4),
                reference,
                "{threads} threads"
            );
        }
    }

    #[test]
    fn seeded_coarse_output_is_independent_of_the_thread_count() {
        let drawing = |threads| seeded_drawing_of(42, threads, 2, ShapeKind::Any, true, COARSE);
        let reference = drawing(1);
        for threads in [3, 8] {
            assert_eq!(drawing(threads), reference, "{threads} threads");
        }
    }

    /// After every step the coarse canvas is the downsample of the canvas,
    /// and its score is that canvas's score against the coarse target.
    #[test]
    fn coarse_canvas_follows_every_committed_shape() {
        use rand::{RngExt, SeedableRng};

        let (width, height) = (41, 33);
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0xc0a);
        let mut pixels = vec![0_u8; (width * height * 3) as usize];
        rng.fill(&mut pixels[..]);
        let target = Buffer::from_rgb(width, height, pixels).expect("valid length");
        let options = ModelOptions {
            seed: Some(3),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target, Color::new(10, 200, 30, 255), options);
        for kind in [ShapeKind::Rectangle, ShapeKind::RotatedEllipse] {
            let before = model.coarse.as_ref().expect("large enough").round().score;
            model.step(kind, Alpha::Auto);
            let round = model.coarse.as_ref().expect("large enough").round();
            let expected = crate::coarse::downsample(&model.current);
            assert_eq!(round.current.pixels(), expected.pixels(), "{kind:?}");
            assert_eq!(
                round.score,
                score::difference_full_raw(round.target, round.current),
                "{kind:?}"
            );
            assert_ne!(round.score, before, "{kind:?}: the shape changed nothing");
        }
    }

    /// The early exit skips work without changing which shape any step
    /// commits: the drawings, and so the SVG written from them, are equal.
    #[test]
    fn seeded_output_is_the_same_without_the_early_exit() {
        let kinds = [ShapeKind::Any, ShapeKind::Quadratic, ShapeKind::Polygon];
        for (seed, kind) in [7, 8, 9].into_iter().zip(kinds) {
            assert_eq!(
                seeded_drawing_of(seed, 2, 4, kind, true, SMALL),
                seeded_drawing_of(seed, 2, 4, kind, false, SMALL),
                "{kind:?}"
            );
        }
        // The search tests cover every kind with a coarse round.
        assert_eq!(
            seeded_drawing_of(7, 2, 1, ShapeKind::Any, true, COARSE),
            seeded_drawing_of(7, 2, 1, ShapeKind::Any, false, COARSE),
            "with a coarse random phase"
        );
    }

    /// The 64-bit FNV-1a digest of `drawing`'s `Debug` form, which spells
    /// every shape's coordinates, colour and alpha.
    fn digest(drawing: &Drawing) -> u64 {
        format!("{drawing:?}")
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            })
    }

    /// Pins the greedy search, so that a change to the refit's moves cannot
    /// change it: the digests of seeded drawings of every kind. Recorded
    /// when the blend became the exact composite rounded once per layer,
    /// an intended change of the greedy output; `quadratic` and `any`
    /// (which draws quadratics too) were recorded again when the quadratic
    /// stroke became 2 px wide with area coverage, another intended change;
    /// `polygon`, `rotated-rectangle` and `any` (both coarse and not) were
    /// recorded again when polygons became strictly convex with every
    /// angle above 15° and rotated rectangles got their 1:8 aspect-ratio
    /// cap, the legibility rules; `rectangle` and `any` (both coarse and
    /// not) again when axis-aligned rectangles got the same cap;
    /// `quadratic`, `rotated-ellipse` and `polygon` again when their steps
    /// got 32 search rounds, and quadratics and rotated ellipses twice the
    /// climb age (`Effort::of`), the quality-first greedy effort. `any`,
    /// which got 32 rounds too, kept its digests: on these targets no
    /// round after the 16th found a better shape.
    #[test]
    fn seeded_greedy_output_is_pinned() {
        let pinned = [
            (ShapeKind::Any, 0xc79e328758b81cac),
            (ShapeKind::Triangle, 0x0b2c1d60c966824f),
            (ShapeKind::Rectangle, 0xd8f95f8e1f029eac),
            (ShapeKind::Ellipse, 0xdd99e621c00e71b6),
            (ShapeKind::Circle, 0x1cc7d6677aa8b599),
            (ShapeKind::RotatedRectangle, 0x52186adc7569380b),
            (ShapeKind::Quadratic, 0xff87fd257f007dce),
            (ShapeKind::RotatedEllipse, 0x02ceb857843573e9),
            (ShapeKind::Polygon, 0xc658d1a5973e7a4d),
        ];
        assert_eq!(pinned.len(), every_kind().len());
        let actual: Vec<_> = pinned
            .iter()
            .map(|&(kind, _)| {
                (
                    kind,
                    digest(&seeded_drawing_of(42, 2, 4, kind, true, SMALL)),
                )
            })
            .collect();
        assert_eq!(actual, pinned);
        assert_eq!(
            digest(&seeded_drawing_of(42, 2, 2, ShapeKind::Any, true, COARSE)),
            0x90ca95129c711074,
            "with a coarse random phase"
        );
    }

    #[test]
    fn different_seeds_give_different_output() {
        assert_ne!(seeded_drawing(1, 4, 4), seeded_drawing(2, 4, 4));
    }

    #[test]
    fn seeds_near_the_top_of_the_range_do_not_overflow() {
        for seed in [u64::MAX, u64::MAX - 1] {
            let drawing = seeded_drawing(seed, 3, 2);
            assert_eq!(drawing.shapes.len(), 2, "seed {seed}");
        }
    }

    /// A model of `kind` after `steps` seeded steps on a noise target.
    fn stepped_model(
        seed: u64,
        (width, height): (u32, u32),
        kind: ShapeKind,
        steps: usize,
    ) -> Model {
        use rand::{RngExt, SeedableRng};

        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed ^ 0x7e57);
        let mut pixels = vec![0_u8; (width * height * 3) as usize];
        rng.fill(&mut pixels[..]);
        let target = Buffer::from_rgb(width, height, pixels).expect("valid length");
        let options = ModelOptions {
            seed: Some(seed),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target, Color::new(0, 0, 0, 255), options);
        for _ in 0..steps {
            model.step(kind, Alpha::Auto);
        }
        model
    }

    /// The canvas replayed from the background through every committed
    /// shape.
    fn replay(model: &Model) -> Buffer {
        let (width, height) = (model.target.width(), model.target.height());
        let mut worker = WorkerCtx::new(width as i32, height as i32, crate::rng::create_rng(0));
        crate::refine::render(width, height, model.background, &model.history, &mut worker)
    }

    /// The canvas is the exact replay of the history, the score its exact
    /// score, and the coarse canvas its downsample.
    fn assert_consistent(model: &Model, context: &str) {
        assert_eq!(
            model.current.pixels(),
            replay(model).pixels(),
            "{context}: canvas"
        );
        assert_eq!(
            model.score,
            score::difference_full_raw(&model.target, &model.current),
            "{context}: score"
        );
        if let Some(coarse) = &model.coarse {
            let round = coarse.round();
            let expected = crate::coarse::downsample(&model.current);
            assert_eq!(
                round.current.pixels(),
                expected.pixels(),
                "{context}: coarse"
            );
            assert_eq!(
                round.score,
                score::difference_full_raw(round.target, round.current),
                "{context}: coarse score"
            );
        }
    }

    fn every_kind() -> Vec<ShapeKind> {
        let mut kinds = vec![ShapeKind::Any];
        kinds.extend_from_slice(ShapeKind::all_kinds());
        kinds
    }

    #[test]
    fn refine_keeps_the_model_consistent_and_never_raises_the_score() {
        let mut improved = 0;
        for (index, kind) in every_kind().into_iter().enumerate() {
            for size in [SMALL, (41, 33)] {
                let mut model = stepped_model(index as u64, size, kind, 4);
                for pass in 0..2 {
                    let context = format!("{kind:?} on {size:?}, pass {pass}");
                    let (before, shapes) = (model.score, model.history.len());
                    let evaluations = model.refine(Alpha::Auto);
                    // Every layer runs every climb for at least the age.
                    let minimum = shapes as u64 * crate::refine::ROUNDS * crate::refine::AGE as u64;
                    assert!(evaluations >= minimum, "{context}: {evaluations}");
                    assert!(model.score <= before, "{context}");
                    assert_eq!(model.history.len(), shapes, "{context}");
                    assert_eq!(model.passes, pass + 1, "{context}");
                    assert_consistent(&model, &context);
                    improved += usize::from(model.score < before);
                }
            }
        }
        assert!(improved >= 18, "only {improved} of 36 passes improved");
    }

    #[test]
    fn refine_with_a_fixed_alpha_keeps_every_alpha() {
        let mut model = stepped_model(4, SMALL, ShapeKind::Any, 0);
        for _ in 0..5 {
            model.step(ShapeKind::Any, fixed_alpha(90));
        }
        let before = model.score;
        model.refine(fixed_alpha(90));
        assert!(model.score < before, "the pass changed nothing");
        assert!(model.history.iter().all(|layer| layer.color.a == 90));
        assert_consistent(&model, "fixed alpha");
    }

    #[test]
    fn refine_without_shapes_is_a_no_op() {
        let mut model = stepped_model(1, SMALL, ShapeKind::Any, 0);
        let (current, score) = (model.current.clone(), model.score);

        assert_eq!(model.refine(Alpha::Auto), 0);

        assert!(model.history.is_empty());
        assert_eq!(model.passes, 0);
        assert_eq!(model.current.pixels(), current.pixels());
        assert_eq!(model.score, score);
    }

    #[test]
    fn every_shape_kind_refines_on_tiny_and_just_coarse_targets() {
        let side = crate::coarse::MIN_SIDE;
        let sizes = [(2, 2), (2, 9), (9, 2), (side, side), (side + 1, side + 2)];
        for (index, size) in sizes.into_iter().enumerate() {
            for kind in every_kind() {
                let mut model = stepped_model(index as u64, size, kind, 2);
                assert_eq!(model.coarse.is_some(), size.0 >= side, "{size:?}");
                let before = model.score;
                model.refine(Alpha::Auto);
                let context = format!("{kind:?} on {size:?}");
                assert!(model.score <= before, "{context}");
                assert_eq!(model.history.len(), 2, "{context}");
                assert_consistent(&model, &context);
            }
        }
    }

    #[test]
    fn a_pass_that_does_not_improve_is_reverted() {
        let mut model = stepped_model(5, SMALL, ShapeKind::Triangle, 5);
        let mut accepted = model.clone();
        accepted.refine(Alpha::Auto);
        assert!(accepted.score < model.score, "the pass would not improve");

        model.reject_passes = true;
        let (history, current, score) = (model.history.clone(), model.current.clone(), model.score);
        assert!(model.refine(Alpha::Auto) > 0);

        assert_eq!(model.history, history);
        assert_eq!(model.current.pixels(), current.pixels());
        assert_eq!(model.score, score);
        assert_eq!(model.passes, 1, "a reverted pass still counts");
        assert_consistent(&model, "reverted");

        // The next pass draws fresh streams: pass 1 does not repeat pass 0.
        model.reject_passes = false;
        model.refine(Alpha::Auto);
        assert_ne!(model.history, accepted.history);
        assert_consistent(&model, "after the revert");
    }

    /// A model whose next refit pass would change it, so a pass left
    /// unchanged by cancellation is observable.
    fn improvable_model() -> Model {
        let model = stepped_model(5, SMALL, ShapeKind::Triangle, 5);
        let mut accepted = model.clone();
        accepted.refine(Alpha::Auto);
        assert!(accepted.score < model.score, "the pass would not improve");
        model
    }

    /// Everything a cancelled pass must leave as it was.
    fn assert_unchanged(model: &Model, before: &Model, context: &str) {
        assert_eq!(model.drawing(), before.drawing(), "{context}: drawing");
        assert_eq!(
            model.score_f64().to_bits(),
            before.score_f64().to_bits(),
            "{context}: score"
        );
        assert_eq!(model.history, before.history, "{context}: history");
        assert_eq!(model.passes, before.passes, "{context}: passes");
        assert_consistent(model, context);
    }

    #[test]
    fn a_pass_cancelled_from_the_start_leaves_the_model_unchanged() {
        let before = improvable_model();
        let mut model = before.clone();

        assert_eq!(model.refine_unless(Alpha::Auto, || true), None);

        assert_unchanged(&model, &before, "cancelled from the start");
    }

    #[test]
    fn a_pass_cancelled_at_any_poll_leaves_the_model_unchanged() {
        let before = improvable_model();
        let polls = std::cell::Cell::new(0_usize);
        let mut finished = before.clone();
        let evaluations = finished.refine_unless(Alpha::Auto, || {
            polls.set(polls.get() + 1);
            false
        });
        assert!(evaluations.is_some());
        let polls = polls.get();
        // Once per layer and once before the exact re-render.
        assert!(polls > before.history.len(), "only {polls} polls");

        for flip in 1..polls {
            let mut model = before.clone();
            let seen = std::cell::Cell::new(0_usize);
            let result = model.refine_unless(Alpha::Auto, || {
                seen.set(seen.get() + 1);
                seen.get() > flip
            });
            let context = format!("cancelled after {flip} of {polls} polls");
            assert_eq!(result, None, "{context}");
            assert_unchanged(&model, &before, &context);
        }
    }

    #[test]
    fn a_pass_never_cancelled_matches_refine() {
        let mut refined = improvable_model();
        let mut model = refined.clone();

        let expected = refined.refine(Alpha::Auto);
        let evaluations = model.refine_unless(Alpha::Auto, || false);

        assert_eq!(evaluations, Some(expected));
        assert_eq!(model.drawing(), refined.drawing());
        assert_eq!(model.score_f64().to_bits(), refined.score_f64().to_bits());
        assert_eq!(model.passes, refined.passes);
        assert_consistent(&model, "never cancelled");
    }

    #[test]
    fn refine_keeps_a_layer_that_covers_no_pixel() {
        // A shape that covers no pixel fits no colour: it is committed
        // with alpha 0, and the pass must accept it, at any alpha mode.
        for alpha in [Alpha::Auto, fixed_alpha(128)] {
            let mut model = stepped_model(3, (2, 2), ShapeKind::Triangle, 0);
            let outside = Shape::Triangle(crate::shapes::Triangle {
                x1: -12,
                y1: -12,
                x2: -6,
                y2: -12,
                x3: -12,
                y3: -6,
            });
            model.add(outside, 128);
            assert_eq!(model.history[0].color.a, 0);
            model.step(ShapeKind::Triangle, alpha);
            let before = model.score;

            model.refine(alpha);

            assert!(model.score <= before, "{alpha:?}");
            assert_eq!(model.history.len(), 2, "{alpha:?}");
            assert_consistent(&model, &format!("{alpha:?}"));
        }
    }

    #[test]
    fn refine_keeps_a_fully_covered_layer() {
        let (width, height) = (12, 10);
        let fill = Color::new(200, 40, 90, 255);
        let target = Buffer::new_from_color(width, height, fill);
        let options = ModelOptions {
            seed: Some(2),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target, Color::new(0, 0, 0, 255), options);
        let hidden = Shape::Rectangle(Rectangle {
            x1: 2,
            y1: 1,
            x2: 7,
            y2: 8,
        });
        model.add(hidden, 120);
        let cover = Shape::Rectangle(Rectangle {
            x1: 0,
            y1: 0,
            x2: width as i32 - 1,
            y2: height as i32 - 1,
        });
        model.add(cover, 255);
        let layers = model.history.clone();
        assert_eq!(model.score, 0);

        model.refine(Alpha::Auto);

        assert_eq!(model.history, layers);
        assert_consistent(&model, "covered");
    }

    /// [`seeded_drawing`] with a refit pass after every other step when
    /// `refine` is set.
    fn refined_drawing(seed: u64, threads: usize, refine: bool) -> Drawing {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("test thread pool");
        pool.install(|| {
            let mut model = stepped_model(seed, SMALL, ShapeKind::Any, 0);
            for step in 0..6 {
                model.step(ShapeKind::Any, Alpha::Auto);
                if refine && step % 2 == 1 {
                    model.refine(Alpha::Auto);
                }
            }
            model.drawing()
        })
    }

    #[test]
    fn seeded_refine_is_independent_of_the_thread_count() {
        let reference = refined_drawing(42, 1, true);
        assert_ne!(
            reference,
            refined_drawing(42, 1, false),
            "no pass changed anything"
        );
        for threads in [2, 3, 8] {
            assert_eq!(
                refined_drawing(42, threads, true),
                reference,
                "{threads} threads"
            );
        }
    }
}
