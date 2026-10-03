//! Joint gradient optimisation of a drawing's triangles and convex
//! polygons, with every other layer fixed in geometry.
//!
//! The greedy search places one shape at a time with the others fixed.
//! [`optimise`] then moves every vertex of every triangle and polygon, and
//! every opacity under [`Alpha::Auto`], at once: Adam on the gradient of
//! the squared error between the composite and the target, through a
//! smooth model of the export's anti-aliased rendering, with every colour
//! refitted at each iteration. Its result is snapped to a quarter pixel and
//! keeps the engine's rules: every angle of a triangle above 15°, and every
//! polygon strictly convex with every angle above 15°.
//!
//! The other layers (rectangles, rotated rectangles, ellipses, circles,
//! rotated ellipses and quadratics) keep their geometry. Their opacity and
//! colour are optimised with the others', through a coverage mask of the
//! engine's own rasterization of the shape, computed once.
//!
//! The forward model (`diff.rs`) covers each pixel of a triangle or a
//! polygon by the product of its edges' exact half-plane areas in the pixel
//! square, and composites in `f32`. Reverse-mode gradients replay the layer
//! stack from canvas checkpoints, about `√N` of them for `N` layers, capped
//! in memory as the refit pass's are. Every pass runs as one task per fixed
//! band of rows, with one fork and join, and its sums are reduced in band
//! order, so the result does not depend on the number of threads. The
//! colours are refitted together, each by a step towards its closed-form
//! fit that can only lower the loss. The rules are kept by a projection
//! after every step and by the snap (`angle.rs` for triangles, `convex.rs`
//! for polygons).
//!
//! # Arithmetic
//!
//! Native and WebAssembly builds must produce bit-identical output, and
//! their math libraries differ. Everything this module computes, the
//! projection and the snap included, may therefore use only:
//!
//! - `+ − × ÷` and `sqrt`, which IEEE 754 rounds exactly;
//! - comparisons, `abs`, `min` and `max`;
//! - `floor`, `ceil`, `round` and `trunc`;
//! - constants written as literals.
//!
//! Never `hypot`, `atan2`, `acos`, `cos`, `sin`, `tan`, `exp`, `ln`,
//! `powi`, `powf` or `mul_add`, whose results can differ in the last bit
//! between platforms. Tests may use them. The masks of the fixed layers
//! are not computed here: they are the scanlines of the engine's
//! rasterizers, which the greedy search's own output already depends on,
//! and which the native–wasm tripwire test covers.

mod angle;
mod convex;
mod diff;

use crate::model::CommittedShape;
use crate::shapes::Shape;
use crate::worker::WorkerCtx;
use crate::{Alpha, Buffer, Color, Drawing, DrawnShape, Geometry, Model, Point};
use diff::{ALPHA, COORDS, Layer, Mask, Outline, PARAMS, Real, Scene, Workspace};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// The default number of Adam iterations for a drawing of `shapes`
/// layers, used when [`Settings::iterations`] is `None`: 80 up to 50
/// layers, linear to 120 at 200 and to 160 at 500, then 160, rounded
/// down.
///
/// Each count is the largest that keeps the optimisation's extra time near
/// 0.5× the greedy search's at 1 thread, measured on the default corpus
/// with triangles; quality keeps improving up to 150 iterations, so the
/// time budget decides. Integer arithmetic keeps native and wasm builds in
/// agreement.
fn default_iterations(shapes: usize) -> u32 {
    let shapes = u32::try_from(shapes).unwrap_or(u32::MAX);
    match shapes {
        0..=50 => 80,
        51..=200 => 80 + (shapes - 50) * 40 / 150,
        201..=500 => 120 + (shapes - 200) * 40 / 300,
        _ => 160,
    }
}

/// `tan 15.5°`: the projection keeps every angle at least 15.5°, half a
/// degree inside the rule, so that the snap rarely needs a repair.
const TAN_PROJECTION: f64 = 0.277_324_544_059_838_4;

/// Settings of [`optimise`].
///
/// Construct with [`Settings::default`] and set the fields you need.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Adam iterations. Each costs a colour fit (about two renders of the
    /// drawing) and a gradient (a render, its replay and the reverse pass).
    /// `Some(0)` only projects, snaps and refits the colours.
    ///
    /// `None`, the default, follows the number of layers: 80 up to 50,
    /// linear to 120 at 200 and to 160 at 500, then 160, rounded down. Each
    /// count is the largest that keeps the optimisation's extra time near
    /// 0.5× the greedy search's at 1 thread on the default corpus of
    /// triangles; quality keeps improving up to 150 iterations, so the time
    /// budget decides.
    pub iterations: Option<u32>,
}

