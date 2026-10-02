//! Per-thread scratch state and shared round context for candidate evaluation.
//!
//! [`WorkerCtx`] owns the mutable buffers needed to rasterize and score a
//! candidate shape without touching shared state, while [`SearchRound`]
//! borrows the read-only data that every worker needs during a single
//! optimization round.

use crate::alpha::Alpha;
use crate::buffer::Buffer;
use crate::error_grid::ErrorGrid;
use crate::optimize::hill_climb;
use crate::scanline::Scanline;
use crate::score;
use crate::shapes::{Shape, ShapeKind};
use crate::state::State;
use rand::{Rng, RngExt};

/// Fraction of samples drawn from the error-biased distribution.
/// The remaining `1 - BIASED_SAMPLING_RATE` are drawn uniformly.
const BIASED_SAMPLING_RATE: f64 = 0.8;
const QUADRATIC_HILL_CLIMB_SEEDS: usize = 2;

/// Per-thread scratch state for candidate evaluation.
///
/// Each worker thread gets its own `WorkerCtx` so that shape rasterization
/// and scoring can proceed without any synchronization.
pub(crate) struct WorkerCtx<R> {
    /// Image width in pixels.
    pub(crate) width: i32,
    /// Image height in pixels.
    pub(crate) height: i32,
    /// Reusable storage for rasterized scanlines.
    pub(crate) lines: Vec<Scanline>,
    /// Reusable storage for per-scanline min bounds during rectangle tracking.
    pub(crate) rect_min: Vec<i32>,
    /// Reusable storage for per-scanline max bounds during rectangle tracking.
    pub(crate) rect_max: Vec<i32>,
    /// The worker's own RNG instance.
    pub(crate) rng: R,
    /// Running count of energy evaluations performed by this worker.
    pub(crate) evaluations: u64,
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
}

impl<R: Rng> WorkerCtx<R> {
    #[must_use]
    pub(crate) fn new(width: i32, height: i32, rng: R) -> Self {
        let edge_capacity = (width + 2 * height) as usize;
        Self {
            width,
            height,
            lines: Vec::with_capacity(4096),
            rect_min: Vec::with_capacity(edge_capacity),
            rect_max: Vec::with_capacity(edge_capacity),
            rng,
            evaluations: 0,
        }
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
    /// 1. Call `rasterize` to populate `self.lines`.
    /// 2. Compute the optimal blending color for those scanlines.
    /// 3. Fused score: compute blend and partial RMS in one pass (no buffer write).
    /// 4. Increment `self.evaluations`.
    pub(crate) fn energy(
        &mut self,
        round: &SearchRound<'_>,
        rasterize: impl FnOnce(&mut Self) -> &[Scanline],
        alpha: i32,
    ) -> u64 {
        let lines = rasterize(self);
        if lines.is_empty() {
            self.evaluations += 1;
            return round.score;
        }

        let color = score::compute_color(round.target, round.current, lines, alpha);
        // Re-borrow lines from self after the closure has returned.
        // Fused blend + diff: avoids any intermediate buffer write.
        let result = score::energy_from_lines_raw(
            round.target,
            round.current,
            &self.lines,
            color,
            round.score,
        );
        self.evaluations += 1;
        result
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
            let energy = state.energy(self, round);
            if energy < best_energy {
                best_energy = energy;
                best_state = state;
            }
        }
        best_state
    }

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

    fn best_random_states(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        limit: usize,
    ) -> Vec<State> {
        assert!(n > 0, "best_random_states requires at least one sample");
        assert!(
            limit > 0,
            "best_random_states requires at least one retained state"
        );

        let mut states = Vec::with_capacity(limit);
        for _ in 0..n {
            let mut state = self.random_state(round, kind, alpha);
            let _ = state.energy(self, round);
            Self::insert_top_state(&mut states, state, limit);
        }
        states
    }

    pub(crate) fn best_hill_climb_state(
        &mut self,
        round: &SearchRound<'_>,
        kind: ShapeKind,
        alpha: Alpha,
        n: usize,
        age: usize,
        m: usize,
    ) -> State {
        assert!(m > 0, "best_hill_climb_state requires at least one round");

        if kind == ShapeKind::Quadratic {
            let mut best_state = None;
            let mut best_energy = u64::MAX;

            for _ in 0..m {
                for seed in
                    self.best_random_states(round, kind, alpha, n, QUADRATIC_HILL_CLIMB_SEEDS)
                {
                    let mut state = hill_climb(&seed, self, round, age);
                    let energy = state.energy(self, round);
                    if energy < best_energy {
                        best_energy = energy;
                        best_state = Some(state);
                    }
                }
            }

            return best_state.expect("quadratic search should retain at least one state");
        }

        let mut best_state = hill_climb(
            &self.best_random_state(round, kind, alpha, n),
            self,
            round,
            age,
        );
        let mut best_energy = best_state.energy(self, round);

        for _ in 1..m {
            let state = self.best_random_state(round, kind, alpha, n);
            let mut state = hill_climb(&state, self, round, age);
            let energy = state.energy(self, round);
            if energy < best_energy {
                best_energy = energy;
                best_state = state;
            }
        }

        best_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;
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
        assert!(w.rect_min.capacity() >= (80 + 2 * 60) as usize);
        assert!(w.rect_max.capacity() >= (80 + 2 * 60) as usize);
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
        tp[3] = 255;

        let current = Buffer::new(4, 4);
        let mut grid = ErrorGrid::new(4, 4, 2, 2);
        grid.compute(&target, &current);

        let base_score = score::difference_full_raw(&target, &current);

        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: base_score,
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
    fn random_state_any_uses_supported_shape_kinds() {
        let target = Buffer::new_from_color(32, 32, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(32, 32, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(32, 32, 4, 4);
        grid.compute(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
        };

        let mut worker = WorkerCtx::new(32, 32, test_rng());
        for _ in 0..32 {
            let state = worker.random_state(&round, ShapeKind::Any, fixed_alpha(128));
            match state.shape {
                crate::shapes::Shape::Triangle(_)
                | crate::shapes::Shape::Rectangle(_)
                | crate::shapes::Shape::Ellipse(_)
                | crate::shapes::Shape::Circle(_)
                | crate::shapes::Shape::RotatedRectangle(_)
                | crate::shapes::Shape::Quadratic(_)
                | crate::shapes::Shape::RotatedEllipse(_)
                | crate::shapes::Shape::Polygon(_) => {}
            }
        }
    }

    #[test]
    fn best_random_state_returns_finite_energy() {
        let target = Buffer::new_from_color(32, 32, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(32, 32, Color::new(0, 0, 0, 255));
        let mut grid = ErrorGrid::new(32, 32, 4, 4);
        grid.compute(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: score::difference_full_raw(&target, &current),
        };

        let mut worker = WorkerCtx::new(32, 32, test_rng());
        let mut state = worker.best_random_state(&round, ShapeKind::Any, fixed_alpha(128), 8);
        let energy = state.energy(&mut worker, &round);

        assert!(energy > 0);
        assert!(worker.evaluations >= 8);
    }
}
