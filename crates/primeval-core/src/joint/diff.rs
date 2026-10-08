//! Differentiable compositing of a drawing's layers: the smooth forward
//! model, reverse-mode gradients through the layer stack and the colour
//! fit. The arithmetic rule of [`super`] applies to everything here.
//!
//! A layer is a triangle, a convex quadrilateral or a rotated rectangle,
//! covered by its exact area in the pixel's filter square: the one edge's
//! box-filtered half-plane where at most one edge cuts the square, and the
//! square clipped to the inside of its cutting edges where two or more do,
//! near a vertex; an axis-aligned rectangle, covered by the product of its
//! four sides' box-filtered half-planes, which for an axis-aligned box is
//! its exact pixel area; or a fixed layer, covered by a [`Mask`] that does
//! not move.
//!
//! Coordinates are the engine's: the centre of pixel `(i, j)` is `(i, j)`,
//! so a vertex `v` is the drawing's `v + 0.5`.

use crate::refine::checkpoint_interval;
use crate::scanline::{Scanline, clamp_line};
use rayon::prelude::*;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Range, Sub};

/// The float the canvas is composited in: `f32` in production, `f64` for
/// the finite-difference checks.
pub(super) trait Real:
    Copy
    + Default
    + Send
    + Sync
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + AddAssign
{
    fn of(value: f64) -> Self;
    fn get(self) -> f64;
}

impl Real for f32 {
    #[inline]
    fn of(value: f64) -> Self {
        value as f32
    }
    #[inline]
    fn get(self) -> f64 {
        f64::from(self)
    }
}

impl Real for f64 {
    #[inline]
    fn of(value: f64) -> Self {
        value
    }
    #[inline]
    fn get(self) -> f64 {
        self
    }
}

/// The over-relaxation `ω` of [`fit`]'s step, in `(0, 2)`, where the step
/// cannot raise the loss. Chosen against `1.0`, `1.3`, `1.7` and `1.9` on
/// the engine runner's corpus.
pub(super) const OVER_RELAXATION: f64 = 1.5;

/// Geometric parameters per layer, at most: a quadrilateral's vertex
/// coordinates `x0, y0, …, x3, y3`. The other outlines use the first
/// [`Outline::params`].
pub(super) const COORDS: usize = 8;
/// The gradient entry of the opacity, after the geometric parameters.
pub(super) const ALPHA: usize = COORDS;
/// Gradient entries per layer: the geometric parameters, then the opacity.
pub(super) const PARAMS: usize = COORDS + 1;

/// What covers a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outline {
    /// A triangle through three vertices, parameters `x0, y0, x1, y1, x2,
    /// y2`.
    Triangle,
    /// A convex quadrilateral through four vertices, parameters `x0, y0,
    /// …, x3, y3`.
    Quad,
    /// An axis-aligned rectangle `x0 ≤ x ≤ x1`, `y0 ≤ y ≤ y1`, parameters
    /// `x0, y0, x1, y1`.
    Rect,
    /// A rectangle of centre `c`, half-side vector `u` and half-width `h`
    /// along `u`'s perpendicular, parameters `cx, cy, ux, uy, h`: its
    /// corners are `c ± u ± (h / |u|) · (−u_y, u_x)` ([`Layer::corners`]).
    Rotated,
    /// A fixed shape: the [`Scene::masks`] entry of this index, which does
    /// not move; only its opacity and colour are optimised.
    Fixed(usize),
}

impl Outline {
    /// The number of edges, `0` for a fixed layer.
    pub(super) fn sides(self) -> usize {
        match self {
            Self::Triangle => 3,
            Self::Quad | Self::Rect | Self::Rotated => 4,
            Self::Fixed(_) => 0,
        }
    }

    /// The number of geometric parameters, `0` for a fixed layer.
    pub(super) fn params(self) -> usize {
        match self {
            Self::Triangle => 6,
            Self::Quad => 8,
            Self::Rect => 4,
            Self::Rotated => 5,
            Self::Fixed(_) => 0,
        }
    }
}

/// One layer's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layer {
    pub(super) outline: Outline,
    /// The geometric parameters of the outline ([`Outline`]), in engine
    /// coordinates; the first [`Outline::params`] are used, the others are
    /// zero.
    pub(super) params: [f64; COORDS],
    /// Opacity, `1..=255`.
    pub(super) alpha: f64,
    /// RGB, `0..=255`.
    pub(super) color: [f64; 3],
}

impl Layer {
    /// A triangle through the six coordinates `v`.
    pub(super) fn triangle(v: [f64; 6], alpha: f64, color: [f64; 3]) -> Self {
        Self {
            outline: Outline::Triangle,
            params: [v[0], v[1], v[2], v[3], v[4], v[5], 0.0, 0.0],
            alpha,
            color,
        }
    }

    /// The vertex coordinates of the outline, `x0, y0, …`, of which the
    /// first `2 · outline.sides()` are used: a triangle's or a
    /// quadrilateral's parameters; a rectangle's corners `(x0, y0)`,
    /// `(x1, y0)`, `(x1, y1)`, `(x0, y1)`; a rotated rectangle's
    /// `c − u − n`, `c + u − n`, `c + u + n` and `c − u + n`, with
    /// `n = (h / |u|) · (−u_y, u_x)`, by `sqrt` alone. All zero for a fixed
    /// layer.
    pub(super) fn corners(&self) -> [f64; COORDS] {
        let p = self.params;
        match self.outline {
            Outline::Triangle | Outline::Quad | Outline::Fixed(_) => p,
            Outline::Rect => [p[0], p[1], p[2], p[1], p[2], p[3], p[0], p[3]],
            Outline::Rotated => {
                let [cx, cy, ux, uy, h, ..] = p;
                let k = h / (ux * ux + uy * uy).sqrt().max(1e-9);
                let (nx, ny) = (-uy * k, ux * k);
                [
                    cx - ux - nx,
                    cy - uy - ny,
                    cx + ux - nx,
                    cy + uy - ny,
                    cx + ux + nx,
                    cy + uy + ny,
                    cx - ux + nx,
                    cy - uy + ny,
                ]
            }
        }
    }

    /// The gradient with respect to the parameters, from `grad`, with
    /// respect to the [`Self::corners`] (and the opacity, which passes
    /// through). The same for every outline but a rotated rectangle's,
    /// whose corners depend on `c`, `u` and `h` through `n`:
    /// `∂n/∂h = (−u_y, u_x) / |u|` and
    /// `∂n/∂u = (h / |u|³) · [[u_x u_y, −u_x²], [u_y², −u_x u_y]]`
    /// (rows `n_x`, `n_y`; columns `u_x`, `u_y`).
    fn chain(&self, grad: [f64; PARAMS]) -> [f64; PARAMS] {
        if self.outline != Outline::Rotated {
            return grad;
        }
        let [_, _, ux, uy, h, ..] = self.params;
        let r = (ux * ux + uy * uy).sqrt().max(1e-9);
        let r3 = r * r * r;
        // The corners' signs on `u` and on `n`.
        let (on_u, on_n) = ([-1.0, 1.0, 1.0, -1.0], [-1.0, -1.0, 1.0, 1.0]);
        let (mut centre, mut along_u, mut along_n) = ([0.0; 2], [0.0; 2], [0.0; 2]);
        for corner in 0..4 {
            for axis in 0..2 {
                let g = grad[2 * corner + axis];
                centre[axis] += g;
                along_u[axis] += on_u[corner] * g;
                along_n[axis] += on_n[corner] * g;
            }
        }
        let mut out = [0.0; PARAMS];
        out[0] = centre[0];
        out[1] = centre[1];
        out[2] = along_u[0] + (along_n[0] * ux * uy + along_n[1] * uy * uy) * h / r3;
        out[3] = along_u[1] - (along_n[0] * ux * ux + along_n[1] * ux * uy) * h / r3;
        out[4] = (-along_n[0] * uy + along_n[1] * ux) / r;
        out[ALPHA] = grad[ALPHA];
        out
    }
}

/// The coverage of a fixed layer: one value in `0..=1` per pixel of a box
/// of the canvas, row-major.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Mask<F> {
    pub(super) x0: usize,
    pub(super) x1: usize,
    pub(super) y0: usize,
    pub(super) y1: usize,
    pub(super) coverage: Vec<F>,
}

impl<F: Real> Mask<F> {
    /// The coverage of the engine's scanlines `lines` on a canvas
    /// `width × height`: each pixel of a line at the line's coverage, as
    /// the engine's own compositing weighs it (`alpha / 0xFFFF`), every
    /// other pixel at zero. The box is the lines' bounding box on the
    /// canvas, empty without lines.
    pub(super) fn from_lines(lines: &[Scanline], width: usize, height: usize) -> Self {
        let spans: Vec<(usize, usize, usize, u32)> = lines
            .iter()
            .filter_map(|line| {
                let (x1, x2) = clamp_line(line, width as i32, height as i32)?;
                Some((line.y as usize, x1 as usize, x2 as usize + 1, line.alpha))
            })
            .collect();
        if spans.is_empty() {
            return Self::default();
        }
        let x0 = spans.iter().map(|span| span.1).min().unwrap_or(0);
        let x1 = spans.iter().map(|span| span.2).max().unwrap_or(0);
        let y0 = spans.iter().map(|span| span.0).min().unwrap_or(0);
        let y1 = spans.iter().map(|span| span.0).max().unwrap_or(0) + 1;
        let box_width = x1 - x0;
        let mut coverage = vec![F::of(0.0); box_width * (y1 - y0)];
        for (y, start, end, alpha) in spans {
            let value = F::of(f64::from(alpha) / 65535.0);
            let row = (y - y0) * box_width;
            for cell in &mut coverage[row + start - x0..row + end - x0] {
                *cell = value;
            }
        }
        Self {
            x0,
            x1,
            y0,
            y1,
            coverage,
        }
    }
}

/// The target and the background a stack of layers is composited on, and
/// the masks of its fixed layers.
pub(super) struct Scene<F> {
    pub(super) width: usize,
    pub(super) height: usize,
    /// RGB per pixel, row-major.
    pub(super) target: Vec<F>,
    pub(super) background: [F; 3],
    /// Width in pixels of the box filter the coverage is convolved with:
    /// `1` is exact pixel-area coverage, and the only width production
    /// uses.
    pub(super) filter: f64,
    /// The coverage of each fixed layer ([`Outline::Fixed`]).
    pub(super) masks: Vec<Mask<F>>,
}

/// One edge's box-filtered half-plane, `F(d)` with `d` the signed distance
/// of a pixel centre from the edge, positive inside.
#[derive(Clone, Copy, Default)]
struct Edge<F> {
    /// `d(x, y) = nx · x + ny · y + c`.
    nx: F,
    ny: F,
    c: F,
    /// The filter square projected on the edge normal is the sum of two
    /// uniforms of widths `wide ≥ narrow`; `F` is the CDF of that sum,
    /// held through `(wide ± narrow) / 2` and the inverses below.
    half_sum: F,
    half_diff: F,
    inv_wide: F,
    /// `1 / (2 · wide · narrow)`, or `0` when `narrow` is `0` (the
    /// quadratic pieces are then empty).
    inv_2ab: F,
    inv_narrow: F,
    /// For the derivatives: the start `P`, the vector `e = Q − P`, `1 / |e|`,
    /// the orientation sign, and `∂wide/∂e`, `∂narrow/∂e`.
    px: F,
    py: F,
    ex: F,
    ey: F,
    inv_l: F,
    sigma: F,
    dwide: [F; 2],
    dnarrow: [F; 2],
}

/// `F(d)` and its partials `∂F/∂d`, `∂F/∂wide`, `∂F/∂narrow`.
#[derive(Clone, Copy)]
struct EdgeValue<F> {
    f: F,
    fd: F,
    fa: F,
    fb: F,
}

