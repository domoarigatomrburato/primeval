//! S-A1: the engine's refit passes on B's smooth forward model.
//!
//! A pass is the engine's (`primeval-core/src/refine.rs`): top-down
//! coordinate descent over the layers, each refitted with every other layer
//! fixed by [`ROUNDS`] independent hill climbs that stop after [`AGE`]
//! rejected moves in a row. A move shifts one vertex by normal offsets of
//! `σ = 16 px × scale` and alpha by a uniform integer within
//! `±max(3, round(10 × scale))`, the scale adapted by the 1/5th success rule
//! from 1 down to [`Search::min_scale`]; it is drawn again until the
//! triangle has every angle above 15°, as the engine's moves are. Every
//! candidate takes its closed-form colour. The best climb replaces the
//! layer only if it is strictly below the layer in its current colour, and
//! a pass is kept only if it lowered the forward loss.
//!
//! What differs from the engine is the model and the coordinates: the
//! energy of a candidate is that of B's forward model (box-filtered
//! coverage, composited in `f32`), and coordinates are continuous (or, for
//! [`Search::integer`], integers moved by rounded offsets, never all zero,
//! as the engine's).
//!
//! # The layer energy
//!
//! The `f32` composite blends `x ← x + w (s − x)`, with `w` the layer's
//! opacity times its coverage. The layers above layer `i` act on every
//! pixel as one affine map, `final = A · X_i + B` ([`Above`]); with `d` the
//! canvas below layer `i`, a candidate of weight `w` and colour `s` gives
//! `final − T = g · s − q` with `g = A w` and `q = T − B − A (1 − w) d`, and
//! with the layer removed the residual is `−r`, `r = T − B − A d`. The
//! candidate's loss is `Σ_all r² + Σ_covered (q − g s)² − r²`; the climbs
//! compare the second term ([`Layer::evaluate`]), whose best colour is
//! `s = Σ g q / Σ g²` per channel, clamped to `0..=255`. The canvas below
//! comes from `f32` checkpoints of the old layers every `⌈√N⌉` layers,
//! valid for the whole top-down pass.

use crate::diff::{self, Prepared, Scene, Tri};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

/// Independent hill climbs per layer, as the engine's refit.
pub(crate) const ROUNDS: u64 = 4;
/// Rejected moves in a row after which a climb stops, as the engine's.
pub(crate) const AGE: usize = 25;
/// The 1/5th rule: a kept move doubles the scale, a rejected one
/// multiplies it by `2^(−1/4)`.
const SCALE_UP: f64 = 2.0;
const SCALE_DOWN: f64 = 0.840_896_415_253_714_5;
/// The coarse `σ` of a vertex move, px.
const POSITION_SIGMA: f64 = 16.0;
/// Alpha moves: up to `ALPHA_STEP × scale`, rounded, but at least
/// `MIN_ALPHA_STEP`, either way.
const ALPHA_STEP: f64 = 10.0;
const MIN_ALPHA_STEP: i32 = 3;
/// The engine's smallest triangle angle.
const MIN_DEGREES: f64 = 15.0;
/// The tag of the climbs' random streams.
const TAG: [u8; 8] = *b"S-A1 lab";

/// The search of one arm.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Search {
    /// The smallest scale of the moves: `σ` of `16 × min_scale` px.
    pub(crate) min_scale: f64,
    /// Integer coordinates: offsets rounded, and drawn again while both
    /// round to zero.
    pub(crate) integer: bool,
    /// How far outside the canvas vertices may go.
    pub(crate) margin: f64,
    pub(crate) seed: u64,
}

impl Search {
    /// Continuous coordinates, moves down to `σ` of 0.25 px.
    pub(crate) fn continuous(margin: f64, seed: u64) -> Self {
        Self {
            min_scale: 0.25 / POSITION_SIGMA,
            integer: false,
            margin,
            seed,
        }
    }

    /// Integer coordinates, moves down to `σ` of 1 px.
    pub(crate) fn integer(margin: f64, seed: u64) -> Self {
        Self {
            min_scale: 1.0 / POSITION_SIGMA,
            integer: true,
            margin,
            seed,
        }
    }
}

