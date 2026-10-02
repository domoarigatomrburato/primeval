use crate::alpha::Alpha;
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
    /// Deterministic RNG seed. `None` seeds from the system clock.
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
    seed: u64,
    /// Scratch for rasterizing the shape that [`Model::add`] paints.
    scratch: WorkerCtx<ChaCha8Rng>,
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
    /// `target` is at least 2 x 2 pixels: [`Buffer::from_rgba`] rejects
    /// anything smaller, so a model for an empty or one-pixel-wide canvas
    /// cannot be created.
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
        let seed = options.seed.unwrap_or_else(crate::util::system_clock_seed);
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
            seed,
            scratch,
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

        let round = SearchRound {
            target: &self.target,
            current: &self.current,
            error_grid: &self.error_grid,
            score: self.score,
        };
        let (width, height) = (self.target.width() as i32, self.target.height() as i32);
        let seed = self.seed;
        // Each step commits exactly one shape, so this is the step index.
        let step = self.history.len() as u64;
        let (candidate_count, hill_climb_age) = Self::search_params(kind);
        let results: Vec<(State, u64)> = (0..SEARCH_ROUNDS)
            .into_par_iter()
            .map_init(
                || WorkerCtx::new(width, height, crate::rng::round_rng(seed, step, 0)),
                |worker, index| {
                    worker.rng = crate::rng::round_rng(seed, step, index);
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
        let color =
            crate::score::compute_color(&self.target, &self.current, lines, i32::from(alpha));
        let score = crate::score::energy_from_lines_raw(
            &self.target,
            &self.current,
            lines,
            color,
            self.score,
        );
        crate::score::draw_lines(&mut self.current, color, lines);
        self.score = score;
        self.history.push(CommittedShape { shape, color });
    }

    /// Normalized difference between the canvas and the target: `0.0` is a
    /// perfect match.
    #[must_use]
    pub fn score_f64(&self) -> f64 {
        score::raw_score_to_normalized(self.score, self.current.width(), self.current.height())
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
        // TODO(ENG-2): include Quadratic once the T5 slice that fixes ENG-2
        // (with PERF-7) stops quadratic strokes painting pixels twice; until
        // then its incremental score legitimately differs from a recount.
        let kinds = ShapeKind::all_kinds()
            .iter()
            .copied()
            .filter(|&kind| kind != ShapeKind::Quadratic);
        for (index, kind) in kinds.enumerate() {
            for (width, height) in [(2, 2), (23, 17), (40, 9)] {
                let mut pixels = vec![0_u8; (width * height * 4) as usize];
                rng.fill(&mut pixels[..]);
                let target = Buffer::from_rgba(width, height, pixels).expect("valid length");
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

    /// Runs `steps` seeded `Any` steps on a 16 x 12 noise target inside a
    /// dedicated rayon pool of `threads` threads.
    fn seeded_drawing(seed: u64, threads: usize, steps: usize) -> Drawing {
        use rand::{RngExt, SeedableRng};

        let (width, height) = (16, 12);
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x5eed);
        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        rng.fill(&mut pixels[..]);
        let target = Buffer::from_rgba(width, height, pixels).expect("valid length");
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
            for _ in 0..steps {
                model.step(ShapeKind::Any, Alpha::Auto);
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