/// How a [`Prepared`] layer covers its pixels.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A fixed layer's mask.
    Masked,
    /// Three edges.
    Triangle,
    /// Four edges: a quadrilateral or a rotated rectangle.
    Quad,
    /// An axis-aligned rectangle's four sides.
    Box,
}

impl Kind {
    /// The number of gradient entries its coverage has: the vertex
    /// coordinates of the edges, or a rectangle's four parameters.
    fn coords(self) -> usize {
        match self {
            Self::Masked => 0,
            Self::Triangle => 6,
            Self::Quad => 8,
            Self::Box => 4,
        }
    }
}

/// A layer ready to composite: the edges of a triangle or a quadrilateral
/// (the first three or four), the sides of an axis-aligned rectangle or a
/// fixed layer's mask, its bounding box on the canvas (`x0..x1`, `y0..y1`,
/// empty if it cannot cover any pixel), its opacity as a fraction and its
/// colour.
#[derive(Clone, Copy)]
pub(super) struct Prepared<'a, F> {
    kind: Kind,
    edges: [Edge<F>; 4],
    /// An axis-aligned rectangle's `x0, y0, x1, y1`, then `1 / filter`;
    /// zero for the others.
    rect: [F; 5],
    /// A fixed layer's coverage over the bounding box, row-major; empty
    /// for the others.
    mask: &'a [F],
    /// Half the filter width and `1 / filter²`, for the exact clip of
    /// [`Self::clip`]; zero for an axis-aligned rectangle and a fixed
    /// layer.
    half: F,
    inv_area: F,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    opacity: F,
    color: [F; 3],
}

/// The most vertices a pixel square clipped by a layer's edges can have:
/// the square's four, and one more per edge, at most four.
const CLIP: usize = 8;

/// No edge of the layer: the label of a side of the pixel square.
const SQUARE: u8 = u8::MAX;

/// The filter square of a pixel clipped to the inside of a layer's
/// cutting edges ([`Prepared::clip`]): a convex polygon, its vertices
/// relative to the pixel centre, and for each vertex `i` the edge of the
/// layer the side from vertex `i` to vertex `i + 1` lies on, or
/// [`SQUARE`].
struct Clipped<F> {
    u: [F; CLIP],
    v: [F; CLIP],
    on: [u8; CLIP],
    len: usize,
}

impl<F: Real> Edge<F> {
    #[inline]
    fn distance(&self, x: F, y: F) -> F {
        self.nx * x + self.ny * y + self.c
    }

    /// `F(d)`: piecewise quadratic, `C¹`.
    #[inline]
    fn cdf(&self, d: F) -> F {
        let (h, k) = (self.half_sum, self.half_diff);
        if d <= -h {
            F::of(0.0)
        } else if d >= h {
            F::of(1.0)
        } else if d < -k {
            let u = d + h;
            u * u * self.inv_2ab
        } else if d <= k {
            F::of(0.5) + d * self.inv_wide
        } else {
            let v = h - d;
            F::of(1.0) - v * v * self.inv_2ab
        }
    }

    #[inline]
    fn cdf_grad(&self, d: F) -> EdgeValue<F> {
        let (h, k) = (self.half_sum, self.half_diff);
        let zero = F::of(0.0);
        if d <= -h {
            EdgeValue {
                f: zero,
                fd: zero,
                fa: zero,
                fb: zero,
            }
        } else if d >= h {
            EdgeValue {
                f: F::of(1.0),
                fd: zero,
                fa: zero,
                fb: zero,
            }
        } else if d < -k {
            let u = d + h;
            let f = u * u * self.inv_2ab;
            let half = u * self.inv_2ab;
            EdgeValue {
                f,
                fd: half + half,
                fa: half - f * self.inv_wide,
                fb: half - f * self.inv_narrow,
            }
        } else if d <= k {
            EdgeValue {
                f: F::of(0.5) + d * self.inv_wide,
                fd: self.inv_wide,
                fa: -d * self.inv_wide * self.inv_wide,
                fb: zero,
            }
        } else {
            let v = h - d;
            let rest = v * v * self.inv_2ab;
            let half = v * self.inv_2ab;
            EdgeValue {
                f: F::of(1.0) - rest,
                fd: half + half,
                fa: rest * self.inv_wide - half,
                fb: rest * self.inv_narrow - half,
            }
        }
    }

    /// `∂F/∂(Px, Py, Qx, Qy)` at the pixel `(x, y)` with distance `d`.
    #[inline]
    fn endpoint_grad(&self, value: EdgeValue<F>, x: F, y: F, d: F) -> [F; 4] {
        let inv_l2 = self.inv_l * self.inv_l;
        let dd_dex = self.sigma * (y - self.py) * self.inv_l - d * self.ex * inv_l2;
        let dd_dey = -self.sigma * (x - self.px) * self.inv_l - d * self.ey * inv_l2;
        let df_dex = value.fd * dd_dex + value.fa * self.dwide[0] + value.fb * self.dnarrow[0];
        let df_dey = value.fd * dd_dey + value.fa * self.dwide[1] + value.fb * self.dnarrow[1];
        let direct_x = value.fd * self.sigma * self.ey * self.inv_l;
        let direct_y = -value.fd * self.sigma * self.ex * self.inv_l;
        [direct_x - df_dex, direct_y - df_dey, df_dex, df_dey]
    }
}

/// The pixels `start..end` of a canvas axis of `size` pixels whose
/// centres an axis-aligned rectangle's sides at `low` and `high` cover with
/// a box filter of width `filter`, or `None` if there are none or the
/// rectangle is empty.
fn box_span(low: f64, high: f64, filter: f64, size: usize) -> Option<(usize, usize)> {
    if low >= high {
        return None;
    }
    let start = (low - filter / 2.0).ceil().max(0.0);
    let end = ((high + filter / 2.0).floor() + 1.0).min(size as f64);
    (start < end).then_some((start as usize, end as usize))
}

/// `1` for a non-negative `value`, `−1` for a negative one: the sign the
/// derivative of `|value|` takes, `+1` at zero.
#[inline]
fn sign(value: f64) -> f64 {
    if value < 0.0 { -1.0 } else { 1.0 }
}

