//! Differentiable compositing of triangles: the smooth forward model,
//! reverse-mode gradients through the layer stack and the per-shape colour
//! fit. The arithmetic rule of [`super`] applies to everything here.
//!
//! Coordinates are the engine's: the centre of pixel `(i, j)` is `(i, j)`,
//! so a vertex `v` is the drawing's `v + 0.5`.

use crate::refine::checkpoint_interval;
use rayon::prelude::*;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

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

/// Gradient entries per triangle: the six vertex coordinates, then alpha.
pub(super) const PARAMS: usize = 7;

/// One triangle's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tri {
    /// `x1, y1, x2, y2, x3, y3` in engine coordinates.
    pub(super) vertices: [f64; 6],
    /// Opacity, `1..=255`.
    pub(super) alpha: f64,
    /// RGB, `0..=255`.
    pub(super) color: [f64; 3],
}

/// The target and the background a stack of triangles is composited on.
pub(super) struct Scene<F> {
    pub(super) width: usize,
    pub(super) height: usize,
    /// RGB per pixel, row-major.
    pub(super) target: Vec<F>,
    pub(super) background: [F; 3],
    /// Width in pixels of the box filter the coverage is convolved with:
    /// `1` is exact pixel-area coverage of each edge's half-plane, and the
    /// only width production uses.
    pub(super) filter: f64,
}

/// One edge's box-filtered half-plane, `F(d)` with `d` the signed distance
/// of a pixel centre from the edge, positive inside.
#[derive(Clone, Copy)]
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

