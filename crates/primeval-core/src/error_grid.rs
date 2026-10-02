//! Spatial error distribution grid for biased shape placement.
//!
//! Divides the image into a grid of cells, measures per-cell error between
//! the target and current approximation, and builds a cumulative distribution
//! function so that random samples concentrate in high-error regions.

use crate::buffer::Buffer;
use rand::{Rng, RngExt};

/// A grid that tracks per-cell RGB error between target and current buffers.
///
/// After calling [`compute`](ErrorGrid::compute), the internal CDF allows
/// [`sample`](ErrorGrid::sample) and [`sample_float`](ErrorGrid::sample_float)
/// to produce coordinates biased toward high-error cells.
pub(crate) struct ErrorGrid {
    cols: u32,
    rows: u32,
    cell_w: u32,
    cell_h: u32,
    img_w: u32,
    img_h: u32,
    errors: Vec<f64>,
    cdf: Vec<f64>,
    total: f64,
}

impl ErrorGrid {
    /// Creates a new error grid for an image of size `img_w x img_h`
    /// divided into `cols` columns and `rows` rows.
    ///
    /// Cell dimensions are floored to at least 1 pixel.
    #[must_use]
    pub(crate) fn new(img_w: u32, img_h: u32, cols: u32, rows: u32) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let cell_w = (img_w / cols).max(1);
        let cell_h = (img_h / rows).max(1);
        let n = (cols * rows) as usize;
        Self {
            cols,
            rows,
            cell_w,
            cell_h,
            img_w,
            img_h,
            errors: vec![0.0; n],
            cdf: vec![0.0; n],
            total: 0.0,
        }
    }

    /// Returns the accumulated total error across all cells.
    ///
    /// This value is meaningful only after calling [`compute`](ErrorGrid::compute).
    #[cfg(test)]
    #[must_use]
    #[inline]
    pub(crate) fn total(&self) -> f64 {
        self.total
    }

    /// Recomputes per-cell errors and the CDF from the given target/current pair.
    ///
    /// Each cell accumulates the sum of squared RGB channel differences for
    /// every pixel it covers. The last column and last row extend to the
    /// image boundary so that no pixels are missed.
    pub(crate) fn compute(&mut self, target: &Buffer, current: &Buffer) {
        self.errors.fill(0.0);

        let img_w = self.img_w;
        let img_h = self.img_h;
        let t_pix = target.pixels();
        let c_pix = current.pixels();

        for row_idx in 0..self.rows {
            let y_start = row_idx * self.cell_h;
            if y_start >= img_h {
                break;
            }
            let y_end = if row_idx == self.rows - 1 {
                img_h
            } else {
                (y_start + self.cell_h).min(img_h)
            };
            let err_row_base = (row_idx * self.cols) as usize;

            for y in y_start..y_end {
                for col_idx in 0..self.cols {
                    let x_start = col_idx * self.cell_w;
                    if x_start >= img_w {
                        break;
                    }
                    let x_end = if col_idx == self.cols - 1 {
                        img_w
                    } else {
                        (x_start + self.cell_w).min(img_w)
                    };

                    let i_start = (y as usize * img_w as usize + x_start as usize) * 4;
                    let i_end = i_start + (x_end - x_start) as usize * 4;
                    let mut i = i_start;
                    while i < i_end {
                        let dr = i32::from(t_pix[i]) - i32::from(c_pix[i]);
                        let dg = i32::from(t_pix[i + 1]) - i32::from(c_pix[i + 1]);
                        let db = i32::from(t_pix[i + 2]) - i32::from(c_pix[i + 2]);
                        self.errors[err_row_base + col_idx as usize] +=
                            f64::from(dr * dr + dg * dg + db * db);
                        i += 4;
                    }
                }
            }
        }

        self.total = 0.0;
        for (i, &e) in self.errors.iter().enumerate() {
            self.total += e;
            self.cdf[i] = self.total;
        }
    }

    /// Samples an integer pixel coordinate biased toward high-error cells.
    ///
    /// The returned `(x, y)` is guaranteed to be within `[0, img_w) x [0, img_h)`.
    #[must_use]
    pub(crate) fn sample<R: Rng>(&self, rng: &mut R) -> (i32, i32) {
        if self.total <= 0.0 {
            if self.img_w == 0 || self.img_h == 0 {
                return (0, 0);
            }
            return (
                rng.random_range(0..self.img_w) as i32,
                rng.random_range(0..self.img_h) as i32,
            );
        }

        let r = rng.random::<f64>() * self.total;
        let idx = self.cdf.partition_point(|&v| v < r).min(self.cdf.len() - 1);
        let row = idx / self.cols as usize;
        let col = idx % self.cols as usize;

        let x_range = self.cell_w as i32;
        let y_range = self.cell_h as i32;
        let x = col as i32 * x_range + rng.random_range(0..x_range);
        let y = row as i32 * y_range + rng.random_range(0..y_range);

        let x = x.min(self.img_w as i32 - 1);
        let y = y.min(self.img_h as i32 - 1);
        (x, y)
    }

    /// Samples a floating-point coordinate biased toward high-error cells.
    ///
    /// The returned `(x, y)` is guaranteed to be within
    /// `[0.0, img_w as f64) x [0.0, img_h as f64)`.
    #[must_use]
    pub(crate) fn sample_float<R: Rng>(&self, rng: &mut R) -> (f64, f64) {
        if self.total <= 0.0 {
            if self.img_w == 0 || self.img_h == 0 {
                return (0.0, 0.0);
            }
            return (
                rng.random::<f64>() * self.img_w as f64,
                rng.random::<f64>() * self.img_h as f64,
            );
        }

        let r = rng.random::<f64>() * self.total;
        let idx = self.cdf.partition_point(|&v| v < r).min(self.cdf.len() - 1);
        let row = idx / self.cols as usize;
        let col = idx % self.cols as usize;

        let x = col as f64 * self.cell_w as f64 + rng.random::<f64>() * self.cell_w as f64;
        let y = row as f64 * self.cell_h as f64 + rng.random::<f64>() * self.cell_h as f64;

        let x = x.min(self.img_w as f64 - 1.0);
        let y = y.min(self.img_h as f64 - 1.0);
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn test_rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    #[test]
    fn new_computes_cell_dimensions() {
        let g = ErrorGrid::new(100, 80, 5, 4);
        assert_eq!(g.cell_w, 20);
        assert_eq!(g.cell_h, 20);
        assert_eq!(g.cols, 5);
        assert_eq!(g.rows, 4);
        assert_eq!(g.errors.len(), 20);
        assert_eq!(g.cdf.len(), 20);
    }

    #[test]
    fn new_clamps_cell_size_to_minimum_one() {
        // Image smaller than grid count — cell size floors to 1.
        let g = ErrorGrid::new(2, 3, 10, 10);
        assert_eq!(g.cell_w, 1);
        assert_eq!(g.cell_h, 1);
    }

    #[test]
    fn compute_handles_grids_larger_than_image() {
        let target = Buffer::new_from_color(2, 2, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(2, 2, Color::new(0, 0, 0, 255));
        let mut g = ErrorGrid::new(2, 2, 10, 10);

        g.compute(&target, &current);

        assert!(g.total() > 0.0);
    }

    #[test]
    fn compute_uniform_buffers_gives_zero_total() {
        let c = Color::new(128, 64, 32, 255);
        let target = Buffer::new_from_color(20, 20, c);
        let current = Buffer::new_from_color(20, 20, c);
        let mut g = ErrorGrid::new(20, 20, 4, 4);
        g.compute(&target, &current);
        assert_eq!(g.total(), 0.0);
        assert!(g.errors.iter().all(|&e| e == 0.0));
    }

    #[test]
    fn new_clamps_zero_grid_dimensions() {
        let g = ErrorGrid::new(20, 20, 0, 0);
        assert_eq!(g.cols, 1);
        assert_eq!(g.rows, 1);
        assert_eq!(g.errors.len(), 1);
        assert_eq!(g.cdf.len(), 1);
    }

    #[test]
    fn compute_known_different_buffers_gives_expected_errors() {
        // 4x4 image, 2x2 grid => each cell is 2x2 pixels.
        // Target: all white (255,255,255,255)
        // Current: all black (0,0,0,255) — alpha matches, so only RGB differs.
        let target = Buffer::new_from_color(4, 4, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(4, 4, Color::new(0, 0, 0, 255));

        let mut g = ErrorGrid::new(4, 4, 2, 2);
        g.compute(&target, &current);

        // Per pixel: dr=255, dg=255, db=255 => 255^2 * 3 = 195_075
        // Each cell has 2*2 = 4 pixels => 4 * 195_075 = 780_300
        let expected_per_cell = 4.0 * 195_075.0;
        for &e in &g.errors {
            assert!(
                (e - expected_per_cell).abs() < 1e-6,
                "expected {expected_per_cell}, got {e}"
            );
        }

        let expected_total = 4.0 * expected_per_cell;
        assert!(
            (g.total() - expected_total).abs() < 1e-6,
            "expected total {expected_total}, got {}",
            g.total()
        );
    }

    #[test]
    fn sample_returns_coordinates_within_bounds() {
        let target = Buffer::new_from_color(50, 30, Color::new(255, 0, 0, 255));
        let current = Buffer::new_from_color(50, 30, Color::new(0, 0, 0, 255));
        let mut g = ErrorGrid::new(50, 30, 5, 3);
        g.compute(&target, &current);

        let mut rng = test_rng();
        for _ in 0..1000 {
            let (x, y) = g.sample(&mut rng);
            assert!((0..50).contains(&x), "x={x} out of bounds");
            assert!((0..30).contains(&y), "y={y} out of bounds");
        }
    }

    #[test]
    fn sample_float_returns_coordinates_within_bounds() {
        let target = Buffer::new_from_color(50, 30, Color::new(255, 0, 0, 255));
        let current = Buffer::new_from_color(50, 30, Color::new(0, 0, 0, 255));
        let mut g = ErrorGrid::new(50, 30, 5, 3);
        g.compute(&target, &current);

        let mut rng = test_rng();
        for _ in 0..1000 {
            let (x, y) = g.sample_float(&mut rng);
            assert!((0.0..50.0).contains(&x), "x={x} out of bounds");
            assert!((0.0..30.0).contains(&y), "y={y} out of bounds");
        }
    }

    #[test]
    fn zero_total_sampling_stays_in_bounds_and_does_not_collapse() {
        let c = Color::new(128, 64, 32, 255);
        let target = Buffer::new_from_color(20, 20, c);
        let current = Buffer::new_from_color(20, 20, c);
        let mut g = ErrorGrid::new(20, 20, 4, 4);
        g.compute(&target, &current);

        let mut rng = test_rng();
        let mut sampled_cells = std::collections::BTreeSet::new();
        let mut sampled_float_cells = std::collections::BTreeSet::new();
        for _ in 0..128 {
            let (x, y) = g.sample(&mut rng);
            assert!((0..20).contains(&x), "x={x} out of bounds");
            assert!((0..20).contains(&y), "y={y} out of bounds");
            sampled_cells.insert((x / 5, y / 5));

            let (fx, fy) = g.sample_float(&mut rng);
            assert!((0.0..20.0).contains(&fx), "x={fx} out of bounds");
            assert!((0.0..20.0).contains(&fy), "y={fy} out of bounds");
            sampled_float_cells.insert(((fx / 5.0) as i32, (fy / 5.0) as i32));
        }

        assert!(
            sampled_cells.len() > 1,
            "integer sampling collapsed to one cell"
        );
        assert!(
            sampled_float_cells.len() > 1,
            "float sampling collapsed to one cell"
        );
    }

    #[test]
    fn zero_dimension_grid_sampling_stays_in_bounds() {
        let target = Buffer::new_from_color(12, 9, Color::new(255, 255, 255, 255));
        let current = Buffer::new_from_color(12, 9, Color::new(0, 0, 0, 255));
        let mut g = ErrorGrid::new(12, 9, 0, 0);
        g.compute(&target, &current);

        let mut rng = test_rng();
        for _ in 0..128 {
            let (x, y) = g.sample(&mut rng);
            assert!((0..12).contains(&x), "x={x} out of bounds");
            assert!((0..9).contains(&y), "y={y} out of bounds");
        }
    }

    #[test]
    fn biased_sampling_concentrates_on_high_error_cell() {
        // 10x10 image, 2x1 grid (2 columns, 1 row).
        // Left half: target red, current red (zero error).
        // Right half: target white, current black (high error).
        let mut target = Buffer::new_from_color(10, 10, Color::new(255, 0, 0, 255));
        let mut current = Buffer::new_from_color(10, 10, Color::new(255, 0, 0, 255));

        // Make right half different: target white, current black.
        for y in 0..10u32 {
            for x in 5..10u32 {
                let off = (y as usize * 10 + x as usize) * 4;
                let tp = target.pixels_mut();
                tp[off] = 255;
                tp[off + 1] = 255;
                tp[off + 2] = 255;
                let cp = current.pixels_mut();
                cp[off] = 0;
                cp[off + 1] = 0;
                cp[off + 2] = 0;
            }
        }

        let mut g = ErrorGrid::new(10, 10, 2, 1);
        g.compute(&target, &current);

        // Left cell should have zero error, right cell should have all error.
        assert_eq!(g.errors[0], 0.0);
        assert!(g.errors[1] > 0.0);

        let mut rng = test_rng();
        let mut right_count = 0u32;
        let total_samples = 1000;
        for _ in 0..total_samples {
            let (x, _) = g.sample(&mut rng);
            if x >= 5 {
                right_count += 1;
            }
        }

        // All samples should land in the right half since left has zero error.
        assert_eq!(
            right_count, total_samples,
            "expected all {total_samples} samples in right half, got {right_count}"
        );
    }
}
