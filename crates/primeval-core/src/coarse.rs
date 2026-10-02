//! The half-resolution target and canvas that the random phase of the
//! search scores against.
//!
//! Each search round first samples many independent random candidates and
//! keeps the best few, then hill-climbs from them. The random phase only
//! has to rank candidates, so it scores them on a 2× box-downsampled copy of
//! the target and the canvas, a quarter of the pixels; the best few are then
//! rescored at full resolution, where the hill climb runs. Only the kinds
//! for which [`ShapeKind::ranks_coarsely`](crate::shapes::ShapeKind::ranks_coarsely)
//! holds use it (see
//! [`WorkerCtx::search_round`](crate::worker::WorkerCtx::search_round)).

use crate::buffer::{BYTES_PER_PIXEL, Buffer};
use crate::error_grid::ErrorGrid;
use crate::score;
use crate::worker::SearchRound;

/// The smallest full-resolution width and height that get a coarse copy:
/// below it the half-resolution canvas is too small to rank shapes, and the
/// search is cheap anyway.
pub(crate) const MIN_SIDE: u32 = 32;

/// The 2× downsampled target and canvas, kept in sync with the model's
/// canvas after every committed shape.
#[derive(Clone)]
pub(crate) struct Coarse {
    target: Buffer,
    current: Buffer,
    /// Only its prefix sums are used: the search samples positions from the
    /// full-resolution grid.
    error_grid: ErrorGrid,
    score: u64,
}

impl Coarse {
    /// The coarse copy of `target` and `current`, or `None` if either side
    /// is below [`MIN_SIDE`].
    #[must_use]
    pub(crate) fn new(target: &Buffer, current: &Buffer) -> Option<Self> {
        if target.width() < MIN_SIDE || target.height() < MIN_SIDE {
            return None;
        }
        let target = downsample(target);
        let current = downsample(current);
        let score = score::difference_full_raw(&target, &current);
        let error_grid = ErrorGrid::new(target.width(), target.height(), 1, 1);
        Some(Self {
            target,
            current,
            error_grid,
            score,
        })
    }

    /// Downsamples `current` again after a shape was drawn on it.
    pub(crate) fn sync(&mut self, current: &Buffer) {
        downsample_into(current, &mut self.current);
        self.score = score::difference_full_raw(&self.target, &self.current);
    }

    /// Rebuilds the prefix sums the round's evaluations read; call once per
    /// step, before [`Coarse::round`].
    pub(crate) fn prepare(&mut self) {
        self.error_grid.compute(&self.target, &self.current);
    }

    /// The search round over the coarse buffers.
    #[must_use]
    pub(crate) fn round(&self) -> SearchRound<'_> {
        SearchRound {
            target: &self.target,
            current: &self.current,
            error_grid: &self.error_grid,
            score: self.score,
            coarse: None,
        }
    }
}

/// The side of the 2× downsample of a side of `side` pixels.
fn half(side: u32) -> u32 {
    side.div_ceil(2)
}

/// The 2× box downsample of `src`: each pixel is the rounded mean of the
/// 2 × 2 block it covers, clipped to `src` on an odd last row or column.
#[must_use]
pub(crate) fn downsample(src: &Buffer) -> Buffer {
    let mut dst = Buffer::new_from_color(
        half(src.width()),
        half(src.height()),
        crate::Color::new(0, 0, 0, 255),
    );
    downsample_into(src, &mut dst);
    dst
}

