//! End-to-end time and quality runner.
//!
//! Usage:
//!
//! ```text
//! cargo run --release -p primeval-render --example quality -- [options]
//!
//!   --quick             smoke run: 10 steps, shape kind any
//!   --image PATH        add an input image (repeatable); replaces the
//!                       default paintings
//!   --no-synthetic      skip the generated images
//!   --shapes LIST       comma-separated shape kinds (default: all, plus any)
//!   --steps LIST        comma-separated step counts (default: 100,200)
//! ```
//!
//! For every image × shape kind × step count it runs [`approximate`] with
//! default options, seed 42 and PNG output, and prints one row of a Markdown
//! table to stdout, sorted by image, shape and steps. Progress goes to
//! stderr. The header records the commit and the machine.
//!
//! Corpus: the public-domain paintings `docs/readme/originals/monalisa.jpg`
//! and `americangothic.jpg` ([`DEFAULT_PHOTOS`], a fixed list, so adding a
//! gallery image does not change the benchmark), plus three deterministic
//! 512 × 512 images this tool generates (`synthetic-gradient`,
//! `synthetic-shapes`, `synthetic-texture`). Inputs are expected to be
//! opaque.
//!
//! Columns:
//!
//! - `search_s`: wall time from the start of the call to the last step's
//!   progress report (decode, thumbnail and search).
//! - `total_s`: wall time of the whole call, including the PNG render and
//!   encode at the output size.
//! - `score`: the engine's final score, the normalised RMSE between the
//!   canvas and the target at working resolution, over the RGB channels:
//!   `sqrt(Σ Δ² / (w · h · 3)) / 255`, the same definition as `png_rmse`.
//!   Tables from before the engine became RGB-only measured four RGBA
//!   channels, which gives this value times `sqrt(3 / 4)`.
//! - `png_rmse`: the normalised RMSE over the RGB channels between the
//!   rendered PNG at output size and the input resampled to the same size
//!   (Catmull-Rom, the filter the engine uses for its working thumbnail):
//!   `sqrt(Σ Δ² / (w · h · 3)) / 255`.
//!
//! Lower is better for both metrics. Times vary between runs; the metrics
//! are deterministic for a given commit and platform, whatever the core
//! count (the search runs one worker per logical core, but its result does
//! not depend on how many there are).

use image::{ImageFormat, Rgb, RgbImage, imageops};
use primeval_render::{
    ApproximateRequest, ApproximateResult, Execution, OutputFormat, ProgressInfo, RenderOptions,
    ShapeKind, approximate,
};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SEED: u64 = 42;
const SYNTHETIC_SIZE: u32 = 512;
const ALL_SHAPES: [ShapeKind; 9] = [
    ShapeKind::Any,
    ShapeKind::Triangle,
    ShapeKind::Rectangle,
    ShapeKind::Ellipse,
    ShapeKind::Circle,
    ShapeKind::RotatedRectangle,
    ShapeKind::Quadratic,
    ShapeKind::RotatedEllipse,
    ShapeKind::Polygon,
];
const DEFAULT_PHOTOS: [&str; 2] = ["monalisa.jpg", "americangothic.jpg"];

type BoxError = Box<dyn std::error::Error>;

struct Config {
    photos: Vec<PathBuf>,
    synthetic: bool,
    shapes: Vec<ShapeKind>,
    steps: Vec<u32>,
}

/// One input image: its name in the table and its encoded bytes.
struct Input {
    name: String,
    bytes: Vec<u8>,
}

struct Row {
    image: String,
    shape: &'static str,
    steps: u32,
    search: Duration,
    total: Duration,
    score: f64,
    png_rmse: f64,
}

