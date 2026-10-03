//! The engine's minimum-angle rule for B's triangles (step 9): every angle
//! strictly above [`MIN_DEGREES`], as `Triangle::is_valid`.
//!
//! - [`project`] moves a triangle back into the set whose angles are all at
//!   least `τ ≥ 15°` by a small change of its vertices: Gauss–Newton steps
//!   of minimal norm in the six coordinates on the violated angles,
//!   linearised (one or two of them, the KKT choice), until every angle is
//!   at least `τ`. A triangle already in the set is left as it is.
//! - [`penalty`] is the smooth penalty `w Σ_k max(0, θ₀ − θ_k)²` on angles
//!   below a threshold `θ₀` above 15°, with its gradient.
//! - [`snap`] rounds the vertices to a lattice and, if the rounded triangle
//!   breaks the rule, takes the valid lattice triangle closest to the
//!   continuous one instead.

pub(crate) use crate::search::{MIN_DEGREES, is_valid};

/// Gauss–Newton steps before [`project`] falls back to [`rebuild`].
const MAX_STEPS: usize = 32;
/// How far above `τ` a projection step aims, in radians, so that it ends
/// at or above `τ` despite rounding.
const OVERSHOOT: f64 = 1e-9;

/// The three angles of the triangle, in radians, at vertices 0, 1, 2, and
/// their gradients with respect to the six coordinates. `None` if two
/// vertices coincide.
///
/// With `D` the signed double area and `d_k = u · w` at corner `k` (`u`,
/// `w` the edges from vertex `k` to the next two),
/// `θ_k = atan2(|D|, d_k)`, so the three angles share one `|D|` and are
/// never negative.
pub(crate) fn angle_gradients(v: &[f64; 6]) -> Option<([f64; 3], [[f64; 6]; 3])> {
    let area = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    let c = sign * area;
    // ∂|D|/∂(x_i, y_i) = sign · (y_{i+1} − y_{i+2}, x_{i+2} − x_{i+1}).
    let mut dc = [0.0; 6];
    for i in 0..3 {
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        dc[2 * i] = sign * (v[2 * j + 1] - v[2 * k + 1]);
        dc[2 * i + 1] = sign * (v[2 * k] - v[2 * j]);
    }
    let mut theta = [0.0; 3];
    let mut grad = [[0.0; 6]; 3];
    for p in 0..3 {
        let (q, r) = ((p + 1) % 3, (p + 2) % 3);
        let (ux, uy) = (v[2 * q] - v[2 * p], v[2 * q + 1] - v[2 * p + 1]);
        let (wx, wy) = (v[2 * r] - v[2 * p], v[2 * r + 1] - v[2 * p + 1]);
        let d = ux * wx + uy * wy;
        let norm = c * c + d * d;
        if (ux == 0.0 && uy == 0.0) || (wx == 0.0 && wy == 0.0) || norm == 0.0 {
            return None;
        }
        theta[p] = c.atan2(d);
        // ∂d/∂q = w, ∂d/∂r = u, ∂d/∂p = −(u + w).
        let mut dd = [0.0; 6];
        dd[2 * q] = wx;
        dd[2 * q + 1] = wy;
        dd[2 * r] = ux;
        dd[2 * r + 1] = uy;
        dd[2 * p] = -(ux + wx);
        dd[2 * p + 1] = -(uy + wy);
        for i in 0..6 {
            grad[p][i] = (d * dc[i] - c * dd[i]) / norm;
        }
    }
    Some((theta, grad))
}

/// The smallest angle of the triangle in radians, 0 if two vertices
/// coincide.
pub(crate) fn min_angle(v: &[f64; 6]) -> f64 {
    angle_gradients(v).map_or(0.0, |(theta, _)| theta[0].min(theta[1]).min(theta[2]))
}

/// What [`project`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Projected {
    /// Every angle was already at least `τ`; the triangle is unchanged.
    Unchanged,
    /// Gauss–Newton steps moved it into the set.
    Moved,
    /// It was degenerate, or the steps did not converge: [`rebuild`].
    Rebuilt,
}

