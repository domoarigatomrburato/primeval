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
#[derive(Clone, Debug)]
struct CommittedShape {
    shape: Shape,
    color: Color,
}

/// Independent search rounds per [`Model::step`]; the best one is painted.
const SEARCH_ROUNDS: u64 = 16;

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
/// approximate a target image.
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
    /// Scratch for rasterizing the shape that [`Model::add`] paints.
    scratch: WorkerCtx<ChaCha8Rng>,
    /// Whether the search may stop evaluations early; see
    /// `WorkerCtx::pruning`.
    #[cfg(test)]
    pruning: bool,
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
            scratch,
            #[cfg(test)]
            pruning: true,
        }
    }

    /// Searches for the best next shape of `kind` and paints it.
    ///
    /// Every step runs 16 independent search rounds as rayon tasks in the
    /// current pool: the global pool, unless the caller runs `step` inside
    /// [`rayon::ThreadPool::install`]. Each round draws from its own random
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
        #[cfg(test)]
        let pruning = self.pruning;
        let results: Vec<(State, u64)> = (0..SEARCH_ROUNDS)
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
        let minimum = SEARCH_ROUNDS * (candidates + age) as u64;

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
}
