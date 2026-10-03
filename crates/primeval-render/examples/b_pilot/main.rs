//! B pilot: joint gradient-based refinement of greedy triangles.
//!
//! Usage:
//!
//! ```text
//! cargo run --release -p primeval-render --features lab --example b_pilot -- [options]
//!
//!   --coverage          compare the forward model's coverage with tiny-skia's
//!                       on single triangles, then exit
//!   --only NAME         run only the corpus image NAME
//!   --steps LIST        checkpoints, in greedy steps (default 50,100,200)
//!   --iterations LIST   B iteration counts K (default 50,150)
//!   --passes P          A1 baseline: end:1 .. end:P (default 8)
//!   --lr-vertex X       Adam step of the vertices, px (default 1)
//!   --lr-alpha X        Adam step of alpha, levels (default 10)
//!   --filter-start X    initial filter width, annealed to 1 (default 1)
//!   --anneal X          share of the iterations the annealing takes
//!                       (default 0.5)
//! ```
//!
//! For every corpus image it runs one greedy triangle search (seed 42,
//! default options, the `lab` path of the engine runner) to the last
//! checkpoint. At each checkpoint it records:
//!
//! - `greedy`: the greedy drawing;
//! - `end:P`: a clone after `P` refit passes (`Model::refine`), for every
//!   `P` up to `--passes`;
//! - `B<K>@0.25`: `K` Adam iterations on every vertex and alpha jointly
//!   (`diff::optimise`), then vertices snapped to 0.25 px, alphas to
//!   integers, one colour refit and the colours rounded; the export takes
//!   these coordinates as they are, the engine cannot;
//! - `B<K>@1`: the same snapped to whole pixels, painted by the engine
//!   (`Model::from_drawing`);
//! - `B<K>@1+A1`: that model after one refit pass.
//!
//! Times are wall time: `time` is the greedy search to the checkpoint plus
//! the variant's own work (`extra`). Metrics are those of `examples/engine.rs`:
//! `score` on the engine's canvas (none for `@0.25`), `rmse256` of the PNG at
//! the working size, `ssim128`, `svg_bytes` at the default output size.

#[path = "../common/mod.rs"]
mod common;
mod diff;

use common::{ALL_SHAPES, BoxError, SEED, rgb_rmse};
use diff::{Scene, Settings, Tri};
use image::{ImageFormat, RgbImage, imageops};
use primeval_core::{Drawing, DrawnShape, Geometry, Model, ModelOptions, Point};
use primeval_render::{Color, OutputFormat, RenderOptions, ShapeKind, lab};
use std::time::{Duration, Instant};

/// Output size of the `ssim128` column.
const SMALL_SIZE: u32 = 128;
/// How far outside the canvas the engine keeps triangle vertices.
const MARGIN: f64 = 16.0;

struct Config {
    coverage: bool,
    only: Option<String>,
    checkpoints: Vec<u32>,
    iterations: Vec<usize>,
    passes: u32,
    settings: Settings,
}

struct Row {
    image: String,
    variant: String,
    steps: u32,
    time: Duration,
    extra: Duration,
    score: Option<f64>,
    rmse256: f64,
    ssim128: f64,
    svg_bytes: usize,
}

fn main() -> Result<(), BoxError> {
    debug_assert!(ALL_SHAPES.contains(&ShapeKind::Triangle));
    let config = parse_args(std::env::args().skip(1))?;
    if config.coverage {
        coverage_report()?;
        return Ok(());
    }
    let mut inputs = common::load_inputs(&common::default_photos(), true)?;
    if let Some(only) = &config.only {
        inputs.retain(|input| &input.name == only);
    }
    println!("# B pilot run");
    println!();
    common::print_commit_and_machine();
    println!(
        "- triangles, seed {SEED}, default options, {} threads; checkpoints {:?}; \
         K {:?}; passes up to {}",
        rayon::current_num_threads(),
        config.checkpoints,
        config.iterations,
        config.passes
    );
    println!("- Adam: {:?}", config.settings);
    println!();
    let mut rows = Vec::new();
    for input in &inputs {
        eprintln!("{}", input.name);
        run_image(&input.name, &input.bytes, &config, &mut rows)?;
    }
    println!(
        "| image | variant | steps | time_s | extra_s | score | rmse256 | ssim128 | svg_bytes |"
    );
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for row in &rows {
        println!(
            "| {} | {} | {} | {:.3} | {:.3} | {} | {:.6} | {:.6} | {} |",
            row.image,
            row.variant,
            row.steps,
            row.time.as_secs_f64(),
            row.extra.as_secs_f64(),
            row.score
                .map_or("-".to_owned(), |score| format!("{score:.6}")),
            row.rmse256,
            row.ssim128,
            row.svg_bytes
        );
    }
    println!();
    summary(&rows, &config.checkpoints);
    Ok(())
}