fn main() -> Result<(), BoxError> {
    let config = parse_args(std::env::args().skip(1))?;
    let mut inputs = Vec::new();
    for path in &config.photos {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("image path has no file name")?
            .to_owned();
        inputs.push(Input {
            name,
            bytes: std::fs::read(path)?,
        });
    }
    if config.synthetic {
        inputs.extend(synthetic_inputs()?);
    }
    inputs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut rows = Vec::new();
    for input in &inputs {
        let original = image::load_from_memory(&input.bytes)?.to_rgb8();
        let mut reference: Option<RgbImage> = None;
        for &shape in &config.shapes {
            for &steps in &config.steps {
                eprintln!("{} {} {steps}", input.name, shape.as_str());
                let run = run(&input.bytes, shape, steps)?;
                let reference = reference.get_or_insert_with(|| {
                    imageops::resize(
                        &original,
                        run.rendered.width(),
                        run.rendered.height(),
                        imageops::FilterType::CatmullRom,
                    )
                });
                rows.push(Row {
                    image: input.name.clone(),
                    shape: shape.as_str(),
                    steps,
                    search: run.search,
                    total: run.total,
                    score: run.score,
                    png_rmse: rgb_rmse(&run.rendered, reference),
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        (&left.image, left.shape, left.steps).cmp(&(&right.image, right.shape, right.steps))
    });

    print_header();
    println!("| image | shape | steps | search_s | total_s | score | png_rmse |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: |");
    for row in &rows {
        println!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.6} | {:.6} |",
            row.image,
            row.shape,
            row.steps,
            row.search.as_secs_f64(),
            row.total.as_secs_f64(),
            row.score,
            row.png_rmse
        );
    }
    let total: Duration = rows.iter().map(|row| row.total).sum();
    println!();
    println!(
        "Total: {:.3} s over {} runs",
        total.as_secs_f64(),
        rows.len()
    );
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, BoxError> {
    let originals = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/readme/originals");
    let mut photos = Vec::new();
    let mut synthetic = true;
    let mut quick = false;
    let mut shapes = None;
    let mut steps = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--quick" => quick = true,
            "--no-synthetic" => synthetic = false,
            "--image" => photos.push(PathBuf::from(value()?)),
            "--shapes" => {
                shapes = Some(
                    value()?
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?,
                );
            }
            "--steps" => {
                steps = Some(
                    value()?
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?,
                );
            }
            other => return Err(format!("unknown argument {other}; see the doc comment").into()),
        }
    }
    if photos.is_empty() {
        photos = DEFAULT_PHOTOS
            .iter()
            .map(|name| originals.join(name))
            .collect();
    }
    let shapes = shapes.unwrap_or_else(|| {
        if quick {
            vec![ShapeKind::Any]
        } else {
            ALL_SHAPES.to_vec()
        }
    });
    let steps = steps.unwrap_or_else(|| if quick { vec![10] } else { vec![100, 200] });
    Ok(Config {
        photos,
        synthetic,
        shapes,
        steps,
    })
}

struct Run {
    search: Duration,
    total: Duration,
    score: f64,
    rendered: RgbImage,
}

fn run(input: &[u8], shape: ShapeKind, steps: u32) -> Result<Run, BoxError> {
    let mut render = RenderOptions::default();
    render.count = steps;
    render.shape = shape;
    render.seed = Some(SEED);

    let start = Instant::now();
    let mut last = None;
    let mut on_progress = |info: ProgressInfo| last = Some((start.elapsed(), info.score));
    let result = approximate(
        ApproximateRequest {
            input: input.to_vec(),
            output: OutputFormat::Png,
            render,
        },
        Execution::new().progress(&mut on_progress),
    )?;
    let total = start.elapsed();
    let (search, score) = last.ok_or("the render reported no steps")?;
    let ApproximateResult::Png { data, .. } = result else {
        return Err("requested PNG output".into());
    };
    let rendered = image::load_from_memory_with_format(&data, ImageFormat::Png)?.to_rgb8();
    Ok(Run {
        search,
        total,
        score,
        rendered,
    })
}

