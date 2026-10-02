use crate::drawing::{Drawing, DrawnShape};
use crate::error_grid::ErrorGrid;
use crate::score;
use crate::shapes::{Shape, ShapeKind};
use crate::state::State;
use crate::worker::{SearchRound, WorkerCtx};
use crate::{Buffer, Color};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct CommittedShape {
    pub shape: Shape,
    pub color: Color,
    pub alpha: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelOptions {
    pub seed: Option<u64>,
    pub workers: usize,
    pub grid_cols: u32,
    pub grid_rows: u32,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            seed: None,
            workers: 1,
            grid_cols: 16,
            grid_rows: 16,
        }
    }
}

pub struct Model {
    pub background: Color,
    pub target: Buffer,
    pub current: Buffer,
    pub(crate) score: u64,
    pub history: Vec<CommittedShape>,
    error_grid: ErrorGrid,
    workers: Vec<WorkerCtx<ChaCha8Rng>>,
}

impl Model {
    fn search_params(kind: ShapeKind) -> (usize, usize) {
        match kind {
            ShapeKind::Quadratic => (900, 100),
            _ => (1000, 100),
        }
    }

    #[must_use]
    pub fn new(target: Buffer, background: Color, options: ModelOptions) -> Self {
        let target_width = target.width();
        let target_height = target.height();
        let current = Buffer::new_from_color(target_width, target_height, background);
        let score = score::difference_full_raw(&target, &current);
        let worker_count = options.workers.max(1);
        let seed = options.seed.unwrap_or_else(crate::util::system_clock_seed);
        let workers = (0..worker_count)
            .map(|index| {
                WorkerCtx::new(
                    target_width as i32,
                    target_height as i32,
                    crate::rng::create_rng(seed + index as u64),
                )
            })
            .collect();

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
            workers,
        }
    }

    pub fn step(&mut self, kind: ShapeKind, alpha: i32) -> Result<u64, String> {
        let evaluations_before: u64 = self.workers.iter().map(|worker| worker.evaluations).sum();
        self.error_grid.compute(&self.target, &self.current);

        let score = self.score;
        let target = &self.target;
        let current = &self.current;
        let error_grid = &self.error_grid;
        let workers = &mut self.workers;
        let round = SearchRound {
            target,
            current,
            error_grid,
            score,
        };
        let worker_count = workers.len().max(1);
        let worker_rounds = 16_usize.div_ceil(worker_count);
        let (candidate_count, hill_climb_age) = Self::search_params(kind);
        let states: Vec<State> = workers
            .par_iter_mut()
            .map(|worker| {
                worker.best_hill_climb_state(
                    &round,
                    kind,
                    alpha,
                    candidate_count,
                    hill_climb_age,
                    worker_rounds,
                )
            })
            .collect();

        let best = states
            .into_iter()
            .min_by(|left, right| {
                let left_energy = left.cached_energy.unwrap_or(u64::MAX);
                let right_energy = right.cached_energy.unwrap_or(u64::MAX);
                left_energy.cmp(&right_energy)
            })
            .ok_or_else(|| "worker search produced no state".to_string())?;

        self.add(best.shape, best.alpha);

        let evaluations_after: u64 = self.workers.iter().map(|worker| worker.evaluations).sum();
        Ok(evaluations_after - evaluations_before)
    }

    pub fn add(&mut self, shape: Shape, alpha: u8) {
        let worker = &mut self.workers[0];
        let lines = shape.rasterize(worker);
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
        self.history.push(CommittedShape {
            shape,
            color,
            alpha,
        });
    }

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
    fn step_reports_only_incremental_evaluations() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            ModelOptions {
                seed: Some(7),
                ..ModelOptions::default()
            },
        );

        let _ = model
            .step(ShapeKind::Triangle, 128)
            .expect("first step should succeed");
        let first_total: u64 = model.workers.iter().map(|worker| worker.evaluations).sum();

        let second_reported = model
            .step(ShapeKind::Triangle, 128)
            .expect("second step should succeed");
        let second_total: u64 = model.workers.iter().map(|worker| worker.evaluations).sum();

        assert_eq!(second_reported, second_total - first_total);
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

        let evaluations = model
            .step(ShapeKind::Triangle, 128)
            .expect("step should succeed");

        assert!(evaluations > 0);
    }
}
