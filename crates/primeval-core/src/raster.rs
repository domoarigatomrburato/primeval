// Rasterization functions naturally take many geometric parameters (points, dimensions).
#![allow(clippy::too_many_arguments)]
use crate::scanline::Scanline;
use crate::worker::WorkerCtx;
use rand::Rng;

/// Rasterises a stroked quadratic Bézier directly into `worker.lines`,
/// bypassing the tiny-skia pixmap pipeline entirely.
///
/// The curve from `(x1,y1)` through control point `(cx,cy)` to `(x2,y2)` is
/// adaptively subdivided via de Casteljau (flatness tolerance 0.5 px) and each
/// flat segment is rasterised with distance-based antialiased coverage using
/// the given stroke `half_width`.
pub(crate) fn stroke_quadratic_direct<R: Rng>(
    worker: &mut WorkerCtx<R>,
    x1: f64,
    y1: f64,
    cx: f64,
    cy: f64,
    x2: f64,
    y2: f64,
    half_width: f64,
) -> &[Scanline] {
    let w = worker.width;
    let h = worker.height;
    worker.lines.clear();
    subdivide_and_stroke(&mut worker.lines, x1, y1, cx, cy, x2, y2, half_width, w, h);
    &worker.lines
}

/// Recursively subdivides the quadratic Bézier until it is flat (distance from
/// control point to chord < 0.5 px), then rasterises each flat segment as a
/// stroked line with the given half-width.
fn subdivide_and_stroke(
    lines: &mut Vec<Scanline>,
    x1: f64,
    y1: f64,
    cx: f64,
    cy: f64,
    x2: f64,
    y2: f64,
    half_width: f64,
    w: i32,
    h: i32,
) {
    let chord_dx = x2 - x1;
    let chord_dy = y2 - y1;
    let chord_len_sq = chord_dx * chord_dx + chord_dy * chord_dy;
    let flat = if chord_len_sq < 1e-6 {
        true
    } else {
        let t = ((cx - x1) * chord_dx + (cy - y1) * chord_dy) / chord_len_sq;
        let proj_x = x1 + t * chord_dx;
        let proj_y = y1 + t * chord_dy;
        (cx - proj_x) * (cx - proj_x) + (cy - proj_y) * (cy - proj_y) < 0.25
    };

    if flat {
        stroke_segment(lines, x1, y1, x2, y2, half_width, w, h);
    } else {
        let mx12 = (x1 + cx) * 0.5;
        let my12 = (y1 + cy) * 0.5;
        let mx23 = (cx + x2) * 0.5;
        let my23 = (cy + y2) * 0.5;
        let mx = (mx12 + mx23) * 0.5;
        let my = (my12 + my23) * 0.5;
        subdivide_and_stroke(lines, x1, y1, mx12, my12, mx, my, half_width, w, h);
        subdivide_and_stroke(lines, mx, my, mx23, my23, x2, y2, half_width, w, h);
    }
}

/// Antialiased stroked segment rasteriser.
///
/// For each integer step along the dominant axis, pixels perpendicular to the
/// line are emitted with alpha based on their distance from the centre line.
/// Coverage is `clamp(half_width + 0.5 - perpendicular_distance, 0, 1)`,
/// giving a smooth antialiased stroke of the specified width.
fn stroke_segment(
    lines: &mut Vec<Scanline>,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    half_width: f64,
    w: i32,
    h: i32,
) {
    let steep = (y1 - y0).abs() > (x1 - x0).abs();
    let (mut ax, mut ay, mut bx, mut by) = if steep {
        (y0, x0, y1, x1)
    } else {
        (x0, y0, x1, y1)
    };
    if ax > bx {
        std::mem::swap(&mut ax, &mut bx);
        std::mem::swap(&mut ay, &mut by);
    }
    let dx = bx - ax;
    if dx < 1e-9 {
        return;
    }
    let gradient = (by - ay) / dx;

    // cos(θ) converts vertical pixel distance to perpendicular distance.
    let seg_len = (dx * dx + (by - ay) * (by - ay)).sqrt();
    let cos_theta = dx / seg_len;

    // How many pixels from the centre we need to check on each side.
    // The antialiased fringe extends half_width + 0.5 perpendicular pixels,
    // which maps to (half_width + 0.5) / cos_theta vertical pixels.
    let band = ((half_width + 0.5) / cos_theta).ceil() as i32 + 1;

    let xi_start = ax.ceil() as i32;
    let xi_end = bx.floor() as i32;
    let mut yf = ay + gradient * (ax.ceil() - ax);

    for xi in xi_start..=xi_end {
        let yi_center = yf.floor() as i32;

        for dy in -band..=band {
            let yi = yi_center + dy;
            // Vertical distance from pixel centre to the line centre.
            let vert_dist = ((yi as f64) + 0.5 - yf).abs();
            // Perpendicular distance to the line.
            let perp_dist = vert_dist * cos_theta;
            // Coverage: 1.0 inside the stroke, linear falloff at edges.
            let coverage = half_width + 0.5 - perp_dist;
            if coverage <= 0.0 {
                continue;
            }
            let alpha = (coverage.min(1.0) * 65535.0) as u32;

            let (sx, sy) = if steep { (yi, xi) } else { (xi, yi) };
            if sx >= 0 && sx < w && sy >= 0 && sy < h {
                lines.push(Scanline {
                    y: sy,
                    x1: sx,
                    x2: sx,
                    alpha,
                });
            }
        }

        yf += gradient;
    }
}