/// The composite of the layers above a layer: `final = A · x + B`.
pub(crate) struct Above {
    pub(crate) transmittance: Vec<f32>,
    pub(crate) offset: Vec<f32>,
}

impl Above {
    pub(crate) fn new(pixels: usize) -> Self {
        Self {
            transmittance: vec![1.0; pixels],
            offset: vec![0.0; 3 * pixels],
        }
    }

    /// Folds `layer` in under the layers already folded:
    /// `B ← B + A · w · s`, `A ← A · (1 − w)`.
    pub(crate) fn fold(&mut self, layer: &Prepared<f32>, width: usize) {
        for y in layer.y0..layer.y1 {
            for x in layer.x0..layer.x1 {
                let cov = layer.coverage(x as f32, y as f32);
                if cov <= 0.0 {
                    continue;
                }
                let p = y * width + x;
                let w = layer.opacity * cov;
                let a = self.transmittance[p];
                for c in 0..3 {
                    self.offset[3 * p + c] += a * w * layer.color[c];
                }
                self.transmittance[p] = a * (1.0 - w);
            }
        }
    }
}

/// One layer of a pass: the scene, the canvas below the layer and the
/// composite above it.
pub(crate) struct Layer<'a> {
    pub(crate) scene: &'a Scene<f32>,
    pub(crate) below: &'a [f32],
    pub(crate) above: &'a Above,
}

/// `Σ g q` per channel, and `Σ g²`, `Σ q²` and `Σ r²` over the channels,
/// over the pixels a candidate covers.
#[derive(Default)]
pub(crate) struct Sums {
    numerators: [f64; 3],
    weights: f64,
    errors: f64,
    removed: f64,
}

impl Sums {
    /// The closed-form colour, or `None` without any weight.
    pub(crate) fn fit(&self) -> Option<[f64; 3]> {
        (self.weights > 0.0).then(|| {
            self.numerators
                .map(|numerator| (numerator / self.weights).clamp(0.0, 255.0))
        })
    }

    /// `Σ_covered (q − g s)² − r²` for the colour `rgb`.
    pub(crate) fn delta(&self, rgb: [f64; 3]) -> f64 {
        let colour: f64 = rgb
            .into_iter()
            .zip(self.numerators)
            .map(|(s, numerator)| s * (s * self.weights - 2.0 * numerator))
            .sum();
        self.errors - self.removed + colour
    }
}

impl Layer<'_> {
    pub(crate) fn sums(&self, tri: &Tri) -> Sums {
        let scene = self.scene;
        let layer = Prepared::<f32>::new(tri, 1.0, scene.width, scene.height);
        let mut sums = Sums::default();
        for y in layer.y0..layer.y1 {
            for x in layer.x0..layer.x1 {
                let cov = layer.coverage(x as f32, y as f32);
                if cov <= 0.0 {
                    continue;
                }
                let p = y * scene.width + x;
                let a = self.above.transmittance[p];
                if a == 0.0 {
                    continue;
                }
                let w = layer.opacity * cov;
                let g = a * w;
                let keep = a * (1.0 - w);
                let g64 = f64::from(g);
                sums.weights += g64 * g64;
                for c in 0..3 {
                    let i = 3 * p + c;
                    let rest = scene.target[i] - self.above.offset[i];
                    let d = self.below[i];
                    let q = f64::from(rest - keep * d);
                    let r = f64::from(rest - a * d);
                    sums.numerators[c] += g64 * q;
                    sums.errors += q * q;
                    sums.removed += r * r;
                }
            }
        }
        sums
    }

    /// The energy of `tri` with its closed-form colour, and that colour;
    /// an invisible triangle keeps its colour, and its energy is zero.
    pub(crate) fn evaluate(&self, tri: &Tri) -> (f64, [f64; 3]) {
        let sums = self.sums(tri);
        match sums.fit() {
            Some(rgb) => (sums.delta(rgb), rgb),
            None => (0.0, tri.color),
        }
    }

    /// `Σ_all r²`: the loss with the layer removed.
    #[cfg(test)]
    pub(crate) fn removed(&self) -> f64 {
        let scene = self.scene;
        let mut total = 0.0;
        for p in 0..scene.width * scene.height {
            let a = self.above.transmittance[p];
            for c in 0..3 {
                let i = 3 * p + c;
                let r = f64::from(scene.target[i] - self.above.offset[i] - a * self.below[i]);
                total += r * r;
            }
        }
        total
    }
}