/// The metrics of one drawing.
struct Measure<'a> {
    working: &'a RgbImage,
    small_reference: &'a RgbImage,
    output_size: u32,
}

impl Measure<'_> {
    fn measure(&self, drawing: &Drawing) -> Result<(f64, f64, usize), BoxError> {
        let size = self.working.width().max(self.working.height());
        let exported = png(drawing, size)?;
        assert_eq!(exported.dimensions(), self.working.dimensions());
        let small = png(drawing, SMALL_SIZE)?;
        let svg = lab::encode(drawing, self.output_size, OutputFormat::Svg)?.into_bytes();
        Ok((
            rgb_rmse(&exported, self.working),
            lab::ssim(&small, self.small_reference),
            svg.len(),
        ))
    }
}

fn run_image(
    name: &str,
    bytes: &[u8],
    config: &Config,
    rows: &mut Vec<Row>,
) -> Result<(), BoxError> {
    let last = *config.checkpoints.last().ok_or("no checkpoints")?;
    let mut render = RenderOptions::default();
    render.count = last;
    render.shape = ShapeKind::Triangle;
    render.seed = Some(SEED);
    let (target, background) = lab::working_target(bytes, &render)?;
    let working = lab::working_image(bytes, &render)?;
    let original = image::load_from_memory(bytes)?.to_rgb8();
    let mut options = ModelOptions::default();
    options.seed = render.seed;
    let mut model = Model::new(target.clone(), background, options);

    let probe = png(&model.drawing(), SMALL_SIZE)?;
    let small_reference = imageops::resize(
        &original,
        probe.width(),
        probe.height(),
        imageops::FilterType::CatmullRom,
    );
    let measure = Measure {
        working: &working,
        small_reference: &small_reference,
        output_size: render.output_size,
    };
    let mut scene = Scene::<f32> {
        width: working.width() as usize,
        height: working.height() as usize,
        target: working.as_raw().iter().map(|&v| f32::from(v)).collect(),
        background: [background.r, background.g, background.b].map(f32::from),
        filter: 1.0,
    };

    let mut greedy = Duration::ZERO;
    let mut record = |variant: String,
                      steps: u32,
                      greedy: Duration,
                      extra: Duration,
                      score: Option<f64>,
                      drawing: &Drawing|
     -> Result<(), BoxError> {
        let (rmse256, ssim128, svg_bytes) = measure.measure(drawing)?;
        rows.push(Row {
            image: name.to_owned(),
            variant,
            steps,
            time: greedy + extra,
            extra,
            score,
            rmse256,
            ssim128,
            svg_bytes,
        });
        Ok(())
    };
    for step in 1..=last {
        let start = Instant::now();
        model.step(render.shape, render.alpha);
        greedy += start.elapsed();
        if !config.checkpoints.contains(&step) {
            continue;
        }
        eprintln!("  {step} steps");
        record(
            "greedy".into(),
            step,
            greedy,
            Duration::ZERO,
            Some(model.score_f64()),
            &model.drawing(),
        )?;

        let mut refined = model.clone();
        let mut extra = Duration::ZERO;
        for pass in 1..=config.passes {
            let start = Instant::now();
            refined.refine(render.alpha);
            extra += start.elapsed();
            record(
                format!("end:{pass}"),
                step,
                greedy,
                extra,
                Some(refined.score_f64()),
                &refined.drawing(),
            )?;
        }

        for &iterations in &config.iterations {
            let settings = Settings {
                iterations,
                ..config.settings
            };
            let start = Instant::now();
            let drawing = model.drawing();
            let mut tris: Vec<Tri> = drawing.shapes.iter().map(to_tri).collect();
            let losses = diff::optimise(&mut scene, &mut tris, &settings, true, MARGIN);
            let optimised = start.elapsed();
            eprintln!(
                "    B{iterations}: model rmse {:.6} -> {:.6} in {:.2}s",
                model_rmse(losses[0], &scene),
                model_rmse(*losses.last().expect("a final loss"), &scene),
                optimised.as_secs_f64()
            );

            let start = Instant::now();
            let quarter = snap(&mut scene, &tris, 0.25, background);
            let snapped = start.elapsed();
            record(
                format!("B{iterations}@0.25"),
                step,
                greedy,
                optimised + snapped,
                None,
                &quarter,
            )?;

            let start = Instant::now();
            let whole = snap(&mut scene, &tris, 1.0, background);
            let mut rebuilt =
                Model::from_drawing(target.clone(), options, &whole).ok_or("not triangles")?;
            let snapped = start.elapsed();
            record(
                format!("B{iterations}@1"),
                step,
                greedy,
                optimised + snapped,
                Some(rebuilt.score_f64()),
                &rebuilt.drawing(),
            )?;
            let start = Instant::now();
            rebuilt.refine(render.alpha);
            let pass = start.elapsed();
            record(
                format!("B{iterations}@1+A1"),
                step,
                greedy,
                optimised + snapped + pass,
                Some(rebuilt.score_f64()),
                &rebuilt.drawing(),
            )?;
        }
    }
    Ok(())
}

