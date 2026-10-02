//! Engine evaluation runner: quality at fixed shape counts.
//!
//! Usage:
//!
//! ```text
//! cargo run --release -p primeval-render --features lab --example engine -- [options]
//!
//!   --quick             smoke run: shape kind any, checkpoints 10,20
//!   --image PATH        add an input image (repeatable); replaces the
//!                       default paintings
//!   --no-synthetic      skip the generated images
//!   --shapes LIST       comma-separated shape kinds (default: all, plus any)
//!   --steps LIST        comma-separated checkpoints, in steps
//!                       (default: 50,100,200,500)
//! ```
//!
//! For every image × shape kind it runs one greedy search to the largest
//! checkpoint, with seed 42 and default options otherwise. It drives
//! [`primeval_core::Model`] itself through `primeval_render::lab`, which
//! reproduces `approximate` exactly, so each checkpoint row is what
//! `approximate` returns for that step count. At each checkpoint it records
//! one row. Progress goes to stderr; stdout gets a header with the commit,
//! the machine and the options, the rows as a Markdown table sorted by
//! image, shape and steps, and a summary table with one line per
//! checkpoint over all rows, so two runs can be compared at a glance.
//!
//! Corpus: the same as the `quality` runner, the public-domain paintings
//! `docs/readme/originals/monalisa.jpg` and `americangothic.jpg` plus three
//! deterministic 512 × 512 images (`synthetic-gradient`, `synthetic-shapes`,
//! `synthetic-texture`), all in `common/mod.rs`. Inputs are expected to be
//! opaque.
//!
//! Columns:
//!
//! - `search_s`: cumulative wall time of the `Model::step` calls up to this
//!   checkpoint. Decoding, the thumbnail, the metrics and the encoding are
//!   outside the clock.
//! - `score`: `Model::score_f64`, the normalised RMSE between the canvas and
//!   the target at working resolution, over the RGB channels:
//!   `sqrt(Σ Δ² / (w · h · 3)) / 255`. Lower is better.
//! - `ssim`: mean SSIM (Wang et al. 2004) between the PNG output at the
//!   default output size and the input resampled to the same size with
//!   Catmull-Rom, the filter the engine uses for its working thumbnail.
//!   Each RGB channel is scored separately with an 11 × 11 Gaussian window
//!   (σ = 1.5, normalised to sum 1), `K1 = 0.01`, `K2 = 0.03`, `L = 255`,
//!   over the window positions inside the image; the value is the mean of
//!   the three channels' mean SSIM maps (`lab::ssim`). Higher is better;
//!   `1` is identical.
//! - `png_rmse`: the normalised RGB RMSE between the same two images,
//!   `sqrt(Σ Δ² / (w · h · 3)) / 255`, as in the `quality` runner. Lower is
//!   better.
//! - `svg_bytes`: the length in bytes of the SVG output at the default
//!   output size.
//!
//! The summary has, per checkpoint, the number of rows, the means of
//! `score`, `ssim`, `png_rmse` and `svg_bytes`, and the sum of `search_s`.
//!
//! Times vary between runs. Every other column is deterministic for a given
//! commit and platform, whatever the thread count (the search runs one
//! worker per logical core, but its result does not depend on how many
//! there are).

mod common;

use common::{ALL_SHAPES, BoxError, SEED, rgb_rmse};
use image::{ImageFormat, RgbImage, imageops};
use primeval_core::{Model, ModelOptions};
use primeval_render::{OutputFormat, RenderOptions, ShapeKind, lab};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_CHECKPOINTS: [u32; 4] = [50, 100, 200, 500];
const QUICK_CHECKPOINTS: [u32; 2] = [10, 20];

struct Config {
    photos: Vec<PathBuf>,
    synthetic: bool,
    shapes: Vec<ShapeKind>,
    checkpoints: Vec<u32>,
}

struct Row {
    image: String,
    shape: &'static str,
    steps: u32,
    search: Duration,
    score: f64,
    ssim: f64,
    png_rmse: f64,
    svg_bytes: usize,
}

