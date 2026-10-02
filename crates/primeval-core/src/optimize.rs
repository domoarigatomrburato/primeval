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
        // Only a strictly lower energy is a move forward.
        if let Some(energy) = current.energy_below(worker, round, best_energy) {
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

/// Hill climbing from `start`, whose energy and payload are `scored`, under
/// any evaluator: `evaluate` scores a state as an energy, lower is better,
/// and a payload that travels with it (the refit's fitted colour, say).
///
/// The moves are [`State::do_move`] and [`State::undo_move`], as in
/// [`hill_climb`]: a move is kept only if its energy is strictly lower than
/// the best so far, and the climb stops after `max_age` consecutive moves
/// that are not. Returns the best state with its energy and payload; an
/// energy that is not a number never counts as lower.
#[must_use]
pub(crate) fn climb<R: Rng, T>(
    start: State,
    scored: (f64, T),
    worker: &mut WorkerCtx<R>,
    max_age: usize,
    mut evaluate: impl FnMut(&State, &mut WorkerCtx<R>) -> (f64, T),
) -> (State, f64, T) {
    let mut current = start;
    let mut best = current.clone();
    let (mut best_energy, mut best_payload) = scored;
    let mut age = 0;

    while age < max_age {
        let undo = current.do_move(worker);
        let (energy, payload) = evaluate(&current, worker);
        if energy < best_energy {
            best_energy = energy;
            best_payload = payload;
            best = current.clone();
            age = 0;
        } else {
            current.undo_move(undo);
            age += 1;
        }
    }

    (best, best_energy, best_payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alpha::Alpha;
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

    #[test]
    fn climb_keeps_the_start_unless_a_move_is_strictly_lower() {
        let (mut worker, _round) = round(16, 16);
        let start = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), fixed_alpha(128));
        let mut calls = 0;

        let (best, energy, payload) = climb(start.clone(), (1.0, 0), &mut worker, 20, |_, _| {
            calls += 1;
            (1.0, calls)
        });

        assert_eq!((best, energy, payload, calls), (start, 1.0, 0, 20));
    }

    #[test]
    fn climb_keeps_the_last_strictly_lower_state_and_restarts_its_age() {
        let (mut worker, _round) = round(16, 16);
        let start = State::new(Shape::Circle(Circle { x: 5, y: 5, r: 3 }), Alpha::Auto);
        let mut calls = 0;
        let mut improved = None;

        // Moves 3 and 7 improve; NaN and equal energies never do, and after
        // move 7 the climb runs `max_age` more moves.
        let (best, energy, payload) = climb(start, (10.0, 0), &mut worker, 5, |state, _| {
            calls += 1;
            match calls {
                3 => (5.0, calls),
                4 => (f64::NAN, calls),
                7 => {
                    improved = Some(state.clone());
                    (4.0, calls)
                }
                _ => (10.0, calls),
            }
        });

        assert_eq!((energy, payload, calls), (4.0, 7, 12));
        assert_eq!(Some(best), improved);
    }
}
