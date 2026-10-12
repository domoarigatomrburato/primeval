//! Per-thread scratch state and shared round context for candidate evaluation.
//!
//! [`WorkerCtx`] owns the mutable buffers needed to rasterize and score a
//! candidate shape without touching shared state, while [`SearchRound`]
//! borrows the read-only data that every worker needs during a single
//! optimization round.

use crate::alpha::Alpha;
use crate::buffer::Buffer;
use crate::drawing::LineCap;
use crate::error_grid::ErrorGrid;
use crate::optimize::hill_climb;
use crate::raster::{RowScratch, StrokeScratch};
use crate::scanline::Scanline;
use crate::score;
use crate::shapes::{Quadratic, Shape, ShapeKind};
use crate::state::State;
use rand::{Rng, RngExt};

/// Fraction of samples drawn from the error-biased distribution.
/// The remaining `1 - BIASED_SAMPLING_RATE` are drawn uniformly.
const BIASED_SAMPLING_RATE: f64 = 0.8;
/// How many of the best candidates of a coarse random phase are rescored
/// at full resolution, then the best `seeds` of those are hill-climbed
/// (at most 128, even if `Search.seeds` is larger). Rescoring only
/// the best few lost quality at high step counts, where shapes are small
/// and the half-resolution ranking is least reliable.
const COARSE_RESCORED_SEEDS: usize = 128;
/// The default climb age before per-kind adjustments: quadratics keep
/// twice this age, while triangles, polygons and circles use 75.
pub(crate) const BASE_AGE: usize = 100;

/// What one search round ([`WorkerCtx::search_round`]) does: it samples
/// `candidates` random candidates, climbs the best `seeds` of them in turn,
/// each until `age` moves in a row are not kept, and returns the best
/// climb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoundEffort {
    pub(crate) candidates: usize,
    pub(crate) age: usize,
    pub(crate) seeds: usize,
}

impl RoundEffort {
    /// The actual default round of `kind`, with candidates, climb age and
    /// seeds chosen for speed at an invisible quality cost. Every step
    /// runs 16 rounds; the lab's explicit settings replace these defaults
    /// before its effort multiples are applied.
    ///
    /// | Kind | Candidates | Age | Seeds | Time ratio at 10 threads | Mean median RMSE change | Worst row, levels |
    /// | --- | ---: | ---: | ---: | --- | ---: | ---: |
    /// | triangle | 1000 | 75 | 1 | 0.87–0.88 | +0.33% | 0.01 |
    /// | polygon | 1000 | 75 | 1 | 0.87–0.89 | +0.09% | 0.06 |
    /// | circle | 500 | 75 | 1 | 0.82–0.85 | +0.09% | 0.05 |
    /// | rotated ellipse | 500 | 100 | 1 | 0.82–0.84 | +0.05% | 0.08 |
    /// | quadratic | 500 | 200 | 2 | 0.88–0.92 | −2.14% | 0.17 |
    /// | rotated rectangle, final joint ×1 (`primeval-render`) | 1000 | 100 | 1 | 0.77–0.82 | +0.39% | 0.47 |
    /// | any, rectangle, ellipse | 1000 | 100 | 1 | unchanged | unchanged | unchanged |
    ///
    /// Measured with the engine runner and the render pipeline on the full
    /// default corpus at 10 threads, and on the two paintings at 1 thread;
    /// quality is the mean of the 100/200-shape median RMSEs. Together with
    /// rotated rectangles' final joint iteration multiple of 1 (their
    /// round stays unchanged), these defaults save
    /// 10–12% overall at both thread counts. Earlier rounds sampled 1000
    /// candidates, or 900 for quadratics, whose longer age and two seeds
    /// remain; the shortened ages and candidate counts reproduce the
    /// October 2026 lab runs.
    pub(crate) const fn of(kind: ShapeKind) -> Self {
        match kind {
            ShapeKind::Triangle | ShapeKind::Polygon => Self {
                candidates: 1000,
                age: 75,
                seeds: 1,
            },
            ShapeKind::Circle => Self {
                candidates: 500,
                age: 75,
                seeds: 1,
            },
            ShapeKind::RotatedEllipse => Self {
                candidates: 500,
                age: BASE_AGE,
                seeds: 1,
            },
            ShapeKind::Quadratic => Self {
                candidates: 500,
                age: 2 * BASE_AGE,
                seeds: 2,
            },
            _ => Self {
                candidates: 1000,
                age: BASE_AGE,
                seeds: 1,
            },
        }
    }
}

/// Scratch state for one independent indexed search round or refit climb.
/// Retained across steps, with logical state reset before each round; no
/// scratch is shared by concurrent tasks.
#[derive(Clone)]
pub(crate) struct WorkerCtx<R> {
    /// Image width in pixels.
    pub(crate) width: i32,
    /// Image height in pixels.
    pub(crate) height: i32,
    /// Reusable storage for rasterized scanlines.
    pub(crate) lines: Vec<Scanline>,
    /// Reusable storage for stroking quadratic curves.
    pub(crate) stroke: StrokeScratch,
    /// Reusable per-row storage for the anti-aliased polygon and
    /// rotated-ellipse fills.
    pub(crate) rows: RowScratch,
    /// The RNG of the search round this context is running.
    pub(crate) rng: R,
    /// Running count of energy evaluations performed by this worker.
    pub(crate) evaluations: u64,
    /// The bounds `(min, max)` of a quadratic curve's stroke width in
    /// working pixels, `1 <= min <= max`, within which random curves and
    /// their moves choose each curve's width: [`Quadratic::STROKE_WIDTHS`]
    /// unless the lab hook `Model::set_quadratic_width` set others. Equal
    /// bounds fix the width.
    pub(crate) quadratic_width: (f64, f64),
    /// How the stroke of every quadratic curve that random curves start
    /// ends, which their moves keep: [`LineCap::Round`] unless the lab hook
    /// `Model::set_quadratic_cap` set another.
    pub(crate) quadratic_cap: LineCap,
    /// Whether bounded evaluations may stop early; tests turn it off to
    /// check that the early exit never changes a result.
    #[cfg(test)]
    pub(crate) pruning: bool,
    /// Bounded evaluations that came back at or above their bound.
    #[cfg(test)]
    pub(crate) rejected: u64,
}