/// Optimises the shapes `model` has committed jointly against its target,
/// on its background, and returns the result, or `None` once `cancelled`
/// returns true. `model` is left unchanged.
///
/// See the module documentation for the method.
/// [`Settings::iterations`] Adam iterations, by default a number that
/// grows with the number of shapes, move the vertices of the triangles and
/// polygons, and every opacity when `alpha` is [`Alpha::Auto`]; with
/// [`Alpha::Fixed`] every opacity stays at the fixed value. Every colour is
/// refitted at each iteration. Every other shape keeps its geometry
/// exactly.
///
/// The result keeps the number, the order and the kind of the shapes. Its
/// triangle and polygon vertices are multiples of 0.25 px; every angle of
/// every triangle is above 15°, every polygon is strictly convex with every
/// angle above 15°, and the opacities and colours are integers.
///
/// `cancelled` is polled before every iteration and before the snap. The
/// result does not depend on the number of threads of the current rayon
/// pool.
#[must_use]
pub fn optimise(
    model: &Model,
    alpha: Alpha,
    settings: Settings,
    cancelled: impl FnMut() -> bool,
) -> Option<Drawing> {
    let (target, background, history) = model.joint_parts();
    optimise_shapes(target, background, history, alpha, settings, cancelled)
}

/// The difference between `drawing`, an [`optimise`] result for `model`,
/// rendered by [`optimise`]'s model, and `model`'s target: the RMSE over
/// the RGB channels divided by 255, as [`Model::score_f64`] measures the
/// engine's canvas. Its triangles and polygons are taken from `drawing`,
/// and the coverage of every other shape from `model`'s.
///
/// # Panics
///
/// If `drawing` does not have `model`'s shapes, kinds, background and size,
/// with the geometry of every shape that is neither a triangle nor a
/// polygon unchanged.
#[must_use]
pub fn score(model: &Model, drawing: &Drawing) -> f64 {
    let (target, background, history) = model.joint_parts();
    assert_eq!(
        (drawing.width, drawing.height, drawing.background),
        (target.width(), target.height(), background),
        "the drawing is not the model's"
    );
    score_shapes(target, background, history, drawing)
}

/// The lattice the exported vertices snap to, in pixels.
const QUANTUM: f64 = 0.25;
/// Adam's step size of the vertices at the first iteration, in pixels.
const LR_VERTEX: f64 = 1.0;
/// Adam's step size of the opacities at the first iteration, in levels of
/// `0..=255`.
const LR_ALPHA: f64 = 10.0;
const BETA1: f64 = 0.9;
const BETA2: f64 = 0.999;
const EPSILON: f64 = 1e-8;
/// How far outside the canvas the vertices may go, as the engine's.
const MARGIN: f64 = 16.0;

/// The forward model's scene of a drawing on `target` and `background`,
/// its layers, and the geometry of its fixed layers, in the order of their
/// masks.
struct Parts<F> {
    scene: Scene<F>,
    layers: Vec<Layer>,
    fixed: Vec<Geometry>,
}

/// The [`Parts`] of the committed shapes `history` on `target` and
/// `background`: triangles and quadrilaterals become layers with their
/// vertices in engine coordinates, every other shape a fixed layer with
/// the mask of the engine's rasterization.
fn parts<F: Real>(target: &Buffer, background: Color, history: &[CommittedShape]) -> Parts<F> {
    let (width, height) = (target.width() as usize, target.height() as usize);
    let mut masks = Vec::new();
    let mut fixed = Vec::new();
    // The rasterizers take a worker; none of them draws from its stream.
    let mut worker = WorkerCtx::new(width as i32, height as i32, ChaCha8Rng::seed_from_u64(0));
    let layers = history
        .iter()
        .map(|committed| {
            let color = committed.color;
            let (alpha, rgb) = (
                f64::from(color.a),
                [color.r, color.g, color.b].map(f64::from),
            );
            match &committed.shape {
                Shape::Triangle(t) => Layer::triangle(
                    [t.x1, t.y1, t.x2, t.y2, t.x3, t.y3].map(f64::from),
                    alpha,
                    rgb,
                ),
                // Polygon vertices are continuous: the engine's coordinates
                // are 0.5 below them.
                Shape::Polygon(polygon) if polygon.order == 3 => Layer::triangle(
                    std::array::from_fn(|k| {
                        let axis = if k % 2 == 0 { &polygon.x } else { &polygon.y };
                        axis[k / 2] - 0.5
                    }),
                    alpha,
                    rgb,
                ),
                Shape::Polygon(polygon) if polygon.order == 4 => Layer {
                    outline: Outline::Quad,
                    vertices: std::array::from_fn(|k| {
                        let axis = if k % 2 == 0 { &polygon.x } else { &polygon.y };
                        axis[k / 2] - 0.5
                    }),
                    alpha,
                    color: rgb,
                },
                shape => {
                    masks.push(Mask::from_lines(
                        shape.rasterize(&mut worker),
                        width,
                        height,
                    ));
                    fixed.push(shape.geometry());
                    Layer {
                        outline: Outline::Fixed(masks.len() - 1),
                        vertices: [0.0; COORDS],
                        alpha,
                        color: rgb,
                    }
                }
            }
        })
        .collect();
    Parts {
        scene: Scene {
            width,
            height,
            target: target
                .pixels()
                .iter()
                .map(|&value| F::of(f64::from(value)))
                .collect(),
            background: [background.r, background.g, background.b].map(|c| F::of(f64::from(c))),
            filter: 1.0,
            masks,
        },
        layers,
        fixed,
    }
}

