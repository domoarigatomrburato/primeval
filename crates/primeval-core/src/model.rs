use crate::error_grid::ErrorGrid;
use crate::optimize::hill_climb;
use crate::score;
use crate::shapes::{Shape, ShapeKind};
use crate::state::State;
use crate::worker::{QuadraticProfileStats, SearchRound, WorkerCtx, merge_quadratic_profile_stats};
use crate::{Buffer, Color};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct CommittedShape {
    pub shape: Shape,
    pub color: Color,
    pub alpha: u8,
    pub score: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelOptions {
    pub seed: Option<u64>,
    pub workers: usize,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub profile_quadratic: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            seed: None,
            workers: 1,
            grid_cols: 16,
            grid_rows: 16,
            profile_quadratic: false,
        }
    }
}

pub struct Model {
    pub working_width: u32,
    pub working_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub scale: f32,
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
    pub fn new(target: Buffer, background: Color, output_size: u32, options: ModelOptions) -> Self {
        let target_width = target.width();
        let target_height = target.height();
        let aspect = target_width as f32 / target_height as f32;
        let (output_width, output_height, scale) = if aspect >= 1.0 {
            let width = output_size;
            let height = ((output_size as f32) / aspect).round().max(1.0) as u32;
            (width, height, output_size as f32 / target_width as f32)
        } else {
            let width = ((output_size as f32) * aspect).round().max(1.0) as u32;
            let height = output_size;
            (width, height, output_size as f32 / target_height as f32)
        };

        let current = Buffer::new_from_color(target_width, target_height, background);
        let score = score::difference_full_raw(&target, &current);
        let worker_count = options.workers.max(1);
        let seed = options.seed.unwrap_or_else(crate::util::system_clock_seed);
        let workers = (0..worker_count)
            .map(|index| {
                WorkerCtx::new_with_quadratic_profiling(
                    target_width as i32,
                    target_height as i32,
                    crate::rng::create_rng(seed + index as u64),
                    options.profile_quadratic,
                )
            })
            .collect();

        Self {
            working_width: target_width,
            working_height: target_height,
            output_width,
            output_height,
            scale,
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

    pub fn step(&mut self, kind: ShapeKind, alpha: i32, repeat: usize) -> Result<u64, String> {
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

        self.add(best.shape.clone(), best.alpha);

        let mut repeat_state = best;
        for _ in 0..repeat {
            let round = SearchRound {
                target: &self.target,
                current: &self.current,
                error_grid: &self.error_grid,
                score: self.score,
            };
            let before = repeat_state.energy(&mut self.workers[0], &round);
            repeat_state = hill_climb(&repeat_state, &mut self.workers[0], &round, hill_climb_age);
            let after = repeat_state.energy(&mut self.workers[0], &round);
            if before == after {
                break;
            }
            self.add(repeat_state.shape.clone(), repeat_state.alpha);
        }

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
            score: self.score_f64(),
        });
    }

    #[must_use]
    pub fn score_f64(&self) -> f64 {
        score::raw_score_to_normalized(self.score, self.current.width(), self.current.height())
    }

    #[must_use]
    pub fn quadratic_profile_stats(&self) -> Option<QuadraticProfileStats> {
        let stats = merge_quadratic_profile_stats(
            self.workers
                .iter()
                .filter_map(WorkerCtx::quadratic_profile_stats),
        );
        (stats != QuadraticProfileStats::default()).then_some(stats)
    }

    #[must_use]
    pub fn render_output(&self) -> Buffer {
        let mut output =
            Buffer::new_from_color(self.output_width, self.output_height, self.background);
        self.replay_history_into(&mut output, None);
        output
    }

    #[must_use]
    pub fn frames(&self, score_delta: f64) -> Vec<Buffer> {
        let mut output =
            Buffer::new_from_color(self.output_width, self.output_height, self.background);
        self.replay_history_into(&mut output, Some(score_delta))
    }

    fn replay_history_into(&self, output: &mut Buffer, score_delta: Option<f64>) -> Vec<Buffer> {
        let mut result = Vec::new();
        if score_delta.is_some() {
            result.push(output.clone());
        }
        let mut previous = 10.0;
        let mut worker = WorkerCtx::new(
            self.output_width as i32,
            self.output_height as i32,
            crate::rng::create_rng(1),
        );

        for committed in &self.history {
            let lines = self.rasterize_output_shape(&committed.shape, &mut worker);
            crate::score::draw_lines(output, committed.color, lines);
            if let Some(score_delta) = score_delta {
                let delta = previous - committed.score;
                if delta >= score_delta {
                    previous = committed.score;
                    result.push(output.clone());
                }
            }
        }

        result
    }