/// A triangle ready to composite: its edges, its bounding box on the
/// canvas (`x0..x1`, `y0..y1`, empty if it cannot cover any pixel), its
/// opacity as a fraction and its colour.
#[derive(Clone, Copy)]
pub(super) struct Prepared<F> {
    edges: [Edge<F>; 3],
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    opacity: F,
    color: [F; 3],
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

/// `1` for a non-negative `value`, `−1` for a negative one: the sign the
/// derivative of `|value|` takes, `+1` at zero.
#[inline]
fn sign(value: f64) -> f64 {
    if value < 0.0 { -1.0 } else { 1.0 }
}

impl<F: Real> Prepared<F> {
    pub(super) fn new(tri: &Tri, filter: f64, width: usize, height: usize) -> Self {
        let v = tri.vertices;
        let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
        let sigma = if cross >= 0.0 { 1.0 } else { -1.0 };
        let mut visible = cross.abs() > 1e-9;
        let edges = std::array::from_fn(|e| {
            let (p, q) = (e, (e + 1) % 3);
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
        let xs = [v[0], v[2], v[4]];
        let ys = [v[1], v[3], v[5]];
        let span = |values: [f64; 3], size: usize| {
            let low = values[0].min(values[1]).min(values[2]) - reach;
            let high = values[0].max(values[1]).max(values[2]) + reach;
            let start = low.ceil().max(0.0);
            let end = (high.floor() + 1.0).min(size as f64);
            if visible && start < end {
                (start as usize, end as usize)
            } else {
                (0, 0)
            }
        };
        let (x0, x1) = span(xs, width);
        let (y0, y1) = span(ys, height);
        let (x0, x1, y0, y1) = if x0 < x1 && y0 < y1 {
            (x0, x1, y0, y1)
        } else {
            (0, 0, 0, 0)
        };
        Self {
            edges,
            x0,
            x1,
            y0,
            y1,
            opacity: F::of(tri.alpha / 255.0),
            color: tri.color.map(F::of),
        }
    }

    fn area(&self) -> usize {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }

    /// The coverage of the pixel centred at `(x, y)`: the product of the
    /// edges' box-filtered half-planes.
    #[inline]
    pub(super) fn coverage(&self, x: F, y: F) -> F {
        let mut product = F::of(1.0);
        for edge in &self.edges {
            let f = edge.cdf(edge.distance(x, y));
            if f <= F::of(0.0) {
                return F::of(0.0);
            }
            product = product * f;
        }
        product
    }

    /// The coverage and its gradient with respect to the six vertex
    /// coordinates, or `None` where the coverage is zero.
    #[inline]
    fn coverage_grad(&self, x: F, y: F) -> Option<(F, [F; 6])> {
        let d = self.edges.map(|edge| edge.distance(x, y));
        let zero = F::of(0.0);
        if (0..3).any(|e| d[e] <= -self.edges[e].half_sum) {
            return None;
        }
        if (0..3).all(|e| d[e] >= self.edges[e].half_sum) {
            return Some((F::of(1.0), [zero; 6]));
        }
        let values: [EdgeValue<F>; 3] = std::array::from_fn(|e| self.edges[e].cdf_grad(d[e]));
        let f = values.map(|value| value.f);
        let others = [f[1] * f[2], f[0] * f[2], f[0] * f[1]];
        let mut grad = [zero; 6];
        for e in 0..3 {
            if values[e].fd == zero && values[e].fa == zero && values[e].fb == zero {
                continue;
            }
            let g = self.edges[e].endpoint_grad(values[e], x, y, d[e]);
            let (p, q) = (e, (e + 1) % 3);
            grad[2 * p] += others[e] * g[0];
            grad[2 * p + 1] += others[e] * g[1];
            grad[2 * q] += others[e] * g[2];
            grad[2 * q + 1] += others[e] * g[3];
        }
        Some((f[0] * others[0], grad))
    }
}

/// Rows per parallel task. Tasks and their partial sums do not depend on
/// the thread count, and the sums are reduced in task order, so every
/// result is the same whatever the number of threads.
const ROWS: usize = 4;
/// Bounding boxes smaller than this many pixels run on the calling thread.
const PARALLEL_PIXELS: usize = 4096;

/// Composites `layer` onto `canvas`.
fn paint<F: Real>(canvas: &mut [F], width: usize, layer: &Prepared<F>) {
    if layer.area() == 0 {
        return;
    }
    let row = 3 * width;
    let rows = &mut canvas[layer.y0 * row..layer.y1 * row];
    let task = |(index, chunk): (usize, &mut [F])| {
        for (local, line) in chunk.chunks_exact_mut(row).enumerate() {
            let y = F::of((layer.y0 + index * ROWS + local) as f64);
            for x in layer.x0..layer.x1 {
                let cov = layer.coverage(F::of(x as f64), y);
                if cov > F::of(0.0) {
                    let w = layer.opacity * cov;
                    for c in 0..3 {
                        let value = &mut line[3 * x + c];
                        *value += w * (layer.color[c] - *value);
                    }
                }
            }
        }
    };
    if layer.area() >= PARALLEL_PIXELS {
        rows.par_chunks_mut(ROWS * row).enumerate().for_each(task);
    } else {
        rows.chunks_mut(ROWS * row).enumerate().for_each(task);
    }
}

/// Reusable buffers of [`sweep`] and [`loss`]: the canvas, the
/// checkpoints, the canvas below each layer of one segment within its
/// bounding box, the residual and the transmittance from above.
#[derive(Default)]
pub(super) struct Workspace<F> {
    canvas: Vec<F>,
    checkpoints: Vec<Vec<F>>,
    below: Vec<Vec<F>>,
    residual: Vec<F>,
    transmittance: Vec<F>,
}

/// Composites `layers` from the background into `work.canvas`, keeping
/// the canvas before every `interval`-th layer in `work.checkpoints` (none
/// with `interval` 0), then sets `work.residual` to `X − T`.
fn forward<F: Real>(
    scene: &Scene<F>,
    layers: &[Prepared<F>],
    interval: usize,
    work: &mut Workspace<F>,
) {
    let pixels = scene.width * scene.height;
    work.canvas.clear();
    work.canvas
        .extend((0..pixels).flat_map(|_| scene.background));
    if interval > 0 {
        work.checkpoints
            .resize_with(layers.len().div_ceil(interval), Vec::new);
    }
    for (index, layer) in layers.iter().enumerate() {
        if interval > 0 && index % interval == 0 {
            let checkpoint = &mut work.checkpoints[index / interval];
            checkpoint.clear();
            checkpoint.extend_from_slice(&work.canvas);
        }
        paint(&mut work.canvas, scene.width, layer);
    }
    work.residual.clear();
    work.residual
        .extend(work.canvas.iter().zip(&scene.target).map(|(&x, &t)| x - t));
}

/// `Σ r²` over a residual, in `f64`, in order.
fn sum_of_squares<F: Real>(residual: &[F]) -> f64 {
    residual.iter().map(|r| r.get() * r.get()).sum()
}

/// `Σ (X − T)²` over every pixel and channel of the composite of `tris`.
pub(super) fn loss<F: Real>(scene: &Scene<F>, tris: &[Tri], work: &mut Workspace<F>) -> f64 {
    let layers: Vec<Prepared<F>> = tris
        .iter()
        .map(|tri| Prepared::new(tri, scene.filter, scene.width, scene.height))
        .collect();
    forward(scene, &layers, 0, work);
    sum_of_squares(&work.residual)
}

/// One forward pass and one top-down reverse pass over `tris`; returns the
/// gradient of the loss `Σ (X − T)²` per triangle, empty without
/// `gradient`.
///
/// With `fit`, each layer's colour is first replaced, on the way down, by
/// its closed-form least-squares fit with every other layer fixed (one
/// Gauss–Seidel sweep, top layer first), and the residual is updated in
/// place. With `gradient`, the gradient of each layer is then taken with
/// the colours of the layers at and above it already refitted, those below
/// not yet: the gradient of the loss at that point, colours held constant.
///
/// The reverse pass needs, at layer `i`, the transmittance `A_i` of the
/// layers above (`final = A_i · X_i + B_i`), folded in as it goes down,
/// the final residual `R`, and the canvas `X_{i−1}` below the layer. The
/// latter comes from canvas checkpoints every `interval` layers
/// ([`checkpoint_interval`]: `⌈√N⌉` unless memory caps them), replayed one
/// segment at a time and kept only inside each layer's bounding box.
pub(super) fn sweep<F: Real>(
    scene: &Scene<F>,
    tris: &mut [Tri],
    fit: bool,
    gradient: bool,
    work: &mut Workspace<F>,
) -> Vec<[f64; PARAMS]> {
    let width = scene.width;
    let pixels = width * scene.height;
    let n = tris.len();
    let mut layers: Vec<Prepared<F>> = tris
        .iter()
        .map(|tri| Prepared::new(tri, scene.filter, width, scene.height))
        .collect();
    let interval = checkpoint_interval(n, 3 * pixels * size_of::<F>());
    let segments = n.div_ceil(interval);
    forward(scene, &layers, interval, work);

    work.transmittance.clear();
    work.transmittance.resize(pixels, F::of(1.0));
    work.below.resize_with(interval, Vec::new);
    let mut gradients = vec![[0.0; PARAMS]; if gradient { n } else { 0 }];
    for segment in (0..segments).rev() {
        let start = segment * interval;
        let end = (start + interval).min(n);
        // Replay the segment, keeping the canvas below each layer inside
        // its bounding box.
        work.canvas.copy_from_slice(&work.checkpoints[segment]);
        for (offset, layer) in layers[start..end].iter().enumerate() {
            let below = &mut work.below[offset];
            below.clear();
            for y in layer.y0..layer.y1 {
                let row = 3 * (y * width);
                below.extend_from_slice(&work.canvas[row + 3 * layer.x0..row + 3 * layer.x1]);
            }
            paint(&mut work.canvas, width, layer);
        }
        for index in (start..end).rev() {
            let layer = &mut layers[index];
            if layer.area() == 0 {
                continue;
            }
            let delta = if fit {
                fit_color(layer, &work.residual, &work.transmittance, width)
            } else {
                None
            };
            if let Some(delta) = delta {
                let tri = &mut tris[index];
                for ((color, prepared), change) in
                    tri.color.iter_mut().zip(&mut layer.color).zip(delta)
                {
                    *color += change;
                    *prepared = F::of(*color);
                }
            }
            descend(
                layer,
                &work.below[index - start],
                &mut work.residual,
                &mut work.transmittance,
                width,
                delta,
                gradient.then(|| &mut gradients[index]),
            );
        }
    }
    gradients
}

/// The change of `layer`'s colour that minimises the loss with every
/// other layer fixed, clamped to `0..=255`: with `g = A · w`,
/// `Δs = −Σ g · R / Σ g²` per channel. `None` if the layer has no weight.
fn fit_color<F: Real>(
    layer: &Prepared<F>,
    residual: &[F],
    transmittance: &[F],
    width: usize,
) -> Option<[f64; 3]> {
    let row = 3 * width;
    let task = |(index, (r, a)): (usize, (&[F], &[F]))| {
        let mut sums = [0.0_f64; 4];
        for local in 0..a.len() / width {
            let y = layer.y0 + index * ROWS + local;
            for x in layer.x0..layer.x1 {
                let cov = layer.coverage(F::of(x as f64), F::of(y as f64));
                if cov > F::of(0.0) {
                    let g = (a[local * width + x] * layer.opacity * cov).get();
                    for c in 0..3 {
                        sums[c] += g * r[local * row + 3 * x + c].get();
                    }
                    sums[3] += g * g;
                }
            }
        }
        sums
    };
    let r = &residual[layer.y0 * row..layer.y1 * row];
    let a = &transmittance[layer.y0 * width..layer.y1 * width];
    let partials: Vec<[f64; 4]> = if layer.area() >= PARALLEL_PIXELS {
        r.par_chunks(ROWS * row)
            .zip(a.par_chunks(ROWS * width))
            .enumerate()
            .map(task)
            .collect()
    } else {
        r.chunks(ROWS * row)
            .zip(a.chunks(ROWS * width))
            .enumerate()
            .map(task)
            .collect()
    };
    let mut sums = [0.0; 4];
    for partial in partials {
        for (sum, value) in sums.iter_mut().zip(partial) {
            *sum += value;
        }
    }
    (sums[3] > 0.0).then(|| {
        std::array::from_fn(|c| {
            let old = layer.color[c].get();
            (old - sums[c] / sums[3]).clamp(0.0, 255.0) - old
        })
    })
}

/// One task of [`descend`]: its index, and its rows of the residual, the
/// transmittance and the canvas below.
type DescendTask<'a, F> = (usize, ((&'a mut [F], &'a mut [F]), &'a [F]));

/// The reverse step through `layer`: applies the colour change `delta` to
/// the residual (`R += g · Δs`), accumulates the gradient
/// `∂L/∂w = 2 A Σ_c R_c (s_c − X_{i−1,c})` into the vertices and alpha, and
/// folds the layer into the transmittance (`A ← A · (1 − w)`).
fn descend<F: Real>(
    layer: &Prepared<F>,
    below: &[F],
    residual: &mut [F],
    transmittance: &mut [F],
    width: usize,
    delta: Option<[f64; 3]>,
    gradient: Option<&mut [f64; PARAMS]>,
) {
    let row = 3 * width;
    let box_width = layer.x1 - layer.x0;
    let want_gradient = gradient.is_some();
    let delta = delta.map(|delta| delta.map(F::of));
    let task = |(index, ((r, a), below)): DescendTask<'_, F>| {
        let mut sums = [0.0_f64; PARAMS];
        for local in 0..a.len() / width {
            let y = F::of((layer.y0 + index * ROWS + local) as f64);
            for x in layer.x0..layer.x1 {
                let fx = F::of(x as f64);
                let (cov, cov_grad) = if want_gradient {
                    match layer.coverage_grad(fx, y) {
                        Some(value) => value,
                        None => continue,
                    }
                } else {
                    let cov = layer.coverage(fx, y);
                    if cov <= F::of(0.0) {
                        continue;
                    }
                    (cov, [F::of(0.0); 6])
                };
                let p = local * width + x;
                let transmitted = a[p];
                let w = layer.opacity * cov;
                let pixel = &mut r[local * row + 3 * x..local * row + 3 * x + 3];
                if let Some(delta) = delta {
                    let g = transmitted * w;
                    for c in 0..3 {
                        pixel[c] += g * delta[c];
                    }
                }
                if want_gradient {
                    let x_below = &below[3 * (local * box_width + x - layer.x0)..][..3];
                    let mut dot = F::of(0.0);
                    for c in 0..3 {
                        dot += pixel[c] * (layer.color[c] - x_below[c]);
                    }
                    let dl_dw = (F::of(2.0) * transmitted * dot).get();
                    if dl_dw != 0.0 {
                        let opacity = layer.opacity.get();
                        for (sum, d) in sums.iter_mut().zip(cov_grad) {
                            *sum += dl_dw * opacity * d.get();
                        }
                        sums[6] += dl_dw * cov.get() / 255.0;
                    }
                }
                a[p] = transmitted * (F::of(1.0) - w);
            }
        }
        sums
    };
    let r = &mut residual[layer.y0 * row..layer.y1 * row];
    let a = &mut transmittance[layer.y0 * width..layer.y1 * width];
    let partials: Vec<[f64; PARAMS]> = if layer.area() >= PARALLEL_PIXELS {
        r.par_chunks_mut(ROWS * row)
            .zip(a.par_chunks_mut(ROWS * width))
            .zip(below.par_chunks(ROWS * 3 * box_width))
            .enumerate()
            .map(task)
            .collect()
    } else {
        r.chunks_mut(ROWS * row)
            .zip(a.chunks_mut(ROWS * width))
            .zip(below.chunks(ROWS * 3 * box_width))
            .enumerate()
            .map(task)
            .collect()
    };
    if let Some(gradient) = gradient {
        for partial in partials {
            for (sum, value) in gradient.iter_mut().zip(partial) {
                *sum += value;
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
        }
    }

    fn random_tris(rng: &mut ChaCha8Rng, count: usize, size: f64) -> Vec<Tri> {
        (0..count)
            .map(|_| Tri {
                vertices: std::array::from_fn(|_| rng.random_range(-3.0..size + 3.0)),
                alpha: rng.random_range(40.0..250.0),
                color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
            })
            .collect()
    }

    fn fresh_loss(scene: &Scene<f64>, tris: &[Tri]) -> f64 {
        loss(scene, tris, &mut Workspace::default())
    }

    /// The analytic gradients of vertices and alphas against central
    /// differences, on 32 × 32 random targets with random triangles, at
    /// the export's filter width and at wider ones. Returns the largest
    /// relative error over the components whose magnitude is at least 1%
    /// of the largest one, the overall relative error, and how many
    /// components were checked.
    fn gradient_check(seed: u64, count: usize, filter: f64) -> (f64, f64, usize) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut scene = random_scene::<f64>(&mut rng, 32, 32);
        scene.filter = filter;
        let tris = random_tris(&mut rng, count, 32.0);
        let analytic = sweep(
            &scene,
            &mut tris.clone(),
            false,
            true,
            &mut Workspace::default(),
        );
        let mut numeric = vec![[0.0; PARAMS]; count];
        for (index, grads) in numeric.iter_mut().enumerate() {
            for (k, grad) in grads.iter_mut().enumerate() {
                let h = if k == 6 { 1e-3 } else { 1e-5 };
                let shifted = |sign: f64| {
                    let mut moved = tris.clone();
                    if k == 6 {
                        moved[index].alpha += sign * h;
                    } else {
                        moved[index].vertices[k] += sign * h;
                    }
                    fresh_loss(&scene, &moved)
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

    #[test]
    fn analytic_gradients_match_finite_differences() {
        for (seed, count, filter) in [
            (1, 3, 1.0),
            (2, 5, 1.0),
            (3, 7, 1.0),
            (4, 5, 1.6),
            (5, 7, 2.0),
        ] {
            let (worst, overall, checked) = gradient_check(seed, count, filter);
            assert!(checked >= count * 4, "seed {seed}: {checked} checked");
            assert!(worst < 1e-3, "seed {seed}: worst {worst}");
            assert!(overall < 1e-4, "seed {seed}: overall {overall}");
        }
    }

    /// The edge model is the exact area of the pixel square on the inside
    /// of the edge's line, against a 1024 × 1024 supersampling.
    #[test]
    fn edge_coverage_is_the_pixel_area_of_the_half_plane() {
        let mut rng = ChaCha8Rng::seed_from_u64(9);
        for _ in 0..40 {
            let tri = Tri {
                vertices: std::array::from_fn(|_| rng.random_range(-20.0..20.0)),
                alpha: 255.0,
                color: [0.0; 3],
            };
            let prepared = Prepared::<f64>::new(&tri, 1.0, 64, 64);
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

    /// The area of the part of the triangle `v` (engine coordinates)
    /// inside the pixel square centred at `(x, y)`: the triangle clipped
    /// to the square's four sides (Sutherland–Hodgman), then the shoelace
    /// formula.
    fn exact_coverage(v: &[f64; 6], x: f64, y: f64) -> f64 {
        let mut polygon: Vec<(f64, f64)> = (0..3).map(|k| (v[2 * k], v[2 * k + 1])).collect();
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

    /// The areas of the pixel square centred at `(x, y)` on the inside of
    /// each of the triangle's three edge lines, by the same clipping: the
    /// pixel clipped to one half-plane at a time.
    fn edge_areas(v: &[f64; 6], x: f64, y: f64) -> [f64; 3] {
        let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
        let sigma = if cross < 0.0 { -1.0 } else { 1.0 };
        std::array::from_fn(|e| {
            let (p, q) = (e, (e + 1) % 3);
            let (px, py) = (v[2 * p], v[2 * p + 1]);
            let (ex, ey) = (v[2 * q] - px, v[2 * q + 1] - py);
            // A triangle far larger than the pixel with this edge: the
            // edge's half-plane, as far as the pixel can tell.
            let far = 1e4 / ex.hypot(ey);
            let (ox, oy) = (-sigma * ey * far, sigma * ex * far);
            let big = [
                px - far * ex,
                py - far * ey,
                px + (1.0 + far) * ex,
                py + (1.0 + far) * ey,
                px + 0.5 * ex + ox,
                py + 0.5 * ey + oy,
            ];
            exact_coverage(&big, x, y)
        })
    }

    /// The forward model's coverage against the exact area of the triangle
    /// inside each pixel, computed independently by clipping.
    ///
    /// The model multiplies the three edges' exact half-plane areas, which
    /// is the exact area wherever at most one edge crosses the pixel. Where
    /// two or three cross (near a vertex) the product is not the area of
    /// the intersection, but both lie within the Fréchet bounds of three
    /// sets of areas `a, b, c` in a unit square,
    /// `max(0, a + b + c − 2) ≤ · ≤ min(a, b, c)`. So every pixel is held
    /// to that interval's width, which is zero on single-edge pixels, plus
    /// `1e-6` (the model treats a filter projection narrower than `1e-6`
    /// as zero).
    ///
    /// Near vertices the product over-covers, which matters only for small
    /// triangles: measured on these seeds, the mean error over edge pixels
    /// falls from 0.035 at 2 px to 0.003 at 48 px, and the excess area from
    /// a median of +45% at 2 px to at most +0.9% from 24 px. The bounds
    /// below sit above those with some margin: 0.05 per edge pixel at every
    /// size, the top of the 0.02–0.05 the pilot measured against
    /// tiny-skia's anti-aliasing, and from 24 px 0.006 and 1.5% of the
    /// area.
    #[test]
    fn coverage_matches_the_exact_pixel_area_of_the_triangle() {
        const SIZE: usize = 64;
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        for size in [2.0, 6.0, 12.0, 24.0, 48.0] {
            let (mut error_sum, mut edge_pixels) = (0.0, 0);
            for _ in 0..20 {
                let centre = SIZE as f64 / 2.0;
                let v: [f64; 6] = loop {
                    let v =
                        std::array::from_fn(|_| centre + rng.random_range(-size / 2.0..size / 2.0));
                    let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
                    if cross.abs() > size * size / 8.0 {
                        break v;
                    }
                };
                let tri = Tri {
                    vertices: v,
                    alpha: 255.0,
                    color: [255.0; 3],
                };
                let prepared = Prepared::<f64>::new(&tri, 1.0, SIZE, SIZE);
                let (mut model_area, mut exact_area) = (0.0, 0.0);
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        let (fx, fy) = (x as f64, y as f64);
                        let exact = exact_coverage(&v, fx, fy);
                        let in_box = (prepared.x0..prepared.x1).contains(&x)
                            && (prepared.y0..prepared.y1).contains(&y);
                        let model = prepared.coverage(fx, fy);
                        assert!(in_box || exact == 0.0, "{v:?}: ({x}, {y}) outside the box");
                        let [a, b, c] = edge_areas(&v, fx, fy);
                        let width = a.min(b).min(c) - (a + b + c - 2.0).max(0.0);
                        let error = (model - exact).abs();
                        assert!(
                            error <= width + 1e-6,
                            "{v:?} at ({x}, {y}): {model} vs {exact}, bound {width}"
                        );
                        model_area += model;
                        exact_area += exact;
                        if (exact > 0.0 && exact < 1.0) || (model > 0.0 && model < 1.0) {
                            error_sum += error;
                            edge_pixels += 1;
                        }
                    }
                }
                if size >= 24.0 {
                    let relative = model_area / exact_area - 1.0;
                    assert!(relative.abs() < 0.015, "{v:?}: area {relative:+.4}");
                }
            }
            let mean = error_sum / f64::from(edge_pixels);
            let bound = if size >= 24.0 { 0.006 } else { 0.05 };
            assert!(mean < bound, "size {size}: mean edge error {mean}");
        }
    }

    /// The residual the colour fit updates in place is the residual of
    /// the refitted stack, the fit never raises the loss, and `f32`
    /// compositing agrees with `f64`.
    #[test]
    fn colour_fit_updates_the_residual_exactly_and_lowers_the_loss() {
        let mut rng = ChaCha8Rng::seed_from_u64(21);
        let scene = random_scene::<f64>(&mut rng, 48, 40);
        let mut tris = random_tris(&mut rng, 9, 48.0);
        let mut work = Workspace::default();
        let mut previous = fresh_loss(&scene, &tris);
        for _ in 0..3 {
            sweep(&scene, &mut tris, true, true, &mut work);
            let fitted = work.residual.clone();
            let fresh = fresh_loss(&scene, &tris);
            assert!(fresh <= previous * (1.0 + 1e-12), "{fresh} vs {previous}");
            let mut fresh_work = Workspace::default();
            loss(&scene, &tris, &mut fresh_work);
            for (a, b) in fitted.iter().zip(&fresh_work.residual) {
                assert!((a - b).abs() <= 1e-9, "{a} vs {b}");
            }
            previous = fresh;
        }
        let single = Scene::<f32> {
            width: scene.width,
            height: scene.height,
            target: scene.target.iter().map(|&v| v as f32).collect(),
            background: scene.background.map(|v| v as f32),
            filter: 1.0,
        };
        let wide = sweep(&scene, &mut tris.clone(), false, true, &mut work);
        let narrow = sweep(
            &single,
            &mut tris.clone(),
            false,
            true,
            &mut Workspace::default(),
        );
        let (wide_loss, narrow_loss) = (
            fresh_loss(&scene, &tris),
            loss(&single, &tris, &mut Workspace::default()),
        );
        assert!((wide_loss - narrow_loss).abs() < 1e-5 * wide_loss);
        for (a, b) in wide.iter().flatten().zip(narrow.iter().flatten()) {
            assert!((a - b).abs() < 1e-3 * a.abs().max(1e3), "{a} vs {b}");
        }
    }

    /// Large triangles take the parallel path; the result does not depend
    /// on the number of threads.
    #[test]
    fn sweeps_do_not_depend_on_the_thread_count() {
        let run = |threads: usize| {
            let mut rng = ChaCha8Rng::seed_from_u64(33);
            let scene = random_scene::<f32>(&mut rng, 120, 100);
            let mut tris = random_tris(&mut rng, 6, 120.0);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            let gradients =
                pool.install(|| sweep(&scene, &mut tris, true, true, &mut Workspace::default()));
            let loss = pool.install(|| loss(&scene, &tris, &mut Workspace::default()));
            (loss, gradients, tris)
        };
        let one = run(1);
        assert!(one.1.iter().any(|g| g.iter().any(|&v| v != 0.0)));
        let four = run(4);
        assert_eq!(one.0.to_bits(), four.0.to_bits());
        assert_eq!(one.1, four.1);
        assert_eq!(one.2, four.2);
    }
}