/// [`optimise`] on its parts.
fn optimise_shapes(
    target: &Buffer,
    background: Color,
    history: &[CommittedShape],
    alpha: Alpha,
    settings: Settings,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Drawing> {
    let Parts {
        scene,
        mut layers,
        fixed,
    } = parts::<f32>(target, background, history);
    let auto_alpha = alpha == Alpha::Auto;
    let iterations = settings
        .iterations
        .unwrap_or_else(|| default_iterations(layers.len())) as usize;
    run(
        &scene,
        &mut layers,
        iterations,
        auto_alpha,
        TAN_PROJECTION,
        &mut cancelled,
    )?;
    if cancelled() {
        return None;
    }
    Some(export(&scene, &layers, &fixed, background))
}

/// [`score`] on its parts.
fn score_shapes(
    target: &Buffer,
    background: Color,
    history: &[CommittedShape],
    drawing: &Drawing,
) -> f64 {
    let Parts {
        scene,
        mut layers,
        fixed,
    } = parts::<f32>(target, background, history);
    assert_eq!(
        drawing.shapes.len(),
        layers.len(),
        "the drawing is not the model's"
    );
    for (layer, shape) in layers.iter_mut().zip(&drawing.shapes) {
        let color = shape.color;
        layer.alpha = f64::from(color.a);
        layer.color = [color.r, color.g, color.b].map(f64::from);
        match (layer.outline, &shape.geometry) {
            (Outline::Fixed(index), geometry) => {
                assert_eq!(geometry, &fixed[index], "a fixed shape moved");
            }
            (outline, Geometry::Polygon(points)) if points.len() == outline.sides() => {
                for (k, point) in points.iter().enumerate() {
                    layer.vertices[2 * k] = point.x - 0.5;
                    layer.vertices[2 * k + 1] = point.y - 0.5;
                }
            }
            (_, geometry) => panic!("not the model's shape: {geometry:?}"),
        }
    }
    let loss = diff::loss(&scene, &layers, &mut Workspace::default());
    (loss / (3 * scene.width * scene.height) as f64).sqrt() / 255.0
}

/// The step-size factor at iteration `t` of `iterations`: the smoothstep
/// `1 − (3p² − 2p³)` of the progress `p`, from 1 to 0 with zero slope at
/// both ends, the shape of a half cosine without the cosine.
fn decay(t: usize, iterations: usize) -> f64 {
    let p = t as f64 / iterations.max(1) as f64;
    1.0 - p * p * (3.0 - 2.0 * p)
}

/// Projects `layer` onto its rule with angles of at least `atan tan_tau`:
/// a triangle by [`angle::project`], a polygon by [`convex::project`];
/// a fixed layer has no vertices. Returns the displacement, or `None` if
/// the layer was already inside.
fn project(layer: &mut Layer, tan_tau: f64) -> Option<[f64; COORDS]> {
    let before = layer.vertices;
    let projected = match layer.outline {
        Outline::Triangle => {
            let mut v: [f64; 6] = std::array::from_fn(|k| layer.vertices[k]);
            let projected = angle::project(&mut v, tan_tau);
            layer.vertices[..6].copy_from_slice(&v);
            projected
        }
        Outline::Quad => convex::project(&mut layer.vertices, tan_tau),
        Outline::Fixed(_) => return None,
    };
    match projected {
        angle::Projected::Unchanged => None,
        angle::Projected::Moved | angle::Projected::Rebuilt => {
            Some(std::array::from_fn(|k| layer.vertices[k] - before[k]))
        }
    }
}

/// Runs `iterations` Adam steps on the vertices and opacities of `layers`
/// (opacities only with `auto_alpha`, and no vertices for fixed layers),
/// each on the [`diff::gradients`] taken after one [`diff::fit`] of every
/// colour; then one more fit refits the colours of the final geometry.
/// `cancelled` is polled before every step; `None` once it returns true.
/// Vertices stay within [`MARGIN`] of the canvas unless a projection moves
/// them out.
///
/// Every triangle and polygon is projected onto its rule ([`project`])
/// before the first step and after every step, after the clamp to the
/// margin. A projected layer loses the component of its first moment
/// along the projection's displacement when that component pushes back
/// out of the set, and its second moments rise to keep every step within
/// the step size ([`redirect_momentum`]): the momentum keeps sliding along
/// the boundary but stops pressing into it.
fn run<F: Real>(
    scene: &Scene<F>,
    layers: &mut [Layer],
    iterations: usize,
    auto_alpha: bool,
    tan_tau: f64,
    cancelled: &mut impl FnMut() -> bool,
) -> Option<()> {
    for layer in layers.iter_mut() {
        project(layer, tan_tau);
    }
    let mut work = Workspace::default();
    let mut first = vec![[0.0; PARAMS]; layers.len()];
    let mut second = vec![[0.0; PARAMS]; layers.len()];
    let (max_x, max_y) = (
        (scene.width - 1) as f64 + MARGIN,
        (scene.height - 1) as f64 + MARGIN,
    );
    // `β^t`, as running products.
    let (mut power1, mut power2) = (1.0, 1.0);
    for t in 0..iterations {
        if cancelled() {
            return None;
        }
        diff::fit(scene, layers, &mut work);
        let gradients = diff::gradients(scene, layers, &mut work);
        let decay = decay(t, iterations);
        power1 *= BETA1;
        power2 *= BETA2;
        let (correction1, correction2) = (1.0 - power1, 1.0 - power2);
        for (index, layer) in layers.iter_mut().enumerate() {
            let grad = gradients[index];
            let coords = 2 * layer.outline.sides();
            for k in (0..coords).chain((auto_alpha).then_some(ALPHA)) {
                let m = &mut first[index][k];
                let v = &mut second[index][k];
                *m = BETA1 * *m + (1.0 - BETA1) * grad[k];
                *v = BETA2 * *v + (1.0 - BETA2) * grad[k] * grad[k];
                let update = adam_update(*m, *v, (correction1, correction2));
                let (value, rate, low, high) = match k {
                    ALPHA => (&mut layer.alpha, LR_ALPHA, 1.0, 255.0),
                    k if k % 2 == 0 => (&mut layer.vertices[k], LR_VERTEX, -MARGIN, max_x),
                    _ => (&mut layer.vertices[k], LR_VERTEX, -MARGIN, max_y),
                };
                *value = (*value - rate * decay * update).clamp(low, high);
            }
            if let Some(displacement) = project(layer, tan_tau) {
                redirect_momentum(
                    &mut first[index],
                    &mut second[index],
                    &displacement[..coords],
                    (correction1, correction2),
                );
            }
        }
    }
    diff::fit(scene, layers, &mut work);
    Some(())
}

/// Adam's update of one parameter, in units of its step size: the
/// first moment `m` over the root of the second moment `v`, each divided
/// by its bias correction `1 − β^t` in `corrections`.
fn adam_update(m: f64, v: f64, corrections: (f64, f64)) -> f64 {
    (m / corrections.0) / ((v / corrections.1).sqrt() + EPSILON)
}

/// The momentum correction of [`run`] after a projection moved a layer's
/// vertex coordinates by `displacement`, on the moments `first` and
/// `second` of its parameters, with Adam's bias `corrections` at this
/// step.
///
/// If the first moment `m` of the vertices pushes back out of the set
/// (`m·d > 0`, the update being `−m`), its component along `d` is removed:
/// `m ← m − (m·d / d·d) d`. That moves first moment into every coordinate
/// the projection moved, including ones whose gradients were near zero,
/// and Adam's step on such a coordinate, `m̂ / (√v̂ + ε)`, would then be
/// about `m̂ / ε`. So each coordinate's second moment is raised, if
/// needed, to the square of its new first moment (`v̂ ≥ m̂²`), as if that
/// momentum had come from gradients of its size: its step is then at most
/// the step size.
fn redirect_momentum(
    first: &mut [f64; PARAMS],
    second: &mut [f64; PARAMS],
    displacement: &[f64],
    (correction1, correction2): (f64, f64),
) {
    let coords = displacement.len();
    let md: f64 = (0..coords).map(|k| first[k] * displacement[k]).sum();
    let dd: f64 = displacement.iter().map(|d| d * d).sum();
    if md > 0.0 && dd > 0.0 {
        for k in 0..coords {
            first[k] -= md / dd * displacement[k];
            let m = first[k] / correction1;
            second[k] = second[k].max(correction2 * m * m);
        }
    }
}

/// `layers` as a drawing on `background`: triangle and polygon vertices
/// snapped to [`QUANTUM`] keeping their rules ([`angle::snap`],
/// [`convex::snap`]), opacities rounded, colours refitted once to the
/// snapped geometry and rounded. A fixed layer keeps its geometry, from
/// `fixed`.
fn export<F: Real>(
    scene: &Scene<F>,
    layers: &[Layer],
    fixed: &[Geometry],
    background: Color,
) -> Drawing {
    let mut snapped: Vec<Layer> = layers
        .iter()
        .map(|layer| {
            let mut vertices = layer.vertices;
            match layer.outline {
                Outline::Triangle => {
                    let v: [f64; 6] = std::array::from_fn(|k| vertices[k]);
                    vertices[..6].copy_from_slice(&angle::snap(&v, QUANTUM).0);
                }
                Outline::Quad => vertices = convex::snap(&vertices, QUANTUM).0,
                Outline::Fixed(_) => {}
            }
            Layer {
                vertices,
                alpha: layer.alpha.round().clamp(1.0, 255.0),
                ..*layer
            }
        })
        .collect();
    diff::fit(scene, &mut snapped, &mut Workspace::default());
    Drawing {
        width: scene.width as u32,
        height: scene.height as u32,
        background,
        shapes: snapped
            .iter()
            .map(|layer| {
                let geometry = match layer.outline {
                    Outline::Fixed(index) => fixed[index].clone(),
                    outline => Geometry::Polygon(
                        (0..outline.sides())
                            .map(|k| {
                                Point::new(
                                    layer.vertices[2 * k] + 0.5,
                                    layer.vertices[2 * k + 1] + 0.5,
                                )
                            })
                            .collect(),
                    ),
                };
                let [r, g, b] = layer.color.map(|c| c.round().clamp(0.0, 255.0) as u8);
                DrawnShape {
                    geometry,
                    color: Color::new(r, g, b, layer.alpha as u8),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::angle::tests::acos_valid;
    use super::*;
    use crate::{ModelOptions, ShapeKind};
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// A smooth target with a few hard edges, `width × height`.
    fn target(width: u32, height: u32) -> Buffer {
        let mut pixels = Vec::with_capacity((3 * width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let inside = (x as i32 - 20).pow(2) + (y as i32 - 14).pow(2) < 90;
                let stripe = (x + 2 * y) % 23 < 7;
                pixels.extend([
                    (x * 255 / width) as u8,
                    if inside {
                        230
                    } else {
                        (y * 200 / height) as u8
                    },
                    if stripe { 20 } else { 160 },
                ]);
            }
        }
        Buffer::from_rgb(width, height, pixels).expect("target")
    }

    fn greedy_kind(target: &Buffer, steps: usize, kind: ShapeKind, alpha: Alpha) -> Model {
        let options = ModelOptions {
            seed: Some(5),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target.clone(), Color::new(90, 60, 30, 255), options);
        for _ in 0..steps {
            model.step(kind, alpha);
        }
        model
    }

    fn greedy(target: &Buffer, steps: usize, alpha: Alpha) -> Model {
        greedy_kind(target, steps, ShapeKind::Triangle, alpha)
    }

    fn vertices(drawing: &Drawing) -> Vec<[f64; 6]> {
        drawing
            .shapes
            .iter()
            .map(|shape| {
                let Geometry::Polygon(points) = &shape.geometry else {
                    panic!("not a triangle: {shape:?}");
                };
                let [a, b, c] = points.as_slice() else {
                    panic!("not a triangle: {shape:?}");
                };
                [a.x, a.y, b.x, b.y, c.x, c.y]
            })
            .collect()
    }

    fn settings(iterations: u32) -> Settings {
        Settings {
            iterations: Some(iterations),
        }
    }

    /// By default the iteration count follows the number of triangles: 80
    /// up to 50, linear to 120 at 200 and to 160 at 500, then 160, rounded
    /// down.
    #[test]
    fn the_default_iterations_follow_the_shape_count() {
        assert_eq!(Settings::default().iterations, None);
        for (shapes, iterations) in [
            (0, 80),
            (1, 80),
            (50, 80),
            (51, 80),
            (100, 93),
            (200, 120),
            (201, 120),
            (350, 140),
            (500, 160),
            (501, 160),
            (100_000, 160),
        ] {
            assert_eq!(default_iterations(shapes), iterations, "{shapes} shapes");
        }
    }

    /// B lowers its own model's error against the greedy drawing, keeps
    /// the triangles' number and order, and exports quarter pixels,
    /// integer colours and valid angles.
    #[test]
    fn optimising_lowers_the_model_rmse_of_the_greedy_drawing() {
        let target = target(48, 40);
        let model = greedy(&target, 20, Alpha::Auto);
        let start = model.drawing();
        let optimised =
            optimise(&model, Alpha::Auto, settings(30), || false).expect("not cancelled");
        let (before, after) = (score(&model, &start), score(&model, &optimised));
        assert!(after < 0.95 * before, "{before} -> {after}");
        assert_eq!(optimised.shapes.len(), start.shapes.len());
        assert_eq!(
            (optimised.width, optimised.height, optimised.background),
            (start.width, start.height, start.background)
        );
        for v in vertices(&optimised) {
            assert!(acos_valid(&v), "{v:?}");
            for value in v {
                assert_eq!((value * 4.0).fract(), 0.0, "{value}");
            }
        }
        // The triangles moved, and most stayed near where greedy put them:
        // the order of the layers is the greedy one.
        let near = vertices(&start)
            .iter()
            .zip(vertices(&optimised))
            .filter(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 8.0))
            .count();
        assert!(near >= 10, "{near} of 20 stayed near");
        assert_ne!(vertices(&start), vertices(&optimised));
    }

    #[test]
    fn a_fixed_alpha_stays_fixed() {
        let target = target(40, 32);
        let alpha = crate::test_util::fixed_alpha(128);
        let start = greedy(&target, 10, alpha);
        let optimised = optimise(&start, alpha, settings(10), || false).expect("not cancelled");
        assert!(optimised.shapes.iter().all(|shape| shape.color.a == 128));
        let auto = optimise(&start, Alpha::Auto, settings(10), || false).expect("not cancelled");
        assert!(auto.shapes.iter().any(|shape| shape.color.a != 128));
    }

    /// From valid triangles near the bound, as greedy's are, B exports
    /// none that breaks the rule, by an `acos` check. Triangles projected
    /// just above 15° need the snap's repair, and export none either.
    #[test]
    fn exported_triangles_keep_the_minimum_angle() {
        let mut rng = ChaCha8Rng::seed_from_u64(77);
        let (width, height) = (40, 32);
        let scene = diff::tests::random_scene::<f32>(&mut rng, width, height);
        let mut start = Vec::new();
        while start.len() < 24 {
            let vertices: [f64; 6] = std::array::from_fn(|_| rng.random_range(-2.0..42.0));
            let near_bound = super::angle::tests::acos_angles(&vertices)
                .is_some_and(|angles| angles.iter().any(|&angle| angle < 25.0));
            if acos_valid(&vertices) && near_bound {
                start.push(Layer::triangle(
                    vertices,
                    rng.random_range(60.0..250.0),
                    [128.0; 3],
                ));
            }
        }
        let background = Color::new(40, 120, 200, 255);
        let mut tris = start.clone();
        run(&scene, &mut tris, 40, true, TAN_PROJECTION, &mut || false).expect("not cancelled");
        for v in vertices(&export(&scene, &tris, &[], background)) {
            assert!(acos_valid(&v), "{v:?}");
        }

        // Slivers projected onto 15° + 1e-9°: they sit on the rule's
        // boundary, so most roundings break it.
        let tan_tau = (15.0 + 1e-9_f64).to_radians().tan();
        let mut boundary = start;
        boundary.retain_mut(|tri| {
            let mut v: [f64; 6] = std::array::from_fn(|k| tri.vertices[k]);
            let cy = (v[1] + v[3] + v[5]) / 3.0;
            for k in 0..3 {
                v[2 * k + 1] = cy + 0.1 * (v[2 * k + 1] - cy);
            }
            let projected = angle::project(&mut v, tan_tau);
            tri.vertices[..6].copy_from_slice(&v);
            projected != angle::Projected::Unchanged
        });
        assert!(boundary.len() >= 12, "{} projected", boundary.len());
        let repaired = boundary
            .iter()
            .filter(|tri| angle::snap(&std::array::from_fn(|k| tri.vertices[k]), QUANTUM).1)
            .count();
        assert!(repaired >= 6, "{repaired} repaired");
        for v in vertices(&export(&scene, &boundary, &[], background)) {
            assert!(acos_valid(&v), "{v:?}");
        }
    }

    /// Adam's moments after `steps` steps of the constant gradient
    /// `grad`, and their bias corrections.
    fn moments(grad: &[f64; PARAMS], steps: i32) -> ([f64; PARAMS], [f64; PARAMS], (f64, f64)) {
        let (mut first, mut second) = ([0.0; PARAMS], [0.0; PARAMS]);
        for _ in 0..steps {
            for k in 0..PARAMS {
                first[k] = BETA1 * first[k] + (1.0 - BETA1) * grad[k];
                second[k] = BETA2 * second[k] + (1.0 - BETA2) * grad[k] * grad[k];
            }
        }
        (
            first,
            second,
            (1.0 - BETA1.powi(steps), 1.0 - BETA2.powi(steps)),
        )
    }

    /// A projection spreads its displacement over every vertex, so the
    /// momentum correction moves first moment into coordinates whose
    /// gradients were zero. Their second moments must follow, or Adam's
    /// next step on them is the first moment over `ε`.
    #[test]
    fn the_momentum_correction_never_steps_beyond_the_learning_rate() {
        // A sliver under the bound, flattened by a gradient on its apex's
        // `y` alone.
        let start = [0.0, 0.0, 20.0, 0.0, 10.0, 2.0];
        let mut projected = start;
        assert_eq!(
            angle::project(&mut projected, TAN_PROJECTION),
            angle::Projected::Moved
        );
        let displacement: [f64; 6] = std::array::from_fn(|k| projected[k] - start[k]);
        let (mut first, mut second, corrections) =
            moments(&[0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 10);
        assert!(
            first[5] * displacement[5] > 0.0,
            "the momentum presses into the bound"
        );
        redirect_momentum(&mut first, &mut second, &displacement, corrections);
        for k in 0..PARAMS {
            let update = adam_update(first[k], second[k], corrections);
            assert!(update.abs() <= 1.0 + 1e-12, "coordinate {k}: {update}");
        }
        // The first moment no longer presses into the bound.
        let pressing: f64 = (0..6).map(|k| first[k] * displacement[k]).sum();
        assert!(pressing <= 1e-12, "{pressing}");

        // Random slivers and moments, some coordinates without gradients:
        // no step grows beyond the larger of its own and the step size.
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let mut corrected = 0;
        for _ in 0..500 {
            let start: [f64; 6] = std::array::from_fn(|_| rng.random_range(0.0..30.0));
            let mut projected = start;
            if angle::project(&mut projected, TAN_PROJECTION) == angle::Projected::Unchanged {
                continue;
            }
            let displacement: [f64; 6] = std::array::from_fn(|k| projected[k] - start[k]);
            let grad: [f64; PARAMS] = std::array::from_fn(|_| {
                if rng.random_bool(0.3) {
                    0.0
                } else {
                    rng.random_range(-5.0..5.0)
                }
            });
            let (mut first, mut second, corrections) = moments(&grad, rng.random_range(1..150));
            let before = first;
            let steps: [f64; PARAMS] =
                std::array::from_fn(|k| adam_update(first[k], second[k], corrections));
            redirect_momentum(&mut first, &mut second, &displacement, corrections);
            if first != before {
                corrected += 1;
            }
            for k in 0..PARAMS {
                let update = adam_update(first[k], second[k], corrections).abs();
                assert!(update <= steps[k].abs().max(1.0) + 1e-12, "{k}: {update}");
            }
            let pressing: f64 = (0..6).map(|k| first[k] * displacement[k]).sum();
            let scale: f64 = (0..6).map(|k| (before[k] * displacement[k]).abs()).sum();
            assert!(pressing <= 1e-12 * scale, "{pressing}");
        }
        assert!(corrected >= 50, "{corrected} corrected");
    }

    #[test]
    fn the_result_does_not_depend_on_the_thread_count() {
        // One large triangle, which crosses every band.
        let target = target(120, 100);
        let model = greedy(&target, 6, Alpha::Auto);
        let (_, background, history) = model.joint_parts();
        let mut history = history.to_vec();
        history[0].shape = Shape::Triangle(crate::shapes::Triangle {
            x1: -10,
            y1: -10,
            x2: 130,
            y2: 5,
            x3: 40,
            y3: 110,
        });
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            pool.install(|| {
                optimise_shapes(
                    &target,
                    background,
                    &history,
                    Alpha::Auto,
                    settings(8),
                    || false,
                )
            })
            .expect("not cancelled")
        };
        let one = run(1);
        for threads in [2, 4, 8] {
            assert_eq!(run(threads), one, "{threads} threads");
        }
    }

    /// `cancelled` is polled before every iteration and before the snap;
    /// at any of those polls, cancelling returns `None` and leaves the
    /// input untouched.
    #[test]
    fn cancellation_returns_none_and_leaves_the_input_untouched() {
        let target = target(40, 32);
        let start = greedy(&target, 8, Alpha::Auto);
        let copy = start.drawing();
        let iterations = 6;
        let mut polls = 0;
        let finished = optimise(&start, Alpha::Auto, settings(iterations), || {
            polls += 1;
            false
        });
        assert!(finished.is_some());
        assert!(polls > iterations as usize, "{polls} polls");
        for cancel_at in 1..=polls {
            let mut count = 0;
            let result = optimise(&start, Alpha::Auto, settings(iterations), || {
                count += 1;
                count >= cancel_at
            });
            assert_eq!(result, None, "cancelled at poll {cancel_at}");
            assert_eq!(start.drawing(), copy);
        }
    }

    /// The quadrilaterals of `drawing`, as `x0, y0, …, x3, y3`.
    fn quads(drawing: &Drawing) -> Vec<[f64; COORDS]> {
        drawing
            .shapes
            .iter()
            .filter_map(|shape| match &shape.geometry {
                Geometry::Polygon(points) if points.len() == 4 => Some(std::array::from_fn(|k| {
                    let point = points[k / 2];
                    if k % 2 == 0 { point.x } else { point.y }
                })),
                _ => None,
            })
            .collect()
    }

    /// The exported polygons keep the rule, checked independently (the
    /// diagonals cross, every `acos` angle above 15°) and by greedy's
    /// check, sit on the quarter-pixel lattice, and lower the model's
    /// error.
    #[test]
    fn exported_polygons_keep_the_rule() {
        let target = target(48, 40);
        let model = greedy_kind(&target, 20, ShapeKind::Polygon, Alpha::Auto);
        let optimised =
            optimise(&model, Alpha::Auto, settings(30), || false).expect("not cancelled");
        let (before, after) = (score(&model, &model.drawing()), score(&model, &optimised));
        assert!(after < 0.95 * before, "{before} -> {after}");
        let exported = quads(&optimised);
        assert_eq!(exported.len(), 20);
        assert_ne!(exported, quads(&model.drawing()));
        for v in exported {
            assert!(convex::tests::acos_valid(&v), "{v:?}");
            let polygon = crate::shapes::Polygon {
                order: 4,
                x: [v[0], v[2], v[4], v[6]],
                y: [v[1], v[3], v[5], v[7]],
            };
            assert!(polygon.is_valid(), "{v:?}");
            for value in v {
                assert_eq!((value * 4.0).fract(), 0.0, "{value}");
            }
        }

        // Random quads near the bound and near 180°, run and exported.
        let mut rng = ChaCha8Rng::seed_from_u64(78);
        let scene = diff::tests::random_scene::<f32>(&mut rng, 40, 32);
        let mut layers: Vec<Layer> = convex::tests::random_quads(9, 200)
            .into_iter()
            .filter(|v| convex::is_valid(v) && v.iter().all(|c| (-10.0..50.0).contains(c)))
            .map(|vertices| Layer {
                outline: Outline::Quad,
                vertices,
                alpha: 128.0,
                color: [128.0; 3],
            })
            .collect();
        assert!(layers.len() >= 10, "{}", layers.len());
        run(&scene, &mut layers, 20, true, TAN_PROJECTION, &mut || false).expect("not cancelled");
        for v in quads(&export(&scene, &layers, &[], Color::new(0, 0, 0, 255))) {
            assert!(convex::tests::acos_valid(&v), "{v:?}");
        }
    }

    /// In a drawing of every kind, the triangles and polygons move and
    /// every other shape keeps its geometry exactly, while the colours and
    /// opacities of all of them are optimised.
    #[test]
    fn fixed_shapes_keep_their_geometry() {
        let target = target(64, 48);
        let model = greedy_kind(&target, 40, ShapeKind::Any, Alpha::Auto);
        let start = model.drawing();
        let optimised =
            optimise(&model, Alpha::Auto, settings(20), || false).expect("not cancelled");
        assert_eq!(optimised.shapes.len(), start.shapes.len());
        let (_, _, history) = model.joint_parts();
        let (mut fixed, mut recoloured, mut moved) = (0, 0, 0);
        for ((before, after), committed) in start.shapes.iter().zip(&optimised.shapes).zip(history)
        {
            match committed.shape {
                Shape::Triangle(_) | Shape::Polygon(_) => {
                    if before.geometry != after.geometry {
                        moved += 1;
                    }
                }
                _ => {
                    assert_eq!(before.geometry, after.geometry, "{committed:?}");
                    fixed += 1;
                    if before.color != after.color {
                        recoloured += 1;
                    }
                }
            }
        }
        assert!(fixed >= 10 && moved >= 5, "{fixed} fixed, {moved} moved");
        assert!(
            recoloured * 2 >= fixed,
            "{recoloured} of {fixed} recoloured"
        );
        let (before, after) = (score(&model, &start), score(&model, &optimised));
        assert!(after < before, "{before} -> {after}");
    }

    /// A fixed layer's mask is the engine's coverage: a drawing of fixed
    /// layers alone composites in the model as on the engine's canvas, up
    /// to the engine's rounding of each blend.
    #[test]
    fn fixed_layers_composite_as_the_engine_draws_them() {
        let target = target(64, 48);
        let mut model = Model::new(
            target,
            Color::new(90, 60, 30, 255),
            ModelOptions {
                seed: Some(9),
                ..ModelOptions::default()
            },
        );
        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Circle,
            ShapeKind::RotatedRectangle,
            ShapeKind::Quadratic,
            ShapeKind::RotatedEllipse,
        ]
        .into_iter()
        .cycle()
        .take(18)
        {
            model.step(kind, Alpha::Auto);
        }
        let (engine, joint) = (model.score_f64(), score(&model, &model.drawing()));
        assert!((engine - joint).abs() < 0.002, "{engine} vs {joint}");
    }

    /// A drawing of every kind, with one large triangle and one large
    /// polygon that cross every band, gives the same result at 1, 2, 4
    /// and 8 threads.
    #[test]
    fn a_mixed_result_does_not_depend_on_the_thread_count() {
        let target = target(120, 100);
        let model = greedy_kind(&target, 16, ShapeKind::Any, Alpha::Auto);
        let (_, background, history) = model.joint_parts();
        let mut history = history.to_vec();
        history[0].shape = Shape::Triangle(crate::shapes::Triangle {
            x1: -10,
            y1: -10,
            x2: 130,
            y2: 5,
            x3: 40,
            y3: 110,
        });
        history[1].shape = Shape::Polygon(crate::shapes::Polygon {
            order: 4,
            x: [5.0, 110.0, 100.0, 10.0],
            y: [-5.0, 10.0, 105.0, 90.0],
        });
        assert!(
            history
                .iter()
                .any(|c| !matches!(c.shape, Shape::Triangle(_) | Shape::Polygon(_)))
        );
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            pool.install(|| {
                optimise_shapes(
                    &target,
                    background,
                    &history,
                    Alpha::Auto,
                    settings(8),
                    || false,
                )
            })
            .expect("not cancelled")
        };
        let one = run(1);
        for threads in [2, 4, 8] {
            assert_eq!(run(threads), one, "{threads} threads");
        }
    }

    /// Cancelling at any poll of a mixed drawing returns `None`.
    #[test]
    fn cancelling_a_mixed_drawing_returns_none() {
        let target = target(40, 32);
        let model = greedy_kind(&target, 12, ShapeKind::Any, Alpha::Auto);
        let copy = model.drawing();
        let iterations = 4;
        let mut polls = 0;
        assert!(
            optimise(&model, Alpha::Auto, settings(iterations), || {
                polls += 1;
                false
            })
            .is_some()
        );
        assert_eq!(polls, iterations as usize + 1);
        for cancel_at in 1..=polls {
            let mut count = 0;
            let result = optimise(&model, Alpha::Auto, settings(iterations), || {
                count += 1;
                count >= cancel_at
            });
            assert_eq!(result, None, "cancelled at poll {cancel_at}");
        }
        assert_eq!(model.drawing(), copy);
    }

    /// Without shapes, the score is the background's RMSE against the
    /// target, normalised as the engine's.
    #[test]
    fn the_score_is_the_normalised_rmse() {
        let target = target(30, 20);
        let background = Color::new(90, 60, 30, 255);
        let model = Model::new(target.clone(), background, ModelOptions::default());
        let squares: f64 = target
            .pixels()
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| {
                [background.r, background.g, background.b]
                    .into_iter()
                    .zip(pixel)
                    .map(|(b, &t)| (f64::from(b) - f64::from(t)).powi(2))
            })
            .sum();
        let expected = (squares / (3.0 * 600.0)).sqrt() / 255.0;
        assert!((score(&model, &model.drawing()) - expected).abs() < 1e-12);
    }
}
