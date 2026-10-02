use crate::alpha::Alpha;
use crate::shapes::Shape;
use crate::worker::{SearchRound, WorkerCtx};
use rand::{Rng, RngExt};

/// Alpha a search starts from when the alpha is automatic.
const AUTO_ALPHA_START: u8 = 128;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State {
    pub(crate) shape: Shape,
    /// Whether moves mutate `alpha` too.
    auto_alpha: bool,
    /// Always `1..=255`.
    pub(crate) alpha: u8,
    pub(crate) cached_energy: Option<u64>,
}

impl State {
    #[must_use]
    pub(crate) fn new(shape: Shape, alpha: Alpha) -> Self {
        let (auto_alpha, alpha) = match alpha {
            Alpha::Auto => (true, AUTO_ALPHA_START),
            Alpha::Fixed(alpha) => (false, alpha.get()),
        };
        Self {
            shape,
            auto_alpha,
            alpha,
            cached_energy: None,
        }
    }

    pub(crate) fn energy<R: Rng>(
        &mut self,
        worker: &mut WorkerCtx<R>,
        round: &SearchRound<'_>,
    ) -> u64 {
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

    pub(crate) fn do_move<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) -> Self {
        let previous = self.clone();
        self.shape.mutate(worker);
        if self.auto_alpha {
            let delta = worker.rng.random_range(0..21) - 10;
            self.alpha = (i32::from(self.alpha) + delta).clamp(1, 255) as u8;
        }
        self.cached_energy = None;
        previous
    }

    pub(crate) fn undo_move(&mut self, previous: Self) {
        *self = previous;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Circle, Shape};
    use crate::test_util::{fixed_alpha, make_test_round};

    fn round(w: u32, h: u32) -> (WorkerCtx<rand_chacha::ChaCha8Rng>, SearchRound<'static>) {
        make_test_round(w, h, 123)
    }

    #[test]
    fn new_with_auto_alpha_starts_at_128() {
        let state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), Alpha::Auto);

        assert!(state.auto_alpha);
        assert_eq!(state.alpha, 128);
        assert_eq!(state.cached_energy, None);
    }

    #[test]
    fn new_with_fixed_alpha_keeps_it() {
        let state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), fixed_alpha(200));

        assert!(!state.auto_alpha);
        assert_eq!(state.alpha, 200);
    }

    #[test]
    fn energy_is_cached_after_first_evaluation() {
        let (mut worker, round) = round(16, 16);
        let mut state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), fixed_alpha(128));

        let first = state.energy(&mut worker, &round);
        let second = state.energy(&mut worker, &round);

        assert_eq!(first, second);
        assert_eq!(worker.evaluations, 1);
    }

    #[test]
    fn do_move_invalidates_cached_energy() {
        let (mut worker, round) = round(16, 16);
        let mut state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), fixed_alpha(128));
        let _ = state.energy(&mut worker, &round);

        let _previous = state.do_move(&mut worker);

        assert_eq!(state.cached_energy, None);
    }
}