impl<'a, F: Real> Prepared<'a, F> {
    pub(super) fn new(layer: &Layer, scene: &'a Scene<F>) -> Self {
        let opacity = F::of(layer.alpha / 255.0);
        let color = layer.color.map(F::of);
        let sides = layer.outline.sides();
        let kind = match layer.outline {
            Outline::Triangle => Kind::Triangle,
            Outline::Quad | Outline::Rotated => Kind::Quad,
            Outline::Rect => Kind::Box,
            Outline::Fixed(index) => {
                let mask = &scene.masks[index];
                return Self {
                    kind: Kind::Masked,
                    edges: [Edge::default(); 4],
                    rect: [F::of(0.0); 5],
                    mask: &mask.coverage,
                    half: F::of(0.0),
                    inv_area: F::of(0.0),
                    x0: mask.x0,
                    x1: mask.x1,
                    y0: mask.y0,
                    y1: mask.y1,
                    opacity,
                    color,
                };
            }
        };
        let filter = scene.filter;
        let v = layer.corners();
        if kind == Kind::Box {
            let p = layer.params;
            // A side's ramp is zero from half the filter outside it.
            let (x0, x1, y0, y1) = match (
                box_span(p[0], p[2], filter, scene.width),
                box_span(p[1], p[3], filter, scene.height),
            ) {
                (Some((x0, x1)), Some((y0, y1))) => (x0, x1, y0, y1),
                _ => (0, 0, 0, 0),
            };
            return Self {
                kind,
                edges: [Edge::default(); 4],
                rect: [p[0], p[1], p[2], p[3], 1.0 / filter].map(F::of),
                mask: &[],
                half: F::of(0.0),
                inv_area: F::of(0.0),
                x0,
                x1,
                y0,
                y1,
                opacity,
                color,
            };
        }
        let cross = if sides == 3 {
            (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0])
        } else {
            // Twice the shoelace area.
            (0..sides)
                .map(|i| {
                    let j = (i + 1) % sides;
                    v[2 * i] * v[2 * j + 1] - v[2 * j] * v[2 * i + 1]
                })
                .sum()
        };
        let sigma = if cross >= 0.0 { 1.0 } else { -1.0 };
        let mut visible = cross.abs() > 1e-9;
        let edges = std::array::from_fn(|e| {
            if e >= sides {
                return Edge::default();
            }
            let (p, q) = (e, (e + 1) % sides);
            let (px, py) = (v[2 * p], v[2 * p + 1]);
            let (ex, ey) = (v[2 * q] - px, v[2 * q + 1] - py);
            let l = (ex * ex + ey * ey).sqrt();
            if l < 1e-9 {
                visible = false;
            }
            let l = l.max(1e-9);
            let inv_l = 1.0 / l;
            let inv_l3 = inv_l * inv_l * inv_l;
            // Widths of the filter square projected on the normal
            // `σ (−ey, ex) / l`, and their gradients with respect to `e`.
            let u = filter * ey.abs() * inv_l;
            let du = [
                -filter * ey.abs() * ex * inv_l3,
                filter * (sign(ey) * inv_l - ey.abs() * ey * inv_l3),
            ];
            let w = filter * ex.abs() * inv_l;
            let dw = [
                filter * (sign(ex) * inv_l - ex.abs() * ex * inv_l3),
                -filter * ex.abs() * ey * inv_l3,
            ];
            let ((wide, dwide), (mut narrow, dnarrow)) = if u >= w {
                ((u, du), (w, dw))
            } else {
                ((w, dw), (u, du))
            };
            if narrow < 1e-6 {
                narrow = 0.0;
            }
            let (inv_2ab, inv_narrow) = if narrow > 0.0 {
                (0.5 / (wide * narrow), 1.0 / narrow)
            } else {
                (0.0, 0.0)
            };
            Edge {
                nx: F::of(-sigma * ey * inv_l),
                ny: F::of(sigma * ex * inv_l),
                c: F::of(sigma * (ey * px - ex * py) * inv_l),
                half_sum: F::of((wide + narrow) / 2.0),
                half_diff: F::of((wide - narrow) / 2.0),
                inv_wide: F::of(1.0 / wide),
                inv_2ab: F::of(inv_2ab),
                inv_narrow: F::of(inv_narrow),
                px: F::of(px),
                py: F::of(py),
                ex: F::of(ex),
                ey: F::of(ey),
                inv_l: F::of(inv_l),
                sigma: F::of(sigma),
                dwide: dwide.map(F::of),
                dnarrow: dnarrow.map(F::of),
            }
        });
        // A pixel centre further than `filter · √2 / 2` outside an edge is
        // not covered.
        let reach = filter * 0.707_2;
        let span = |offset: usize, size: usize| {
            let values = (1..sides).map(|k| v[2 * k + offset]);
            let low = values.clone().fold(v[offset], f64::min) - reach;
            let high = values.fold(v[offset], f64::max) + reach;
            let start = low.ceil().max(0.0);
            let end = (high.floor() + 1.0).min(size as f64);
            if visible && start < end {
                (start as usize, end as usize)
            } else {
                (0, 0)
            }
        };
        let (x0, x1) = span(0, scene.width);
        let (y0, y1) = span(1, scene.height);
        let (x0, x1, y0, y1) = if x0 < x1 && y0 < y1 {
            (x0, x1, y0, y1)
        } else {
            (0, 0, 0, 0)
        };
        Self {
            kind,
            edges,
            rect: [F::of(0.0); 5],
            mask: &[],
            half: F::of(filter / 2.0),
            inv_area: F::of(1.0 / (filter * filter)),
            x0,
            x1,
            y0,
            y1,
            opacity,
            color,
        }
    }

    /// The rows of `y0..y1` the bounding box reaches, empty if none.
    fn rows(&self, y0: usize, y1: usize) -> Range<usize> {
        self.y0.max(y0)..self.y1.min(y1).max(self.y0.max(y0))
    }

    /// The coverage of the pixel `(x, y)` inside the bounding box, with
    /// `fy` the row as a float, by the layer's [`Cover`].
    #[cfg(test)]
    fn pixel(&self, x: usize, y: usize, fy: F) -> F {
        match self.kind {
            Kind::Masked => Masked::cover(self, x, y, fy),
            Kind::Triangle => Edges::<3>::cover(self, x, y, fy),
            Kind::Quad => Edges::<4>::cover(self, x, y, fy),
            Kind::Box => Boxed::cover(self, x, y, fy),
        }
    }

    /// The coverage of the pixel centred at `(x, y)` by the edges or the
    /// sides: the layer's exact area in the pixel's filter square.
    #[cfg(test)]
    fn coverage(&self, x: F, y: F) -> F {
        match self.kind {
            Kind::Triangle => self.edge_coverage::<3>(x, y),
            Kind::Quad => self.edge_coverage::<4>(x, y),
            Kind::Box => self.box_coverage(x, y),
            Kind::Masked => panic!("a mask has no edges"),
        }
    }

    /// The coverage of the pixel centred at `(x, y)` by an axis-aligned
    /// rectangle, as [`Self::box_coverage_grad`] computes it.
    #[inline]
    fn box_coverage(&self, x: F, y: F) -> F {
        let [x0, y0, x1, y1, inv] = self.rect;
        let (zero, one, half) = (F::of(0.0), F::of(1.0), F::of(0.5));
        let mut product = one;
        for d in [x - x0, y - y0, x1 - x, y1 - y] {
            let t = half + d * inv;
            if t <= zero {
                return zero;
            }
            if t < one {
                product = product * t;
            }
        }
        product
    }

    /// The coverage of the pixel centred at `(x, y)` by an axis-aligned
    /// rectangle and its gradient with respect to `x0, y0, x1, y1`, or
    /// `None` where the pixel is strictly outside a side's ramp.
    ///
    /// Each side's box-filtered half-plane is a ramp, `clamp(t)` with
    /// `t = 1/2 + d / f`, `d` the signed distance inside the side and `f`
    /// the filter width. The coverage is the product of the four ramps:
    /// the left and right ones give the overlap of the filter square with
    /// `x0..x1` along `x` as long as `x1 − x0 ≥ f`, so that no square
    /// crosses both sides, and likewise along `y`; the box filter is
    /// separable, so the product is the exact area of the rectangle in the
    /// filter square. Rectangles keep their sides at least 1 px, the
    /// export's filter width.
    ///
    /// A ramp's slope is `1 / f` for `0 < t < 1`, and half that at `t = 0`
    /// and `t = 1`, the mean of its one-sided slopes. The greedy search's
    /// rectangles have their sides on pixel boundaries, where the pixels on
    /// both sides sit at those ends: with the slope of either side alone
    /// the side would have no gradient there, or twice its one-sided ones.
    /// So a pixel at `t = 0` of a ramp, which has no coverage, still has a
    /// gradient.
    #[inline]
    fn box_coverage_grad(&self, x: F, y: F) -> Option<(F, [F; COORDS])> {
        let [x0, y0, x1, y1, inv] = self.rect;
        let (zero, one, half) = (F::of(0.0), F::of(1.0), F::of(0.5));
        let ramp = |d: F| {
            let t = half + d * inv;
            if t < zero {
                None
            } else if t > zero && t < one {
                Some((t, inv))
            } else if t == zero {
                Some((zero, half * inv))
            } else if t == one {
                Some((one, half * inv))
            } else {
                Some((one, zero))
            }
        };
        let (left, dleft) = ramp(x - x0)?;
        let (top, dtop) = ramp(y - y0)?;
        let (right, dright) = ramp(x1 - x)?;
        let (bottom, dbottom) = ramp(y1 - y)?;
        let (across, down) = (left * right, top * bottom);
        let mut grad = [zero; COORDS];
        grad[0] = -dleft * right * down;
        grad[1] = -dtop * bottom * across;
        grad[2] = dright * left * down;
        grad[3] = dbottom * top * across;
        Some((across * down, grad))
    }

    /// [`Self::coverage`] of the first `N` edges, three or four.
    #[inline]
    fn edge_coverage<const N: usize>(&self, x: F, y: F) -> F {
        let d: [F; N] = std::array::from_fn(|e| self.edges[e].distance(x, y));
        let Some((cutting, last)) = self.cutting(&d) else {
            return F::of(0.0);
        };
        match cutting {
            0 => F::of(1.0),
            1 => self.edges[last].cdf(d[last]),
            _ => self.clip(&d).coverage(self.inv_area),
        }
    }

    /// The number of the first `N` edges whose reach holds the filter
    /// square of a pixel at the signed distances `d` from them, those with
    /// `−half_sum < d < half_sum`, and the last of them; `None` if the
    /// square is outside an edge.
    #[inline]
    fn cutting<const N: usize>(&self, d: &[F; N]) -> Option<(usize, usize)> {
        let (mut cutting, mut last) = (0, 0);
        for (e, &d) in d.iter().enumerate() {
            let reach = self.edges[e].half_sum;
            if d <= -reach {
                return None;
            }
            if d < reach {
                cutting += 1;
                last = e;
            }
        }
        Some((cutting, last))
    }

    /// The filter square of a pixel at the signed distances `d` from the
    /// first `N` edges, clipped to the inside of each edge whose reach
    /// holds it, in edge order (Sutherland–Hodgman). Every edge outside the
    /// square's reach holds it whole and clips nothing.
    #[inline]
    fn clip<const N: usize>(&self, d: &[F; N]) -> Clipped<F> {
        let (zero, h) = (F::of(0.0), self.half);
        let mut polygon = Clipped::empty();
        for (u, v) in [(-h, -h), (h, -h), (h, h), (-h, h)] {
            polygon.push(u, v, SQUARE);
        }
        for (e, &d) in d.iter().enumerate() {
            let edge = &self.edges[e];
            if d >= edge.half_sum {
                continue;
            }
            let mut inside = [zero; CLIP];
            for (i, inside) in inside.iter_mut().enumerate().take(polygon.len) {
                *inside = edge.nx * polygon.u[i] + edge.ny * polygon.v[i] + d;
            }
            let mut clipped = Clipped::empty();
            for i in 0..polygon.len {
                let j = if i + 1 == polygon.len { 0 } else { i + 1 };
                let (si, sj) = (inside[i], inside[j]);
                let crossing = || {
                    let t = si / (si - sj);
                    (
                        polygon.u[i] + t * (polygon.u[j] - polygon.u[i]),
                        polygon.v[i] + t * (polygon.v[j] - polygon.v[i]),
                    )
                };
                if si >= zero {
                    clipped.push(polygon.u[i], polygon.v[i], polygon.on[i]);
                    if sj < zero {
                        let (u, v) = crossing();
                        clipped.push(u, v, e as u8);
                    }
                } else if sj >= zero {
                    let (u, v) = crossing();
                    clipped.push(u, v, polygon.on[i]);
                }
            }
            polygon = clipped;
            if polygon.len == 0 {
                break;
            }
        }
        polygon
    }

    /// The coverage of the pixel centred at `(x, y)` by the first `N`
    /// edges, three or four, and its gradient with respect to the
    /// vertex coordinates, or `None` where the coverage is zero.
    ///
    /// The coverage is the exact area of the polygon in the filter square,
    /// over the square's area. Where at most one edge cuts the square, it
    /// is that edge's box-filtered half-plane `F(d)` ([`Edge::cdf`]), with
    /// its derivative. Where two or more do, it is the area of the square
    /// clipped to the inside of each ([`Self::clip`]), by the shoelace
    /// formula, and its derivative is the first variation of that area:
    /// the square's sides do not move, so only the clipped segments of the
    /// edges contribute, each by its outward normal integrated against the
    /// velocity of its points. The two agree where a second edge's reach
    /// ends on the square's corner, in value and in gradient.
    #[inline]
    fn edge_coverage_grad<const N: usize>(&self, x: F, y: F) -> Option<(F, [F; COORDS])> {
        let zero = F::of(0.0);
        let d: [F; N] = std::array::from_fn(|e| self.edges[e].distance(x, y));
        let (cutting, last) = self.cutting(&d)?;
        let mut grad = [zero; COORDS];
        match cutting {
            0 => Some((F::of(1.0), grad)),
            1 => {
                let edge = &self.edges[last];
                let value = edge.cdf_grad(d[last]);
                let g = edge.endpoint_grad(value, x, y, d[last]);
                let (p, q) = (last, (last + 1) % N);
                grad[2 * p] += g[0];
                grad[2 * p + 1] += g[1];
                grad[2 * q] += g[2];
                grad[2 * q + 1] += g[3];
                Some((value.f, grad))
            }
            _ => {
                let polygon = self.clip(&d);
                if polygon.len == 0 {
                    return None;
                }
                // Only the clipped segments of the edges move: each moves
                // the boundary along its outward normal `−(nx, ny)` at the
                // velocity `(1 − t) δP + t δQ` of its point at `t` along
                // `PQ`. With `s = t · |PQ|` from `s0` to `s1` on the
                // segment, the area moves by `L ∫ (1 − t) dt =
                // (s1 − s0) − (s1² − s0²) / (2 |PQ|)` per unit of `P` and
                // `(s1² − s0²) / (2 |PQ|)` per unit of `Q`.
                for i in 0..polygon.len {
                    let e = usize::from(polygon.on[i]);
                    if e >= N {
                        continue;
                    }
                    let j = if i + 1 == polygon.len { 0 } else { i + 1 };
                    let edge = &self.edges[e];
                    let (tx, ty) = (edge.ex * edge.inv_l, edge.ey * edge.inv_l);
                    let centre = (x - edge.px) * tx + (y - edge.py) * ty;
                    let s0 = centre + polygon.u[i] * tx + polygon.v[i] * ty;
                    let s1 = centre + polygon.u[j] * tx + polygon.v[j] * ty;
                    let (low, high) = if s0 <= s1 { (s0, s1) } else { (s1, s0) };
                    let length = high - low;
                    let to_q = F::of(0.5) * length * (high + low) * edge.inv_l;
                    let to_p = length - to_q;
                    let (ox, oy) = (-edge.nx * self.inv_area, -edge.ny * self.inv_area);
                    let (p, q) = (e, (e + 1) % N);
                    grad[2 * p] += ox * to_p;
                    grad[2 * p + 1] += oy * to_p;
                    grad[2 * q] += ox * to_q;
                    grad[2 * q + 1] += oy * to_q;
                }
                Some((polygon.coverage(self.inv_area), grad))
            }
        }
    }
}

impl<F: Real> Clipped<F> {
    fn empty() -> Self {
        Self {
            u: [F::of(0.0); CLIP],
            v: [F::of(0.0); CLIP],
            on: [SQUARE; CLIP],
            len: 0,
        }
    }

    /// Appends the vertex `(u, v)`, the side from it to the next lying on
    /// `on`. Clipping a convex polygon by a half-plane adds at most one
    /// vertex, so there is room for every vertex; should rounding ever
    /// make the signs of the vertices alternate more than twice, the
    /// vertices past [`CLIP`] are dropped.
    #[inline]
    fn push(&mut self, u: F, v: F, on: u8) {
        if self.len < CLIP {
            self.u[self.len] = u;
            self.v[self.len] = v;
            self.on[self.len] = on;
            self.len += 1;
        }
    }