/// [`downsample`] into `dst`, which must already have the downsampled size.
fn downsample_into(src: &Buffer, dst: &mut Buffer) {
    assert!(
        dst.width() == half(src.width()) && dst.height() == half(src.height()),
        "the coarse buffer must be half the size of the source"
    );
    let (width, height) = (src.width() as usize, src.height() as usize);
    let src_pixels = src.pixels();
    let dst_width = dst.width() as usize;
    for (index, dst_pixel) in dst
        .pixels_mut()
        .as_chunks_mut::<BYTES_PER_PIXEL>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = (2 * (index % dst_width), 2 * (index / dst_width));
        let (xs, ys) = (x..(x + 2).min(width), y..(y + 2).min(height));
        let count = (xs.len() * ys.len()) as u32;
        let mut sums = [0_u32; BYTES_PER_PIXEL];
        for sy in ys {
            let row = &src_pixels[(sy * width + xs.start) * BYTES_PER_PIXEL
                ..(sy * width + xs.end) * BYTES_PER_PIXEL];
            for pixel in row.as_chunks::<BYTES_PER_PIXEL>().0 {
                for (sum, &value) in sums.iter_mut().zip(pixel) {
                    *sum += u32::from(value);
                }
            }
        }
        for (value, sum) in dst_pixel.iter_mut().zip(sums) {
            *value = ((sum + count / 2) / count) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn noise(width: u32, height: u32, rng: &mut ChaCha8Rng) -> Buffer {
        let mut pixels = vec![0_u8; (width * height) as usize * BYTES_PER_PIXEL];
        rng.fill(&mut pixels[..]);
        Buffer::from_rgb(width, height, pixels).expect("valid length")
    }

    /// The rounded mean of every source pixel inside the block, computed
    /// one destination pixel at a time.
    fn reference(src: &Buffer) -> Vec<u8> {
        let (width, height) = (src.width(), src.height());
        let mut out = Vec::new();
        for y in 0..half(height) {
            for x in 0..half(width) {
                for channel in 0..BYTES_PER_PIXEL {
                    let mut sum = 0;
                    let mut count = 0;
                    for sy in 2 * y..(2 * y + 2).min(height) {
                        for sx in 2 * x..(2 * x + 2).min(width) {
                            let offset = src.pix_offset(sx as i32, sy as i32) + channel;
                            sum += u32::from(src.pixels()[offset]);
                            count += 1;
                        }
                    }
                    out.push(((sum + count / 2) / count) as u8);
                }
            }
        }
        out
    }

    /// The coverage-weighted area and centroid of `lines`, in the
    /// continuous coordinates of their canvas scaled by `scale`.
    fn moments(lines: &[crate::scanline::Scanline], scale: f64) -> (f64, f64, f64) {
        let (mut area, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for line in lines {
            let weight = f64::from(line.alpha) / f64::from(0xFFFF_u32);
            let pixels = f64::from(line.x2 - line.x1 + 1);
            let x_mid = (f64::from(line.x1) + f64::from(line.x2) + 1.0) / 2.0;
            let y_mid = f64::from(line.y) + 0.5;
            area += weight * pixels;
            sx += weight * pixels * x_mid;
            sy += weight * pixels * y_mid;
        }
        (area * scale * scale, sx / area * scale, sy / area * scale)
    }

    /// The coarse rasterization of every kind covers, scaled back up, about
    /// the area of the full one, centred where it is, and stays inside the
    /// coarse canvas.
    #[test]
    fn coarse_rasterization_matches_the_full_shape_scaled_by_one_half() {
        use crate::shapes::{Shape, ShapeKind};
        use crate::worker::WorkerCtx;

        let mut rng = ChaCha8Rng::seed_from_u64(0x5ca1e);
        let (width, height) = (96, 64);
        let target = noise(width, height, &mut rng);
        let current = noise(width, height, &mut rng);
        let mut grid = ErrorGrid::new(width, height, 4, 4);
        grid.compute(&target, &current);
        let round = SearchRound {
            target: &target,
            current: &current,
            error_grid: &grid,
            score: 0,
            coarse: None,
        };
        let (coarse_width, coarse_height) = (half(width) as i32, half(height) as i32);
        for &kind in ShapeKind::all_kinds() {
            let mut full = WorkerCtx::new(width as i32, height as i32, rng.clone());
            let mut coarse = WorkerCtx::new(coarse_width, coarse_height, rng.clone());
            let (mut full_area, mut coarse_area, mut offsets, mut shapes) = (0.0, 0.0, 0.0, 0);
            for _ in 0..300 {
                let shape = Shape::random(kind, &mut full, &round);
                let lines = shape.rasterize_coarse(&mut coarse);
                for line in lines {
                    assert!(
                        (0..coarse_height).contains(&line.y)
                            && 0 <= line.x1
                            && line.x1 <= line.x2
                            && line.x2 < coarse_width,
                        "{kind:?}: {line:?} outside the coarse canvas"
                    );
                }
                let (area, cx, cy) = moments(lines, 2.0);
                let (expected_area, ex, ey) = moments(shape.rasterize(&mut full), 1.0);
                full_area += expected_area;
                coarse_area += area;
                if expected_area >= 32.0 && area > 0.0 {
                    offsets += (cx - ex).hypot(cy - ey);
                    shapes += 1;
                }
            }
            let ratio = coarse_area / full_area;
            assert!(
                (0.85..1.15).contains(&ratio),
                "{kind:?}: area ratio {ratio}"
            );
            assert!(shapes > 30, "{kind:?}: only {shapes} large shapes");
            let mean = offsets / f64::from(shapes);
            assert!(mean < 0.8, "{kind:?}: mean centroid offset {mean} px");
        }
    }

    #[test]
    fn downsample_averages_each_two_by_two_block() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xc0a5);
        for (width, height) in [(2, 2), (3, 3), (4, 2), (5, 8), (33, 17), (64, 48)] {
            let src = noise(width, height, &mut rng);
            let dst = downsample(&src);
            assert_eq!(
                (dst.width(), dst.height()),
                (half(width), half(height)),
                "{width}x{height}"
            );
            assert_eq!(dst.pixels(), &reference(&src)[..], "{width}x{height}");
        }
    }

    #[test]
    fn new_needs_both_sides_at_least_the_minimum() {
        let mut rng = ChaCha8Rng::seed_from_u64(0x51de);
        let small = MIN_SIDE - 1;
        for (width, height, some) in [
            (MIN_SIDE, MIN_SIDE, true),
            (small, MIN_SIDE, false),
            (MIN_SIDE, small, false),
        ] {
            let target = noise(width, height, &mut rng);
            let current = noise(width, height, &mut rng);
            assert_eq!(
                Coarse::new(&target, &current).is_some(),
                some,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn sync_downsamples_the_canvas_and_rescores_it() {
        let mut rng = ChaCha8Rng::seed_from_u64(0x5c0e);
        let target = noise(40, 33, &mut rng);
        let mut coarse = Coarse::new(&target, &noise(40, 33, &mut rng)).expect("large enough");
        let current = noise(40, 33, &mut rng);

        coarse.sync(&current);

        let round = coarse.round();
        assert_eq!(round.current.pixels(), downsample(&current).pixels());
        assert_eq!(round.target.pixels(), downsample(&target).pixels());
        assert_eq!(
            round.score,
            score::difference_full_raw(round.target, round.current)
        );
    }
}
