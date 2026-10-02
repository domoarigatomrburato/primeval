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
//!   --refine SCHEDULE   refit passes (`Model::refine`, default: none):
//!                       `end:P` runs P passes at each checkpoint,
//!                       `every:K` one pass after every K-th step
//! ```
//!
//! For every image × shape kind it runs one greedy search to the largest
//! checkpoint, with seed 42 and default options otherwise. It drives
//! [`primeval_core::Model`] itself through `primeval_render::lab`, which
//! reproduces `approximate` exactly, so without `--refine` each checkpoint
//! row is what `approximate` returns for that step count. At each
//! checkpoint it records one row.
//!
//! With `--refine end:P`, each checkpoint clones the model, runs `P` refit
//! passes on the clone and records the clone, while the search itself goes
//! on greedily: each row is "greedy to n steps, then P passes". With
//! `--refine every:K` the search itself runs one pass after every `K`-th
//! step, so later steps build on the refitted shapes. Passes get the
//! render's alpha.
//!
//! Progress goes to stderr; stdout gets a header with the commit, the
//! machine, the options and the refine schedule, the rows as a Markdown
//! table sorted by image, shape and steps, and a summary table with one
//! line per checkpoint over all rows, so two runs can be compared at a
//! glance.
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
//!   checkpoint, plus the refit passes: with `every:K` every pass so far,
//!   with `end:P` only this checkpoint's passes. Decoding, the thumbnail,
//!   the clone, the metrics and the encoding are outside the clock.
//! - `refine_s`, only with `--refine`: the part of `search_s` spent in
//!   refit passes.
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
//! `score`, `ssim`, `png_rmse` and `svg_bytes`, and the sum of `search_s`
//! (and of `refine_s` with `--refine`).
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
    refine: Refine,
}

/// When the search runs refit passes; see the doc comment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refine {
    None,
    /// Passes on a clone at each checkpoint.
    End(u32),
    /// One pass after every this many steps.
    Every(u32),
}

impl Refine {
    fn parse(value: &str) -> Result<Self, BoxError> {
        let invalid = || format!("--refine: expected end:P or every:K, got {value}");
        let (schedule, count) = value.split_once(':').ok_or_else(invalid)?;
        let count: u32 = count.parse().map_err(|_| invalid())?;
        if count == 0 {
            return Err(invalid().into());
        }
        match schedule {
            "end" => Ok(Self::End(count)),
            "every" => Ok(Self::Every(count)),
            _ => Err(invalid().into()),
        }
    }

    fn describe(self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::End(passes) => format!("end:{passes} ({passes} passes at each checkpoint)"),
            Self::Every(steps) => format!("every:{steps} (one pass after every {steps} steps)"),
        }
    }
}