/// The normalised RGB RMSE of a forward-model loss.
fn model_rmse(loss: f64, scene: &Scene<f32>) -> f64 {
    (loss / (3 * scene.width * scene.height) as f64).sqrt() / 255.0
}

/// A drawing's triangle as B's parameters.
fn to_tri(shape: &DrawnShape) -> Tri {
    let Geometry::Polygon(points) = &shape.geometry else {
        panic!("triangles only");
    };
    let [a, b, c] = points.as_slice() else {
        panic!("triangles only");
    };
    Tri {
        vertices: [a.x, a.y, b.x, b.y, c.x, c.y].map(|v| v - 0.5),
        alpha: f64::from(shape.color.a),
        color: [shape.color.r, shape.color.g, shape.color.b].map(f64::from),
    }
}

/// `tris` with vertices rounded to multiples of `quantum` px and alphas to
/// integers, the colours refitted once (one top-down sweep at filter 1)
/// and rounded, as a drawing.
fn snap(scene: &mut Scene<f32>, tris: &[Tri], quantum: f64, background: Color) -> Drawing {
    let mut snapped: Vec<Tri> = tris
        .iter()
        .map(|tri| Tri {
            vertices: tri.vertices.map(|v| (v / quantum).round() * quantum),
            alpha: tri.alpha.round().clamp(1.0, 255.0),
            color: tri.color,
        })
        .collect();
    scene.filter = 1.0;
    diff::sweep(
        scene,
        &mut snapped,
        true,
        false,
        &mut diff::Workspace::default(),
    );
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

/// `drawing` encoded as a PNG at `output_size` and decoded again.
fn png(drawing: &Drawing, output_size: u32) -> Result<RgbImage, BoxError> {
    let bytes = lab::encode(drawing, output_size, OutputFormat::Png)?.into_bytes();
    Ok(image::load_from_memory_with_format(&bytes, ImageFormat::Png)?.to_rgb8())
}

/// The median of `values`, which is not empty.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

fn summary(rows: &[Row], checkpoints: &[u32]) {
    println!("Summary per variant and checkpoint (medians over images; times summed):");
    println!();
    println!(
        "| steps | variant | median score | median rmse256 | median ssim128 | mean svg_bytes \
         | total time_s | total extra_s | mean Δrmse256 vs greedy |"
    );
    println!("| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for &steps in checkpoints {
        let mut variants: Vec<&str> = Vec::new();
        for row in rows.iter().filter(|row| row.steps == steps) {
            if !variants.contains(&row.variant.as_str()) {
                variants.push(&row.variant);
            }
        }
        for variant in variants {
            let group: Vec<&Row> = rows
                .iter()
                .filter(|row| row.steps == steps && row.variant == variant)
                .collect();
            let scores: Vec<f64> = group.iter().filter_map(|row| row.score).collect();
            let score = if scores.len() == group.len() {
                format!("{:.6}", median(scores))
            } else {
                "-".to_owned()
            };
            let change = group
                .iter()
                .map(|row| {
                    let greedy = rows
                        .iter()
                        .find(|other| {
                            other.image == row.image
                                && other.steps == steps
                                && other.variant == "greedy"
                        })
                        .expect("a greedy row");
                    row.rmse256 / greedy.rmse256 - 1.0
                })
                .sum::<f64>()
                / group.len() as f64;
            let time: Duration = group.iter().map(|row| row.time).sum();
            let extra: Duration = group.iter().map(|row| row.extra).sum();
            println!(
                "| {steps} | {variant} | {score} | {:.6} | {:.6} | {:.1} | {:.3} | {:.3} | {:+.2}% |",
                median(group.iter().map(|row| row.rmse256).collect()),
                median(group.iter().map(|row| row.ssim128).collect()),
                group.iter().map(|row| row.svg_bytes as f64).sum::<f64>() / group.len() as f64,
                time.as_secs_f64(),
                extra.as_secs_f64(),
                100.0 * change,
            );
        }
    }
}

/// Compares, on single opaque white triangles on black, the coverage of
/// the forward model (box-filtered half-planes, multiplied), of the
/// linear ramp `clamp(d + 0.5, 0, 1)` multiplied, and of the engine's
/// binary pixel-centre test against tiny-skia's anti-aliased fill through
/// the export path (the PNG at scale 1).
fn coverage_report() -> Result<(), BoxError> {
    use rand::{RngExt, SeedableRng};
    const SIZE: usize = 64;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(7);
    println!("# Forward model against tiny-skia coverage");
    println!();
    println!(
        "Single white triangles on black, {SIZE} × {SIZE}, PNG at scale 1. Edge pixels are those \
         where tiny-skia or the model is strictly between 0 and 1. `area` is Σ model / Σ \
         tiny-skia − 1."
    );
    println!();
    println!(
        "| triangles | vertices | model | mean abs (edge px) | rms (edge px) | max abs | area |"
    );
    println!("| --- | --- | --- | ---: | ---: | ---: | ---: |");
    for size in [3.0, 6.0, 12.0, 24.0, 48.0] {
        for integral in [true, false] {
            let mut stats = [[0.0_f64; 5]; 3];
            let mut skia_area = 0.0;
            for _ in 0..40 {
                let centre = (SIZE as f64 / 2.0, SIZE as f64 / 2.0);
                let tri = loop {
                    let vertices: [f64; 6] = std::array::from_fn(|i| {
                        let c = if i % 2 == 0 { centre.0 } else { centre.1 };
                        let v = c + rng.random_range(-size / 2.0..size / 2.0);
                        if integral { v.round() } else { v }
                    });
                    let v = vertices;
                    let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
                    if cross.abs() > size * size / 8.0 {
                        break Tri {
                            vertices,
                            alpha: 255.0,
                            color: [255.0; 3],
                        };
                    }
                };
                let drawing = Drawing {
                    width: SIZE as u32,
                    height: SIZE as u32,
                    background: Color::new(0, 0, 0, 255),
                    shapes: vec![DrawnShape {
                        geometry: Geometry::Polygon(
                            (0..3)
                                .map(|i| {
                                    Point::new(
                                        tri.vertices[2 * i] + 0.5,
                                        tri.vertices[2 * i + 1] + 0.5,
                                    )
                                })
                                .collect(),
                        ),
                        color: Color::new(255, 255, 255, 255),
                    }],
                };
                let skia = png(&drawing, SIZE as u32)?;
                let prepared = diff::Prepared::<f64>::new(&tri, 1.0, SIZE, SIZE);
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        let reference = f64::from(skia.get_pixel(x as u32, y as u32)[0]) / 255.0;
                        skia_area += reference;
                        let distances = signed_distances(&tri.vertices, x as f64, y as f64);
                        let models = [
                            prepared.coverage(x as f64, y as f64),
                            distances
                                .iter()
                                .map(|d| (d + 0.5).clamp(0.0, 1.0))
                                .product::<f64>(),
                            // The engine's top-left rule: a centre on a left
                            // or top edge is in, on a right or bottom edge out.
                            if signed_distances(&tri.vertices, x as f64 + 1e-6, y as f64 + 1e-9)
                                .iter()
                                .all(|&d| d > 0.0)
                            {
                                1.0
                            } else {
                                0.0
                            },
                        ];
                        for (stat, model) in stats.iter_mut().zip(models) {
                            stat[3] += model;
                            let partial = |v: f64| v > 0.0 && v < 1.0;
                            if partial(reference) || partial(model) {
                                let diff = model - reference;
                                stat[0] += diff.abs();
                                stat[1] += diff * diff;
                                stat[2] = stat[2].max(diff.abs());
                                stat[4] += 1.0;
                            }
                        }
                    }
                }
            }
            for (name, stat) in ["box product (B)", "ramp product", "binary centre"]
                .iter()
                .zip(stats)
            {
                println!(
                    "| 40 of ~{size} px | {} | {name} | {:.4} | {:.4} | {:.3} | {:+.4} |",
                    if integral { "integer" } else { "real" },
                    stat[0] / stat[4],
                    (stat[1] / stat[4]).sqrt(),
                    stat[2],
                    stat[3] / skia_area - 1.0,
                );
            }
        }
    }
    Ok(())
}