/// The random stream of one climb: its own for every seed, pass, layer and
/// round, whatever thread runs it.
fn stream(seed: u64, pass: u64, layer: u64, round: u64) -> ChaCha8Rng {
    let mut key = [0_u8; 32];
    key[..8].copy_from_slice(&seed.to_le_bytes());
    key[8..16].copy_from_slice(&pass.to_le_bytes());
    key[16..24].copy_from_slice(&TAG);
    key[24..32].copy_from_slice(&layer.to_le_bytes());
    let mut rng = ChaCha8Rng::from_seed(key);
    rng.set_stream(round);
    rng
}

/// A standard normal sample (Box–Muller).
fn normal(rng: &mut ChaCha8Rng) -> f64 {
    let u1 = 1.0 - rng.random::<f64>();
    let u2 = rng.random::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Every angle of the triangle above [`MIN_DEGREES`], as the engine's
/// `Triangle::is_valid`.
fn is_valid(v: &[f64; 6]) -> bool {
    let angle = |ax: f64, ay: f64, bx: f64, by: f64| {
        let (da, db) = (ax.hypot(ay), bx.hypot(by));
        if da == 0.0 || db == 0.0 {
            return None;
        }
        let dot = ((ax / da) * (bx / db) + (ay / da) * (by / db)).clamp(-1.0, 1.0);
        Some(dot.acos().to_degrees())
    };
    let Some(a1) = angle(v[2] - v[0], v[3] - v[1], v[4] - v[0], v[5] - v[1]) else {
        return false;
    };
    let Some(a2) = angle(v[0] - v[2], v[1] - v[3], v[4] - v[2], v[5] - v[3]) else {
        return false;
    };
    let a3 = 180.0 - a1 - a2;
    a1 > MIN_DEGREES && a2 > MIN_DEGREES && a3 > MIN_DEGREES
}

/// The smallest angle of a triangle, in degrees.
pub(crate) fn min_angle(v: &[f64; 6]) -> f64 {
    (0..3)
        .map(|k| {
            let (p, q, r) = (k, (k + 1) % 3, (k + 2) % 3);
            let (ax, ay) = (v[2 * q] - v[2 * p], v[2 * q + 1] - v[2 * p + 1]);
            let (bx, by) = (v[2 * r] - v[2 * p], v[2 * r + 1] - v[2 * p + 1]);
            let (da, db) = (ax.hypot(ay), bx.hypot(by));
            if da == 0.0 || db == 0.0 {
                return 0.0;
            }
            ((ax * bx + ay * by) / (da * db))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        })
        .fold(f64::INFINITY, f64::min)
}

/// The engine's move at `scale`: one vertex, drawn again (cumulatively, as
/// the engine's) until the triangle is valid, then alpha.
fn mutate(tri: &mut Tri, scale: f64, search: &Search, bounds: (f64, f64), rng: &mut ChaCha8Rng) {
    let sigma = POSITION_SIGMA * scale;
    loop {
        let vertex = rng.random_range(0..3_usize);
        let (dx, dy) = if search.integer {
            loop {
                let dx = (normal(rng) * sigma).round();
                let dy = (normal(rng) * sigma).round();
                if (dx, dy) != (0.0, 0.0) {
                    break (dx, dy);
                }
            }
        } else {
            (normal(rng) * sigma, normal(rng) * sigma)
        };
        let x = &mut tri.vertices[2 * vertex];
        *x = (*x + dx).clamp(-search.margin, bounds.0);
        let y = &mut tri.vertices[2 * vertex + 1];
        *y = (*y + dy).clamp(-search.margin, bounds.1);
        if is_valid(&tri.vertices) {
            break;
        }
    }
    let reach = ((ALPHA_STEP * scale).round() as i32).clamp(MIN_ALPHA_STEP, ALPHA_STEP as i32);
    let delta = rng.random_range(-reach..=reach);
    tri.alpha = (tri.alpha + f64::from(delta)).clamp(1.0, 255.0);
}

/// One hill climb from `start` at energy `energy`: the best triangle, with
/// its colour, its energy, and the evaluations made.
fn climb(
    layer: &Layer<'_>,
    start: Tri,
    energy: f64,
    search: &Search,
    bounds: (f64, f64),
    rng: &mut ChaCha8Rng,
) -> (Tri, f64, u64) {
    let mut current = start;
    let mut best = start;
    let mut best_energy = energy;
    let mut scale = 1.0_f64;
    let mut age = 0;
    let mut evaluations = 0;
    while age < AGE {
        let previous = current;
        mutate(&mut current, scale, search, bounds, rng);
        let (candidate, color) = layer.evaluate(&current);
        evaluations += 1;
        let kept = candidate < best_energy;
        let factor = if kept { SCALE_UP } else { SCALE_DOWN };
        scale = (scale * factor).clamp(search.min_scale, 1.0);
        if kept {
            current.color = color;
            best = current;
            best_energy = candidate;
            age = 0;
        } else {
            current = previous;
            age += 1;
        }
    }
    (best, best_energy, evaluations)
}

/// The canvas of `layers` composited on the background.
fn forward(scene: &Scene<f32>, layers: &[Prepared<f32>]) -> Vec<f32> {
    let mut canvas: Vec<f32> = (0..scene.width * scene.height)
        .flat_map(|_| scene.background)
        .collect();
    for layer in layers {
        diff::paint(&mut canvas, scene.width, layer);
    }
    canvas
}

/// `Σ (X − T)²`.
pub(crate) fn loss(scene: &Scene<f32>, canvas: &[f32]) -> f64 {
    canvas
        .iter()
        .zip(&scene.target)
        .map(|(&x, &t)| {
            let r = f64::from(x - t);
            r * r
        })
        .sum()
}

fn prepare(scene: &Scene<f32>, tris: &[Tri]) -> Vec<Prepared<f32>> {
    tris.iter()
        .map(|tri| Prepared::new(tri, 1.0, scene.width, scene.height))
        .collect()
}

/// What a [`pass`] did.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pass {
    /// The forward loss before and after (equal if the pass was not kept).
    pub(crate) before: f64,
    pub(crate) after: f64,
    /// Layers replaced.
    pub(crate) changed: usize,
    pub(crate) evaluations: u64,
}