/// Read-only shared state for a single search round, borrowed from the model.
///
/// All workers in a round share the same target, current approximation,
/// error grid, and baseline score.
pub(crate) struct SearchRound<'a> {
    /// The original target image.
    pub(crate) target: &'a Buffer,
    /// The current best approximation.
    pub(crate) current: &'a Buffer,
    /// Pre-computed spatial error distribution.
    pub(crate) error_grid: &'a ErrorGrid,
    /// Baseline raw squared-difference score of `current` against `target`.
    pub(crate) score: u64,
    /// The same round at half resolution, which the random phase ranks
    /// candidates against; `None` scores them at full resolution.
    pub(crate) coarse: Option<&'a SearchRound<'a>>,
}

/// Sorted handles into stable slots: insertions move only usize values.
/// The extra insertion slot is reserved once; ties use the original binary
/// search over the same list length, preserving its precise tie order.
struct TopStates {
    slots: Vec<Option<State>>,
    order: Vec<usize>,
    spare: Option<usize>,
    limit: usize,
}

impl TopStates {
    fn new(limit: usize) -> Self {
        Self {
            slots: Vec::with_capacity(limit + 1),
            order: Vec::with_capacity(limit + 1),
            spare: None,
            limit,
        }
    }

    fn energy(&self, at: usize) -> u64 {
        self.slots[self.order[at]]
            .as_ref()
            .expect("occupied slot")
            .cached_energy
            .unwrap_or(u64::MAX)
    }

    fn insert(&mut self, state: State) {
        let energy = state.cached_energy.unwrap_or(u64::MAX);
        let at = self
            .order
            .binary_search_by_key(&energy, |&slot| {
                self.slots[slot]
                    .as_ref()
                    .expect("occupied slot")
                    .cached_energy
                    .unwrap_or(u64::MAX)
            })
            .unwrap_or_else(|index| index);
        if at >= self.limit {
            return;
        }
        let slot = match self.spare.take() {
            Some(slot) => {
                self.slots[slot] = Some(state);
                slot
            }
            None => {
                let slot = self.slots.len();
                self.slots.push(Some(state));
                slot
            }
        };
        self.order.insert(at, slot);
        if self.order.len() > self.limit {
            let slot = self.order.pop().expect("extra slot");
            self.slots[slot] = None;
            self.spare = Some(slot);
        }
    }

    #[cfg(test)]
    fn ordered(&self) -> impl Iterator<Item = &State> {
        self.order
            .iter()
            .map(|&slot| self.slots[slot].as_ref().expect("occupied slot"))
    }

    fn into_states(mut self) -> Vec<State> {
        self.order
            .into_iter()
            .map(|slot| self.slots[slot].take().expect("occupied slot"))
            .collect()
    }
}

impl<R: Rng> WorkerCtx<R> {
    #[must_use]
    pub(crate) fn new(width: i32, height: i32, rng: R) -> Self {
        Self {
            width,
            height,
            lines: Vec::with_capacity(4096),
            stroke: StrokeScratch::default(),
            rows: RowScratch::default(),
            rng,
            evaluations: 0,
            quadratic_width: Quadratic::STROKE_WIDTHS,
            quadratic_cap: LineCap::Round,
            #[cfg(test)]
            pruning: true,
            #[cfg(test)]
            rejected: 0,
        }
    }

    /// A new worker of this one's size and quadratic stroke width bounds
    /// and cap, on `rng`: how the search's and the refit pass's workers
    /// take the model's settings from its scratch worker. Within a step or
    /// a pass only their streams change, so a worker can be reused round
    /// after round by resetting its `rng`; the rest is scratch space.
    pub(crate) fn like<S: Rng>(&self, rng: S) -> WorkerCtx<S> {
        let mut worker = WorkerCtx::new(self.width, self.height, rng);
        worker.quadratic_width = self.quadratic_width;
        worker.quadratic_cap = self.quadratic_cap;
        worker
    }

    /// Exactly a fresh `like` worker's logical state, retaining storage.
    /// Configuration comes from the model each round, since lab hooks may
    /// change between steps. Test counters and hooks never leak across rounds.
    pub(crate) fn reset_like(&mut self, template: &Self, rng: R) {
        self.width = template.width;
        self.height = template.height;
        self.quadratic_width = template.quadratic_width;
        self.quadratic_cap = template.quadratic_cap;
        self.rng = rng;
        self.evaluations = 0;
        self.lines.clear();
        self.stroke.reset();
        self.rows.reset();
        #[cfg(test)]
        {
            self.pruning = true;
            self.rejected = 0;
        }
    }

    /// Runs `run` on this worker and returns its result with the number of
    /// evaluations it made.
    pub(crate) fn counted<T>(&mut self, run: impl FnOnce(&mut Self) -> T) -> (T, u64) {
        let before = self.evaluations;
        let result = run(self);
        (result, self.evaluations - before)
    }
}