fn dot(a: &[f64; 6], b: &[f64; 6]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

/// Moves `v` into the set whose angles are all at least `min_degrees`
/// (at least [`MIN_DEGREES`]), leaving it unchanged if it is there.
///
/// Each step solves, on the angles linearised at `v`, for the change of
/// least Euclidean norm in the six coordinates that lifts the violated
/// angles to `τ`: along the gradient of the smallest angle alone if that
/// keeps the second smallest at or above `τ` to first order, otherwise on
/// both (two at most can be below `τ ≤ 60°`). Every vertex moves, in
/// proportion to its leverage on the angle: a needle opens by spreading
/// its short edge, a cap by lifting its flat vertex.
pub(crate) fn project(v: &mut [f64; 6], min_degrees: f64) -> Projected {
    debug_assert!((MIN_DEGREES..60.0).contains(&min_degrees));
    let tau = min_degrees.to_radians();
    let start = *v;
    let mut moved = false;
    for _ in 0..MAX_STEPS {
        let Some((theta, grad)) = angle_gradients(v) else {
            break;
        };
        let mut order = [0, 1, 2];
        order.sort_by(|&a, &b| theta[a].total_cmp(&theta[b]));
        let [first, second, _] = order;
        if theta[first] >= tau {
            return if moved {
                Projected::Moved
            } else {
                Projected::Unchanged
            };
        }
        let target = tau + OVERSHOOT;
        let (g0, g1) = (&grad[first], &grad[second]);
        let (n00, n01, n11) = (dot(g0, g0), dot(g0, g1), dot(g1, g1));
        let (r0, r1) = (target - theta[first], target - theta[second]);
        let single = r0 / n00;
        let mut step: [f64; 6] = std::array::from_fn(|i| single * g0[i]);
        if theta[second] + dot(g1, &step) < target {
            let det = n00 * n11 - n01 * n01;
            if det > 1e-12 * n00 * n11 {
                let l0 = (r0 * n11 - r1 * n01) / det;
                let l1 = (r1 * n00 - r0 * n01) / det;
                step = std::array::from_fn(|i| l0 * g0[i] + l1 * g1[i]);
            }
        }
        if !step.iter().all(|s| s.is_finite()) {
            break;
        }
        for (value, change) in v.iter_mut().zip(step) {
            *value += change;
        }
        moved = true;
    }
    if min_angle(v) >= tau {
        return Projected::Moved;
    }
    *v = start;
    rebuild(v, tau);
    Projected::Rebuilt
}

/// The fallback of [`project`] for degenerate triangles: keeps the longest
/// edge and puts the third vertex over its midpoint, on its side, at base
/// angles `τ + 1°`; three coincident vertices become a triangle of side
/// 1 px around them.
fn rebuild(v: &mut [f64; 6], tau: f64) {
    let length = |p: usize, q: usize| (v[2 * q] - v[2 * p]).hypot(v[2 * q + 1] - v[2 * p + 1]);
    let (p, q, r) = [(0, 1, 2), (1, 2, 0), (2, 0, 1)]
        .into_iter()
        .max_by(|a, b| length(a.0, a.1).total_cmp(&length(b.0, b.1)))
        .expect("three edges");
    let base = length(p, q);
    if base < 1e-6 {
        let (cx, cy) = ((v[0] + v[2] + v[4]) / 3.0, (v[1] + v[3] + v[5]) / 3.0);
        let h = 3.0_f64.sqrt() / 2.0;
        *v = [
            cx - 0.5,
            cy - h / 3.0,
            cx + 0.5,
            cy - h / 3.0,
            cx,
            cy + 2.0 * h / 3.0,
        ];
        return;
    }
    let (ex, ey) = (
        (v[2 * q] - v[2 * p]) / base,
        (v[2 * q + 1] - v[2 * p + 1]) / base,
    );
    let side = ex * (v[2 * r + 1] - v[2 * p + 1]) - ey * (v[2 * r] - v[2 * p]);
    let sign = if side < 0.0 { -1.0 } else { 1.0 };
    let height = base / 2.0 * (tau + 1.0_f64.to_radians()).tan();
    let (mx, my) = (
        (v[2 * p] + v[2 * q]) / 2.0,
        (v[2 * p + 1] + v[2 * q + 1]) / 2.0,
    );
    v[2 * r] = mx - sign * ey * height;
    v[2 * r + 1] = my + sign * ex * height;
}

/// `w Σ_k max(0, θ₀ − θ_k)²` with `θ₀ = threshold_degrees` in radians, and
/// its gradient. Zero for a degenerate triangle, which [`project`] handles.
pub(crate) fn penalty(v: &[f64; 6], threshold_degrees: f64, weight: f64) -> (f64, [f64; 6]) {
    let threshold = threshold_degrees.to_radians();
    let mut value = 0.0;
    let mut gradient = [0.0; 6];
    if let Some((theta, grad)) = angle_gradients(v) {
        for (angle, grad) in theta.iter().zip(&grad) {
            let gap = threshold - angle;
            if gap > 0.0 {
                value += weight * gap * gap;
                for (sum, d) in gradient.iter_mut().zip(grad) {
                    *sum -= 2.0 * weight * gap * d;
                }
            }
        }
    }
    (value, gradient)
}

/// `v` with every coordinate rounded to a multiple of `quantum`; if that
/// breaks the engine's rule, the valid lattice triangle whose vertices are
/// closest to `v`'s (least sum of squared displacements, each coordinate
/// within 2, then 4 quanta of its rounding). Returns it and whether the
/// rounding was replaced. Panics if no valid triangle is that close, which
/// a triangle with every angle above 15° never meets (see the tests).
pub(crate) fn snap(v: &[f64; 6], quantum: f64) -> ([f64; 6], bool) {
    let rounded = v.map(|value| (value / quantum).round() * quantum);
    if is_valid(&rounded) {
        return (rounded, false);
    }
    for radius in [2_i32, 4] {
        let candidates: Vec<Vec<(f64, f64, f64)>> = (0..3)
            .map(|vertex| {
                let (x, y) = (v[2 * vertex], v[2 * vertex + 1]);
                let (rx, ry) = (rounded[2 * vertex], rounded[2 * vertex + 1]);
                let mut list: Vec<(f64, f64, f64)> = (-radius..=radius)
                    .flat_map(|j| (-radius..=radius).map(move |i| (i, j)))
                    .map(|(i, j)| {
                        let (cx, cy) = (rx + f64::from(i) * quantum, ry + f64::from(j) * quantum);
                        ((cx - x).powi(2) + (cy - y).powi(2), cx, cy)
                    })
                    .collect();
                // Stable: ties keep the generation order.
                list.sort_by(|a, b| a.0.total_cmp(&b.0));
                list
            })
            .collect();
        let mut best: Option<(f64, [f64; 6])> = None;
        let bound = |best: &Option<(f64, [f64; 6])>| best.map_or(f64::INFINITY, |b| b.0);
        for a in &candidates[0] {
            if a.0 >= bound(&best) {
                break;
            }
            for b in &candidates[1] {
                if a.0 + b.0 >= bound(&best) {
                    break;
                }
                for c in &candidates[2] {
                    let cost = a.0 + b.0 + c.0;
                    if cost >= bound(&best) {
                        break;
                    }
                    let tri = [a.1, a.2, b.1, b.2, c.1, c.2];
                    if is_valid(&tri) {
                        best = Some((cost, tri));
                        break;
                    }
                }
            }
        }
        if let Some((_, tri)) = best {
            return (tri, true);
        }
    }
    panic!("no valid lattice triangle near {v:?}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// Random triangles of every shape: generic ones, needles (one small
    /// angle) and caps (one angle near 180°), down to 0.01°, at sizes from
    /// 0.5 to 60 px.
    fn random_triangles(seed: u64, count: usize) -> Vec<[f64; 6]> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        (0..count)
            .map(|index| {
                let size = 10.0_f64.powf(rng.random_range(-0.3..1.8));
                let (cx, cy) = (rng.random_range(-20.0..80.0), rng.random_range(-20.0..80.0));
                let turn = rng.random_range(0.0..std::f64::consts::TAU);
                let local: [(f64, f64); 3] = match index % 3 {
                    0 => std::array::from_fn(|_| {
                        (rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0))
                    }),
                    1 => {
                        // A needle: apex at the origin, a small angle.
                        let angle = 10.0_f64.powf(rng.random_range(-2.0..1.4)).to_radians();
                        let (l1, l2) = (rng.random_range(0.3..1.0), rng.random_range(0.3..1.0));
                        [(0.0, 0.0), (l1, 0.0), (l2 * angle.cos(), l2 * angle.sin())]
                    }
                    _ => {
                        // A cap: the third vertex close to the base.
                        let along = rng.random_range(-0.4..0.4);
                        let lift = 10.0_f64.powf(rng.random_range(-3.0..-0.5));
                        [(-0.5, 0.0), (0.5, 0.0), (along, lift)]
                    }
                };
                let mut v = [0.0; 6];
                for (k, (x, y)) in local.iter().enumerate() {
                    v[2 * k] = cx + size * (x * turn.cos() - y * turn.sin());
                    v[2 * k + 1] = cy + size * (x * turn.sin() + y * turn.cos());
                }
                v
            })
            .collect()
    }

    /// Central differences against `f`'s analytic gradient; returns the
    /// largest error relative to the gradient's norm.
    fn worst_relative_error(
        v: &[f64; 6],
        f: impl Fn(&[f64; 6]) -> f64,
        analytic: &[f64; 6],
    ) -> f64 {
        let h = 1e-6 * v.iter().fold(1.0_f64, |m, x| m.max(x.abs()));
        let scale = dot(analytic, analytic).sqrt();
        (0..6)
            .map(|i| {
                let mut plus = *v;
                let mut minus = *v;
                plus[i] += h;
                minus[i] -= h;
                let numeric = (f(&plus) - f(&minus)) / (2.0 * h);
                (numeric - analytic[i]).abs() / scale
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn angle_gradients_match_finite_differences() {
        let mut checked = 0;
        for v in random_triangles(1, 600) {
            let (theta, grad) = angle_gradients(&v).expect("not degenerate");
            assert!((theta.iter().sum::<f64>() - std::f64::consts::PI).abs() < 1e-9);
            // Skip triangles too flat for a finite difference to resolve.
            if theta.iter().any(|&t| t < 1e-3) {
                continue;
            }
            for k in 0..3 {
                let f = |w: &[f64; 6]| angle_gradients(w).expect("not degenerate").0[k];
                let error = worst_relative_error(&v, f, &grad[k]);
                assert!(error < 1e-5, "angle {k} of {v:?}: {error}");
            }
            checked += 1;
        }
        assert!(checked > 400, "{checked}");
        assert!(angle_gradients(&[1.0, 2.0, 1.0, 2.0, 5.0, 0.0]).is_none());
    }

    #[test]
    fn penalty_gradient_matches_finite_differences() {
        let mut active = 0;
        for v in random_triangles(2, 600) {
            if min_angle(&v) < 1e-3 {
                continue;
            }
            let (value, gradient) = penalty(&v, 20.0, 1e6);
            if value == 0.0 {
                assert_eq!(gradient, [0.0; 6]);
                continue;
            }
            active += 1;
            let f = |w: &[f64; 6]| penalty(w, 20.0, 1e6).0;
            let error = worst_relative_error(&v, f, &gradient);
            assert!(error < 1e-5, "{v:?}: {error}");
        }
        assert!(active > 300, "{active}");
    }

    #[test]
    fn projection_keeps_valid_triangles_and_makes_invalid_ones_valid() {
        let (mut unchanged, mut moved, mut rebuilt) = (0, 0, 0);
        for tau in [15.0 + 1e-6, 15.5, 16.0] {
            let tau_rad = f64::to_radians(tau);
            for v in random_triangles(3, 3000) {
                let before = min_angle(&v);
                let was_valid = is_valid(&v);
                let mut projected = v;
                match project(&mut projected, tau) {
                    Projected::Unchanged => {
                        assert!(before >= tau_rad);
                        assert_eq!(projected, v);
                        unchanged += 1;
                    }
                    Projected::Moved => moved += 1,
                    Projected::Rebuilt => rebuilt += 1,
                }
                let after = min_angle(&projected);
                assert!(
                    after >= tau_rad,
                    "{v:?} -> {projected:?}: {}",
                    after.to_degrees()
                );
                assert!(after >= before.min(tau_rad), "{v:?}");
                assert!(is_valid(&projected), "{v:?} -> {projected:?}");
                if was_valid && before >= tau_rad {
                    assert_eq!(projected, v);
                }
            }
        }
        eprintln!("projection: {unchanged} unchanged, {moved} moved, {rebuilt} rebuilt");
        assert!(unchanged > 1000 && moved > 3000, "{unchanged} {moved}");
        // Coincident vertices are rebuilt; collinear distinct ones are
        // lifted by the steps. Both end valid.
        for (mut v, expected) in [
            ([3.0, 3.0, 3.0, 3.0, 3.0, 3.0], Projected::Rebuilt),
            ([0.0, 0.0, 10.0, 0.0, 10.0, 0.0], Projected::Rebuilt),
            ([0.0, 0.0, 10.0, 0.0, 4.0, 0.0], Projected::Moved),
        ] {
            assert_eq!(project(&mut v, 15.5), expected);
            assert!(
                is_valid(&v) && min_angle(&v) >= f64::to_radians(15.5),
                "{v:?}"
            );
        }
    }

    /// The projection moves the six coordinates no further than the best
    /// move of a single vertex, found by brute force: a stand-in for
    /// minimal displacement.
    #[test]
    fn projection_moves_no_more_than_the_best_single_vertex_move() {
        let tau = 15.5;
        let tau_rad = f64::to_radians(tau);
        let mut compared = 0;
        for v in random_triangles(4, 300) {
            if min_angle(&v) >= tau_rad || min_angle(&v) < 1e-6 {
                continue;
            }
            let mut projected = v;
            if project(&mut projected, tau) != Projected::Moved {
                continue;
            }
            let moved = v
                .iter()
                .zip(&projected)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            // The shortest distance one vertex must move, over 720
            // directions, by bisection on the feasible distance.
            let scale = (0..3)
                .map(|k| (v[2 * k] - v[0]).hypot(v[2 * k + 1] - v[1]))
                .fold(0.0, f64::max)
                * 4.0;
            let mut single = f64::INFINITY;
            for vertex in 0..3 {
                for step in 0..720 {
                    let angle = f64::from(step) * std::f64::consts::TAU / 720.0;
                    let at = |t: f64| {
                        let mut w = v;
                        w[2 * vertex] += t * angle.cos();
                        w[2 * vertex + 1] += t * angle.sin();
                        min_angle(&w) >= tau_rad
                    };
                    if !at(scale) {
                        continue;
                    }
                    let (mut low, mut high) = (0.0, scale);
                    for _ in 0..60 {
                        let middle = (low + high) / 2.0;
                        if at(middle) {
                            high = middle;
                        } else {
                            low = middle;
                        }
                    }
                    single = single.min(high);
                }
            }
            assert!(moved <= single * 1.001, "{v:?}: {moved} against {single}");
            compared += 1;
        }
        assert!(compared > 100, "{compared}");
    }

    #[test]
    fn snapped_triangles_keep_the_rule() {
        let mut repaired = 0;
        for quantum in [0.25, 1.0] {
            for (index, mut v) in random_triangles(5, 6000).into_iter().enumerate() {
                // Every size down to well below one quantum.
                if index % 2 == 0 {
                    let (cx, cy) = (v[0], v[1]);
                    for k in 0..3 {
                        v[2 * k] = cx + (v[2 * k] - cx) * 0.05;
                        v[2 * k + 1] = cy + (v[2 * k + 1] - cy) * 0.05;
                    }
                }
                project(&mut v, 15.5);
                let (snapped, replaced) = snap(&v, quantum);
                assert!(is_valid(&snapped), "{v:?} -> {snapped:?}");
                for (s, c) in snapped.iter().zip(&v) {
                    assert_eq!((s / quantum).fract(), 0.0);
                    assert!((s - c).abs() <= 4.5 * quantum, "{v:?} -> {snapped:?}");
                }
                let rounded = v.map(|value| (value / quantum).round() * quantum);
                if replaced {
                    assert!(!is_valid(&rounded));
                    repaired += 1;
                } else {
                    assert_eq!(snapped, rounded);
                }
            }
        }
        assert!(repaired > 100, "{repaired}");
    }
}
