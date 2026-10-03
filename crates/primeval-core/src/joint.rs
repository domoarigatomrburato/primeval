//! Joint gradient optimisation of a drawing's triangles.
//!
//! The greedy search places one shape at a time with the others fixed.
//! [`optimise`] then moves every vertex of every triangle, and every
//! opacity under [`Alpha::Auto`], at once: Adam on the gradient of the
//! squared error between the composite and the target, through a smooth
//! model of the export's anti-aliased rendering, with every colour refitted
//! in closed form at each iteration. Its result is snapped to a quarter
//! pixel and keeps the engine's minimum angle of 15°.
//!
//! The forward model (`diff.rs`) covers each pixel by the product of the
//! three edges' exact half-plane areas in the pixel square, and composites
//! in `f32`. Reverse-mode gradients replay the layer stack from canvas
//! checkpoints, about `√N` of them for `N` layers, capped in memory as the
//! refit pass's are. The minimum angle is kept by a projection after every
//! step and by the snap (`angle.rs`).
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
//! between platforms. Tests may use them.

mod angle;
mod diff;

use crate::{Alpha, Buffer, Color, Drawing, DrawnShape, Geometry, Point};
use diff::{PARAMS, Real, Scene, Tri, Workspace};

/// The default number of Adam iterations, [`Settings::iterations`].
const DEFAULT_ITERATIONS: u32 = 50;
/// `tan 15.5°`: the projection keeps every angle at least 15.5°, half a
/// degree inside the rule, so that the snap rarely needs a repair.
const TAN_PROJECTION: f64 = 0.277_324_544_059_838_4;

/// Settings of [`optimise`].
///
/// Construct with [`Settings::default`] and set the fields you need.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Adam iterations; default 50. Each costs about one render of the
    /// drawing and its reverse pass. `0` only projects, snaps and refits
    /// the colours.
    pub iterations: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            iterations: DEFAULT_ITERATIONS,
        }
    }
}