/// Fills a closed polygon directly into `worker.lines`, bypassing the
/// tiny-skia pixmap pipeline.
///
/// Uses scanline intersection with 4× sub-pixel vertical antialiasing.
/// Each polygon edge is intersected at 4 sub-rows per pixel row, and the
/// coverage for each pixel is the fraction of sub-rows where the pixel is
/// inside the polygon (non-zero winding rule).
pub(crate) fn fill_polygon_direct(
    lines: &mut Vec<Scanline>,
    vertices: &[(f64, f64)],
    w: i32,
    h: i32,
) {
    lines.clear();

    let n = vertices.len();
    if n < 3 {
        return;
    }

    // Find y range.
    let mut y_min = f64::MAX;
    let mut y_max = f64::MIN;
    for &(_, y) in vertices {
        y_min = y_min.min(y);
        y_max = y_max.max(y);
    }
    let iy_min = (y_min.floor() as i32).max(0);
    let iy_max = (y_max.ceil() as i32).min(h);

    const NUM_AA: usize = 4;

    for iy in iy_min..iy_max {
        // Collect x intersections for each sub-row.
        // For a quadrilateral (n=4), each sub-row produces at most 4 intersections.
        // We pack all sub-row intersections into one array and track sub-row ownership.
        let mut x_hits: [(f64, usize); 64] = [(0.0, 0); 64];
        let mut num_hits = 0;

        for s in 0..NUM_AA {
            let y_sub = iy as f64 + (s as f64 + 0.5) / NUM_AA as f64;

            for i in 0..n {
                let (x0, y0) = vertices[i];
                let (x1, y1) = vertices[(i + 1) % n];
                let (y_lo, y_hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y_sub < y_lo || y_sub >= y_hi {
                    continue;
                }
                let t = (y_sub - y0) / (y1 - y0);
                let x = x0 + t * (x1 - x0);
                if num_hits < 64 {
                    x_hits[num_hits] = (x, s);
                    num_hits += 1;
                }
            }
        }

        if num_hits == 0 {
            continue;
        }

        // Determine the pixel x range touched by any intersection.
        let mut x_min_f = f64::MAX;
        let mut x_max_f = f64::MIN;
        for &(x, _) in &x_hits[..num_hits] {
            x_min_f = x_min_f.min(x);
            x_max_f = x_max_f.max(x);
        }
        let ix_min = (x_min_f.floor() as i32).max(0);
        let ix_max = ((x_max_f.ceil() as i32) - 1).min(w - 1);
        if ix_min > ix_max {
            continue;
        }

        // Sort all intersections within each sub-row.
        // Since we packed them together, sort the full array and process per sub-row.
        x_hits[..num_hits].sort_unstable_by(|a, b| a.1.cmp(&b.1).then(a.0.total_cmp(&b.0)));

        // Build per-sub-row span pairs.
        // For each sub-row, pair consecutive intersections (even-odd).
        let mut spans: [(f64, f64, usize); 32] = [(0.0, 0.0, 0); 32];
        let mut num_spans = 0;
        let mut idx = 0;
        while idx < num_hits {
            let sub = x_hits[idx].1;
            let start = idx;
            while idx < num_hits && x_hits[idx].1 == sub {
                idx += 1;
            }
            let sub_hits = &x_hits[start..idx];
            for pair in sub_hits.chunks(2) {
                if pair.len() == 2 && num_spans < 32 {
                    spans[num_spans] = (pair[0].0, pair[1].0, sub);
                    num_spans += 1;
                }
            }
        }

        // For each pixel column in the range, count how many sub-rows include it.
        // This is the inner hot loop — keep it tight.
        let mut run_start = ix_min;
        let mut run_alpha: u32 = 0;

        for px in ix_min..=ix_max {
            let px_left = px as f64;
            let px_right = px_left + 1.0;
            let mut coverage: u32 = 0;

            for &(sl, sr, _) in &spans[..num_spans] {
                // How much of this pixel is inside this span?
                let overlap_l = sl.max(px_left);
                let overlap_r = sr.min(px_right);
                if overlap_r > overlap_l {
                    let frac = overlap_r - overlap_l; // 0..1
                    coverage += (frac * (65535.0 / NUM_AA as f64)) as u32;
                }
            }
            let alpha = coverage.min(0xFFFF);

            if alpha != run_alpha || px == ix_min {
                // Flush previous run if it had coverage.
                if run_alpha > 0 && px > run_start {
                    lines.push(Scanline {
                        y: iy,
                        x1: run_start,
                        x2: px - 1,
                        alpha: run_alpha,
                    });
                }
                run_start = px;
                run_alpha = alpha;
            }
        }
        // Flush last run.
        if run_alpha > 0 {
            lines.push(Scanline {
                y: iy,
                x1: run_start,
                x2: ix_max,
                alpha: run_alpha,
            });
        }
    }
}

/// Fills a rotated ellipse directly into `lines` using 4x vertical antialiasing.
///
/// The ellipse is centered at `(cx, cy)` with radii `rx` and `ry`, rotated by
/// `angle` radians. Each row is intersected at 4 sub-row sample positions, then
/// the exact horizontal overlap of each sub-row span with each pixel is summed
/// into a 16-bit alpha value.
pub(crate) fn fill_rotated_ellipse_direct(
    lines: &mut Vec<Scanline>,
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    angle: f64,
    w: i32,
    h: i32,
) {
    lines.clear();
    if rx <= 0.0 || ry <= 0.0 || w <= 0 || h <= 0 {
        return;
    }

    const NUM_AA: usize = 4;
    let (sin_t, cos_t) = angle.sin_cos();
    let inv_rx2 = 1.0 / (rx * rx);
    let inv_ry2 = 1.0 / (ry * ry);
    let coeff_a = cos_t * cos_t * inv_rx2 + sin_t * sin_t * inv_ry2;
    let coeff_b = 2.0 * cos_t * sin_t * (inv_rx2 - inv_ry2);
    let coeff_c = sin_t * sin_t * inv_rx2 + cos_t * cos_t * inv_ry2;

    let half_height = ((rx * sin_t).powi(2) + (ry * cos_t).powi(2)).sqrt();
    let iy_min = ((cy - half_height - 1.0).floor() as i32).max(0);
    let iy_max = ((cy + half_height + 1.0).ceil() as i32).min(h - 1);

    for iy in iy_min..=iy_max {
        let mut spans = [None; NUM_AA];
        let mut row_x_min = f64::MAX;
        let mut row_x_max = f64::MIN;

        for (sub, span) in spans.iter_mut().enumerate() {
            let y_sub = iy as f64 + (sub as f64 + 0.5) / NUM_AA as f64;
            let dy = y_sub - cy;
            let quadratic_b = coeff_b * dy;
            let quadratic_c = coeff_c * dy * dy - 1.0;
            let discriminant = quadratic_b * quadratic_b - 4.0 * coeff_a * quadratic_c;
            if discriminant < 0.0 {
                continue;
            }

            let root = discriminant.sqrt();
            let x1 = cx + (-quadratic_b - root) / (2.0 * coeff_a);
            let x2 = cx + (-quadratic_b + root) / (2.0 * coeff_a);
            let left = x1.min(x2);
            let right = x1.max(x2);
            *span = Some((left, right));
            row_x_min = row_x_min.min(left);
            row_x_max = row_x_max.max(right);
        }

        if row_x_min == f64::MAX {
            continue;
        }

        let ix_min = (row_x_min.floor() as i32).max(0);
        let ix_max = ((row_x_max.ceil() as i32) - 1).min(w - 1);
        if ix_min > ix_max {
            continue;
        }

        let mut run_start = ix_min;
        let mut run_alpha = 0;

        for px in ix_min..=ix_max {
            let pixel_left = px as f64;
            let pixel_right = pixel_left + 1.0;
            let mut coverage = 0_u32;

            for &(span_left, span_right) in spans.iter().flatten() {
                let overlap_left = span_left.max(pixel_left);
                let overlap_right = span_right.min(pixel_right);
                if overlap_right > overlap_left {
                    let overlap = overlap_right - overlap_left;
                    coverage += (overlap * (65535.0 / NUM_AA as f64)) as u32;
                }
            }

            let alpha = coverage.min(0xFFFF);
            if px == ix_min {
                run_alpha = alpha;
                continue;
            }
            if alpha != run_alpha {
                if run_alpha > 0 {
                    lines.push(Scanline {
                        y: iy,
                        x1: run_start,
                        x2: px - 1,
                        alpha: run_alpha,
                    });
                }
                run_start = px;
                run_alpha = alpha;
            }
        }

        if run_alpha > 0 {
            lines.push(Scanline {
                y: iy,
                x1: run_start,
                x2: ix_max,
                alpha: run_alpha,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::color::Color;
    use crate::score;
    use crate::shapes::{Ellipse, Shape};
    use crate::worker::WorkerCtx;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn render_mask(lines: &[Scanline], width: u32, height: u32) -> Buffer {
        let mut buffer = Buffer::new(width, height);
        score::draw_lines(&mut buffer, Color::new(255, 255, 255, 255), lines);
        buffer
    }

    #[test]
    fn stroke_quadratic_direct_produces_scanlines() {
        let mut worker = WorkerCtx::new(64, 64, ChaCha8Rng::seed_from_u64(3));
        // A clear, shallow arc across the middle of the image.
        let lines = stroke_quadratic_direct(&mut worker, 5.0, 32.0, 32.0, 10.0, 59.0, 32.0, 0.25);
        assert!(
            !lines.is_empty(),
            "a quadratic bezier across the image should produce scanlines"
        );
    }

    #[test]
    fn stroke_quadratic_direct_stays_in_bounds() {
        let mut worker = WorkerCtx::new(64, 64, ChaCha8Rng::seed_from_u64(4));
        // Control points that extend well outside the image.
        let lines =
            stroke_quadratic_direct(&mut worker, -20.0, -20.0, 32.0, 100.0, 100.0, 100.0, 0.25);
        assert!(
            lines
                .iter()
                .all(|l| l.x1 >= 0 && l.x2 < 64 && l.y >= 0 && l.y < 64),
            "stroke scanlines escaped image bounds: {lines:?}"
        );
    }

    #[test]
    fn stroke_quadratic_direct_single_pixel_spans() {
        // All emitted spans must be single-pixel (x1 == x2) for correct
        // blending in energy_from_lines when alpha varies per pixel.
        let mut worker = WorkerCtx::new(64, 64, ChaCha8Rng::seed_from_u64(5));
        let lines = stroke_quadratic_direct(&mut worker, 10.0, 10.0, 32.0, 5.0, 55.0, 10.0, 0.25);
        assert!(
            lines.iter().all(|l| l.x1 == l.x2),
            "stroke must emit single-pixel spans; got multi-pixel span: {lines:?}"
        );
    }

    #[test]
    fn fill_rotated_ellipse_direct_zero_rotation_matches_ellipse() {
        let mut expected_worker = WorkerCtx::new(64, 64, ChaCha8Rng::seed_from_u64(11));
        let expected = Shape::Ellipse(Ellipse {
            x: 32,
            y: 32,
            rx: 12,
            ry: 8,
        })
        .rasterize(&mut expected_worker)
        .to_vec();

        let mut actual = Vec::new();
        fill_rotated_ellipse_direct(&mut actual, 32.0, 32.0, 12.0, 8.0, 0.0, 64, 64);

        let expected_mask = render_mask(&expected, 64, 64);
        let actual_mask = render_mask(&actual, 64, 64);
        let diff = score::difference_full(&expected_mask, &actual_mask);
        let center = expected_mask.pix_offset(32, 32);

        assert_eq!(
            actual_mask.pixels()[center..center + 4],
            expected_mask.pixels()[center..center + 4]
        );
        assert!(diff < 0.08, "diff={diff}");
    }

    #[test]
    fn fill_rotated_ellipse_direct_90deg_swaps_axes() {
        let mut vertical = Vec::new();
        let mut swapped = Vec::new();

        fill_rotated_ellipse_direct(
            &mut vertical,
            24.0,
            24.0,
            10.0,
            6.0,
            std::f64::consts::FRAC_PI_2,
            48,
            48,
        );
        fill_rotated_ellipse_direct(&mut swapped, 24.0, 24.0, 6.0, 10.0, 0.0, 48, 48);

        let vertical_mask = render_mask(&vertical, 48, 48);
        let swapped_mask = render_mask(&swapped, 48, 48);

        assert_eq!(vertical_mask.pixels(), swapped_mask.pixels());
    }

    #[test]
    fn fill_rotated_ellipse_direct_bounds_checking() {
        let mut lines = Vec::new();
        fill_rotated_ellipse_direct(&mut lines, -4.0, 3.0, 9.0, 5.0, 0.4, 16, 12);

        assert!(!lines.is_empty());
        assert!(lines.iter().all(|line| line.y >= 0 && line.y < 12));
        assert!(lines.iter().all(|line| line.x1 >= 0 && line.x1 <= line.x2));
        assert!(lines.iter().all(|line| line.x2 < 16));
        assert!(lines.iter().all(|line| line.alpha <= 0xFFFF));
    }
}