    /// The polygon's area times `inv_area`, `1 / filter²`, by the shoelace
    /// formula, in `0..=1`.
    #[inline]
    fn coverage(&self, inv_area: F) -> F {
        let mut twice = F::of(0.0);
        for i in 0..self.len {
            let j = if i + 1 == self.len { 0 } else { i + 1 };
            twice += self.u[i] * self.v[j] - self.u[j] * self.v[i];
        }
        let coverage = F::of(0.5) * twice * inv_area;
        if coverage < F::of(0.0) {
            F::of(0.0)
        } else if coverage > F::of(1.0) {
            F::of(1.0)
        } else {
            coverage
        }
    }
}

/// How a layer covers its pixels. The pixel loops are compiled once per
/// implementation and chosen once per layer ([`Prepared::kind`]), so that
/// no pixel pays for the choice.
trait Cover {
    /// The coverage of the pixel `(x, y)` inside the layer's bounding box,
    /// with `fy` the row as a float.
    fn cover<F: Real>(layer: &Prepared<'_, F>, x: usize, y: usize, fy: F) -> F;

    /// The coverage and its gradient with respect to the vertex
    /// coordinates, or an axis-aligned rectangle's parameters, if the layer
    /// has them, or `None` where the coverage and its gradient are zero.
    fn cover_grad<F: Real>(
        layer: &Prepared<'_, F>,
        x: usize,
        y: usize,
        fy: F,
    ) -> Option<(F, Option<[F; COORDS]>)>;
}

/// A fixed layer's mask.
struct Masked;

/// An axis-aligned rectangle's four sides.
struct Boxed;

impl Cover for Boxed {
    #[inline]
    fn cover<F: Real>(layer: &Prepared<'_, F>, x: usize, _y: usize, fy: F) -> F {
        layer.box_coverage(F::of(x as f64), fy)
    }

    #[inline]
    fn cover_grad<F: Real>(
        layer: &Prepared<'_, F>,
        x: usize,
        _y: usize,
        fy: F,
    ) -> Option<(F, Option<[F; COORDS]>)> {
        layer
            .box_coverage_grad(F::of(x as f64), fy)
            .map(|(cov, grad)| (cov, Some(grad)))
    }
}

/// The exact area of a polygon of `N` edges in the filter square.
struct Edges<const N: usize>;

impl Cover for Masked {
    #[inline]
    fn cover<F: Real>(layer: &Prepared<'_, F>, x: usize, y: usize, _fy: F) -> F {
        layer.mask[(y - layer.y0) * (layer.x1 - layer.x0) + x - layer.x0]
    }

    #[inline]
    fn cover_grad<F: Real>(
        layer: &Prepared<'_, F>,
        x: usize,
        y: usize,
        fy: F,
    ) -> Option<(F, Option<[F; COORDS]>)> {
        let cov = Self::cover(layer, x, y, fy);
        (cov > F::of(0.0)).then_some((cov, None))
    }
}

impl<const N: usize> Cover for Edges<N> {
    #[inline]
    fn cover<F: Real>(layer: &Prepared<'_, F>, x: usize, _y: usize, fy: F) -> F {
        layer.edge_coverage::<N>(F::of(x as f64), fy)
    }

    #[inline]
    fn cover_grad<F: Real>(
        layer: &Prepared<'_, F>,
        x: usize,
        _y: usize,
        fy: F,
    ) -> Option<(F, Option<[F; COORDS]>)> {
        layer
            .edge_coverage_grad::<N>(F::of(x as f64), fy)
            .map(|(cov, grad)| (cov, Some(grad)))
    }
}

/// Evaluates `$call` with the type `$cover` standing for the [`Cover`] of
/// `$layer`.
macro_rules! by_cover {
    ($layer:expr, $cover:ident => $call:expr) => {
        match $layer.kind {
            Kind::Masked => {
                type $cover = Masked;
                $call
            }
            Kind::Triangle => {
                type $cover = Edges<3>;
                $call
            }
            Kind::Quad => {
                type $cover = Edges<4>;
                $call
            }
            Kind::Box => {
                type $cover = Boxed;
                $call
            }
        }
    };
}

/// Rows per band. Every pass splits the canvas into bands of this many
/// rows, the last one shorter, whatever the number of threads; each band
/// is one task, and the bands' partial sums are reduced in band order, so
/// every result is the same whatever the number of threads.
///
/// Measured against 8 and 32 rows on the engine runner's corpus: 8 runs
/// slightly faster on 8 threads and slower on 1.
const BAND_ROWS: usize = 16;

/// In a fit pass, the entry of a layer's partial sums that holds its
/// weight `Σ g · (1 − T)`; entries `0..3` hold `Σ g · R_c`.
const FIT_WEIGHT: usize = 3;

/// What a pass computes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The colour fit's sums, from the transmittance alone.
    Fit,
    /// The gradient, which also needs the canvas below each layer.
    Gradient,
}

/// One band's rows of the canvas, its checkpoints, the canvas below each
/// layer of one segment, the residual and the transmittances, and its
/// partial sums per layer.
#[derive(Default)]
struct Band<F> {
    /// The band's rows, `y0..y1`.
    y0: usize,
    y1: usize,
    canvas: Vec<F>,
    /// The band's canvas before every `interval`-th layer, one after the
    /// other.
    checkpoints: Vec<F>,
    /// The canvas below each layer of the segment being replayed, inside
    /// the layer's bounding box and the band.
    below: Vec<Vec<F>>,
    residual: Vec<F>,
    /// The transmittance of the layers above the current one.
    transmittance: Vec<F>,
    /// The transmittance `T` of the whole stack, `Π (1 − w)` over every
    /// layer; only for a fit.
    uncovered: Vec<F>,
    /// The gradient per layer, or the fit's sums ([`FIT_WEIGHT`]).
    sums: Vec<[f64; PARAMS]>,
    /// `Σ r²` over the band's residual, in `f64`, in order.
    squares: f64,
}

/// Reusable buffers of [`fit`], [`gradients`] and [`loss`]: one [`Band`]
/// per band.
#[derive(Default)]
pub(super) struct Workspace<F> {
    bands: Vec<Band<F>>,
}

impl<F: Real> Workspace<F> {
    /// The bands of a canvas `height` rows high.
    fn bands(&mut self, height: usize) -> &mut [Band<F>] {
        self.bands
            .resize_with(height.div_ceil(BAND_ROWS), Band::default);
        for (index, band) in self.bands.iter_mut().enumerate() {
            band.y0 = index * BAND_ROWS;
            band.y1 = (band.y0 + BAND_ROWS).min(height);
        }
        &mut self.bands
    }
}

/// Composites `layer` onto `canvas`, the rows from `top` on, within `rows`,
/// and folds it into `uncovered` if given.
fn paint<F: Real>(
    canvas: &mut [F],
    width: usize,
    top: usize,
    rows: Range<usize>,
    layer: &Prepared<'_, F>,
    uncovered: Option<&mut [F]>,
) {
    by_cover!(
        layer,
        C => paint_with::<C, F>(canvas, width, top, rows, layer, uncovered)
    );
}

/// [`paint`] with the layer's [`Cover`] `C`.
fn paint_with<C: Cover, F: Real>(
    canvas: &mut [F],
    width: usize,
    top: usize,
    rows: Range<usize>,
    layer: &Prepared<'_, F>,
    mut uncovered: Option<&mut [F]>,
) {
    for y in rows {
        let line = &mut canvas[3 * width * (y - top)..][..3 * width];
        let fy = F::of(y as f64);
        for x in layer.x0..layer.x1 {
            let cov = C::cover(layer, x, y, fy);
            if cov > F::of(0.0) {
                let w = layer.opacity * cov;
                for c in 0..3 {
                    let value = &mut line[3 * x + c];
                    *value += w * (layer.color[c] - *value);
                }
                if let Some(uncovered) = uncovered.as_deref_mut() {
                    let p = width * (y - top) + x;
                    uncovered[p] = uncovered[p] * (F::of(1.0) - w);
                }
            }
        }
    }
}

impl<F: Real> Band<F> {
    /// Composites `layers` from the background into the band's canvas,
    /// keeping the canvas before every `interval`-th layer (none with
    /// `interval` 0) and, with `uncovered`, the transmittance of the whole
    /// stack; then sets the residual to `X − T`.
    fn forward(
        &mut self,
        scene: &Scene<F>,
        layers: &[Prepared<'_, F>],
        interval: usize,
        uncovered: bool,
    ) {
        let width = scene.width;
        let (y0, y1) = (self.y0, self.y1);
        self.canvas.clear();
        self.canvas
            .extend((0..width * (y1 - y0)).flat_map(|_| scene.background));
        self.checkpoints.clear();
        self.uncovered.clear();
        if uncovered {
            self.uncovered.resize(width * (y1 - y0), F::of(1.0));
        }
        for (index, layer) in layers.iter().enumerate() {
            if interval > 0 && index % interval == 0 {
                self.checkpoints.extend_from_slice(&self.canvas);
            }
            let rows = layer.rows(y0, y1);
            let uncovered = uncovered.then_some(self.uncovered.as_mut_slice());
            paint(&mut self.canvas, width, y0, rows, layer, uncovered);
        }
        let target = &scene.target[3 * width * y0..3 * width * y1];
        self.residual.clear();
        self.residual
            .extend(self.canvas.iter().zip(target).map(|(&x, &t)| x - t));
    }

    /// One band's part of [`fit`] or [`gradients`]: the forward pass, then
    /// the top-down reverse pass, which sets `sums` to the band's partial
    /// sums per layer.
    fn pass(&mut self, scene: &Scene<F>, layers: &[Prepared<'_, F>], interval: usize, pass: Pass) {
        let width = scene.width;
        let n = layers.len();
        let (y0, y1) = (self.y0, self.y1);
        let fit = pass == Pass::Fit;
        // The fit needs only the transmittances, which need no canvas below
        // each layer.
        self.forward(scene, layers, if fit { 0 } else { interval }, fit);
        self.sums.clear();
        self.sums.resize(n, [0.0; PARAMS]);
        self.transmittance.clear();
        self.transmittance.resize(width * (y1 - y0), F::of(1.0));
        if fit {
            for (layer, sums) in layers.iter().zip(&mut self.sums).rev() {
                let rows = layer.rows(y0, y1);
                if !rows.is_empty() {
                    let reverse = Reverse {
                        layer,
                        width,
                        top: y0,
                        rows,
                    };
                    reverse.fit(
                        &self.residual,
                        &self.uncovered,
                        &mut self.transmittance,
                        sums,
                    );
                }
            }
            return;
        }
        let size = self.canvas.len();
        self.below.resize_with(interval, Vec::new);
        for segment in (0..n.div_ceil(interval)).rev() {
            let start = segment * interval;
            let end = (start + interval).min(n);
            // Replay the segment, keeping the canvas below each layer
            // inside its bounding box.
            self.canvas
                .copy_from_slice(&self.checkpoints[segment * size..][..size]);
            for (layer, below) in layers[start..end].iter().zip(&mut self.below) {
                below.clear();
                let rows = layer.rows(y0, y1);
                for y in rows.clone() {
                    let row = 3 * width * (y - y0);
                    below.extend_from_slice(&self.canvas[row + 3 * layer.x0..row + 3 * layer.x1]);
                }
                paint(&mut self.canvas, width, y0, rows, layer, None);
            }
            for index in (start..end).rev() {
                let layer = &layers[index];
                let rows = layer.rows(y0, y1);
                if !rows.is_empty() {
                    let reverse = Reverse {
                        layer,
                        width,
                        top: y0,
                        rows,
                    };
                    reverse.gradient(
                        &self.below[index - start],
                        &self.residual,
                        &mut self.transmittance,
                        &mut self.sums[index],
                    );
                }
            }
        }
    }
}

/// The reverse step through `layer` on the rows `rows` of a band whose
/// first row is `top`. Each pixel's `A` is the transmittance of the layers
/// above, `w` the layer's opacity times its coverage, and `g = A · w` the
/// weight of the layer's colour in the final composite.
struct Reverse<'a, F> {
    layer: &'a Prepared<'a, F>,
    width: usize,
    top: usize,
    rows: Range<usize>,
}

impl<F: Real> Reverse<'_, F> {
    /// Accumulates the colour fit's `Σ g · R_c` and `Σ g · (1 − T)` into
    /// `sums` ([`FIT_WEIGHT`]), then folds the layer into the
    /// transmittance (`A ← A · (1 − w)`).
    fn fit(
        &self,
        residual: &[F],
        uncovered: &[F],
        transmittance: &mut [F],
        sums: &mut [f64; PARAMS],
    ) {
        by_cover!(
            self.layer,
            C => self.fit_with::<C>(residual, uncovered, transmittance, sums)
        );
    }