/// Optimises every triangle of `drawing` jointly against `target`, on
/// `drawing`'s background, and returns the result, or `None` once
/// `cancelled` returns true.
///
/// See the module documentation for the method.
/// [`Settings::iterations`] Adam iterations move the vertices, and the
/// opacities when `alpha` is [`Alpha::Auto`]; with [`Alpha::Fixed`] every
/// opacity stays at the fixed value. Every colour is refitted at each
/// iteration. The result keeps the number and order of the triangles. Its
/// vertices are multiples of 0.25 px, every angle of every triangle is
/// above 15°, and its opacities and colours are integers.
///
/// `cancelled` is polled before every iteration and before the snap. The
/// result does not depend on the number of threads of the current rayon
/// pool.
///
/// # Panics
///
/// If a shape of `drawing` is not a triangle (a polygon of three points),
/// or `drawing` and `target` differ in size.
#[must_use]
pub fn optimise(
    drawing: &Drawing,
    target: &Buffer,
    alpha: Alpha,
    settings: Settings,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Drawing> {
    let scene = scene::<f32>(drawing, target);
    let mut tris = triangles(drawing);
    let auto_alpha = alpha == Alpha::Auto;
    let iterations = settings.iterations as usize;
    run(
        &scene,
        &mut tris,
        iterations,
        auto_alpha,
        TAN_PROJECTION,
        &mut cancelled,
    )?;
    if cancelled() {
        return None;
    }
    Some(export(&scene, &tris, drawing.background))
}

/// The difference between `drawing`, rendered by [`optimise`]'s model,
/// and `target`: the RMSE over the RGB channels divided by 255, as
/// [`Model::score_f64`](crate::Model::score_f64) measures the engine's
/// canvas.
///
/// # Panics
///
/// As [`optimise`].
#[must_use]
pub fn score(drawing: &Drawing, target: &Buffer) -> f64 {
    let scene = scene::<f32>(drawing, target);
    let loss = diff::loss(&scene, &triangles(drawing), &mut Workspace::default());
    (loss / (3 * scene.width * scene.height) as f64).sqrt() / 255.0
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

/// The target and background of `drawing` for the forward model.
fn scene<F: Real>(drawing: &Drawing, target: &Buffer) -> Scene<F> {
    assert_eq!(
        (drawing.width, drawing.height),
        (target.width(), target.height()),
        "the drawing and the target differ in size"
    );
    let background = drawing.background;
    Scene {
        width: target.width() as usize,
        height: target.height() as usize,
        target: target
            .pixels()
            .iter()
            .map(|&value| F::of(f64::from(value)))
            .collect(),
        background: [background.r, background.g, background.b].map(|c| F::of(f64::from(c))),
        filter: 1.0,
    }
}

/// The triangles of `drawing` in engine coordinates.
fn triangles(drawing: &Drawing) -> Vec<Tri> {
    drawing
        .shapes
        .iter()
        .map(|shape| {
            let Geometry::Polygon(points) = &shape.geometry else {
                panic!("joint optimisation takes triangles only");
            };
            let [a, b, c] = points.as_slice() else {
                panic!("joint optimisation takes triangles only");
            };
            let color = shape.color;
            Tri {
                vertices: [a.x, a.y, b.x, b.y, c.x, c.y].map(|v| v - 0.5),
                alpha: f64::from(color.a),
                color: [color.r, color.g, color.b].map(f64::from),
            }
        })
        .collect()
}

/// The step-size factor at iteration `t` of `iterations`: the smoothstep
/// `1 − (3p² − 2p³)` of the progress `p`, from 1 to 0 with zero slope at
/// both ends, the shape of a half cosine without the cosine.
fn decay(t: usize, iterations: usize) -> f64 {
    let p = t as f64 / iterations.max(1) as f64;
    1.0 - p * p * (3.0 - 2.0 * p)
}

/// Runs `iterations` Adam steps on the vertices and opacities of `tris`
/// (opacities only with `auto_alpha`), each after one [`diff::sweep`] that
/// refits every colour; then one more sweep refits the colours of the
/// final geometry. `cancelled` is polled before every step; `None` once it
/// returns true. Vertices stay within [`MARGIN`] of the canvas unless a
/// projection moves them out.
///
/// Every triangle is projected onto angles of at least `atan tan_tau`
/// ([`angle::project`]) before the first step and after every step, after
/// the clamp to the margin. A projected triangle keeps its second moments
/// and loses the component of its first moment along the projection's
/// displacement when that component pushes back out of the set
/// (`m ← m − (m·d / d·d) d` if `m·d > 0`, the update being `−m`): the
/// momentum keeps sliding along the boundary but stops pressing into it.
fn run<F: Real>(
    scene: &Scene<F>,
    tris: &mut [Tri],
    iterations: usize,
    auto_alpha: bool,
    tan_tau: f64,
    cancelled: &mut impl FnMut() -> bool,
) -> Option<()> {
    let project = |vertices: &mut [f64; 6]| -> Option<[f64; 6]> {
        let before = *vertices;
        match angle::project(vertices, tan_tau) {
            angle::Projected::Unchanged => None,
            angle::Projected::Moved | angle::Projected::Rebuilt => {
                Some(std::array::from_fn(|k| vertices[k] - before[k]))
            }
        }
    };
    for tri in tris.iter_mut() {
        project(&mut tri.vertices);
    }
    let mut work = Workspace::default();
    let mut first = vec![[0.0; PARAMS]; tris.len()];
    let mut second = vec![[0.0; PARAMS]; tris.len()];
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
        let gradients = diff::sweep(scene, tris, true, true, &mut work);
        let decay = decay(t, iterations);
        power1 *= BETA1;
        power2 *= BETA2;
        let (correction1, correction2) = (1.0 - power1, 1.0 - power2);
        for (index, tri) in tris.iter_mut().enumerate() {
            let grad = gradients[index];
            for k in 0..PARAMS {
                if k == 6 && !auto_alpha {
                    continue;
                }
                let m = &mut first[index][k];
                let v = &mut second[index][k];
                *m = BETA1 * *m + (1.0 - BETA1) * grad[k];
                *v = BETA2 * *v + (1.0 - BETA2) * grad[k] * grad[k];
                let update = (*m / correction1) / ((*v / correction2).sqrt() + EPSILON);
                let (value, rate, low, high) = match k {
                    6 => (&mut tri.alpha, LR_ALPHA, 1.0, 255.0),
                    k if k % 2 == 0 => (&mut tri.vertices[k], LR_VERTEX, -MARGIN, max_x),
                    _ => (&mut tri.vertices[k], LR_VERTEX, -MARGIN, max_y),
                };
                *value = (*value - rate * decay * update).clamp(low, high);
            }
            if let Some(displacement) = project(&mut tri.vertices) {
                let m = &mut first[index];
                let md: f64 = (0..6).map(|k| m[k] * displacement[k]).sum();
                let dd: f64 = displacement.iter().map(|d| d * d).sum();
                if md > 0.0 && dd > 0.0 {
                    for k in 0..6 {
                        m[k] -= md / dd * displacement[k];
                    }
                }
            }
        }
    }
    diff::sweep(scene, tris, true, false, &mut work);
    Some(())
}

/// `tris` as a drawing on `background`: vertices snapped to [`QUANTUM`]
/// keeping the rule ([`angle::snap`]), opacities rounded, colours refitted
/// once to the snapped geometry and rounded.
fn export<F: Real>(scene: &Scene<F>, tris: &[Tri], background: Color) -> Drawing {
    let mut snapped: Vec<Tri> = tris
        .iter()
        .map(|tri| Tri {
            vertices: angle::snap(&tri.vertices, QUANTUM).0,
            alpha: tri.alpha.round().clamp(1.0, 255.0),
            color: tri.color,
        })
        .collect();
    diff::sweep(scene, &mut snapped, true, false, &mut Workspace::default());
    Drawing {
        width: scene.width as u32,
        height: scene.height as u32,
        background,
        shapes: snapped
            .iter()
            .map(|tri| {
                let v = tri.vertices.map(|v| v + 0.5);
                let [r, g, b] = tri.color.map(|c| c.round().clamp(0.0, 255.0) as u8);
                DrawnShape {
                    geometry: Geometry::Polygon(vec![
                        Point::new(v[0], v[1]),
                        Point::new(v[2], v[3]),
                        Point::new(v[4], v[5]),
                    ]),
                    color: Color::new(r, g, b, tri.alpha as u8),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::angle::tests::acos_valid;
    use super::*;
    use crate::{Model, ModelOptions, ShapeKind};
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

    fn greedy(target: &Buffer, steps: usize, alpha: Alpha) -> Drawing {
        let options = ModelOptions {
            seed: Some(5),
            ..ModelOptions::default()
        };
        let mut model = Model::new(target.clone(), Color::new(90, 60, 30, 255), options);
        for _ in 0..steps {
            model.step(ShapeKind::Triangle, alpha);
        }
        model.drawing()
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
        Settings { iterations }
    }

    #[test]
    fn the_default_runs_fifty_iterations() {
        assert_eq!(Settings::default().iterations, 50);
    }

    /// B lowers its own model's error against the greedy drawing, keeps
    /// the triangles' number and order, and exports quarter pixels,
    /// integer colours and valid angles.
    #[test]
    fn optimising_lowers_the_model_rmse_of_the_greedy_drawing() {
        let target = target(48, 40);
        let start = greedy(&target, 20, Alpha::Auto);
        let optimised =
            optimise(&start, &target, Alpha::Auto, settings(30), || false).expect("not cancelled");
        let (before, after) = (score(&start, &target), score(&optimised, &target));
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
        let optimised =
            optimise(&start, &target, alpha, settings(10), || false).expect("not cancelled");
        assert!(optimised.shapes.iter().all(|shape| shape.color.a == 128));
        let auto =
            optimise(&start, &target, Alpha::Auto, settings(10), || false).expect("not cancelled");
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
                start.push(diff::Tri {
                    vertices,
                    alpha: rng.random_range(60.0..250.0),
                    color: [128.0; 3],
                });
            }
        }
        let background = Color::new(40, 120, 200, 255);
        let mut tris = start.clone();
        run(&scene, &mut tris, 40, true, TAN_PROJECTION, &mut || false).expect("not cancelled");
        for v in vertices(&export(&scene, &tris, background)) {
            assert!(acos_valid(&v), "{v:?}");
        }

        // Slivers projected onto 15° + 1e-9°: they sit on the rule's
        // boundary, so most roundings break it.
        let tan_tau = (15.0 + 1e-9_f64).to_radians().tan();
        let mut boundary = start;
        boundary.retain_mut(|tri| {
            let v = &mut tri.vertices;
            let cy = (v[1] + v[3] + v[5]) / 3.0;
            for k in 0..3 {
                v[2 * k + 1] = cy + 0.1 * (v[2 * k + 1] - cy);
            }
            angle::project(v, tan_tau) != angle::Projected::Unchanged
        });
        assert!(boundary.len() >= 12, "{} projected", boundary.len());
        let repaired = boundary
            .iter()
            .filter(|tri| angle::snap(&tri.vertices, QUANTUM).1)
            .count();
        assert!(repaired >= 6, "{repaired} repaired");
        for v in vertices(&export(&scene, &boundary, background)) {
            assert!(acos_valid(&v), "{v:?}");
        }
    }

    #[test]
    fn the_result_does_not_depend_on_the_thread_count() {
        // Large triangles, so that the sweeps take their parallel path.
        let target = target(120, 100);
        let mut start = greedy(&target, 6, Alpha::Auto);
        let big = [-10.0, -10.0, 130.0, 5.0, 40.0, 110.0];
        start.shapes[0].geometry = Geometry::Polygon(
            (0..3)
                .map(|k| crate::Point::new(big[2 * k], big[2 * k + 1]))
                .collect(),
        );
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            pool.install(|| optimise(&start, &target, Alpha::Auto, settings(8), || false))
                .expect("not cancelled")
        };
        let one = run(1);
        for threads in [2, 4] {
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
        let copy = start.clone();
        let iterations = 6;
        let mut polls = 0;
        let finished = optimise(&start, &target, Alpha::Auto, settings(iterations), || {
            polls += 1;
            false
        });
        assert!(finished.is_some());
        assert!(polls > iterations as usize, "{polls} polls");
        for cancel_at in 1..=polls {
            let mut count = 0;
            let result = optimise(&start, &target, Alpha::Auto, settings(iterations), || {
                count += 1;
                count >= cancel_at
            });
            assert_eq!(result, None, "cancelled at poll {cancel_at}");
            assert_eq!(start, copy);
        }
    }

    /// Without shapes, the score is the background's RMSE against the
    /// target, normalised as the engine's.
    #[test]
    fn the_score_is_the_normalised_rmse() {
        let target = target(30, 20);
        let background = Color::new(90, 60, 30, 255);
        let empty = Drawing {
            width: 30,
            height: 20,
            background,
            shapes: Vec::new(),
        };
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
        assert!((score(&empty, &target) - expected).abs() < 1e-12);
    }
}
