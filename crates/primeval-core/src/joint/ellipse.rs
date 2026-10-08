//! The rule of curved layers in the joint optimisation: every radius at
//! least 1 px, the greedy search's own bound, and each kind kept by its
//! parameters: a circle has one radius, an axis-aligned ellipse no angle.
//! The arithmetic rule of [`super`] applies to everything here.
//!
//! A circle has the parameters `cx, cy, r`, an axis-aligned ellipse
//! `cx, cy, rx, ry`, a rotated one `cx, cy, ax, ay, b`, of semi-axes
//! `|a|` and `b` (`diff::Outline`).
//!
//! - [`project`] moves the parameters to the nearest ones that keep the
//!   rule, Euclidean in the parameters: each radius below 1 to 1, and a
//!   rotated ellipse's `a`, if shorter than 1, to length 1 along its
//!   direction, which is the nearest point since for any `a'`,
//!   `|a − a'| ≥ ||a| − |a'||`, with equality along `a`.
//! - [`snap`] rounds the parameters to a lattice and, if a rotated
//!   ellipse's rounded `a` is shorter than 1, lengthens it on the lattice.

use super::angle::Projected;
use super::diff::{COORDS, Outline};

/// The least radius of a circle or an ellipse, as the greedy search's.
const LEAST: f64 = 1.0;

/// Projects the curved layer `p` of `outline` onto the rule ([`project`]
/// in the module documentation). A zero `a` takes the direction of the
/// `x` axis.
pub(super) fn project(outline: Outline, p: &mut [f64; COORDS]) -> Projected {
    let before = *p;
    match outline {
        Outline::Circle => p[2] = p[2].max(LEAST),
        Outline::Ellipse => {
            p[2] = p[2].max(LEAST);
            p[3] = p[3].max(LEAST);
        }
        Outline::RotatedEllipse => {
            let length = (p[2] * p[2] + p[3] * p[3]).sqrt();
            if length < LEAST {
                if length > 0.0 {
                    p[2] /= length;
                    p[3] /= length;
                } else {
                    p[2] = LEAST;
                    p[3] = 0.0;
                }
            }
            p[4] = p[4].max(LEAST);
        }
        outline => panic!("not a curved outline: {outline:?}"),
    }
    if *p == before {
        Projected::Unchanged
    } else {
        Projected::Moved
    }
}

/// `value` rounded to the lattice of `quantum`.
fn round_to(value: f64, quantum: f64) -> f64 {
    (value / quantum).round() * quantum
}

/// The curved layer `p` of `outline` with every parameter rounded to the
/// lattice of `quantum` (`1/4`, `1/2` or `1`, so that 1 is on it), every
/// radius kept at least 1.
///
/// A rotated ellipse's `a`, rounded componentwise, can be shorter than 1
/// where `p`'s is not: it is then rounded away from zero instead, and
/// lengthened on the lattice, its larger component first, while it still
/// is shorter (a zero `a` becomes `(1, 0)`). The checks on the lattice are
/// exact: lattice values are small multiples of a power of two, so their
/// squares and sums are exact in `f64`.
pub(super) fn snap(outline: Outline, p: &[f64; COORDS], quantum: f64) -> [f64; COORDS] {
    let mut out = [0.0; COORDS];
    for k in 0..outline.params() {
        out[k] = round_to(p[k], quantum);
    }
    match outline {
        Outline::Circle => out[2] = out[2].max(LEAST),
        Outline::Ellipse => {
            out[2] = out[2].max(LEAST);
            out[3] = out[3].max(LEAST);
        }
        Outline::RotatedEllipse => {
            let short = |out: &[f64; COORDS]| out[2] * out[2] + out[3] * out[3] < LEAST * LEAST;
            if short(&out) {
                let away = |value: f64| {
                    let steps = (value.abs() / quantum).ceil() * quantum;
                    if value < 0.0 { -steps } else { steps }
                };
                out[2] = away(p[2]);
                out[3] = away(p[3]);
                while short(&out) {
                    let k = if out[2].abs() >= out[3].abs() { 2 } else { 3 };
                    out[k] += if out[k] < 0.0 { -quantum } else { quantum };
                }
            }
            out[4] = out[4].max(LEAST);
        }
        outline => panic!("not a curved outline: {outline:?}"),
    }
    out
}