impl<R: Rng> WorkerCtx<R> {
    /// Samples an integer pixel coordinate, biased toward high-error regions.
    ///
    /// With probability `BIASED_SAMPLING_RATE`, the coordinate is drawn
    /// from the error grid's CDF; otherwise it is drawn uniformly.
    #[inline]
    pub(crate) fn sample_xy(&mut self, round: &SearchRound<'_>) -> (i32, i32) {
        if self.rng.random::<f64>() < BIASED_SAMPLING_RATE {
            round.error_grid.sample(&mut self.rng)
        } else {
            let x = self.rng.random_range(0..self.width);
            let y = self.rng.random_range(0..self.height);
            (x, y)
        }
    }

    /// Samples a floating-point coordinate, biased toward high-error regions.
    ///
    /// With probability `BIASED_SAMPLING_RATE`, the coordinate is drawn
    /// from the error grid's CDF; otherwise it is drawn uniformly.
    #[inline]
    pub(crate) fn sample_xy_float(&mut self, round: &SearchRound<'_>) -> (f64, f64) {
        if self.rng.random::<f64>() < BIASED_SAMPLING_RATE {
            round.error_grid.sample_float(&mut self.rng)
        } else {
            let x = self.rng.random::<f64>() * self.width as f64;
            let y = self.rng.random::<f64>() * self.height as f64;
            (x, y)
        }
    }

    /// Evaluates a candidate shape and returns its raw squared-difference energy.
    ///
    /// The caller provides a `rasterize` closure that fills the worker's
    /// `lines` buffer with the shape's scanlines. This decouples shape
    /// rasterization from scoring and avoids the circular dependency the
    /// Go code had (shapes storing `*Worker`).
    ///
    /// Steps:
    /// 1. Increment `self.evaluations`.
    /// 2. Call `rasterize` to populate `self.lines`.
    /// 3. Fit the blending colour for those scanlines and sum the old error
    ///    of their pixels, from the error grid's prefix sums, so the grid
    ///    must be computed for `round`'s buffers.
    /// 4. Add the error of each pixel after blending (no buffer write).
    pub(crate) fn energy(
        &mut self,
        round: &SearchRound<'_>,
        rasterize: impl FnOnce(&mut Self) -> &[Scanline],
        alpha: i32,
    ) -> u64 {
        self.evaluate(round, rasterize, alpha, None)
            .expect("an evaluation without a bound always has an energy")
    }

    /// [`Self::energy`] if it is below `bound`, `None` otherwise; the
    /// evaluation stops as soon as the shape cannot get below `bound`.
    pub(crate) fn energy_below(
        &mut self,
        round: &SearchRound<'_>,
        rasterize: impl FnOnce(&mut Self) -> &[Scanline],
        alpha: i32,
        bound: u64,
    ) -> Option<u64> {
        let energy = self.evaluate(round, rasterize, alpha, Some(bound));
        #[cfg(test)]
        {
            self.rejected += u64::from(energy.is_none());
        }
        energy
    }

    /// The energy of `state` on the coarse round `coarse` (see
    /// [`crate::coarse`]), below `bound` if there is one, like
    /// [`Self::energy_below`]. The worker rasterizes the shape at half
    /// scale on a canvas of the coarse size; nothing is cached on `state`.
    fn coarse_energy(
        &mut self,
        coarse: &SearchRound<'_>,
        state: &State,
        bound: Option<u64>,
    ) -> Option<u64> {
        let full = (self.width, self.height);
        self.width = coarse.target.width() as i32;
        self.height = coarse.target.height() as i32;
        let alpha = i32::from(state.alpha);
        let energy = self.evaluate(
            coarse,
            |ctx| state.shape.rasterize_coarse(ctx),
            alpha,
            bound,
        );
        (self.width, self.height) = full;
        #[cfg(test)]
        {
            self.rejected += u64::from(bound.is_some() && energy.is_none());
        }
        energy
    }

    fn evaluate(
        &mut self,
        round: &SearchRound<'_>,
        rasterize: impl FnOnce(&mut Self) -> &[Scanline],
        alpha: i32,
        bound: Option<u64>,
    ) -> Option<u64> {
        #[cfg(test)]
        let pruning = self.pruning;
        #[cfg(not(test))]
        let pruning = true;

        self.evaluations += 1;
        let lines = rasterize(self);
        let below = |energy: u64| bound.is_none_or(|bound| energy < bound);
        if lines.is_empty() {
            return Some(round.score).filter(|&energy| below(energy));
        }

        let (target, current) = (round.target, round.current);
        let fit = score::fit(target, current, Some(round.error_grid.sums()), lines, alpha);
        match bound {
            Some(bound) if pruning => {
                score::energy_below(target, current, lines, fit, round.score, bound)
            }
            _ => Some(score::energy(target, current, lines, fit, round.score))
                .filter(|&energy| below(energy)),
        }
    }

    pub(crate) fn random_state(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
    ) -> State {
        State::new(Shape::random(kind, self, round), alpha)
    }

    pub(crate) fn best_random_state(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
    ) -> State {
        assert!(n > 0, "best_random_state requires at least one sample");

        let mut best_state = self.random_state(round, kind, alpha);
        let mut best_energy = best_state.energy(self, round);
        for _ in 1..n {
            let mut state = self.random_state(round, kind, alpha);
            // Only a strictly lower energy replaces the best, so ties keep
            // the earliest candidate.
            if let Some(energy) = state.energy_below(self, round, best_energy) {
                best_energy = energy;
                best_state = state;
            }
        }
        best_state
    }

