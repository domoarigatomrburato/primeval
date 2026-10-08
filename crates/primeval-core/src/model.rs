use crate::alpha::Alpha;
use crate::coarse::Coarse;
use crate::drawing::{Drawing, DrawnShape};
use crate::error_grid::ErrorGrid;
use crate::refine::RefineEffort;
use crate::score;
use crate::shapes::{Quadratic, Shape, ShapeKind};
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
    /// The effort for `kind`: 16 rounds, and twice the climb age for
    /// quadratics.
    ///
    /// Measured with the engine runner on its corpus, with the passes and
    /// final stage `primeval-render` runs around the search, by the mean of
    /// the 100- and 200-shape median RMSE of the export against the
    /// previous `approximate` (16 rounds, then one refit pass) and by the
    /// time over the same rows, on an Apple M2 Pro
    /// (`docs/algorithm-leap-review-2026-10-07.md`). Twice the age buys
    /// quadratics 2.6 points for 57% more time; it stays because they have
    /// no cheaper lever in the pipeline. 32 rounds bought `any` 0.8 points
    /// for 19% more time, and 32 rounds with twice the age bought rotated
    /// ellipses 2.2 points for 90% more, so both went back to 16 rounds.
    /// Polygons had gone back before them: the doubled rounds were most of
    /// their pipeline's cost. For the other kinds, twice or four times the
    /// rounds or the age never gained 0.5%.
    const fn of(kind: ShapeKind) -> Self {
        let age = match kind {
            ShapeKind::Quadratic => 2,
            _ => 1,
        };
        Self {
            rounds: 16,
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
    /// Scratch for rasterizing the shape that [`Model::add`] paints; the
    /// refit pass's workers take their quadratic width bounds from it.
    scratch: WorkerCtx<ChaCha8Rng>,
    /// The search effort of [`Model::step`] for every kind, set only by
    /// the lab hook; otherwise each kind's [`Effort::of`].
    effort: Option<Effort>,
    /// The effort of every refit pass: [`RefineEffort::DEFAULT`] unless the
    /// lab hook set it.
    refine_effort: RefineEffort,
    /// The bounds of a quadratic curve's stroke width that every worker
    /// of the search and of the refit pass gets (see
    /// `WorkerCtx::quadratic_width`): `Quadratic::STROKE_WIDTHS` unless
    /// the lab hook set them.
    quadratic_width: (f64, f64),
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
            refine_effort: RefineEffort::DEFAULT,
            quadratic_width: Quadratic::STROKE_WIDTHS,
            #[cfg(test)]
            pruning: true,
            #[cfg(test)]
            reject_passes: false,
        }
    }

    /// Searches for the best next shape of `kind` and paints it.
    ///
    /// Every step runs 16 independent search rounds as rayon tasks in the
    /// current pool: the global pool, unless the caller runs `step` inside
    /// [`rayon::ThreadPool::install`]. A step of [`ShapeKind::Quadratic`]
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
        let quadratic_width = self.quadratic_width;
        #[cfg(test)]
        let pruning = self.pruning;
        let results: Vec<(State, u64)> = (0..rounds)
            .into_par_iter()
            .map_init(
                || WorkerCtx::new(width, height, crate::rng::round_rng(seed, step, 0)),
                |worker, index| {
                    worker.rng = crate::rng::round_rng(seed, step, index);
                    worker.quadratic_width = quadratic_width;
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
    /// rounds instead of 16, each sampling `candidates` times as many
    /// random candidates and climbing until `age` times as many moves in a
    /// row are not kept, whatever the kind (by default only quadratics
    /// climb twice as long). All three are at least 1. Not part of the
    /// supported API.
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

    /// Lab only: makes every later [`Model::refine`] run `rounds`
    /// independent hill climbs per layer instead of 4, each stopping after
    /// `age` moves in a row are not kept instead of 25. Both are at least
    /// 1. Not part of the supported API.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub fn set_refine_effort(&mut self, rounds: u64, age: usize) {
        assert!(rounds > 0 && age > 0, "refine effort must be positive");
        self.refine_effort = RefineEffort { rounds, age };
    }

    /// Lab only: makes every later [`Model::step`] and [`Model::refine`]
    /// choose each quadratic curve's stroke width between `min` and `max`
    /// working pixels instead of the default 2 to 6 px: random curves draw
    /// it uniformly and a fourth move shifts it. `1 <= min <= max`; equal
    /// bounds fix the width, `2:2` as it was before the search chose it.
    /// Not part of the supported API.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub fn set_quadratic_width(&mut self, min: f64, max: f64) {
        assert!(
            1.0 <= min && min <= max,
            "quadratic width bounds must satisfy 1 <= min <= max"
        );
        self.quadratic_width = (min, max);
        self.scratch.quadratic_width = (min, max);
    }

    /// Replaces the committed shapes with `drawing`, a
    /// [`crate::joint::optimise`] result for this model, if the exact
    /// canvas repainted with them scores strictly lower than the current
    /// one, or whatever it scores when `force` is set; later steps then
    /// build on them. Returns whether they were kept, or `None`, with the
    /// model unchanged, if `drawing` is not one this model can take: it
    /// has another size, background or number of shapes, moves a shape
    /// the joint optimisation keeps fixed, or has a shape that does not
    /// convert into a valid one of its kind.
    ///
    /// Every colour, opacity included, comes from `drawing`, and every
    /// shape is converted back into the engine's own:
    ///
    /// - a triangle, or a polygon, becomes the polygon of the drawing's
    ///   vertices, which the engine's continuous polygons take exactly; a
    ///   triangle is then a three-vertex polygon;
    /// - an axis-aligned rectangle rounds the joint result's half-pixel
    ///   edges to the engine's pixel bounds, each edge moving at most
    ///   0.5 px: lossy by design;
    /// - a rotated rectangle does not convert (`None`): recovering its
    ///   integer centre, sides and angle from the corners takes `atan2` and
    ///   `hypot`, which the arithmetic rule of [`crate::joint`] forbids.
    ///   Only the lab build recovers them, rounded, and lossy too;
    /// - an ellipse, a circle or a rotated ellipse that moved (the joint
    ///   optimisation's curved outlines, [`crate::joint::Settings::curved`])
    ///   converts only in the lab build: an ellipse or a circle rounds to
    ///   its integer centre and radii, lossy, a rotated ellipse takes the
    ///   drawing's centre, radii and rotation exactly; a radius under 1 does
    ///   not convert. The production build refuses it (`None`);
    /// - every other shape keeps its geometry, which the drawing must not
    ///   have moved.
    ///
    /// The canvas is repainted exactly with the converted shapes, as a
    /// refit pass verifies its result, before that score decides. The
    /// refit pass index does not change.
    #[must_use]
    pub fn adopt(&mut self, drawing: &Drawing, force: bool) -> Option<bool> {
        let (width, height) = (self.target.width(), self.target.height());
        if (drawing.width, drawing.height, drawing.background) != (width, height, self.background)
            || drawing.shapes.len() != self.history.len()
        {
            return None;
        }
        let history = self
            .history
            .iter()
            .zip(&drawing.shapes)
            .map(|(committed, drawn)| {
                Some(CommittedShape {
                    shape: committed.shape.adopted(&drawn.geometry)?,
                    color: drawn.color,
                })
            })
            .collect::<Option<Vec<_>>>()?;

        // The exact canvas decides, as in `refine_unless`.
        let current =
            crate::refine::render(width, height, self.background, &history, &mut self.scratch);
        let score = score::difference_full_raw(&self.target, &current);
        let kept = force || score < self.score;
        if kept {
            self.history = history;
            self.current = current;
            self.score = score;
            if let Some(coarse) = &mut self.coarse {
                coarse.sync(&self.current);
            }
        }
        Some(kept)
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
            effort: self.refine_effort,
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

    /// The target the model approximates, at the working size of its
    /// canvas and of [`Model::drawing`].
    #[must_use]
    pub fn target(&self) -> &Buffer {
        &self.target
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
    /// round after the 16th found a better shape. `quadratic` and
    /// `polygon` again when they went back to 16 rounds, quadratics
    /// keeping twice the climb age; `polygon`'s digest is the one it had
    /// before the 32 rounds. `rotated-ellipse` again when it went back to
    /// 16 rounds and the climb age ×1; `any`, back to 16 rounds as well,
    /// kept its digests. `quadratic` and `any` (both coarse and not) again
    /// when the search began to draw and move each quadratic curve's
    /// stroke width, between 2 and 6 px, instead of fixing it at 2 px.
    #[test]
    fn seeded_greedy_output_is_pinned() {
        let pinned = [
            (ShapeKind::Any, 0xb126c51184c55dc3),
            (ShapeKind::Triangle, 0x0b2c1d60c966824f),
            (ShapeKind::Rectangle, 0xd8f95f8e1f029eac),
            (ShapeKind::Ellipse, 0xdd99e621c00e71b6),
            (ShapeKind::Circle, 0x1cc7d6677aa8b599),
            (ShapeKind::RotatedRectangle, 0x52186adc7569380b),
            (ShapeKind::Quadratic, 0xb32c52aba93ea445),
            (ShapeKind::RotatedEllipse, 0xf9f17763932224cc),
            (ShapeKind::Polygon, 0xb57dc794666b2a2b),
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
            0x82420a134976eb5b,
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

    /// A lower refit effort runs fewer evaluations and still leaves a
    /// consistent model that scores no worse.
    #[cfg(feature = "lab")]
    #[test]
    fn a_lower_refine_effort_runs_fewer_evaluations() {
        let before = improvable_model();
        let mut default = before.clone();
        let expected = default.refine(Alpha::Auto);

        let mut model = before.clone();
        model.set_refine_effort(1, 1);
        let evaluations = model.refine(Alpha::Auto);

        assert!(evaluations < expected, "{evaluations} against {expected}");
        assert!(model.score <= before.score);
        assert_eq!(model.history.len(), before.history.len());
        assert_consistent(&model, "refine effort 1:1");
    }

    /// A higher refit effort runs at least its own climbs' evaluations and
    /// leaves a consistent model that scores no worse than before the pass.
    /// It need not score below the default pass: at the first layer where
    /// the two passes differ its refit has a strictly lower model energy,
    /// since its first climbs draw the default's streams, but every layer
    /// below then sees different layers above. On this model it ends 82
    /// above the default, of 2.69 million;
    /// `refine::tests::more_climbs_per_layer_extend_the_default_climbs`
    /// checks the property per layer.
    #[cfg(feature = "lab")]
    #[test]
    fn a_higher_refine_effort_keeps_the_model_consistent() {
        let before = improvable_model();
        let mut model = before.clone();
        model.set_refine_effort(8, 25);
        let evaluations = model.refine(Alpha::Auto);

        let minimum = before.history.len() as u64 * 8 * 25;
        assert!(evaluations >= minimum, "{evaluations}");
        assert!(model.score < before.score, "the pass changed nothing");
        assert_eq!(model.history.len(), before.history.len());
        assert_consistent(&model, "refine effort 8:25");
    }

    /// The stroke widths of `drawing`'s quadratic curves.
    fn quadratic_widths(drawing: &Drawing) -> Vec<f64> {
        drawing
            .shapes
            .iter()
            .filter_map(|shape| match shape.geometry {
                crate::drawing::Geometry::Quadratic { width, .. } => Some(width),
                _ => None,
            })
            .collect()
    }

    /// By default the search chooses each curve's width between 2 and 6
    /// working pixels. (On a noise target every curve takes the widest
    /// stroke, so the target mixes thin and wide features.)
    #[test]
    fn quadratic_widths_default_to_between_2_and_6_pixels() {
        let worker = WorkerCtx::new(8, 8, crate::rng::create_rng(0));
        assert_eq!(worker.quadratic_width, (2.0, 6.0));
        // Thin lines on the left, where thin strokes fit, and a solid block
        // on the right, where wide strokes fit.
        let (width, height) = (48_u32, 36_u32);
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let lit = if x < width / 2 {
                    y % 8 < 2
                } else {
                    (8..28).contains(&y)
                };
                [if lit { 255 } else { 0 }; 3]
            })
            .collect();
        let target = Buffer::from_rgb(width, height, pixels).expect("valid length");
        let options = ModelOptions {
            seed: Some(5),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target, Color::new(0, 0, 0, 255), options);
        assert_eq!(model.quadratic_width, (2.0, 6.0));
        assert_eq!(model.scratch.quadratic_width, (2.0, 6.0));
        for _ in 0..8 {
            model.step(ShapeKind::Quadratic, Alpha::Auto);
        }
        let widths = quadratic_widths(&model.drawing());
        assert_eq!(widths.len(), 8);
        assert!(
            widths.iter().all(|width| (2.0..=6.0).contains(width)),
            "{widths:?}"
        );
        assert!(widths.iter().any(|&width| width != widths[0]), "{widths:?}");
    }

    /// With wider bounds, the search chooses each curve's width within
    /// them.
    #[cfg(feature = "lab")]
    #[test]
    fn a_variable_quadratic_width_searches_widths_within_its_bounds() {
        let mut model = stepped_model(5, (48, 36), ShapeKind::Quadratic, 0);
        model.set_quadratic_width(1.5, 6.0);
        for _ in 0..8 {
            model.step(ShapeKind::Quadratic, Alpha::Auto);
        }
        let widths = quadratic_widths(&model.drawing());
        assert_eq!(widths.len(), 8);
        assert!(
            widths.iter().all(|width| (1.5..=6.0).contains(width)),
            "{widths:?}"
        );
        assert!(widths.iter().any(|&width| width != 2.0), "{widths:?}");
        assert_consistent(&model, "quadratic width 1.5:6");
    }

    /// The refit pass's climbs move the widths too.
    #[cfg(feature = "lab")]
    #[test]
    fn a_refit_pass_moves_quadratic_widths_within_their_bounds() {
        let mut model = stepped_model(5, (48, 36), ShapeKind::Quadratic, 0);
        model.set_quadratic_width(1.5, 6.0);
        for _ in 0..8 {
            model.step(ShapeKind::Quadratic, Alpha::Auto);
        }
        let before = quadratic_widths(&model.drawing());
        model.refine(Alpha::Auto);
        let after = quadratic_widths(&model.drawing());
        assert!(
            after.iter().all(|width| (1.5..=6.0).contains(width)),
            "{after:?}"
        );
        assert_ne!(after, before, "the pass changed no width");
        assert_consistent(&model, "refit at quadratic width 1.5:6");
    }

    /// [`seeded_drawing`] with a refit pass after every other step when
    /// `refine` is set.
    fn refined_drawing(seed: u64, threads: usize, refine: bool) -> Drawing {
        refined_drawing_with(seed, threads, refine, |_| {})
    }

    /// [`refined_drawing`] of a model that `configure` set up first.
    fn refined_drawing_with(
        seed: u64,
        threads: usize,
        refine: bool,
        configure: impl FnOnce(&mut Model) + Send,
    ) -> Drawing {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("test thread pool");
        pool.install(|| {
            let mut model = stepped_model(seed, SMALL, ShapeKind::Any, 0);
            configure(&mut model);
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

    /// The refit's climbs per layer change the pass's parallelism, not its
    /// result: twice the default climbs give the same drawing on any number
    /// of threads.
    #[cfg(feature = "lab")]
    #[test]
    fn seeded_refine_at_a_higher_effort_is_independent_of_the_thread_count() {
        let drawing = |threads| {
            refined_drawing_with(42, threads, true, |model| model.set_refine_effort(8, 25))
        };
        let reference = drawing(1);
        assert_ne!(
            reference,
            refined_drawing(42, 1, true),
            "the effort changed nothing"
        );
        for threads in [2, 4, 8] {
            assert_eq!(drawing(threads), reference, "{threads} threads");
        }
    }

    /// Whether every committed shape of `model` keeps its kind's rule.
    fn every_shape_is_valid(model: &Model) -> bool {
        model
            .history
            .iter()
            .all(|committed| match &committed.shape {
                Shape::Triangle(shape) => shape.is_valid(),
                Shape::Rectangle(shape) => shape.is_valid(),
                Shape::RotatedRectangle(shape) => {
                    shape.sx >= 1 && shape.sy >= 1 && shape.is_valid()
                }
                Shape::Polygon(shape) => shape.is_valid(),
                Shape::Ellipse(_)
                | Shape::Circle(_)
                | Shape::Quadratic(_)
                | Shape::RotatedEllipse(_) => true,
            })
    }

    /// The joint optimisation of `model` with the curved outlines switched
    /// off and no iterations: it only projects, snaps and refits the
    /// colours, and the curved shapes keep their geometry.
    fn snapped(model: &Model) -> Drawing {
        let settings = crate::joint::Settings {
            iterations: Some(0),
            curved: false,
            ..crate::joint::Settings::default()
        };
        crate::joint::optimise(model, Alpha::Auto, settings, || false).expect("not cancelled")
    }

    /// The centre, the sides and the direction of the first side of the
    /// rectangle whose corners are `geometry`, in [`Shape::geometry`]'s
    /// order.
    fn rotated_parameters(geometry: &crate::Geometry) -> [f64; 5] {
        let crate::Geometry::Polygon(points) = geometry else {
            panic!("not a rotated rectangle: {geometry:?}");
        };
        let [a, b, c, d] = points.as_slice() else {
            panic!("not a rotated rectangle: {geometry:?}");
        };
        [
            (a.x + b.x + c.x + d.x) / 4.0,
            (a.y + b.y + c.y + d.y) / 4.0,
            (b.x - a.x).hypot(b.y - a.y),
            (c.x - b.x).hypot(c.y - b.y),
            (b.y - a.y).atan2(b.x - a.x).to_degrees(),
        ]
    }

    /// Without iterations the joint optimisation keeps a rotated
    /// rectangle's centre exactly, and its half-side vector on the
    /// quarter-pixel lattice moves each side by less than 0.5 px, so the
    /// adopted rectangle has the original centre and sides. The lattice can
    /// turn the vector by more than half a degree (up to about 2° for a
    /// side of 10 px), so the adopted angle is the joint result's, rounded
    /// to the degree, and not always the original's.
    fn assert_rotated_round_trip(
        adopted: &crate::Geometry,
        optimised: &crate::Geometry,
        original: &crate::Geometry,
        context: &str,
    ) {
        let [x, y, sx, sy, angle] = rotated_parameters(adopted);
        let [ox, oy, osx, osy, _] = rotated_parameters(original);
        let [.., joint_angle] = rotated_parameters(optimised);
        for (value, expected) in [(x, ox), (y, oy), (sx, osx), (sy, osy)] {
            assert!((value - expected).abs() < 1e-9, "{context}: {adopted:?}");
        }
        let turn = (angle - joint_angle).rem_euclid(360.0);
        assert!(
            turn.min(360.0 - turn) <= 0.5 + 1e-9,
            "{context}: {angle} against {joint_angle}"
        );
    }

    /// A joint result without iterations adopts exactly for triangles and
    /// polygons, which become continuous polygons; rectangles recover their
    /// integer bounds, rotated rectangles, in the lab build only, their
    /// centre and sides and the joint result's angle
    /// ([`assert_rotated_round_trip`]), and the fixed kinds keep their
    /// geometry. The canvas is the exact replay of the adopted shapes, with
    /// every colour of the joint result. In the production build a drawing
    /// with a rotated rectangle is refused, and the model left as it was.
    #[test]
    fn adopt_round_trips_a_joint_result_of_every_kind() {
        for (index, kind) in every_kind().into_iter().enumerate() {
            for size in [SMALL, (41, 33)] {
                let context = format!("{kind:?} on {size:?}");
                let mut model = stepped_model(index as u64, size, kind, 6);
                let before = model.drawing();
                let joint = snapped(&model);
                let kinds: Vec<Shape> = model.history.iter().map(|c| c.shape.clone()).collect();

                let rotated = kinds
                    .iter()
                    .any(|shape| matches!(shape, Shape::RotatedRectangle(_)));
                if rotated && !cfg!(feature = "lab") {
                    let unchanged = model.clone();
                    assert_eq!(model.adopt(&joint, true), None, "{context}");
                    assert_unchanged(&model, &unchanged, &context);
                    continue;
                }
                assert_eq!(model.adopt(&joint, true), Some(true), "{context}");

                let after = model.drawing();
                assert_eq!(after.shapes.len(), before.shapes.len(), "{context}");
                for (layer, shape) in kinds.iter().enumerate() {
                    let (adopted, optimised, original) = (
                        &after.shapes[layer],
                        &joint.shapes[layer],
                        &before.shapes[layer],
                    );
                    let context = format!("{context}: layer {layer}");
                    assert_eq!(adopted.color, optimised.color, "{context}");
                    match shape {
                        Shape::Triangle(_) | Shape::Polygon(_) => {
                            assert_eq!(adopted.geometry, optimised.geometry, "{context}");
                        }
                        Shape::RotatedRectangle(_) => assert_rotated_round_trip(
                            &adopted.geometry,
                            &optimised.geometry,
                            &original.geometry,
                            &context,
                        ),
                        _ => assert_eq!(adopted.geometry, original.geometry, "{context}"),
                    }
                }
                assert!(every_shape_is_valid(&model), "{context}");
                assert_consistent(&model, &context);
            }
        }
    }

    /// The joint optimisation of `model` with the curved outlines switched
    /// on and no iterations: it only projects, snaps and refits the
    /// colours.
    fn snapped_curved(model: &Model) -> Drawing {
        let settings = crate::joint::Settings {
            iterations: Some(0),
            curved: true,
            ..crate::joint::Settings::default()
        };
        crate::joint::optimise(model, Alpha::Auto, settings, || false).expect("not cancelled")
    }

    /// A joint result of ellipses, circles or rotated ellipses with the
    /// curved outlines and no iterations adopts, in the lab build, as the
    /// joint result's geometry: ellipses and circles keep their integer
    /// centre and radii, which the quarter-pixel snap keeps, and rotated
    /// ellipses take the joint result's continuous centre, radii and
    /// angle. Each shape moved by a pixel adopts as moved. The canvas is
    /// the exact replay of the adopted shapes. In the production build, a
    /// drawing whose curved shapes moved is refused, and the model left as
    /// it was.
    #[test]
    fn adopt_round_trips_a_curved_joint_result() {
        use crate::Geometry;

        let kinds = [
            ShapeKind::Ellipse,
            ShapeKind::Circle,
            ShapeKind::RotatedEllipse,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            for size in [SMALL, (41, 33)] {
                let context = format!("{kind:?} on {size:?}");
                let original = stepped_model(index as u64, size, kind, 6);
                let before = original.drawing();
                let joint = snapped_curved(&original);
                let mut moved = joint.clone();
                for shape in &mut moved.shapes {
                    let Geometry::Ellipse { cx, .. } = &mut shape.geometry else {
                        panic!("{context}: not an ellipse: {shape:?}");
                    };
                    *cx += 1.0;
                }
                if !cfg!(feature = "lab") {
                    let mut model = original.clone();
                    assert_eq!(model.adopt(&moved, true), None, "{context}");
                    assert_unchanged(&model, &original, &context);
                    continue;
                }
                for (drawing, label) in [(&joint, "snapped"), (&moved, "moved")] {
                    let context = format!("{context}, {label}");
                    let mut model = original.clone();
                    assert_eq!(model.adopt(drawing, true), Some(true), "{context}");
                    let after = model.drawing();
                    assert_eq!(after.shapes.len(), before.shapes.len(), "{context}");
                    for (layer, adopted) in after.shapes.iter().enumerate() {
                        let expected = &drawing.shapes[layer];
                        assert_eq!(adopted, expected, "{context}: layer {layer}");
                        if label == "snapped" && kind != ShapeKind::RotatedEllipse {
                            assert_eq!(
                                adopted.geometry, before.shapes[layer].geometry,
                                "{context}: layer {layer}"
                            );
                        }
                    }
                    assert!(every_shape_is_valid(&model), "{context}");
                    assert_consistent(&model, &context);
                }
            }
        }
    }

    /// Without `force`, a drawing that repaints further from the target is
    /// not kept, and the model is left exactly as it was.
    #[test]
    fn adopt_keeps_the_model_when_the_drawing_repaints_worse() {
        // Not `any`: its stack holds shapes the production build cannot adopt.
        for kind in [
            ShapeKind::Triangle,
            ShapeKind::Rectangle,
            ShapeKind::Polygon,
        ] {
            let before = stepped_model(3, (41, 33), kind, 6);
            let joint = snapped(&before);
            let mut moved = joint.clone();
            for shape in &mut moved.shapes {
                let c = shape.color;
                shape.color = Color::new(255 - c.r, 255 - c.g, 255 - c.b, c.a.max(200));
            }
            let mut model = before.clone();
            assert_eq!(model.adopt(&moved, false), Some(false), "{kind:?}");
            assert_unchanged(&model, &before, &format!("{kind:?}"));
            // The same drawing, forced, is kept and scores worse.
            assert_eq!(model.adopt(&moved, true), Some(true), "{kind:?}");
            assert!(model.score > before.score, "{kind:?}");
            assert_consistent(&model, &format!("{kind:?}, forced"));
        }
    }

    /// A drawing that is not this model's, or whose shapes do not convert
    /// into valid ones of their kinds, is refused, and the model is left
    /// exactly as it was.
    #[test]
    fn adopt_refuses_a_drawing_it_cannot_convert() {
        use crate::{Geometry, Point};

        let refused = |model: &Model, drawing: &Drawing, context: &str| {
            let mut copy = model.clone();
            assert_eq!(copy.adopt(drawing, true), None, "{context}");
            assert_unchanged(&copy, model, context);
        };
        let rectangles = stepped_model(2, (41, 33), ShapeKind::Rectangle, 4);
        let joint = snapped(&rectangles);
        let mut fewer = joint.clone();
        fewer.shapes.pop();
        refused(&rectangles, &fewer, "one shape fewer");
        let mut wider = joint.clone();
        wider.width += 1;
        refused(&rectangles, &wider, "another size");
        let mut background = joint.clone();
        background.background = Color::new(1, 2, 3, 255);
        refused(&rectangles, &background, "another background");
        let rect = |x, y, width, height| Geometry::Rect {
            x,
            y,
            width,
            height,
        };
        for (geometry, context) in [
            (rect(3.0, 4.0, 18.0, 2.0), "aspect 9"),
            // Halves round away from zero: 16 / 2 becomes 17 / 2.
            (rect(-0.5, 4.0, 16.0, 2.0), "aspect 8, rounded to 17 / 2"),
            (rect(3.0, 4.0, 0.4, 2.0), "a side that rounds to 0"),
            (
                Geometry::Polygon(vec![Point::new(1.0, 1.0); 3]),
                "a polygon in place of a rectangle",
            ),
        ] {
            let mut drawing = joint.clone();
            drawing.shapes[1].geometry = geometry;
            refused(&rectangles, &drawing, context);
        }

        let triangles = stepped_model(2, SMALL, ShapeKind::Triangle, 4);
        let mut flat = snapped(&triangles);
        flat.shapes[0].geometry = Geometry::Polygon(vec![
            Point::new(1.0, 1.0),
            Point::new(5.0, 1.25),
            Point::new(9.0, 1.5),
        ]);
        refused(&triangles, &flat, "a flat triangle");

        let rotated = stepped_model(2, SMALL, ShapeKind::RotatedRectangle, 4);
        let mut thin = snapped(&rotated);
        thin.shapes[0].geometry = Geometry::Polygon(vec![
            Point::new(1.0, 1.0),
            Point::new(10.0, 1.0),
            Point::new(10.0, 1.25),
            Point::new(1.0, 1.25),
        ]);
        // Refused in the production build whatever its sides, since a
        // rotated rectangle never converts there.
        refused(&rotated, &thin, "a rotated side that rounds to 0");

        let ellipses = stepped_model(2, SMALL, ShapeKind::Ellipse, 4);
        // The lab build converts a moved ellipse
        // (`adopt_round_trips_a_curved_joint_result`).
        if !cfg!(feature = "lab") {
            let mut moved = snapped(&ellipses);
            let Geometry::Ellipse { cx, .. } = &mut moved.shapes[2].geometry else {
                panic!("not an ellipse");
            };
            *cx += 1.0;
            refused(&ellipses, &moved, "a fixed shape that moved");
        }
        for kind in [
            ShapeKind::Ellipse,
            ShapeKind::Circle,
            ShapeKind::RotatedEllipse,
        ] {
            let model = stepped_model(2, SMALL, kind, 4);
            let mut thin = snapped(&model);
            let Geometry::Ellipse { rx, ry, .. } = &mut thin.shapes[1].geometry else {
                panic!("not an ellipse");
            };
            (*rx, *ry) = (0.4, 0.4);
            refused(&model, &thin, &format!("{kind:?}: a radius under 1"));
        }
    }

    /// Adopting leaves the refit pass index alone: the next pass draws the
    /// streams it would have drawn on a model that got the same shapes
    /// otherwise.
    #[test]
    fn adopt_keeps_the_refit_pass_index() {
        let mut model = stepped_model(6, SMALL, ShapeKind::Triangle, 5);
        model.refine(Alpha::Auto);
        let joint = snapped(&model);
        let mut assigned = model.clone();
        assert_eq!(model.adopt(&joint, true), Some(true));
        assert_eq!(model.passes, 1);
        assigned.history = model.history.clone();
        assigned.current = model.current.clone();
        assigned.score = model.score;

        model.refine(Alpha::Auto);
        assigned.refine(Alpha::Auto);

        assert_eq!(model.history, assigned.history);
        assert_eq!(model.score, assigned.score);
        assert_eq!(model.passes, 2);
    }
}