struct Row {
    image: String,
    shape: &'static str,
    steps: u32,
    search: Duration,
    refine: Duration,
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
            for checkpoint in search(&input.bytes, shape, &config.checkpoints, config.refine)? {
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
                    refine: checkpoint.refine,
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

    let refined = config.refine != Refine::None;
    print_header(&config.checkpoints, config.refine);
    if refined {
        println!(
            "| image | shape | steps | search_s | refine_s | score | ssim | png_rmse | svg_bytes |"
        );
        println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    } else {
        println!("| image | shape | steps | search_s | score | ssim | png_rmse | svg_bytes |");
        println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    }
    for row in &rows {
        let refine = if refined {
            format!(" {:.3} |", row.refine.as_secs_f64())
        } else {
            String::new()
        };
        println!(
            "| {} | {} | {} | {:.3} |{refine} {:.6} | {:.6} | {:.6} | {} |",
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
    print_summary(&rows, &config.checkpoints, refined);
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, BoxError> {
    let mut photos = Vec::new();
    let mut synthetic = true;
    let mut quick = false;
    let mut shapes = None;
    let mut checkpoints: Option<Vec<u32>> = None;
    let mut refine = Refine::None;
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
            "--refine" => refine = Refine::parse(&value()?)?,
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
        refine,
    })
}

/// The state of one search at one checkpoint.
struct Checkpoint {
    steps: u32,
    search: Duration,
    refine: Duration,
    score: f64,
    rendered: RgbImage,
    svg_bytes: usize,
}

/// Runs one search of `shape` to the last of `checkpoints` (sorted, unique
/// and positive) exactly as `approximate` would, with the refit passes of
/// `refine`, and records each checkpoint.
fn search(
    input: &[u8],
    shape: ShapeKind,
    checkpoints: &[u32],
    refine: Refine,
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
    let mut refined = Duration::ZERO;
    let mut recorded = Vec::with_capacity(checkpoints.len());
    let mut next = checkpoints.iter().copied().peekable();
    for step in 1..=last {
        let start = Instant::now();
        model.step(render.shape, render.alpha);
        search += start.elapsed();
        if let Refine::Every(every) = refine
            && step % every == 0
        {
            let start = Instant::now();
            model.refine(render.alpha);
            let elapsed = start.elapsed();
            search += elapsed;
            refined += elapsed;
        }

        if next.next_if_eq(&step).is_none() {
            continue;
        }
        let refitted;
        let (model, search, refined) = match refine {
            Refine::End(passes) => {
                let mut clone = model.clone();
                let start = Instant::now();
                for _ in 0..passes {
                    clone.refine(render.alpha);
                }
                let elapsed = start.elapsed();
                refitted = clone;
                (&refitted, search + elapsed, elapsed)
            }
            Refine::None | Refine::Every(_) => (&model, search, refined),
        };
        let drawing = model.drawing();
        let png = lab::encode(&drawing, render.output_size, OutputFormat::Png)?.into_bytes();
        let rendered = image::load_from_memory_with_format(&png, ImageFormat::Png)?.to_rgb8();
        let svg_bytes = lab::encode(&drawing, render.output_size, OutputFormat::Svg)?
            .into_bytes()
            .len();
        recorded.push(Checkpoint {
            steps: step,
            search,
            refine: refined,
            score: model.score_f64(),
            rendered,
            svg_bytes,
        });
    }
    Ok(recorded)
}

fn print_header(checkpoints: &[u32], refine: Refine) {
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
    if refine != Refine::None {
        println!("- refine: {}", refine.describe());
    }
    println!();
}

fn print_summary(rows: &[Row], checkpoints: &[u32], refined: bool) {
    println!("Summary per checkpoint, over all rows:");
    println!();
    let (refine_head, refine_rule) = if refined {
        (" total refine_s |", " ---: |")
    } else {
        ("", "")
    };
    println!(
        "| steps | rows | mean score | mean ssim | mean png_rmse | total search_s |{refine_head} \
         mean svg_bytes |"
    );
    println!("| ---: | ---: | ---: | ---: | ---: | ---: |{refine_rule} ---: |");
    for &steps in checkpoints {
        let group: Vec<&Row> = rows.iter().filter(|row| row.steps == steps).collect();
        if group.is_empty() {
            continue;
        }
        let count = group.len() as f64;
        let mean =
            |metric: fn(&Row) -> f64| group.iter().map(|row| metric(row)).sum::<f64>() / count;
        let search: Duration = group.iter().map(|row| row.search).sum();
        let refine = if refined {
            let refine: Duration = group.iter().map(|row| row.refine).sum();
            format!(" {:.3} |", refine.as_secs_f64())
        } else {
            String::new()
        };
        println!(
            "| {steps} | {} | {:.6} | {:.6} | {:.6} | {:.3} |{refine} {:.1} |",
            group.len(),
            mean(|row| row.score),
            mean(|row| row.ssim),
            mean(|row| row.png_rmse),
            search.as_secs_f64(),
            mean(|row| row.svg_bytes as f64)
        );
    }
}