fn main() -> Result<(), BoxError> {
    let config = parse_args(std::env::args().skip(1))?;
    let inputs = common::load_inputs(&config.photos, config.synthetic)?;

    let mut rows = Vec::new();
    for input in &inputs {
        let original = image::load_from_memory(&input.bytes)?.to_rgb8();
        let mut reference: Option<RgbImage> = None;
        for &shape in &config.shapes {
            eprintln!("{} {}", input.name, shape.as_str());
            for checkpoint in search(&input.bytes, shape, &config.checkpoints)? {
                let reference = reference.get_or_insert_with(|| {
                    imageops::resize(
                        &original,
                        checkpoint.rendered.width(),
                        checkpoint.rendered.height(),
                        imageops::FilterType::CatmullRom,
                    )
                });
                rows.push(Row {
                    image: input.name.clone(),
                    shape: shape.as_str(),
                    steps: checkpoint.steps,
                    search: checkpoint.search,
                    score: checkpoint.score,
                    ssim: lab::ssim(&checkpoint.rendered, reference),
                    png_rmse: rgb_rmse(&checkpoint.rendered, reference),
                    svg_bytes: checkpoint.svg_bytes,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        (&left.image, left.shape, left.steps).cmp(&(&right.image, right.shape, right.steps))
    });

    print_header(&config.checkpoints);
    println!("| image | shape | steps | search_s | score | ssim | png_rmse | svg_bytes |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    for row in &rows {
        println!(
            "| {} | {} | {} | {:.3} | {:.6} | {:.6} | {:.6} | {} |",
            row.image,
            row.shape,
            row.steps,
            row.search.as_secs_f64(),
            row.score,
            row.ssim,
            row.png_rmse,
            row.svg_bytes
        );
    }
    println!();
    print_summary(&rows, &config.checkpoints);
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, BoxError> {
    let mut photos = Vec::new();
    let mut synthetic = true;
    let mut quick = false;
    let mut shapes = None;
    let mut checkpoints: Option<Vec<u32>> = None;
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
                checkpoints = Some(
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
        photos = common::default_photos();
    }
    let shapes = shapes.unwrap_or_else(|| {
        if quick {
            vec![ShapeKind::Any]
        } else {
            ALL_SHAPES.to_vec()
        }
    });
    let mut checkpoints = checkpoints.unwrap_or_else(|| {
        if quick {
            QUICK_CHECKPOINTS.to_vec()
        } else {
            DEFAULT_CHECKPOINTS.to_vec()
        }
    });
    checkpoints.sort_unstable();
    checkpoints.dedup();
    if checkpoints.first() == Some(&0) {
        return Err("--steps: checkpoints must be positive".into());
    }
    Ok(Config {
        photos,
        synthetic,
        shapes,
        checkpoints,
    })
}

/// The state of one search at one checkpoint.
struct Checkpoint {
    steps: u32,
    search: Duration,
    score: f64,
    rendered: RgbImage,
    svg_bytes: usize,
}

/// Runs one search of `shape` to the last of `checkpoints` (sorted, unique
/// and positive) exactly as `approximate` would, and records each
/// checkpoint.
fn search(
    input: &[u8],
    shape: ShapeKind,
    checkpoints: &[u32],
) -> Result<Vec<Checkpoint>, BoxError> {
    let last = *checkpoints.last().ok_or("--steps: no checkpoints")?;
    let mut render = RenderOptions::default();
    render.count = last;
    render.shape = shape;
    render.seed = Some(SEED);

    let (target, background) = lab::working_target(input, &render)?;
    let mut options = ModelOptions::default();
    options.seed = render.seed;
    let mut model = Model::new(target, background, options);

    let mut search = Duration::ZERO;
    let mut recorded = Vec::with_capacity(checkpoints.len());
    let mut next = checkpoints.iter().copied().peekable();
    for step in 1..=last {
        let start = Instant::now();
        model.step(render.shape, render.alpha);
        search += start.elapsed();

        if next.next_if_eq(&step).is_none() {
            continue;
        }
        let drawing = model.drawing();
        let png = lab::encode(&drawing, render.output_size, OutputFormat::Png)?.into_bytes();
        let rendered = image::load_from_memory_with_format(&png, ImageFormat::Png)?.to_rgb8();
        let svg_bytes = lab::encode(&drawing, render.output_size, OutputFormat::Svg)?
            .into_bytes()
            .len();
        recorded.push(Checkpoint {
            steps: step,
            search,
            score: model.score_f64(),
            rendered,
            svg_bytes,
        });
    }
    Ok(recorded)
}

fn print_header(checkpoints: &[u32]) {
    let defaults = RenderOptions::default();
    println!("# primeval engine run");
    println!();
    common::print_commit_and_machine();
    println!(
        "- options: seed {SEED}, resize_input {}, output_size {}, alpha auto, background auto, \
         {} threads",
        defaults.resize_input,
        defaults.output_size,
        rayon::current_num_threads()
    );
    let checkpoints: Vec<String> = checkpoints.iter().map(u32::to_string).collect();
    println!("- checkpoints: {} steps", checkpoints.join(", "));
    println!();
}

fn print_summary(rows: &[Row], checkpoints: &[u32]) {
    println!("Summary per checkpoint, over all rows:");
    println!();
    println!(
        "| steps | rows | mean score | mean ssim | mean png_rmse | total search_s | mean svg_bytes |"
    );
    println!("| ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for &steps in checkpoints {
        let group: Vec<&Row> = rows.iter().filter(|row| row.steps == steps).collect();
        if group.is_empty() {
            continue;
        }
        let count = group.len() as f64;
        let mean =
            |metric: fn(&Row) -> f64| group.iter().map(|row| metric(row)).sum::<f64>() / count;
        let search: Duration = group.iter().map(|row| row.search).sum();
        println!(
            "| {steps} | {} | {:.6} | {:.6} | {:.6} | {:.3} | {:.1} |",
            group.len(),
            mean(|row| row.score),
            mean(|row| row.ssim),
            mean(|row| row.png_rmse),
            search.as_secs_f64(),
            mean(|row| row.svg_bytes as f64)
        );
    }
}