    fn rasterize_output_shape<'a>(
        &self,
        shape: &Shape,
        worker: &'a mut WorkerCtx<ChaCha8Rng>,
    ) -> &'a [crate::scanline::Scanline] {
        if self.scale > 1.0 {
            let scale = f64::from(self.scale);
            match shape {
                Shape::Ellipse(ellipse) => {
                    crate::raster::fill_rotated_ellipse_direct(
                        &mut worker.lines,
                        (f64::from(ellipse.x) + 0.5) * scale,
                        (f64::from(ellipse.y) + 0.5) * scale,
                        f64::from(ellipse.rx) * scale,
                        f64::from(ellipse.ry) * scale,
                        0.0,
                        worker.width,
                        worker.height,
                    );
                    return &worker.lines;
                }
                Shape::Circle(circle) => {
                    crate::raster::fill_rotated_ellipse_direct(
                        &mut worker.lines,
                        (f64::from(circle.x) + 0.5) * scale,
                        (f64::from(circle.y) + 0.5) * scale,
                        f64::from(circle.r) * scale,
                        f64::from(circle.r) * scale,
                        0.0,
                        worker.width,
                        worker.height,
                    );
                    return &worker.lines;
                }
                _ => {}
            }
        }

        shape.scaled(self.scale).rasterize(worker)
    }

    #[must_use]
    pub fn svg(&self) -> String {
        let mut lines = Vec::with_capacity(self.history.len() + 5);
        lines.push(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{}\" height=\"{}\">",
            self.output_width, self.output_height
        ));
        lines.push(format!(
            "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"#{:02x}{:02x}{:02x}\" />",
            self.output_width,
            self.output_height,
            self.background.r,
            self.background.g,
            self.background.b
        ));
        lines.push(format!(
            "<g transform=\"scale({}) translate(0.5 0.5)\">",
            self.scale
        ));
        for committed in &self.history {
            let attrs = format!(
                "fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{}\"",
                committed.color.r,
                committed.color.g,
                committed.color.b,
                f64::from(committed.color.a) / 255.0
            );
            lines.push(committed.shape.to_svg(&attrs));
        }
        lines.push("</g>".to_string());
        lines.push("</svg>".to_string());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::fill_rotated_ellipse_direct;
    use crate::score;
    use crate::shapes::{Circle, Ellipse, Rectangle, Shape};

    #[test]
    fn search_params_keeps_quadratic_budget_near_default() {
        assert_eq!(Model::search_params(ShapeKind::Quadratic), (900, 100));
        assert_eq!(Model::search_params(ShapeKind::Circle), (1000, 100));
    }

    #[test]
    fn render_output_replays_scaled_history() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            16,
            ModelOptions::default(),
        );

        model.add(
            Shape::Rectangle(Rectangle {
                x1: 1,
                y1: 1,
                x2: 3,
                y2: 3,
            }),
            255,
        );

        let rendered = model.render_output();
        let mut expected =
            Buffer::new_from_color(model.output_width, model.output_height, model.background);
        let committed = &model.history[0];
        let scaled = committed.shape.scaled(model.scale);
        let mut worker = WorkerCtx::new(
            model.output_width as i32,
            model.output_height as i32,
            crate::rng::create_rng(1),
        );
        let lines = scaled.rasterize(&mut worker).to_vec();
        score::draw_lines(&mut expected, committed.color, &lines);

        assert_eq!(rendered.pixels(), expected.pixels());
    }

    #[test]
    fn render_output_replays_scaled_ellipse_with_antialiasing() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            16,
            ModelOptions::default(),
        );

        model.add(
            Shape::Ellipse(Ellipse {
                x: 3,
                y: 4,
                rx: 2,
                ry: 1,
            }),
            255,
        );

        let rendered = model.render_output();
        let mut expected =
            Buffer::new_from_color(model.output_width, model.output_height, model.background);
        let committed = &model.history[0];
        let mut worker = WorkerCtx::new(
            model.output_width as i32,
            model.output_height as i32,
            crate::rng::create_rng(1),
        );
        fill_rotated_ellipse_direct(
            &mut worker.lines,
            (3.0 + 0.5) * f64::from(model.scale),
            (4.0 + 0.5) * f64::from(model.scale),
            2.0 * f64::from(model.scale),
            1.0 * f64::from(model.scale),
            0.0,
            model.output_width as i32,
            model.output_height as i32,
        );
        score::draw_lines(&mut expected, committed.color, &worker.lines);

        assert_eq!(rendered.pixels(), expected.pixels());
    }

    #[test]
    fn render_output_replays_scaled_circle_with_antialiasing() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(
            target,
            Color::new(0, 0, 0, 255),
            16,
            ModelOptions::default(),
        );

        model.add(Shape::Circle(Circle { x: 3, y: 4, r: 2 }), 255);

        let rendered = model.render_output();
        let mut expected =
            Buffer::new_from_color(model.output_width, model.output_height, model.background);
        let committed = &model.history[0];
        let mut worker = WorkerCtx::new(
            model.output_width as i32,
            model.output_height as i32,
            crate::rng::create_rng(1),
        );
        fill_rotated_ellipse_direct(
            &mut worker.lines,
            (3.0 + 0.5) * f64::from(model.scale),
            (4.0 + 0.5) * f64::from(model.scale),
            2.0 * f64::from(model.scale),
            2.0 * f64::from(model.scale),
            0.0,
            model.output_width as i32,
            model.output_height as i32,
        );
        score::draw_lines(&mut expected, committed.color, &worker.lines);

        assert_eq!(rendered.pixels(), expected.pixels());
    }

    #[test]
    fn add_score_matches_full_recomputation() {
        let target = Buffer::new_from_color(8, 8, Color::new(255, 255, 255, 255));
        let mut model = Model::new(target, Color::new(0, 0, 0, 255), 8, ModelOptions::default());

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
            8,
            ModelOptions {
                seed: Some(7),
                ..ModelOptions::default()
            },
        );

        let _ = model
            .step(ShapeKind::Triangle, 128, 0)
            .expect("first step should succeed");
        let first_total: u64 = model.workers.iter().map(|worker| worker.evaluations).sum();

        let second_reported = model
            .step(ShapeKind::Triangle, 128, 0)
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
            8,
            ModelOptions {
                seed: Some(7),
                grid_cols: 0,
                grid_rows: 0,
                ..ModelOptions::default()
            },
        );

        let evaluations = model
            .step(ShapeKind::Triangle, 128, 0)
            .expect("step should succeed");

        assert!(evaluations > 0);
    }
}