/// Signed distances of `(x, y)` from the three edges, positive inside.
fn signed_distances(v: &[f64; 6], x: f64, y: f64) -> [f64; 3] {
    let cross = (v[2] - v[0]) * (v[5] - v[1]) - (v[3] - v[1]) * (v[4] - v[0]);
    let sigma = cross.signum();
    std::array::from_fn(|e| {
        let (p, q) = (e, (e + 1) % 3);
        let (px, py) = (v[2 * p], v[2 * p + 1]);
        let (ex, ey) = (v[2 * q] - px, v[2 * q + 1] - py);
        sigma * (ex * (y - py) - ey * (x - px)) / ex.hypot(ey)
    })
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, BoxError> {
    let list = |value: String| -> Result<Vec<usize>, BoxError> {
        Ok(value.split(',').map(str::parse).collect::<Result<_, _>>()?)
    };
    let mut config = Config {
        coverage: false,
        only: None,
        checkpoints: vec![50, 100, 200],
        iterations: vec![50, 150],
        passes: 8,
        settings: Settings {
            iterations: 0,
            lr_vertex: 1.0,
            lr_alpha: 10.0,
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1e-8,
            filter_start: 1.0,
            anneal: 0.5,
        },
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--coverage" => config.coverage = true,
            "--only" => config.only = Some(value()?),
            "--steps" => {
                config.checkpoints = list(value()?)?.into_iter().map(|v| v as u32).collect();
                config.checkpoints.sort_unstable();
            }
            "--iterations" => config.iterations = list(value()?)?,
            "--passes" => config.passes = value()?.parse()?,
            "--lr-vertex" => config.settings.lr_vertex = value()?.parse()?,
            "--lr-alpha" => config.settings.lr_alpha = value()?.parse()?,
            "--filter-start" => config.settings.filter_start = value()?.parse()?,
            "--anneal" => config.settings.anneal = value()?.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    Ok(config)
}