    /// The best `seeds` of the [`COARSE_RESCORED_SEEDS`] coarse-best of `n`
    /// random states, rescored at full resolution, lowest first; ties keep
    /// the coarse-better state first. The states have their
    /// full-resolution energy cached. With one seed, a state replaces the
    /// best only if it is strictly lower, so every rescoring after the
    /// first can stop early at the best energy.
    fn best_rescored_states(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        seeds: usize,
    ) -> Vec<State> {
        let mut best: Vec<State> = Vec::with_capacity(seeds + 1);
        for mut state in self.best_random_states(round, kind, alpha, n, COARSE_RESCORED_SEEDS) {
            let energy = if best.len() < seeds {
                state.energy(self, round)
            } else {
                let worst = best[seeds - 1].cached_energy.expect("a rescored energy");
                match state.energy_below(self, round, worst) {
                    Some(energy) => energy,
                    None => continue,
                }
            };
            // After every equal energy, so ties keep the coarse order.
            let at = best.partition_point(|other| other.cached_energy <= Some(energy));
            best.insert(at, state);
            best.truncate(seeds);
        }
        best
    }

    #[cfg(test)]
    fn insert_top_state(states: &mut Vec<State>, state: State, limit: usize) {
        if limit == 0 {
            return;
        }

        let energy = state.cached_energy.unwrap_or(u64::MAX);
        let insert_at = states
            .binary_search_by_key(&energy, |candidate| {
                candidate.cached_energy.unwrap_or(u64::MAX)
            })
            .unwrap_or_else(|index| index);
        if insert_at >= limit {
            return;
        }

        states.insert(insert_at, state);
        if states.len() > limit {
            states.pop();
        }
    }

    /// The `limit` lowest-energy states of `n` random ones, lowest first,
    /// in [`TopStates::insert`] order. With a coarse round the states
    /// are ranked by their coarse energy and returned without a cached
    /// energy.
    fn best_random_states(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        limit: usize,
    ) -> Vec<State> {
        match round.coarse {
            None => self.top_random_states(round, kind, alpha, n, limit, |worker, state, bound| {
                match bound {
                    Some(bound) => state.energy_below(worker, round, bound),
                    None => Some(state.energy(worker, round)),
                }
            }),
            Some(coarse) => {
                let mut states =
                    self.top_random_states(round, kind, alpha, n, limit, |worker, state, bound| {
                        // Ranks by the coarse energy, cleared below.
                        state.cached_energy = worker.coarse_energy(coarse, state, bound);
                        state.cached_energy
                    });
                for state in &mut states {
                    state.cached_energy = None;
                }
                states
            }
        }
    }

    /// [`Self::best_random_states`] with the energy from `score_state`,
    /// which returns the energy if it is below the bound, when there is one,
    /// and leaves it in the state's `cached_energy`.
    fn top_random_states(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        limit: usize,
        mut score_state: impl FnMut(&mut Self, &mut State, Option<u64>) -> Option<u64>,
    ) -> Vec<State> {
        assert!(n > 0, "best_random_states requires at least one sample");
        assert!(
            limit > 0,
            "best_random_states requires at least one retained state"
        );

        let mut states = TopStates::new(limit);
        for _ in 0..n {
            let mut state = self.random_state(round, kind, alpha);
            // A full list takes a state whose energy is at most its worst
            // (`insert_top_state` places a tie before the equal entry), so
            // anything at or above `worst + 1` can stop early.
            let bound = (states.order.len() == limit)
                .then(|| states.energy(limit - 1))
                .and_then(|worst| worst.checked_add(1));
            if score_state(self, &mut state, bound).is_none() && bound.is_some() {
                continue;
            }
            states.insert(state);
        }
        states.into_states()
    }

    /// Runs one search round as `effort` says: samples its random
    /// candidates, hill-climbs the best `effort.seeds` of them in turn
    /// until `effort.age` consecutive moves fail to improve one, and
    /// returns the lowest climb with its energy cached, ties going to the
    /// earlier seed.
    ///
    /// With a coarse round, the kinds for which
    /// [`ShapeKind::ranks_coarsely`] holds rank the candidates on it and
    /// rescore the best [`COARSE_RESCORED_SEEDS`] at full resolution, the
    /// lowest of which are the seeds; the climbs always run at full
    /// resolution. The other kinds ignore the coarse round and climb the
    /// lowest candidates: with one seed, the earliest of the lowest; with
    /// more, in [`TopStates::insert`] order, as quadratics always have.
    pub(crate) fn search_round(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        effort: RoundEffort,
    ) -> State {
        let full;
        let round = if kind.ranks_coarsely() {
            round
        } else {
            full = SearchRound {
                coarse: None,
                ..*round
            };
            &full
        };
        let RoundEffort {
            candidates: n,
            age,
            seeds,
        } = effort;
        let seeds = if round.coarse.is_some() {
            self.best_rescored_states(round, kind, alpha, n, seeds)
        } else if seeds == 1 {
            // Not `best_random_states` with one seed, which would keep the
            // latest of tied candidates: the default search keeps the
            // earliest.
            vec![self.best_random_state(round, kind, alpha, n)]
        } else {
            self.best_random_states(round, kind, alpha, n, seeds)
        };

        let mut best_state: Option<State> = None;
        let mut best_energy = u64::MAX;
        for seed in seeds {
            let mut state = hill_climb(&seed, self, round, age);
            let energy = state.energy(self, round);
            if best_state.is_none() || energy < best_energy {
                best_energy = energy;
                best_state = Some(state);
            }
        }
        best_state.expect("a search round climbs at least one seed")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn indexed_top_states_match_reference_with_ties() {
        use rand::RngExt;
        let mut rng = crate::rng::create_rng(0x70f);
        for limit in [1, 2, 3, 17, 128] {
            let mut actual = TopStates::new(limit);
            let mut expected = Vec::with_capacity(limit + 1);
            for i in 0..10000 {
                let mut state = state_with_energy(rng.random_range(0..19));
                state.shape =
                    crate::shapes::Shape::Circle(crate::shapes::Circle { x: i, y: 0, r: 1 });
                WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut expected, state.clone(), limit);
                actual.insert(state);
                assert_eq!(
                    actual.ordered().collect::<Vec<_>>(),
                    expected.iter().collect::<Vec<_>>()
                );
                assert_eq!(actual.order.capacity(), limit + 1);
                assert_eq!(actual.slots.capacity(), limit + 1);
            }
            assert_eq!(actual.into_states(), expected);
        }
    }

