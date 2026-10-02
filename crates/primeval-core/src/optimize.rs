use crate::state::State;
use crate::worker::{SearchRound, WorkerCtx};
use rand::Rng;

#[must_use]
pub(crate) fn hill_climb<R: Rng>(
    state: &State,
    worker: &mut WorkerCtx<R>,
    round: &SearchRound<'_>,
    max_age: usize,
) -> State {
    let mut current = state.clone();
    let mut best_state = current.clone();
    let mut best_energy = current.energy(worker, round);
    let mut age = 0;

    while age < max_age {
        let undo = current.do_move(worker);
        let energy = current.energy(worker, round);
        if energy >= best_energy {
            current.undo_move(undo);
            age += 1;
        } else {
            best_energy = energy;
            best_state = current.clone();
            age = 0;
        }
    }

    best_state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Circle, Shape};
    use crate::state::State;
    use crate::test_util::{fixed_alpha, make_test_round};

    fn round(w: u32, h: u32) -> (WorkerCtx<rand_chacha::ChaCha8Rng>, SearchRound<'static>) {
        make_test_round(w, h, 456)
    }

    #[test]
    fn hill_climb_with_zero_age_returns_equivalent_state() {
        let (mut worker, round) = round(16, 16);
        let state = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), fixed_alpha(128));

        let result = hill_climb(&state, &mut worker, &round, 0);

        assert_eq!(result.shape, state.shape);
        assert_eq!(result.alpha, state.alpha);
    }
}
