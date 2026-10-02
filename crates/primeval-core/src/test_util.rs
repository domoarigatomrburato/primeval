use crate::alpha::Alpha;
use crate::buffer::Buffer;
use crate::color::Color;
use crate::error_grid::ErrorGrid;
use crate::rng::create_rng;
use crate::score;
use crate::worker::{SearchRound, WorkerCtx};

/// Creates a `(WorkerCtx, SearchRound)` pair for tests.
///
/// The target is solid white, current is solid black, and the error grid
/// is computed from them. Each call site should pass a different `seed`
/// to avoid accidental cross-test correlation.
pub(crate) fn make_test_round(
    width: u32,
    height: u32,
    seed: u64,
) -> (WorkerCtx<rand_chacha::ChaCha8Rng>, SearchRound<'static>) {
    let target = Box::leak(Box::new(Buffer::new_from_color(
        width,
        height,
        Color::new(255, 255, 255, 255),
    )));
    let current = Box::leak(Box::new(Buffer::new_from_color(
        width,
        height,
        Color::new(0, 0, 0, 255),
    )));
    let mut grid = ErrorGrid::new(width, height, 4, 4);
    grid.compute(target, current);
    let grid = Box::leak(Box::new(grid));
    let round = SearchRound {
        target,
        current,
        error_grid: grid,
        score: score::difference_full_raw(target, current),
        coarse: None,
    };
    let worker = WorkerCtx::new(width as i32, height as i32, create_rng(seed));
    (worker, round)
}

/// A fixed alpha for tests; panics on zero.
pub(crate) fn fixed_alpha(alpha: u8) -> Alpha {
    Alpha::Fixed(std::num::NonZeroU8::new(alpha).expect("non-zero alpha"))
}