    use super::*;
    use crate::Color;
    use crate::coarse::Coarse;
    use crate::shapes::ShapeKind;
    use crate::state::State;
    use crate::test_util::fixed_alpha;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn test_rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(99)
    }

    fn state_with_energy(energy: u64) -> State {
        let mut state = State::new(
            crate::shapes::Shape::Circle(crate::shapes::Circle { x: 5, y: 5, r: 3 }),
            fixed_alpha(128),
        );
        state.cached_energy = Some(energy);
        state
    }

    #[test]
    fn new_allocates_correct_dimensions() {
        let w: WorkerCtx<ChaCha8Rng> = WorkerCtx::new(80, 60, test_rng());
        assert_eq!(w.width, 80);
        assert_eq!(w.height, 60);
        assert_eq!(w.evaluations, 0);
        assert!(w.lines.capacity() >= 4096);
    }

    /// A worker like another has its size and quadratic stroke, its own
    /// stream and no evaluations.
    #[test]
    fn a_worker_like_another_takes_its_size_and_quadratic_stroke() {
        let mut model = WorkerCtx::new(30, 20, ChaCha8Rng::seed_from_u64(1));
        model.quadratic_width = (1.5, 4.0);
        model.quadratic_cap = LineCap::Square;
        model.evaluations = 9;
        let worker = model.like(ChaCha8Rng::seed_from_u64(2));
        assert_eq!((worker.width, worker.height), (30, 20));
        assert_eq!(worker.rng, ChaCha8Rng::seed_from_u64(2));
        assert_eq!(worker.quadratic_width, (1.5, 4.0));
        assert_eq!(worker.quadratic_cap, LineCap::Square);
        assert_eq!(worker.evaluations, 0);
    }

    /// `counted` returns its run's result and the evaluations the run
    /// made, not the worker's running total.
    #[test]
    fn counted_returns_the_evaluations_of_its_run() {
        let mut worker = WorkerCtx::new(8, 8, test_rng());
        worker.evaluations = 5;
        let (value, count) = worker.counted(|worker| {
            worker.evaluations += 3;
            "done"
        });
        assert_eq!((value, count, worker.evaluations), ("done", 3, 8));
    }

    #[test]
    fn insert_top_state_keeps_lowest_energies_sorted() {
        let mut top = Vec::new();

        WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut top, state_with_energy(9), 2);
        WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut top, state_with_energy(4), 2);
        WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut top, state_with_energy(7), 2);
        WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut top, state_with_energy(3), 2);

        let energies: Vec<u64> = top
            .into_iter()
            .map(|state| state.cached_energy.unwrap())
            .collect();
        assert_eq!(energies, vec![3, 4]);
    }

    #[test]
    fn sample_xy_returns_in_bounds() {
        let target = Buffer::new_from_color(50, 30, Color::new(200, 100, 50, 255));
        let current = Buffer::new_from_color(50, 30, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(50, 30, 5, 3);
        grid.compute(&target, &current);

        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };

        let mut w = WorkerCtx::new(50, 30, test_rng());
        for _ in 0..1000 {
            let (x, y) = w.sample_xy(&round);
            assert!((0..50).contains(&x), "x={x} out of bounds");
            assert!((0..30).contains(&y), "y={y} out of bounds");
        }
    }

    #[test]
    fn sample_xy_float_returns_in_bounds() {
        let target = Buffer::new_from_color(50, 30, Color::new(200, 100, 50, 255));
        let current = Buffer::new_from_color(50, 30, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(50, 30, 5, 3);
        grid.compute(&target, &current);

        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };

        let mut w = WorkerCtx::new(50, 30, test_rng());
        for _ in 0..1000 {
            let (x, y) = w.sample_xy_float(&round);
            assert!((0.0..50.0).contains(&x), "x={x} out of bounds");
            assert!((0.0..30.0).contains(&y), "y={y} out of bounds");
        }
    }

    #[test]
    fn energy_computes_valid_score() {
        // Target: red pixel, Current: black pixel. A shape covering the
        // entire 1x1 image should produce a score different from the baseline.
        let mut target = Buffer::new(4, 4);
        let tp = target.pixels_mut();
        // Make pixel (0,0) bright red
        tp[0] = 255;

        let current = Buffer::new(4, 4);
        let mut grid = ErrorGrid::new(4, 4, 2, 2);
        grid.compute(&target, &current);

        let base_score = score::difference_full_raw(&target, &current);

        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: base_score,
            coarse: None,
        };

        let mut w = WorkerCtx::new(4, 4, test_rng());
        let alpha = 128;

        let energy = w.energy(
            &round,
            |ctx| {
                ctx.lines.clear();
                ctx.lines.push(Scanline {
                    y: 0,
                    x1: 0,
                    x2: 3,
                    alpha: 0xFFFF,
                });
                &ctx.lines
            },
            alpha,
        );

        // Drawing something should change the score from the baseline.
        assert_ne!(energy, base_score, "energy should differ from base_score");
        assert_eq!(w.evaluations, 1);
    }

    #[test]
    fn energy_with_empty_lines_returns_baseline() {
        let target = Buffer::new(4, 4);
        let current = Buffer::new(4, 4);
        let mut grid = ErrorGrid::new(4, 4, 2, 2);
        grid.compute(&target, &current);

        let base_score = score::difference_full_raw(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: base_score,
            coarse: None,
        };

        let mut w = WorkerCtx::new(4, 4, test_rng());
        let energy = w.energy(
            &round,
            |ctx| {
                ctx.lines.clear();
                &ctx.lines
            },
            128,
        );

        assert_eq!(energy, base_score, "empty lines should return base score");
        assert_eq!(w.evaluations, 1);
    }

    #[test]
    fn random_state_any_draws_every_concrete_kind() {
        let target = Buffer::new_from_color(32, 32, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(32, 32, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(32, 32, 4, 4);
        grid.compute(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };

        let mut worker = WorkerCtx::new(32, 32, test_rng());
        let mut drawn = std::collections::HashSet::new();
        for _ in 0..256 {
            let state = worker.random_state(&round, ShapeKind::Any, fixed_alpha(128));
            drawn.insert(std::mem::discriminant(&state.shape));
        }
        assert_eq!(drawn.len(), ShapeKind::all_kinds().len());
    }

    /// A seeded noise target over its top half and a flat colour over its
    /// bottom half, against a canvas of other noise on top and the same
    /// flat colour below.
    fn noise_round(width: u32, height: u32) -> (Buffer, Buffer) {
        use rand::RngExt;
        let mut rng = ChaCha8Rng::seed_from_u64(0x9a7e);
        let mut target = Buffer::new_from_color(width, height, Color::new(90, 140, 60, 255));
        let mut current = target.clone();
        let half = target.pixels().len() / 2;
        rng.fill(&mut target.pixels_mut()[..half]);
        rng.fill(&mut current.pixels_mut()[..half]);
        (target, current)
    }

    /// The energy of `state` on the coarse round, rasterized on a scratch
    /// worker of the coarse size and evaluated in full; counts as one
    /// evaluation of `worker`.
    fn reference_coarse_energy(
        worker: &mut WorkerCtx<ChaCha8Rng>,
        coarse: &SearchRound<'_>,
        state: &State,
    ) -> u64 {
        let (width, height) = (coarse.target.width(), coarse.target.height());
        let mut scratch = WorkerCtx::new(width as i32, height as i32, ChaCha8Rng::seed_from_u64(0));
        let lines = state.shape.rasterize_coarse(&mut scratch);
        worker.evaluations += 1;
        if lines.is_empty() {
            return coarse.score;
        }
        let alpha = i32::from(state.alpha);
        let fit = score::fit(coarse.target, coarse.current, None, lines, alpha);
        score::energy(coarse.target, coarse.current, lines, fit, coarse.score)
    }

    /// The `limit` best of `n` random states in `insert_top_state` order,
    /// by their full energy or, with a coarse round, by their coarse energy
    /// (then returned without a cached energy).
    fn reference_top_states(
        worker: &mut WorkerCtx<ChaCha8Rng>,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        limit: usize,
    ) -> Vec<State> {
        let mut states = Vec::new();
        for _ in 0..n {
            let mut state = worker.random_state(round, kind, alpha);
            match round.coarse {
                Some(coarse) => {
                    state.cached_energy = Some(reference_coarse_energy(worker, coarse, &state));
                }
                None => {
                    let _ = state.energy(worker, round);
                }
            }
            WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut states, state, limit);
        }
        if round.coarse.is_some() {
            for state in &mut states {
                state.cached_energy = None;
            }
        }
        states
    }

    /// The settings of a search round of `kind` with `n` candidates and
    /// climb age `age`, and the kind's default seeds.
    fn effort(kind: ShapeKind, n: usize, age: usize) -> RoundEffort {
        RoundEffort {
            candidates: n,
            age,
            seeds: RoundEffort::of(kind).seeds,
        }
    }

    /// The search as it ran before the early exit: every candidate is
    /// evaluated in full and compared with `<`, so ties keep the earlier
    /// state. Without a coarse round one seed is the earliest of the
    /// lowest candidates, and several seeds go through `insert_top_state`.
    /// With a coarse round, the random phase of the kinds that rank
    /// coarsely ranks by the coarse energy, and the best
    /// [`COARSE_RESCORED_SEEDS`] are rescored at full resolution in that
    /// order, the seeds being the lowest of them, ties in that order. Each
    /// seed is climbed in turn, and the lowest climb wins, ties going to the
    /// earlier seed.
    fn reference_search_round(
        worker: &mut WorkerCtx<ChaCha8Rng>,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        effort: RoundEffort,
    ) -> State {
        fn climb(
            worker: &mut WorkerCtx<ChaCha8Rng>,
            round: &SearchRound<'_>,
            state: &State,
            max_age: usize,
        ) -> State {
            let mut current = state.clone();
            let mut best_state = current.clone();
            let mut best_energy = current.energy(worker, round);
            let mut age = 0;
            while age < max_age {
                let undo = current.do_move(worker);
                let energy = current.energy(worker, round);
                if energy < best_energy {
                    best_energy = energy;
                    best_state = current.clone();
                    age = 0;
                } else {
                    current.undo_move(undo);
                    age += 1;
                }
            }
            best_state
        }

        let full;
        let round = if kind.ranks_coarsely() {
            round
        } else {
            full = SearchRound {
                coarse: None,
                ..*round
            };
            &full
        };
        let RoundEffort {
            candidates: n,
            age,
            seeds,
        } = effort;
        let seeds = match round.coarse {
            Some(_) => {
                let rescored =
                    reference_top_states(worker, round, kind, alpha, n, COARSE_RESCORED_SEEDS);
                let mut ranked: Vec<(u64, State)> = Vec::new();
                for mut state in rescored {
                    let energy = state.energy(worker, round);
                    // After every equal energy: ties keep the coarse order.
                    let at = ranked.partition_point(|(other, _)| *other <= energy);
                    ranked.insert(at, (energy, state));
                }
                ranked
                    .into_iter()
                    .take(seeds)
                    .map(|(_, state)| state)
                    .collect()
            }
            None if seeds == 1 => {
                let mut best: Option<(u64, State)> = None;
                for _ in 0..n {
                    let mut state = worker.random_state(round, kind, alpha);
                    let energy = state.energy(worker, round);
                    if best.as_ref().is_none_or(|(lowest, _)| energy < *lowest) {
                        best = Some((energy, state));
                    }
                }
                vec![best.unwrap().1]
            }
            None => reference_top_states(worker, round, kind, alpha, n, seeds),
        };
        let (mut best_state, mut best_energy) = (None, u64::MAX);
        for seed in seeds {
            let mut state = climb(worker, round, &seed, age);
            let energy = state.energy(worker, round);
            if best_state.is_none() || energy < best_energy {
                best_energy = energy;
                best_state = Some(state);
            }
        }
        best_state.unwrap()
    }

    /// The early exit only skips work: every kind's search picks the same
    /// shape, alpha and energy, after the same number of evaluations, as
    /// the search without it, and candidates are rejected, with and without
    /// a coarse random phase, with one to three seeds. A
    /// black canvas that already matches its black target gives every
    /// candidate the energy zero, so every comparison there is a tie.
    #[test]
    fn search_round_matches_the_search_without_the_early_exit() {
        let (width, height) = (40, 32);
        let black = Buffer::new(width, height);
        let rounds = [noise_round(width, height), (black.clone(), black)];
        for ((target, current), coarse) in rounds.iter().flat_map(|r| [(r, false), (r, true)]) {
            let mut grid = ErrorGrid::new(width, height, 4, 4);
            grid.compute(target, current);
            let mut coarse = coarse.then(|| Coarse::new(target, current).expect("large enough"));
            let coarse_round = coarse.as_mut().map(|coarse| coarse.round(current));
            let round = SearchRound {
                target,
                current,
                error_grid: &grid,
                score: score::difference_full_raw(target, current),
                coarse: coarse_round.as_ref(),
            };

            let kinds =
                std::iter::once(ShapeKind::Any).chain(ShapeKind::all_kinds().iter().copied());
            for kind in kinds {
                let mut rejected = 0;
                let efforts = [1, 2, 3].map(|seeds| RoundEffort {
                    seeds,
                    ..effort(kind, 40, 30)
                });
                for effort in efforts {
                    for seed in 0..4 {
                        let alpha = if seed % 2 == 0 {
                            Alpha::Auto
                        } else {
                            fixed_alpha(100)
                        };
                        let rng = || ChaCha8Rng::seed_from_u64(seed);
                        let mut bounded = WorkerCtx::new(width as i32, height as i32, rng());
                        let mut reference = WorkerCtx::new(width as i32, height as i32, rng());

                        let expected =
                            reference_search_round(&mut reference, &round, kind, alpha, effort);
                        let actual = bounded.search_round(&round, kind, alpha, effort);
                        let case = format!(
                            "{kind:?} {effort:?} seed {seed} score {} coarse {}",
                            round.score,
                            round.coarse.is_some()
                        );
                        assert_eq!(actual, expected, "{case}");
                        assert_eq!(bounded.evaluations, reference.evaluations, "{case}");
                        rejected += bounded.rejected;
                    }
                }
                assert!(rejected > 0, "{kind:?}: no candidate was rejected");
            }
        }
    }

    /// More seeds climb more and never end higher: the first seeds are the
    /// same and climb from the same random stream, so a round with `K + 1`
    /// seeds repeats the climbs of the round with `K` and keeps the lowest,
    /// with and without a coarse round.
    #[test]
    fn more_seeds_never_climb_to_a_higher_energy() {
        let (width, height) = (40, 32);
        let (target, current) = noise_round(width, height);
        let mut grid = ErrorGrid::new(width, height, 4, 4);
        grid.compute(&target, &current);
        let mut coarse = Coarse::new(&target, &current).expect("large enough");
        let coarse_round = coarse.round(&current);
        let full = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };
        let with_coarse = SearchRound {
            coarse: Some(&coarse_round),
            ..full
        };
        let kinds = std::iter::once(ShapeKind::Any).chain(ShapeKind::all_kinds().iter().copied());
        for kind in kinds {
            for round in [&full, &with_coarse] {
                let mut last: Option<(u64, u64)> = None;
                for seeds in 1..=3 {
                    let effort = RoundEffort {
                        seeds,
                        ..effort(kind, 40, 30)
                    };
                    let mut worker =
                        WorkerCtx::new(width as i32, height as i32, ChaCha8Rng::seed_from_u64(5));
                    let state = worker.search_round(round, kind, Alpha::Auto, effort);
                    let energy = state.cached_energy.expect("a cached energy");
                    let case = format!("{kind:?} coarse {}", round.coarse.is_some());
                    if let Some((lowest, evaluations)) = last {
                        assert!(energy <= lowest, "{case}: {seeds} seeds");
                        assert!(worker.evaluations > evaluations, "{case}: {seeds} seeds");
                    }
                    last = Some((energy, worker.evaluations));
                }
            }
        }
    }

    /// The kinds that do not rank coarsely search exactly as without a
    /// coarse round: same state, same energy, same evaluations.
    #[test]
    fn search_round_ignores_the_coarse_round_for_kinds_that_rank_at_full_resolution() {
        let (width, height) = (40, 32);
        let (target, current) = noise_round(width, height);
        let mut grid = ErrorGrid::new(width, height, 4, 4);
        grid.compute(&target, &current);
        let mut coarse = Coarse::new(&target, &current).expect("large enough");
        let coarse_round = coarse.round(&current);
        let full = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };
        let with_coarse = SearchRound {
            coarse: Some(&coarse_round),
            ..full
        };
        let kinds = [
            ShapeKind::Quadratic,
            ShapeKind::Rectangle,
            ShapeKind::RotatedRectangle,
            ShapeKind::Triangle,
        ];
        for kind in kinds {
            for seed in 0..4 {
                let alpha = if seed % 2 == 0 {
                    Alpha::Auto
                } else {
                    fixed_alpha(100)
                };
                let rng = || ChaCha8Rng::seed_from_u64(seed);
                let mut plain = WorkerCtx::new(width as i32, height as i32, rng());
                let mut coarsened = WorkerCtx::new(width as i32, height as i32, rng());
                let mut expected = plain.search_round(&full, kind, alpha, effort(kind, 40, 30));
                let mut actual =
                    coarsened.search_round(&with_coarse, kind, alpha, effort(kind, 40, 30));
                let case = format!("{kind:?} seed {seed}");
                assert_eq!(actual, expected, "{case}");
                assert_eq!(
                    actual.energy(&mut coarsened, &full),
                    expected.energy(&mut plain, &full),
                    "{case}"
                );
                assert_eq!(coarsened.evaluations, plain.evaluations, "{case}");
            }
        }
    }

    /// The kinds that rank coarsely do use the coarse round: their search
    /// differs from the same search without it, in its result or in the
    /// evaluations it took.
    #[test]
    fn search_round_uses_the_coarse_round_for_kinds_that_rank_coarsely() {
        let (width, height) = (40, 32);
        let (target, current) = noise_round(width, height);
        let mut grid = ErrorGrid::new(width, height, 4, 4);
        grid.compute(&target, &current);
        let mut coarse = Coarse::new(&target, &current).expect("large enough");
        let coarse_round = coarse.round(&current);
        let full = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };
        let with_coarse = SearchRound {
            coarse: Some(&coarse_round),
            ..full
        };
        let kinds = [
            ShapeKind::Any,
            ShapeKind::Circle,
            ShapeKind::Ellipse,
            ShapeKind::RotatedEllipse,
            ShapeKind::Polygon,
        ];
        for kind in kinds {
            let rng = || ChaCha8Rng::seed_from_u64(0);
            let mut plain = WorkerCtx::new(width as i32, height as i32, rng());
            let mut coarsened = WorkerCtx::new(width as i32, height as i32, rng());
            let expected = plain.search_round(&full, kind, Alpha::Auto, effort(kind, 40, 30));
            let actual =
                coarsened.search_round(&with_coarse, kind, Alpha::Auto, effort(kind, 40, 30));
            assert_ne!(
                (actual, coarsened.evaluations),
                (expected, plain.evaluations),
                "{kind:?}"
            );
        }
    }

    /// The bounded top-state sampling keeps exactly the states that full
    /// evaluation and `insert_top_state` keep, ties included: the all-black
    /// round ties every candidate at zero, so there every candidate is
    /// below the bound `worst + 1` and the ties decide the order.
    #[test]
    fn best_random_states_match_the_states_kept_without_the_early_exit() {
        let (width, height) = (40, 32);
        let black = Buffer::new(width, height);
        for (target, current) in [noise_round(width, height), (black.clone(), black)] {
            let mut grid = ErrorGrid::new(width, height, 4, 4);
            grid.compute(&target, &current);
            let round = SearchRound {
                target: &target,
                current: &current,
                error_grid: &grid,
                score: score::difference_full_raw(&target, &current),
                coarse: None,
            };
            for kind in [ShapeKind::Quadratic, ShapeKind::Triangle] {
                for limit in [1, 2, 3] {
                    let rng = || ChaCha8Rng::seed_from_u64(limit as u64);
                    let mut bounded = WorkerCtx::new(width as i32, height as i32, rng());
                    let mut reference = WorkerCtx::new(width as i32, height as i32, rng());
                    let mut expected = Vec::new();
                    for _ in 0..60 {
                        let mut state = reference.random_state(&round, kind, Alpha::Auto);
                        let _ = state.energy(&mut reference, &round);
                        WorkerCtx::<ChaCha8Rng>::insert_top_state(&mut expected, state, limit);
                    }
                    let actual = bounded.best_random_states(&round, kind, Alpha::Auto, 60, limit);
                    assert_eq!(actual, expected, "{kind:?} limit {limit}");
                    // Only the noise round has candidates above the bound.
                    if round.score > 0 {
                        assert!(bounded.rejected > 0, "{kind:?} limit {limit}");
                    }
                }
            }
        }
    }

    #[test]
    fn best_random_state_keeps_the_lowest_energy_candidate() {
        let target = Buffer::new_from_color(32, 32, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(32, 32, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(32, 32, 4, 4);
        grid.compute(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
            coarse: None,
        };

        let mut worker = WorkerCtx::new(32, 32, test_rng());
        let mut state = worker.best_random_state(&round, ShapeKind::Any, fixed_alpha(128), 8);
        let energy = state.energy(&mut worker, &round);
        assert_eq!(worker.evaluations, 8, "the cached energy is reused");

        // Replay the same candidates from the same seed.
        let mut replay = WorkerCtx::new(32, 32, test_rng());
        let candidates: Vec<(Shape, u64)> = (0..8)
            .map(|_| {
                let mut candidate = replay.random_state(&round, ShapeKind::Any, fixed_alpha(128));
                let energy = candidate.energy(&mut replay, &round);
                (candidate.shape, energy)
            })
            .collect();
        let lowest = candidates.iter().map(|&(_, energy)| energy).min().unwrap();
        assert_eq!(energy, lowest);
        assert!(
            lowest < round.score,
            "a fitted shape improves black towards white"
        );
        let first_lowest = candidates.iter().find(|&&(_, e)| e == lowest).unwrap();
        assert_eq!(
            state.shape, first_lowest.0,
            "ties keep the earliest candidate"
        );
    }
}