    /// [`Self::fit`] with the layer's [`Cover`] `C`.
    fn fit_with<C: Cover>(
        &self,
        residual: &[F],
        uncovered: &[F],
        transmittance: &mut [F],
        sums: &mut [f64; PARAMS],
    ) {
        let layer = self.layer;
        for y in self.rows.clone() {
            let fy = F::of(y as f64);
            for x in layer.x0..layer.x1 {
                let cov = C::cover(layer, x, y, fy);
                if cov <= F::of(0.0) {
                    continue;
                }
                let p = self.width * (y - self.top) + x;
                let transmitted = transmittance[p];
                let w = layer.opacity * cov;
                let g = (transmitted * w).get();
                for c in 0..3 {
                    sums[c] += g * residual[3 * p + c].get();
                }
                sums[FIT_WEIGHT] += g * (F::of(1.0) - uncovered[p]).get();
                transmittance[p] = transmitted * (F::of(1.0) - w);
            }
        }
    }

    /// Accumulates the gradient `∂L/∂w = 2 A Σ_c R_c (s_c − X_{i−1,c})`
    /// into the coverage's coordinates ([`Kind::coords`]) and the alpha in
    /// `sums`, with `below` the canvas below the layer inside its bounding
    /// box and the band, then folds the layer into the transmittance. A
    /// fixed layer has the alpha's alone.
    fn gradient(
        &self,
        below: &[F],
        residual: &[F],
        transmittance: &mut [F],
        sums: &mut [f64; PARAMS],
    ) {
        by_cover!(
            self.layer,
            C => self.gradient_with::<C>(below, residual, transmittance, sums)
        );
    }

    /// [`Self::gradient`] with the layer's [`Cover`] `C`.
    fn gradient_with<C: Cover>(
        &self,
        below: &[F],
        residual: &[F],
        transmittance: &mut [F],
        sums: &mut [f64; PARAMS],
    ) {
        let layer = self.layer;
        let box_width = layer.x1 - layer.x0;
        let coords = layer.kind.coords();
        let opacity = layer.opacity.get();
        for (local, y) in self.rows.clone().enumerate() {
            let fy = F::of(y as f64);
            for x in layer.x0..layer.x1 {
                let Some((cov, cov_grad)) = C::cover_grad(layer, x, y, fy) else {
                    continue;
                };
                let p = self.width * (y - self.top) + x;
                let transmitted = transmittance[p];
                let w = layer.opacity * cov;
                let pixel = &residual[3 * p..3 * p + 3];
                let x_below = &below[3 * (local * box_width + x - layer.x0)..][..3];
                let mut dot = F::of(0.0);
                for c in 0..3 {
                    dot += pixel[c] * (layer.color[c] - x_below[c]);
                }
                let dl_dw = (F::of(2.0) * transmitted * dot).get();
                if dl_dw != 0.0 {
                    if let Some(cov_grad) = cov_grad {
                        for (sum, d) in sums[..coords].iter_mut().zip(cov_grad) {
                            *sum += dl_dw * opacity * d.get();
                        }
                    }
                    sums[ALPHA] += dl_dw * cov.get() / 255.0;
                }
                transmittance[p] = transmitted * (F::of(1.0) - w);
            }
        }
    }
}

/// `layers`, ready to composite.
fn prepare<'a, F: Real>(scene: &'a Scene<F>, layers: &[Layer]) -> Vec<Prepared<'a, F>> {
    layers
        .iter()
        .map(|layer| Prepared::new(layer, scene))
        .collect()
}

/// `Σ (X − T)²` over every pixel and channel of the composite of
/// `layers`, in `f64`: in order within each band, then over the bands in
/// order.
pub(super) fn loss<F: Real>(scene: &Scene<F>, layers: &[Layer], work: &mut Workspace<F>) -> f64 {
    let layers = prepare(scene, layers);
    let bands = work.bands(scene.height);
    bands.par_iter_mut().for_each(|band| {
        band.forward(scene, &layers, 0, false);
        band.squares = band.residual.iter().map(|r| r.get() * r.get()).sum();
    });
    bands.iter().map(|band| band.squares).sum()
}

/// Runs `pass` over `layers` as one task per band of [`BAND_ROWS`] rows,
/// with a single fork and join, and returns the bands' partial sums per
/// layer reduced in band order.
///
/// Each band composites every layer that reaches it, then runs the
/// top-down reverse pass over its own rows. The reverse pass needs, at
/// layer `i`, the transmittance `A_i` of the layers above
/// (`final = A_i · X_i + B_i`), folded in as it goes down, the final
/// residual `R`, and, for the gradient, the canvas `X_{i−1}` below the
/// layer. The latter comes from each band's canvas checkpoints every
/// `interval` layers ([`checkpoint_interval`]: `⌈√N⌉` unless memory caps
/// them; the bands' checkpoints together hold as many canvases as one set
/// of whole-canvas checkpoints would), replayed one segment at a time and
/// kept only inside each layer's bounding box.
fn run<F: Real>(
    scene: &Scene<F>,
    layers: &[Layer],
    pass: Pass,
    work: &mut Workspace<F>,
) -> Vec<[f64; PARAMS]> {
    let n = layers.len();
    let layers = prepare(scene, layers);
    let interval = checkpoint_interval(n, 3 * scene.width * scene.height * size_of::<F>());
    let bands = work.bands(scene.height);
    bands
        .par_iter_mut()
        .for_each(|band| band.pass(scene, &layers, interval, pass));
    let mut totals = vec![[0.0; PARAMS]; n];
    for band in bands.iter() {
        for (total, sums) in totals.iter_mut().zip(&band.sums) {
            for (total, value) in total.iter_mut().zip(sums) {
                *total += value;
            }
        }
    }
    totals
}

/// The gradient of the loss `Σ (X − T)²` per layer of `layers` with
/// respect to its parameters ([`Outline`]) and its opacity, the colours
/// held constant: zero for the parameters a layer does not use, every one
/// of a fixed layer's among them. A rotated rectangle's comes from its
/// corners' ([`Layer::chain`]).
pub(super) fn gradients<F: Real>(
    scene: &Scene<F>,
    layers: &[Layer],
    work: &mut Workspace<F>,
) -> Vec<[f64; PARAMS]> {
    let totals = run(scene, layers, Pass::Gradient, work);
    layers
        .iter()
        .zip(totals)
        .map(|(layer, grad)| layer.chain(grad))
        .collect()
}

