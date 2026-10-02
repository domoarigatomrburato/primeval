use crate::shapes::Shape;
use crate::worker::{SearchRound, WorkerCtx};
use rand::{Rng, RngExt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    Fixed,
    Auto,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub shape: Shape,
    pub alpha_mode: AlphaMode,
    pub alpha: u8,
    pub cached_energy: Option<u64>,
}

impl State {
    #[must_use]
    pub fn new(shape: Shape, alpha: i32) -> Self {
        if alpha == 0 {
            return Self {
                shape,
                alpha_mode: AlphaMode::Auto,
                alpha: 128,
                cached_energy: None,
            };
        }

        Self {
            shape,
            alpha_mode: AlphaMode::Fixed,
            alpha: alpha.clamp(1, 255) as u8,
            cached_energy: None,
        }
    }

    pub fn energy<R: Rng>(&mut self, worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> u64 {
        if let Some(energy) = self.cached_energy {
            return energy;
        }

        let energy = worker.energy(
            round,
            |ctx| self.shape.rasterize(ctx),
            i32::from(self.alpha),
        );
        self.cached_energy = Some(energy);
        energy
    }

    pub fn do_move<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) -> Self {
        let previous = self.clone();
        self.shape.mutate(worker);
        if self.alpha_mode == AlphaMode::Auto {
            let delta = worker.rng.random_range(0..21) - 10;
            self.alpha = (i32::from(self.alpha) + delta).clamp(1, 255) as u8;
        }
        self.cached_energy = None;
        previous
    }

    pub fn undo_move(&mut self, previous: Self) {
        *self = previous;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Circle, Shape};
    use crate::test_util::make_test_round;

    fn round(w: u32, h: u32) -> (WorkerCtx<rand_chacha::ChaCha8Rng>, SearchRound<'static>) {
        make_test_round(w, h, 123)
    }

    #[test]
    fn new_with_zero_alpha_enables_auto_mode() {
        let state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), 0);

        assert_eq!(state.alpha_mode, AlphaMode::Auto);
        assert_eq!(state.alpha, 128);
        assert_eq!(state.cached_energy, None);
    }

    #[test]
    fn energy_is_cached_after_first_evaluation() {
        let (mut worker, round) = round(16, 16);
        let mut state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), 128);

        let first = state.energy(&mut worker, &round);
        let second = state.energy(&mut worker, &round);

        assert_eq!(first, second);
        assert_eq!(worker.evaluations, 1);
    }

    #[test]
    fn do_move_invalidates_cached_energy() {
        let (mut worker, round) = round(16, 16);
        let mut state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), 128);
        let _ = state.energy(&mut worker, &round);

        let _previous = state.do_move(&mut worker);

        assert_eq!(state.cached_energy, None);
    }
}