/// One top-down refit pass over `tris` on `scene` (filter 1), kept only if
/// it lowers the forward loss.
pub(crate) fn pass(scene: &Scene<f32>, tris: &mut [Tri], search: &Search, number: u64) -> Pass {
    assert!((scene.filter - 1.0).abs() < f64::EPSILON, "filter 1 only");
    let (width, height) = (scene.width, scene.height);
    let pixels = width * height;
    let n = tris.len();
    let bounds = (
        (width - 1) as f64 + search.margin,
        (height - 1) as f64 + search.margin,
    );
    let old = prepare(scene, tris);
    let interval = diff::ceil_sqrt(n);
    let mut checkpoints = Vec::with_capacity(n.div_ceil(interval));
    let mut canvas: Vec<f32> = (0..pixels).flat_map(|_| scene.background).collect();
    for (index, layer) in old.iter().enumerate() {
        if index % interval == 0 {
            checkpoints.push(canvas.clone());
        }
        diff::paint(&mut canvas, width, layer);
    }
    let before = loss(scene, &canvas);

    let original = tris.to_vec();
    let mut above = Above::new(pixels);
    let mut below = vec![0.0_f32; 3 * pixels];
    let mut changed = 0;
    let mut evaluations = 0;
    for index in (0..n).rev() {
        let start = index - index % interval;
        below.copy_from_slice(&checkpoints[index / interval]);
        for layer in &old[start..index] {
            diff::paint(&mut below, width, layer);
        }
        let layer = Layer {
            scene,
            below: &below,
            above: &above,
        };
        let committed = tris[index];
        let bar = layer.sums(&committed).delta(committed.color);
        let (energy, color) = layer.evaluate(&committed);
        let fitted = Tri { color, ..committed };
        let results: Vec<(Tri, f64, u64)> = (0..ROUNDS)
            .into_par_iter()
            .map(|round| {
                let mut rng = stream(search.seed, number, index as u64, round);
                climb(&layer, fitted, energy, search, bounds, &mut rng)
            })
            .collect();
        evaluations += 2;
        let mut best: Option<(Tri, f64)> = None;
        // Round order; only a strictly lower energy replaces the best.
        for (tri, energy, count) in results {
            evaluations += count;
            if best.is_none_or(|(_, best)| energy < best) {
                best = Some((tri, energy));
            }
        }
        if let Some((tri, energy)) = best
            && energy < bar
        {
            tris[index] = tri;
            changed += 1;
        }
        above.fold(&Prepared::new(&tris[index], 1.0, width, height), width);
    }
    let after = loss(scene, &forward(scene, &prepare(scene, tris)));
    if after < before {
        Pass {
            before,
            after,
            changed,
            evaluations,
        }
    } else {
        tris.copy_from_slice(&original);
        Pass {
            before,
            after: before,
            changed: 0,
            evaluations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};

    fn random_scene(rng: &mut ChaCha8Rng, width: usize, height: usize) -> Scene<f32> {
        Scene {
            width,
            height,
            target: (0..3 * width * height)
                .map(|_| rng.random_range(0.0..255.0))
                .collect(),
            background: [40.0, 120.0, 200.0],
            filter: 1.0,
        }
    }

    fn random_tris(rng: &mut ChaCha8Rng, count: usize, size: f64) -> Vec<Tri> {
        (0..count)
            .map(|_| {
                loop {
                    let tri = Tri {
                        vertices: std::array::from_fn(|_| rng.random_range(-3.0..size + 3.0)),
                        alpha: f64::from(rng.random_range(40..250)),
                        color: std::array::from_fn(|_| rng.random_range(0.0..255.0)),
                    };
                    if is_valid(&tri.vertices) {
                        break tri;
                    }
                }
            })
            .collect()
    }

    fn f64_scene(scene: &Scene<f32>) -> Scene<f64> {
        Scene {
            width: scene.width,
            height: scene.height,
            target: scene.target.iter().map(|&v| f64::from(v)).collect(),
            background: scene.background.map(f64::from),
            filter: 1.0,
        }
    }

    fn f64_loss(scene: &Scene<f64>, tris: &[Tri]) -> f64 {
        let mut tris = tris.to_vec();
        diff::sweep(
            scene,
            &mut tris,
            false,
            false,
            &mut diff::Workspace::default(),
        )
        .loss
    }

    /// The model's loss of a random candidate at every layer, and of the
    /// layer removed, against the forward render of the substituted stack
    /// in `f64`, within `f32` tolerance; and the fitted colour is the
    /// minimum of the forward loss over colours.
    #[test]
    fn layer_energy_matches_the_forward_render() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let (width, height) = (48, 40);
        let scene = random_scene(&mut rng, width, height);
        let exact = f64_scene(&scene);
        let tris = random_tris(&mut rng, 12, 48.0);
        let old = prepare(&scene, &tris);
        let mut worst = 0.0_f64;
        let (mut worst_delta, mut compared) = (0.0_f64, 0);
        let mut above = Above::new(width * height);
        for index in (0..tris.len()).rev() {
            let below = forward(&scene, &old[..index]);
            let layer = Layer {
                scene: &scene,
                below: &below,
                above: &above,
            };
            let mut without = tris.clone();
            without.remove(index);
            let removed = f64_loss(&exact, &without);
            let scale = removed;
            worst = worst.max((layer.removed() - removed).abs() / scale);
            for _ in 0..4 {
                let candidate = random_tris(&mut rng, 1, 48.0)[0];
                let (energy, color) = layer.evaluate(&candidate);
                let mut stack = tris.clone();
                stack[index] = Tri { color, ..candidate };
                let model = layer.removed() + energy;
                let truth = f64_loss(&exact, &stack);
                worst = worst.max((model - truth).abs() / scale);
                // The candidate's own term against the forward change.
                let change = truth - removed;
                if change.abs() > 1e-4 * removed {
                    worst_delta = worst_delta.max((energy - change).abs() / change.abs());
                    compared += 1;
                }
                // The fitted colour minimises the forward loss.
                for c in 0..3 {
                    for shift in [-1.0, 1.0] {
                        let mut moved = stack.clone();
                        moved[index].color[c] = (color[c] + shift).clamp(0.0, 255.0);
                        assert!(f64_loss(&exact, &moved) >= truth * (1.0 - 1e-6));
                    }
                }
                // Any colour: the model's delta is the forward difference.
                let other = Tri {
                    color: [10.0, 200.0, 90.0],
                    ..candidate
                };
                stack[index] = other;
                let delta = layer.sums(&other).delta(other.color);
                let truth = f64_loss(&exact, &stack);
                worst = worst.max((layer.removed() + delta - truth).abs() / scale);
                let change = truth - removed;
                if change.abs() > 1e-4 * removed {
                    worst_delta = worst_delta.max((delta - change).abs() / change.abs());
                    compared += 1;
                }
            }
            above.fold(&old[index], width);
        }
        eprintln!(
            "layer energy: worst error {worst:.2e} of the loss; worst error {worst_delta:.2e} \
             of the candidate's change over {compared} candidates"
        );
        assert!(worst < 1e-5, "{worst}");
        assert!(compared >= 48, "{compared}");
        assert!(worst_delta < 1e-3, "{worst_delta}");
    }

    /// A pass lowers the forward loss it reports, the reported loss is the
    /// forward loss of the result, the result does not depend on the
    /// thread count, and integer search keeps integer vertices.
    #[test]
    fn pass_lowers_the_loss_and_does_not_depend_on_threads() {
        for integer in [false, true] {
            let run = |threads: usize| {
                let mut rng = ChaCha8Rng::seed_from_u64(5);
                let scene = random_scene(&mut rng, 64, 48);
                let mut tris = random_tris(&mut rng, 10, 64.0);
                if integer {
                    for tri in &mut tris {
                        tri.vertices = tri.vertices.map(f64::round);
                    }
                }
                let search = if integer {
                    Search::integer(16.0, 3)
                } else {
                    Search::continuous(16.0, 3)
                };
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .expect("pool");
                let result = pool.install(|| pass(&scene, &mut tris, &search, 0));
                let truth = loss(&scene, &forward(&scene, &prepare(&scene, &tris)));
                (result, truth, tris)
            };
            let (one, truth, tris) = run(1);
            assert!(one.changed > 0);
            assert!(one.after < one.before, "{one:?}");
            assert_eq!(one.after.to_bits(), truth.to_bits());
            if integer {
                assert!(
                    tris.iter()
                        .all(|tri| tri.vertices.iter().all(|v| v.fract() == 0.0))
                );
            } else {
                assert!(
                    tris.iter()
                        .any(|tri| tri.vertices.iter().any(|v| v.fract() != 0.0))
                );
            }
            assert!(
                tris.iter()
                    .all(|tri| tri.alpha.fract() == 0.0 && is_valid(&tri.vertices))
            );
            let (four, _, four_tris) = run(4);
            assert_eq!(one.after.to_bits(), four.after.to_bits());
            assert_eq!(one.evaluations, four.evaluations);
            assert_eq!(tris, four_tris);
        }
    }

    /// The moves shrink to the smallest scale: a long run of rejections
    /// moves a vertex by about `σ` of the smallest scale.
    #[test]
    fn smallest_moves_are_sub_pixel_or_one_pixel() {
        for (search, low, high) in [
            (Search::continuous(16.0, 1), 0.15, 0.35),
            (Search::integer(16.0, 1), 0.9, 1.6),
        ] {
            let mut rng = stream(1, 0, 0, 0);
            let start = Tri {
                vertices: [10.0, 10.0, 40.0, 12.0, 20.0, 40.0],
                alpha: 128.0,
                color: [0.0; 3],
            };
            let mut total = 0.0;
            let count = 2000;
            for _ in 0..count {
                let mut tri = start;
                mutate(
                    &mut tri,
                    search.min_scale,
                    &search,
                    (100.0, 100.0),
                    &mut rng,
                );
                let moved: f64 = tri
                    .vertices
                    .iter()
                    .zip(start.vertices)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                // One vertex, two coordinates: mean squared offset per
                // coordinate.
                total += moved / 2.0;
                assert!((tri.alpha - start.alpha).abs() <= f64::from(MIN_ALPHA_STEP));
            }
            let sigma = (total / f64::from(count)).sqrt();
            assert!((low..high).contains(&sigma), "{search:?}: {sigma}");
        }
    }
}