/// Normalised RGB RMSE: `sqrt(Σ Δ² / (w · h · 3)) / 255`.
fn rgb_rmse(left: &RgbImage, right: &RgbImage) -> f64 {
    assert_eq!(left.dimensions(), right.dimensions(), "size mismatch");
    let sum: u64 = left
        .as_raw()
        .iter()
        .zip(right.as_raw())
        .map(|(&a, &b)| u64::from(a.abs_diff(b)).pow(2))
        .sum();
    (sum as f64 / left.as_raw().len() as f64).sqrt() / 255.0
}

fn print_header() {
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty());
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get);
    let defaults = RenderOptions::default();
    println!("# primeval quality run");
    println!();
    println!("- commit: {commit}{}", if dirty { " (dirty)" } else { "" });
    println!(
        "- machine: {} {}, {}, {cores} logical cores",
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu_model().unwrap_or_else(|| "unknown CPU".into())
    );
    println!(
        "- options: seed {SEED}, resize_input {}, output_size {}, alpha auto, background auto, \
         {} threads",
        defaults.resize_input,
        defaults.output_size,
        rayon::current_num_threads()
    );
    println!();
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn cpu_model() -> Option<String> {
    if cfg!(target_os = "macos") {
        let output = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        let model = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return (!model.is_empty()).then_some(model);
    }
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name")?.split_once(':'))
        .map(|(_, model)| model.trim().to_owned())
}

/// Computes one pixel of a generated image.
type Generator = fn(u32, u32) -> Rgb<u8>;

/// The generated images, encoded as PNG.
fn synthetic_inputs() -> Result<Vec<Input>, BoxError> {
    let generators: [(&str, Generator); 3] = [
        ("synthetic-gradient", gradient),
        ("synthetic-shapes", hard_shapes),
        ("synthetic-texture", texture),
    ];
    generators
        .into_iter()
        .map(|(name, pixel)| {
            let image = RgbImage::from_fn(SYNTHETIC_SIZE, SYNTHETIC_SIZE, pixel);
            let mut bytes = Vec::new();
            image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
            Ok(Input {
                name: name.to_owned(),
                bytes,
            })
        })
        .collect()
}

/// Unit coordinates of a synthetic pixel.
fn unit(x: u32, y: u32) -> (f64, f64) {
    let side = f64::from(SYNTHETIC_SIZE);
    (f64::from(x) / side, f64::from(y) / side)
}

fn channel(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A smooth two-axis colour gradient with a radial term.
fn gradient(x: u32, y: u32) -> Rgb<u8> {
    let (u, v) = unit(x, y);
    let radial = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt();
    Rgb([channel(u), channel(v), channel(1.0 - radial * 1.4)])
}

/// Flat-coloured, hard-edged shapes on a flat background.
fn hard_shapes(x: u32, y: u32) -> Rgb<u8> {
    let (u, v) = unit(x, y);
    if (u - 0.3).powi(2) + (v - 0.3).powi(2) < 0.04 {
        Rgb([200, 30, 40])
    } else if (0.55..0.9).contains(&u) && (0.15..0.45).contains(&v) {
        Rgb([30, 90, 200])
    } else if v > 0.55 && v < 0.95 && (u - 0.5).abs() < (v - 0.55) {
        Rgb([240, 200, 30])
    } else if (0.05..0.25).contains(&u) && v > 0.6 {
        Rgb([20, 20, 20])
    } else {
        Rgb([235, 235, 225])
    }
}

/// High-frequency detail: fine stripes, a checkerboard and hashed noise.
fn texture(x: u32, y: u32) -> Rgb<u8> {
    let (u, v) = unit(x, y);
    let stripes = 0.5 + 0.5 * (u * 90.0 + v * 30.0).sin();
    let checker = if (x / 4 + y / 4).is_multiple_of(2) {
        1.0
    } else {
        0.0
    };
    let mut hash = u64::from(x) << 32 | u64::from(y);
    hash = hash.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hash ^= hash >> 29;
    let noise = (hash & 0xFF) as f64 / 255.0;
    Rgb([
        channel(stripes),
        channel(0.25 + 0.5 * checker),
        channel(0.3 * noise + 0.7 * v),
    ])
}