/// Refits every colour of `layers` together, from the residual of the stack
/// as it is (one Jacobi step), so that the pass needs no barrier per layer.
///
/// The loss is quadratic in the colours, with Hessian `2 Gᵀ G` where
/// `G[p][i] = g_i(p)`, per channel. Every `g` is non-negative and, at each
/// pixel, the weights of all layers add up to `1 − T`, with `T` the
/// transmittance of the whole stack, so the diagonal
/// `D_i = Σ_p g_i · (1 − T)` (each row's sum) majorises the Hessian. Each
/// channel then moves to `s − ω Σ g · R / D_i`, clamped to `0..=255`, with
/// `ω` the [`OVER_RELAXATION`]: for `ω < 2` this projected step on a
/// majorised quadratic never raises the loss, however much the layers
/// overlap, where a plain Jacobi step (`Σ g²` for `D`) can diverge.
pub(super) fn fit<F: Real>(scene: &Scene<F>, layers: &mut [Layer], work: &mut Workspace<F>) {
    let totals = run(scene, layers, Pass::Fit, work);
    for (layer, total) in layers.iter_mut().zip(&totals) {
        let weight = total[FIT_WEIGHT];
        if weight > 0.0 {
            for (c, color) in layer.color.iter_mut().enumerate() {
                *color = (*color - OVER_RELAXATION * total[c] / weight).clamp(0.0, 255.0);
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    pub(in crate::joint) fn random_scene<F: Real>(
        rng: &mut ChaCha8Rng,
        width: usize,
        height: usize,
    ) -> Scene<F> {
        Scene {
            width,
            height,
            target: (0..3 * width * height)
                .map(|_| F::of(rng.random_range(0.0..255.0)))
                .collect(),
            background: [F::of(40.0), F::of(120.0), F::of(200.0)],
            filter: 1.0,
            masks: Vec::new(),
        }
    }

    /// A scene without a target, for the coverage alone.
    fn blank_scene(width: usize, height: usize) -> Scene<f64> {
        Scene {
            width,
            height,
            target: vec![0.0; 3 * width * height],
            background: [0.0; 3],
            filter: 1.0,
            masks: Vec::new(),
        }
    }

    fn random_tris(rng: &mut ChaCha8Rng, count: usize, size: f64) -> Vec<Layer> {
        (0..count)
            .map(|_| {
                let vertices = std::array::from_fn(|_| rng.random_range(-3.0..size + 3.0));
                let alpha = rng.random_range(40.0..250.0);
                Layer::triangle(
                    vertices,
                    alpha,
                    std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                )
            })
            .collect()
    }

    /// The direction at `degrees` from `x`, turned by `turn` radians.
    fn direction(degrees: f64, turn: f64) -> (f64, f64) {
        let (sin, cos) = (degrees.to_radians() + turn).sin_cos();
        (cos, sin)
    }

    /// Triangles with a vertex of 20° and one of 155° (whose other two
    /// angles are acute too), each in both orientations, at random places
    /// and turns in a canvas of `size`: every vertex lies inside a pixel,
    /// where two edges cut its square.
    fn sharp_tris(rng: &mut ChaCha8Rng, size: f64) -> Vec<Layer> {
        let mut layers = Vec::new();
        for angle in [20.0, 155.0] {
            let (ax, ay) = (
                rng.random_range(size / 4.0..3.0 * size / 4.0),
                rng.random_range(size / 4.0..3.0 * size / 4.0),
            );
            let turn = rng.random_range(0.0..std::f64::consts::TAU);
            let (r1, r2) = (rng.random_range(4.0..10.0), rng.random_range(4.0..10.0));
            let (b, c) = (direction(0.0, turn), direction(angle, turn));
            let v = [
                ax,
                ay,
                ax + r1 * b.0,
                ay + r1 * b.1,
                ax + r2 * c.0,
                ay + r2 * c.1,
            ];
            let reversed = [v[4], v[5], v[2], v[3], v[0], v[1]];
            for v in [v, reversed] {
                let alpha = rng.random_range(40.0..250.0);
                let color = std::array::from_fn(|_| rng.random_range(0.0..255.0));
                layers.push(Layer::triangle(v, alpha, color));
            }
        }
        layers
    }

    /// Convex quadrilaterals `A, B, C, D` with a vertex `A` of 165° or of
    /// 25°, `C` on the diagonal from `A` through the middle of `BD`, at or
    /// beyond the parallelogram's fourth vertex, so that each has an angle
    /// of at least 150° and one of at most 30°, each in both orientations,
    /// at random places and turns in a canvas of `size`.
    fn sharp_quads(rng: &mut ChaCha8Rng, size: f64) -> Vec<Layer> {
        let mut layers = Vec::new();
        for angle in [165.0, 25.0] {
            let (ax, ay) = (
                rng.random_range(size / 4.0..3.0 * size / 4.0),
                rng.random_range(size / 4.0..3.0 * size / 4.0),
            );
            let turn = rng.random_range(0.0..std::f64::consts::TAU);
            let (r1, r2) = (rng.random_range(4.0..10.0), rng.random_range(4.0..10.0));
            let (b, d) = (direction(0.0, turn), direction(angle, turn));
            let (b, d) = ((r1 * b.0, r1 * b.1), (r2 * d.0, r2 * d.1));
            let s = rng.random_range(1.0..1.3);
            let v = [
                ax,
                ay,
                ax + b.0,
                ay + b.1,
                ax + s * (b.0 + d.0),
                ay + s * (b.1 + d.1),
                ax + d.0,
                ay + d.1,
            ];
            let angles: [f64; 4] = std::array::from_fn(|k| {
                let (o, p, q) = (k, (k + 3) % 4, (k + 1) % 4);
                let (ux, uy) = (v[2 * p] - v[2 * o], v[2 * p + 1] - v[2 * o + 1]);
                let (wx, wy) = (v[2 * q] - v[2 * o], v[2 * q + 1] - v[2 * o + 1]);
                ((ux * wx + uy * wy) / (ux.hypot(uy) * wx.hypot(wy)))
                    .acos()
                    .to_degrees()
            });
            assert!(
                (angles.iter().sum::<f64>() - 360.0).abs() < 1e-6,
                "{angles:?}"
            );
            assert!(angles.iter().any(|&a| a >= 150.0), "{angles:?}");
            assert!(angles.iter().any(|&a| a <= 30.0), "{angles:?}");
            let reversed = [v[6], v[7], v[4], v[5], v[2], v[3], v[0], v[1]];
            for params in [v, reversed] {
                layers.push(Layer {
                    outline: Outline::Quad,
                    params,
                    alpha: rng.random_range(40.0..250.0),
                    color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                });
            }
        }
        layers
    }

    /// The vertices of a random strictly convex quadrilateral around
    /// `centre`, with radii up to `size`: four points on an ellipse whose
    /// angles are at least 20° apart, in a random orientation.
    pub(in crate::joint) fn random_convex(
        rng: &mut ChaCha8Rng,
        centre: (f64, f64),
        size: f64,
    ) -> [f64; COORDS] {
        let at: [f64; 4] = loop {
            let mut at: [f64; 4] = std::array::from_fn(|_| rng.random_range(0.0..360.0));
            at.sort_by(f64::total_cmp);
            let gaps = [
                at[1] - at[0],
                at[2] - at[1],
                at[3] - at[2],
                360.0 + at[0] - at[3],
            ];
            if gaps.iter().all(|&gap| (20.0..=160.0).contains(&gap)) {
                break at;
            }
        };
        let (rx, ry) = (
            rng.random_range(0.3..1.0) * size,
            rng.random_range(0.3..1.0) * size,
        );
        let turn = rng.random_range(0.0..std::f64::consts::TAU);
        let reverse = rng.random_bool(0.5);
        let mut v = [0.0; COORDS];
        for k in 0..4 {
            let (sin, cos) = at[if reverse { 3 - k } else { k }].to_radians().sin_cos();
            let (x, y) = (rx * cos, ry * sin);
            v[2 * k] = centre.0 + x * turn.cos() - y * turn.sin();
            v[2 * k + 1] = centre.1 + x * turn.sin() + y * turn.cos();
        }
        v
    }

    fn random_quads(rng: &mut ChaCha8Rng, count: usize, size: f64) -> Vec<Layer> {
        (0..count)
            .map(|_| {
                let centre = (rng.random_range(0.0..size), rng.random_range(0.0..size));
                Layer {
                    outline: Outline::Quad,
                    params: random_convex(rng, centre, size / 2.0),
                    alpha: rng.random_range(40.0..250.0),
                    color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                }
            })
            .collect()
    }

    /// Random axis-aligned rectangles in a canvas of `size`, keeping the
    /// rule: sides at least 1 px and at most 8 times each other.
    pub(in crate::joint) fn random_boxes(
        rng: &mut ChaCha8Rng,
        count: usize,
        size: f64,
    ) -> Vec<Layer> {
        (0..count)
            .map(|_| {
                let (w, h) = loop {
                    let (w, h) = (
                        rng.random_range(1.0..size / 2.0),
                        rng.random_range(1.0..size / 2.0),
                    );
                    if w <= 8.0 * h && h <= 8.0 * w {
                        break (w, h);
                    }
                };
                let (x, y) = (
                    rng.random_range(-3.0..size - 2.0),
                    rng.random_range(-3.0..size - 2.0),
                );
                let mut params = [0.0; COORDS];
                params[..4].copy_from_slice(&[x, y, x + w, y + h]);
                Layer {
                    outline: Outline::Rect,
                    params,
                    alpha: rng.random_range(40.0..250.0),
                    color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                }
            })
            .collect()
    }

    /// Random rotated rectangles in a canvas of `size`, keeping the rule:
    /// `|u|` and `h` at least 1/2 and at most 8 times each other.
    pub(in crate::joint) fn random_rotated(
        rng: &mut ChaCha8Rng,
        count: usize,
        size: f64,
    ) -> Vec<Layer> {
        (0..count)
            .map(|_| {
                let (r, h) = loop {
                    let (r, h) = (
                        rng.random_range(0.5..size / 3.0),
                        rng.random_range(0.5..size / 3.0),
                    );
                    if r <= 8.0 * h && h <= 8.0 * r {
                        break (r, h);
                    }
                };
                let turn = rng.random_range(0.0..std::f64::consts::TAU);
                let mut params = [0.0; COORDS];
                params[..5].copy_from_slice(&[
                    rng.random_range(0.0..size),
                    rng.random_range(0.0..size),
                    r * turn.cos(),
                    r * turn.sin(),
                    h,
                ]);
                Layer {
                    outline: Outline::Rotated,
                    params,
                    alpha: rng.random_range(40.0..250.0),
                    color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                }
            })
            .collect()
    }

    /// A random mask on a `width × height` canvas: a box partly off the
    /// canvas clipped to it, with pixels uncovered, covered and partly
    /// covered.
    fn random_mask<F: Real>(rng: &mut ChaCha8Rng, width: usize, height: usize) -> Mask<F> {
        let x0 = rng.random_range(0..width - 4);
        let y0 = rng.random_range(0..height - 4);
        let x1 = rng.random_range(x0 + 2..=width);
        let y1 = rng.random_range(y0 + 2..=height);
        let coverage = (0..(x1 - x0) * (y1 - y0))
            .map(|_| match rng.random_range(0..4) {
                0 => F::of(0.0),
                1 => F::of(rng.random_range(0.0..1.0)),
                _ => F::of(1.0),
            })
            .collect();
        Mask {
            x0,
            x1,
            y0,
            y1,
            coverage,
        }
    }

    /// A scene `width × height` with `masks` random fixed layers, and its
    /// layers: triangles, quads and the fixed layers, interleaved.
    fn mixed_scene(
        rng: &mut ChaCha8Rng,
        width: usize,
        height: usize,
        counts: (usize, usize, usize),
    ) -> (Scene<f64>, Vec<Layer>) {
        let mut scene = random_scene::<f64>(rng, width, height);
        let size = width.min(height) as f64;
        let mut tris = random_tris(rng, counts.0, size).into_iter();
        let mut quads = random_quads(rng, counts.1, size).into_iter();
        let mut layers = Vec::new();
        for index in 0..counts.2.max(counts.0).max(counts.1) {
            layers.extend(tris.next());
            if index < counts.2 {
                scene.masks.push(random_mask(rng, width, height));
                layers.push(Layer {
                    outline: Outline::Fixed(index),
                    params: [0.0; COORDS],
                    alpha: rng.random_range(40.0..250.0),
                    color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                });
            }
            layers.extend(quads.next());
        }
        (scene, layers)
    }

    fn fresh_loss(scene: &Scene<f64>, layers: &[Layer]) -> f64 {
        loss(scene, layers, &mut Workspace::default())
    }

    /// The analytic gradients of vertices and alphas of `layers` on
    /// `scene` against central differences. Returns the largest relative
    /// error over the components whose magnitude is at least 1% of the
    /// largest one, the overall relative error, and how many components
    /// were checked. The components a layer does not use, every vertex
    /// coordinate of a fixed layer among them, must be exactly zero.
    fn gradient_check(scene: &Scene<f64>, layers: &[Layer]) -> (f64, f64, usize) {
        let analytic = gradients(scene, layers, &mut Workspace::default());
        let mut numeric = vec![[0.0; PARAMS]; layers.len()];
        for (index, grads) in numeric.iter_mut().enumerate() {
            let coords = layers[index].outline.params();
            for (k, grad) in grads.iter_mut().enumerate() {
                if k < ALPHA && k >= coords {
                    assert_eq!(analytic[index][k], 0.0, "layer {index}, {k}");
                    continue;
                }
                let h = if k == ALPHA { 1e-3 } else { 1e-5 };
                let shifted = |sign: f64| {
                    let mut moved = layers.to_vec();
                    if k == ALPHA {
                        moved[index].alpha += sign * h;
                    } else {
                        moved[index].params[k] += sign * h;
                    }
                    fresh_loss(scene, &moved)
                };
                *grad = (shifted(1.0) - shifted(-1.0)) / (2.0 * h);
            }
        }
        let scale = numeric
            .iter()
            .flatten()
            .fold(0.0_f64, |max, value| max.max(value.abs()));
        let mut worst = 0.0_f64;
        let mut checked = 0;
        let mut squared = (0.0, 0.0);
        for (a, f) in analytic.iter().flatten().zip(numeric.iter().flatten()) {
            squared.0 += (a - f) * (a - f);
            squared.1 += f * f;
            if f.abs() >= 0.01 * scale {
                checked += 1;
                worst = worst.max((a - f).abs() / f.abs());
            }
        }
        let overall = (squared.0 / squared.1).sqrt();
        (worst, overall, checked)
    }

    /// On 32 × 32 random targets with random triangles, and triangles
    /// with acute and obtuse vertices ([`sharp_tris`]), at the export's
    /// filter width and at wider ones.
    #[test]
    fn analytic_gradients_match_finite_differences() {
        for (seed, count, filter) in [
            (1, 3, 1.0),
            (2, 5, 1.0),
            (3, 7, 1.0),
            (4, 5, 1.6),
            (5, 7, 2.0),
        ] {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let mut scene = random_scene::<f64>(&mut rng, 32, 32);
            scene.filter = filter;
            let mut tris = random_tris(&mut rng, count, 32.0);
            tris.extend(sharp_tris(&mut rng, 32.0));
            let (worst, overall, checked) = gradient_check(&scene, &tris);
            assert!(checked >= count * 4, "seed {seed}: {checked} checked");
            assert!(worst < 1e-3, "seed {seed}: worst {worst}");
            assert!(overall < 1e-4, "seed {seed}: overall {overall}");
        }
    }

    /// The same with random convex quadrilaterals, and quadrilaterals with
    /// acute and obtuse vertices ([`sharp_quads`]).
    #[test]
    fn analytic_gradients_of_polygons_match_finite_differences() {
        for (seed, count, filter) in [
            (11, 3, 1.0),
            (12, 5, 1.0),
            (13, 7, 1.0),
            (14, 5, 1.6),
            (15, 7, 2.0),
        ] {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let mut scene = random_scene::<f64>(&mut rng, 32, 32);
            scene.filter = filter;
            let mut quads = random_quads(&mut rng, count, 32.0);
            quads.extend(sharp_quads(&mut rng, 32.0));
            let (worst, overall, checked) = gradient_check(&scene, &quads);
            assert!(checked >= count * 6, "seed {seed}: {checked} checked");
            assert!(worst < 1e-3, "seed {seed}: worst {worst}");
            assert!(overall < 1e-4, "seed {seed}: overall {overall}");
        }
    }

    /// The same with axis-aligned and rotated rectangles, among them
    /// rectangles on the rule's boundary: sides of exactly 1 px, half-sides
    /// of exactly 1/2, and long sides of exactly 8 times the short ones.
    #[test]
    fn analytic_gradients_of_rectangles_match_finite_differences() {
        for (seed, count, filter) in [
            (31, 3, 1.0),
            (32, 5, 1.0),
            (33, 7, 1.0),
            (34, 5, 1.6),
            (35, 7, 2.0),
        ] {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let mut scene = random_scene::<f64>(&mut rng, 32, 32);
            scene.filter = filter;
            let mut layers = random_boxes(&mut rng, count, 32.0);
            layers.extend(random_rotated(&mut rng, count, 32.0));
            // On the boundary of the rule.
            layers[0].params[2] = layers[0].params[0] + 1.0;
            layers[1].params[3] =
                layers[1].params[1] + 8.0 * (layers[1].params[2] - layers[1].params[0]);
            let [_, _, ux, uy, ..] = layers[count].params;
            let r = ux.hypot(uy);
            layers[count].params[2] = 0.5 * ux / r;
            layers[count].params[3] = 0.5 * uy / r;
            layers[count].params[4] = 4.0;
            layers[count + 1].params[4] =
                layers[count + 1].params[2].hypot(layers[count + 1].params[3]) / 8.0;
            // On pixel boundaries, as the greedy search's: central
            // differences there are the mean of the one-sided slopes.
            layers[2].params[..4].copy_from_slice(&[3.5, 4.5, 12.5, 20.5]);
            let (worst, overall, checked) = gradient_check(&scene, &layers);
            let aligned = gradients(&scene, &layers, &mut Workspace::default())[2];
            assert!(aligned[..4].iter().all(|&g| g != 0.0), "{aligned:?}");
            assert!(checked >= count * 6, "seed {seed}: {checked} checked");
            assert!(worst < 1e-3, "seed {seed}: worst {worst}");
            assert!(overall < 1e-4, "seed {seed}: overall {overall}");
        }
    }

    /// The same with triangles, quadrilaterals and fixed layers
    /// interleaved: the fixed layers have an opacity gradient and no
    /// vertex gradient.
    #[test]
    fn analytic_gradients_of_a_mixed_scene_match_finite_differences() {
        for (seed, counts) in [(21, (2, 2, 2)), (22, (3, 2, 4)), (23, (1, 3, 5))] {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let (scene, layers) = mixed_scene(&mut rng, 32, 32, counts);
            let (worst, overall, checked) = gradient_check(&scene, &layers);
            assert!(
                checked >= layers.len() * 3,
                "seed {seed}: {checked} checked"
            );
            assert!(worst < 1e-3, "seed {seed}: worst {worst}");
            assert!(overall < 1e-4, "seed {seed}: overall {overall}");
            let analytic = gradients(&scene, &layers, &mut Workspace::default());
            for (layer, grad) in layers.iter().zip(&analytic) {
                if let Outline::Fixed(_) = layer.outline {
                    assert!(grad[..ALPHA].iter().all(|&g| g == 0.0), "{grad:?}");
                    assert_ne!(grad[ALPHA], 0.0);
                }
            }
        }
    }

    /// The edge model is the exact area of the pixel square on the inside
    /// of the edge's line, against a 1024 × 1024 supersampling.
    #[test]
    fn edge_coverage_is_the_pixel_area_of_the_half_plane() {
        let mut rng = ChaCha8Rng::seed_from_u64(9);
        for _ in 0..40 {
            let tri = Layer::triangle(
                std::array::from_fn(|_| rng.random_range(-20.0..20.0)),
                255.0,
                [0.0; 3],
            );
            let scene = blank_scene(64, 64);
            let prepared = Prepared::<f64>::new(&tri, &scene);
            let edge = prepared.edges[0];
            let d = rng.random_range(-0.8..0.8);
            // A pixel centre at signed distance `d`: the origin shifted
            // along the normal.
            let (cx, cy) = (
                d * edge.nx - edge.c * edge.nx,
                d * edge.ny - edge.c * edge.ny,
            );
            assert!((edge.distance(cx, cy) - d).abs() < 1e-9);
            let n = 1024;
            let inside = (0..n * n)
                .filter(|i| {
                    let sx = cx - 0.5 + ((i % n) as f64 + 0.5) / n as f64;
                    let sy = cy - 0.5 + ((i / n) as f64 + 0.5) / n as f64;
                    edge.distance(sx, sy) >= 0.0
                })
                .count();
            let area = inside as f64 / (n * n) as f64;
            let model = edge.cdf(d);
            assert!((model - area).abs() < 2e-3, "{model} vs {area} at {d}");
        }
    }

    /// A quadrilateral whose vertex at the centre of a pixel has an
    /// interior angle of 175°: its two edges nearly coincide there, each
    /// covering about half the pixel, and the wedge between them covers
    /// `1/2 − tan(2.5°) / 4` of it, about 0.489, in either orientation.
    #[test]
    fn a_vertex_of_175_degrees_covers_about_half_its_pixel() {
        let scene = blank_scene(32, 32);
        let t = 2.5_f64.to_radians().tan();
        let v = [
            10.0,
            10.0,
            16.0,
            10.0 + 6.0 * t,
            10.0,
            20.0,
            4.0,
            10.0 + 6.0 * t,
        ];
        let reversed = [v[6], v[7], v[4], v[5], v[2], v[3], v[0], v[1]];
        let expected = 0.5 - t / 4.0;
        for params in [v, reversed] {
            let layer = Layer {
                outline: Outline::Quad,
                params,
                alpha: 255.0,
                color: [255.0; 3],
            };
            let prepared = Prepared::<f64>::new(&layer, &scene);
            let cover = prepared.coverage(10.0, 10.0);
            assert!((cover - expected).abs() < 1e-12, "{cover} vs {expected}");
            let (value, _) = prepared
                .edge_coverage_grad::<4>(10.0, 10.0)
                .expect("covered");
            assert!((value - expected).abs() < 1e-12, "{value} vs {expected}");
        }
    }

    /// A triangle whose vertex at the centre of a pixel has an angle of
    /// 20°, its bisector along `x`: the wedge `|y − 10| ≤ (x − 10) ·
    /// tan(10°)` for `10 ≤ x ≤ 10.5` stays inside the pixel, so it covers
    /// `∫₀^½ 2 u tan(10°) du = tan(10°) / 4`, about 0.044, in either
    /// orientation, where each edge alone covers half the pixel.
    #[test]
    fn a_vertex_of_20_degrees_covers_its_wedge() {
        let scene = blank_scene(32, 32);
        let t = 10.0_f64.to_radians().tan();
        let v = [10.0, 10.0, 16.0, 10.0 - 6.0 * t, 16.0, 10.0 + 6.0 * t];
        let reversed = [v[4], v[5], v[2], v[3], v[0], v[1]];
        let expected = t / 4.0;
        for v in [v, reversed] {
            let layer = Layer::triangle(v, 255.0, [255.0; 3]);
            let prepared = Prepared::<f64>::new(&layer, &scene);
            let cover = prepared.coverage(10.0, 10.0);
            assert!((cover - expected).abs() < 1e-12, "{cover} vs {expected}");
            let (value, _) = prepared
                .edge_coverage_grad::<3>(10.0, 10.0)
                .expect("covered");
            assert!((value - expected).abs() < 1e-12, "{value} vs {expected}");
        }
    }

    /// The area of the part of the convex polygon `v` (engine coordinates,
    /// `x0, y0, x1, y1, …`) inside the pixel square centred at `(x, y)`:
    /// the polygon clipped to the square's four sides (Sutherland–Hodgman),
    /// then the shoelace formula.
    fn exact_coverage(v: &[f64], x: f64, y: f64) -> f64 {
        let mut polygon: Vec<(f64, f64)> =
            v.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
        // Each side as `inside(p) = s · (p[axis] − bound) ≥ 0`.
        for (axis, bound, s) in [
            (0, x - 0.5, 1.0),
            (0, x + 0.5, -1.0),
            (1, y - 0.5, 1.0),
            (1, y + 0.5, -1.0),
        ] {
            let side = |p: (f64, f64)| s * (if axis == 0 { p.0 } else { p.1 } - bound);
            let mut clipped = Vec::with_capacity(polygon.len() + 1);
            for (i, &p) in polygon.iter().enumerate() {
                let q = polygon[(i + 1) % polygon.len()];
                let (sp, sq) = (side(p), side(q));
                if sp >= 0.0 {
                    clipped.push(p);
                }
                if (sp >= 0.0) != (sq >= 0.0) {
                    let t = sp / (sp - sq);
                    clipped.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
                }
            }
            polygon = clipped;
            if polygon.is_empty() {
                return 0.0;
            }
        }
        let twice: f64 = (0..polygon.len())
            .map(|i| {
                let (p, q) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                p.0 * q.1 - q.0 * p.1
            })
            .sum();
        twice.abs() / 2.0
    }

    /// The forward model's coverage of `layer` against the exact area of
    /// the polygon inside each pixel of a `SIZE × SIZE` canvas, computed
    /// independently by clipping the polygon to the pixel
    /// ([`exact_coverage`]): every pixel within `1e-9`, inside the layer's
    /// bounding box wherever it is covered, and the same coverage from the
    /// gradient's path. Returns the model's and the exact total area.
    fn compare_coverage(layer: &Layer) -> (f64, f64) {
        const SIZE: usize = 64;
        let scene = blank_scene(SIZE, SIZE);
        let corners = layer.corners();
        let v = &corners[..2 * layer.outline.sides()];
        let prepared = Prepared::<f64>::new(layer, &scene);
        let (mut model_area, mut exact_area) = (0.0, 0.0);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (fx, fy) = (x as f64, y as f64);
                let exact = exact_coverage(v, fx, fy);
                let in_box = (prepared.x0..prepared.x1).contains(&x)
                    && (prepared.y0..prepared.y1).contains(&y);
                let model = prepared.coverage(fx, fy);
                assert!(in_box || exact == 0.0, "{v:?}: ({x}, {y}) outside the box");
                assert!(
                    (model - exact).abs() < 1e-9,
                    "{v:?} at ({x}, {y}): {model} vs {exact}"
                );
                let with_grad = match layer.outline.sides() {
                    3 => prepared.edge_coverage_grad::<3>(fx, fy),
                    _ => prepared.edge_coverage_grad::<4>(fx, fy),
                };
                assert_eq!(with_grad.map_or(0.0, |(cover, _)| cover), model);
                model_area += model;
                exact_area += exact;
            }
        }
        assert!(
            (model_area - exact_area).abs() < 1e-9 * exact_area.max(1.0),
            "{v:?}: area {model_area} vs {exact_area}"
        );
        (model_area, exact_area)
    }

    /// Random triangles from 2 to 48 px are covered by their exact area in
    /// every pixel, those around their vertices included. (The product of
    /// the edges' half-planes, which the model used before, over-covered
    /// near acute vertices: a mean error of 0.035 per edge pixel and a
    /// median excess area of +45% at 2 px.)
    #[test]
    fn coverage_matches_the_exact_pixel_area_of_the_triangle() {
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        for size in [2.0, 6.0, 12.0, 24.0, 48.0_f64] {
            for _ in 0..20 {
                let centre = 32.0;
                let v: [f64; 6] = loop {
                    let v =
                        std::array::from_fn(|_| centre + rng.random_range(-size / 2.0..size / 2.0));
                    let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
                    if cross.abs() > size * size / 8.0 {
                        break v;
                    }
                };
                compare_coverage(&Layer::triangle(v, 255.0, [255.0; 3]));
            }
        }
    }

    /// The same for convex quadrilaterals.
    #[test]
    fn coverage_matches_the_exact_pixel_area_of_a_convex_polygon() {
        let mut rng = ChaCha8Rng::seed_from_u64(8);
        for size in [2.0, 6.0, 12.0, 24.0, 48.0_f64] {
            for _ in 0..20 {
                let layer = Layer {
                    outline: Outline::Quad,
                    params: random_convex(&mut rng, (32.0, 32.0), size / 2.0),
                    alpha: 255.0,
                    color: [255.0; 3],
                };
                compare_coverage(&layer);
            }
        }
    }

    /// An axis-aligned rectangle with sides of at least 1 px covers each
    /// pixel by exactly its area there.
    #[test]
    fn coverage_of_an_axis_aligned_rectangle_is_exact() {
        let mut rng = ChaCha8Rng::seed_from_u64(10);
        let scene = blank_scene(64, 64);
        let mut partial = 0;
        for layer in random_boxes(&mut rng, 200, 64.0) {
            let prepared = Prepared::<f64>::new(&layer, &scene);
            let corners = layer.corners();
            let mut total = 0.0;
            for y in 0..64 {
                for x in 0..64 {
                    let (fx, fy) = (x as f64, y as f64);
                    let exact = exact_coverage(&corners, fx, fy);
                    let in_box = (prepared.x0..prepared.x1).contains(&x)
                        && (prepared.y0..prepared.y1).contains(&y);
                    assert!(
                        in_box || exact == 0.0,
                        "{layer:?}: ({x}, {y}) outside the box"
                    );
                    let model = if in_box {
                        prepared.pixel(x, y, fy)
                    } else {
                        0.0
                    };
                    assert!(
                        (model - exact).abs() < 1e-12,
                        "{layer:?} at ({x}, {y}): {model} vs {exact}"
                    );
                    partial += usize::from(exact > 0.0 && exact < 1.0);
                    total += model;
                }
            }
            let [x0, y0, x1, y1, ..] = layer.params;
            let visible = (x1.min(63.5) - x0.max(-0.5)) * (y1.min(63.5) - y0.max(-0.5));
            assert!(
                (total - visible).abs() < 1e-9,
                "{layer:?}: {total} vs {visible}"
            );
        }
        assert!(partial > 5000, "{partial} partly covered pixels");
    }

    /// The same for rotated rectangles, whose exact area is `4 |u| h`.
    #[test]
    fn coverage_matches_the_exact_pixel_area_of_a_rotated_rectangle() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        for size in [2.0, 6.0, 12.0, 24.0, 48.0_f64] {
            for mut layer in random_rotated(&mut rng, 20, size) {
                // Centred within a pixel of the canvas's centre, inside it.
                layer.params[0] = 31.0 + 2.0 * layer.params[0] / size;
                layer.params[1] = 31.0 + 2.0 * layer.params[1] / size;
                let (_, exact_area) = compare_coverage(&layer);
                let [_, _, ux, uy, h, ..] = layer.params;
                let area = 4.0 * ux.hypot(uy) * h;
                assert!(
                    (exact_area - area).abs() < 1e-9 * area.max(1.0),
                    "{layer:?}"
                );
            }
        }
    }

    /// Every layer's colour after one Jacobi step from `tris`, computed
    /// pixel by pixel from [`Prepared::coverage`] alone: each layer moves
    /// against the residual of the stack as it is, with every other layer
    /// at its colour, to `s − ω Σ g R / D` clamped to `0..=255`, with
    /// `g = A · opacity · coverage`. `D` is `Σ g Σ_j g_j` (the sum over
    /// every layer `j` at the pixel) with `majorised`, and `Σ g²` (the
    /// closed-form fit of the layer alone) without.
    fn jacobi_colours(
        scene: &Scene<f64>,
        tris: &[Layer],
        majorised: bool,
        omega: f64,
    ) -> Vec<Layer> {
        let layers: Vec<Prepared<f64>> = tris.iter().map(|tri| Prepared::new(tri, scene)).collect();
        let mut sums = vec![[0.0; 4]; tris.len()];
        for y in 0..scene.height {
            for x in 0..scene.width {
                let fy = y as f64;
                // The model composites each layer inside its bounding box
                // only.
                let weights: Vec<f64> = layers
                    .iter()
                    .map(|layer| {
                        let inside =
                            (layer.x0..layer.x1).contains(&x) && (layer.y0..layer.y1).contains(&y);
                        if inside {
                            layer.opacity * layer.pixel(x, y, fy)
                        } else {
                            0.0
                        }
                    })
                    .collect();
                let mut canvas = scene.background;
                for (layer, w) in layers.iter().zip(&weights) {
                    for (value, color) in canvas.iter_mut().zip(layer.color) {
                        *value += w * (color - *value);
                    }
                }
                let p = 3 * (y * scene.width + x);
                let residual: [f64; 3] = std::array::from_fn(|c| canvas[c] - scene.target[p + c]);
                let mut g = vec![0.0; layers.len()];
                let mut above = 1.0;
                for (g, w) in g.iter_mut().zip(&weights).rev() {
                    *g = above * w;
                    above *= 1.0 - w;
                }
                let all: f64 = g.iter().sum();
                for (sum, g) in sums.iter_mut().zip(&g) {
                    for c in 0..3 {
                        sum[c] += g * residual[c];
                    }
                    sum[3] += g * if majorised { all } else { *g };
                }
            }
        }
        tris.iter()
            .zip(sums)
            .map(|(tri, sum)| Layer {
                color: std::array::from_fn(|c| {
                    let old = tri.color[c];
                    if sum[3] > 0.0 {
                        (old - omega * sum[c] / sum[3]).clamp(0.0, 255.0)
                    } else {
                        old
                    }
                }),
                ..*tri
            })
            .collect()
    }

    /// A fit refits every colour together from the residual of the stack
    /// as it is (one Jacobi step), with the majorised weights, fixed layers
    /// included. The scene's 40 rows are not a multiple of the band height.
    #[test]
    fn colours_are_refitted_together_from_the_same_residual() {
        let mut rng = ChaCha8Rng::seed_from_u64(21);
        let (scene, tris) = mixed_scene(&mut rng, 48, 40, (4, 3, 3));
        let expected = jacobi_colours(&scene, &tris, true, OVER_RELAXATION);
        let mut fitted = tris.clone();
        fit(&scene, &mut fitted, &mut Workspace::default());
        for (index, (tri, expected)) in fitted.iter().zip(&expected).enumerate() {
            for c in 0..3 {
                assert!(
                    (tri.color[c] - expected.color[c]).abs() <= 1e-9,
                    "layer {index}: {:?} vs {:?}",
                    tri.color,
                    expected.color
                );
            }
        }
        assert_ne!(fitted, tris);
        assert_eq!(
            fitted
                .iter()
                .map(|tri| (tri.params, tri.alpha))
                .collect::<Vec<_>>(),
            tris.iter()
                .map(|tri| (tri.params, tri.alpha))
                .collect::<Vec<_>>()
        );
    }

    /// On a deep stack of large translucent layers, where a plain Jacobi
    /// step raises the loss, every fit lowers it.
    #[test]
    fn colour_fits_lower_the_loss_where_plain_jacobi_raises_it() {
        let mut rng = ChaCha8Rng::seed_from_u64(23);
        let scene = random_scene::<f64>(&mut rng, 48, 40);
        let mut tris: Vec<Layer> = (0..30)
            .map(|_| {
                let vertices = std::array::from_fn(|k| {
                    let size = if k % 2 == 0 { 48.0 } else { 40.0 };
                    rng.random_range(-0.5 * size..1.5 * size)
                });
                Layer::triangle(vertices, rng.random_range(60.0..200.0), [128.0; 3])
            })
            .collect();
        let start = fresh_loss(&scene, &tris);
        let plain = fresh_loss(&scene, &jacobi_colours(&scene, &tris, false, 1.0));
        assert!(plain > start, "plain Jacobi: {start} -> {plain}");
        let mut work = Workspace::default();
        let mut previous = start;
        for _ in 0..5 {
            fit(&scene, &mut tris, &mut work);
            let fresh = fresh_loss(&scene, &tris);
            assert!(fresh <= previous * (1.0 + 1e-12), "{fresh} vs {previous}");
            previous = fresh;
        }
        assert!(previous < start, "{start} -> {previous}");
    }

    /// `f32` compositing agrees with `f64`.
    #[test]
    fn single_precision_agrees_with_double() {
        let mut rng = ChaCha8Rng::seed_from_u64(21);
        let (scene, mut tris) = mixed_scene(&mut rng, 48, 40, (5, 4, 3));
        fit(&scene, &mut tris, &mut Workspace::default());
        let single = to_single(&scene);
        let wide = gradients(&scene, &tris, &mut Workspace::default());
        let narrow = gradients(&single, &tris, &mut Workspace::default());
        let (wide_loss, narrow_loss) = (
            fresh_loss(&scene, &tris),
            loss(&single, &tris, &mut Workspace::default()),
        );
        assert!((wide_loss - narrow_loss).abs() < 1e-5 * wide_loss);
        for (a, b) in wide.iter().flatten().zip(narrow.iter().flatten()) {
            assert!((a - b).abs() < 1e-3 * a.abs().max(1e3), "{a} vs {b}");
        }
    }

    fn to_single(scene: &Scene<f64>) -> Scene<f32> {
        Scene {
            width: scene.width,
            height: scene.height,
            target: scene.target.iter().map(|&v| v as f32).collect(),
            background: scene.background.map(|v| v as f32),
            filter: scene.filter,
            masks: scene
                .masks
                .iter()
                .map(|mask| Mask {
                    x0: mask.x0,
                    x1: mask.x1,
                    y0: mask.y0,
                    y1: mask.y1,
                    coverage: mask.coverage.iter().map(|&v| v as f32).collect(),
                })
                .collect(),
        }
    }

    /// The loss, the gradients and the fit do not depend on the number of
    /// threads.
    #[test]
    fn passes_do_not_depend_on_the_thread_count() {
        let run = |threads: usize| {
            let mut rng = ChaCha8Rng::seed_from_u64(33);
            let (scene, mut tris) = mixed_scene(&mut rng, 120, 100, (3, 3, 3));
            let scene = to_single(&scene);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            pool.install(|| {
                let mut work = Workspace::default();
                let gradients = gradients(&scene, &tris, &mut work);
                fit(&scene, &mut tris, &mut work);
                let loss = loss(&scene, &tris, &mut work);
                (loss, gradients, tris)
            })
        };
        let one = run(1);
        assert!(one.1.iter().any(|g| g.iter().any(|&v| v != 0.0)));
        for threads in [2, 4, 8] {
            let other = run(threads);
            assert_eq!(one.0.to_bits(), other.0.to_bits(), "{threads} threads");
            assert_eq!(one.1, other.1, "{threads} threads");
            assert_eq!(one.2, other.2, "{threads} threads");
        }
    }
}
