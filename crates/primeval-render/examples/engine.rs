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
//!                       `every:K` one pass after every K-th step,
//!                       `final` runs `approximate`'s final stage at each
//!                       checkpoint
//!   --iterations K      with `--refine final`: the joint optimisation's
//!                       iteration count (default: `approximate`'s,
//!                       which grows with the shape count: 80 up to 50
//!                       triangles, 160 from 500)
//! ```
//!
//! For every image × shape kind it runs one greedy search to the largest
//! checkpoint, with seed 42 and default options otherwise. It drives
//! [`primeval_core::Model`] itself through `primeval_render::lab`, which
//! reproduces `approximate`'s search and encoding exactly. `approximate`
//! ends with its final stage: for triangles the joint gradient
//! optimisation of every shape (`primeval_core::joint`), for every other
//! kind one refit pass. The rows of `--refine final` are what it returns
//! for that step count (and of `--refine end:1` too, for the kinds other
//! than triangles); rows without `--refine` are the greedy search alone. At
//! each checkpoint it records one row.
//!
//! With `--refine end:P`, each checkpoint clones the model, runs `P` refit
//! passes on the clone and records the clone, while the search itself goes
//! on greedily: each row is "greedy to n steps, then P passes". With
//! `--refine final`, each checkpoint runs `approximate`'s final stage on a
//! clone (`lab::final_stage`) and records its drawing, in the same way.
//! With `--refine every:K` the search itself runs one pass after every
//! `K`-th step, so later steps build on the refitted shapes. Passes and the
//! final stage get the render's alpha.
//!
//! Progress goes to stderr; stdout gets a header with the commit, the
//! machine, the options and the refine schedule, the rows as a Markdown
//! table sorted by image, shape and steps, and two summary tables, so two
//! runs can be compared at a glance: one line per checkpoint over all rows,
//! then one line per shape kind × checkpoint.
//!
//! Corpus: the same as the `quality` runner, the public-domain paintings
//! `docs/readme/originals/monalisa.jpg` and `americangothic.jpg` plus three
//! deterministic 512 × 512 images (`synthetic-gradient`, `synthetic-shapes`,
//! `synthetic-texture`), all in `common/mod.rs`. Inputs are expected to be
//! opaque.
//!
//! Terms used below:
//!
//! - the *working target* is the thumbnail the engine optimises, with the
//!   default `resize_input` of 256 (`lab::working_target`; the runner reads
//!   its pixels through `lab::working_image`, which shares its code path). Its longer side is 256 px for every input of the default
//!   corpus; an input already smaller than that is used at its own size.
//! - *normalised RGB RMSE* between two images of the same size is
//!   `sqrt(Σ Δ² / (w · h · 3)) / 255`, over the RGB channels of every
//!   pixel, as in the `quality` runner. Lower is better.
//! - *SSIM* is mean SSIM (Wang et al. 2004): each RGB channel is scored
//!   separately with an 11 × 11 Gaussian window (σ = 1.5, normalised to
//!   sum 1), `K1 = 0.01`, `K2 = 0.03`, `L = 255`, over the window positions
//!   inside the image; the value is the mean of the three channels' mean
//!   SSIM maps (`lab::ssim`). Higher is better; `1` is identical.
//! - *the PNG at size s* is the PNG `approximate` would return for the
//!   drawing with `output_size` s (`lab::encode`): the vector drawing
//!   rendered anti-aliased with tiny-skia, its longer side s px.
//!
//! Columns:
//!
//! - `search_s`: cumulative wall time of the `Model::step` calls up to this
//!   checkpoint, plus the refit passes: with `every:K` every pass so far,
//!   with `end:P` only this checkpoint's passes, with `final` only this
//!   checkpoint's final stage. Decoding, the thumbnail, the clone, the
//!   metrics and the encodings are outside the clock, as are all the
//!   columns below.
//! - `refine_s`, only with `--refine`: the part of `search_s` spent in
//!   refit passes or the final stage.
//! - `score`: `Model::score_f64`, the normalised RGB RMSE between the
//!   engine's own canvas and the working target. Most shape kinds draw on
//!   that canvas with binary (not anti-aliased) coverage. With `--refine
//!   final`, the score `lab::final_stage` returns: for triangles, the joint
//!   optimisation's model RMSE of its exported drawing (`joint::score`), so
//!   `gap` measures how well that model agrees with the export.
//! - `rmse256`: the normalised RGB RMSE between the PNG at the working
//!   target's longer side, which has exactly the working target's
//!   dimensions (scale 1 against the drawing's view box; the runner
//!   asserts the dimensions), and the working target itself. It is
//!   `score` measured on the exported image instead of the engine's canvas.
//! - `gap`: `rmse256 / score − 1`, signed. Positive means the exported image
//!   is further from the target than the engine believes; negative, closer.
//! - `ssim128`: SSIM between the PNG at size 128 and the input resampled to
//!   the same dimensions with Catmull-Rom, the filter the engine uses for
//!   its working thumbnail: the quality at a placeholder-like size.
//! - `ssim1024`: the same at the default output size, 1024 (the `ssim`
//!   column of earlier runs).
//! - `svg_bytes`: the length in bytes of the SVG output at the default
//!   output size.
//!
//! Summaries. The median of an even number of values is the mean of the two
//! middle ones.
//!
//! - Per checkpoint, over all rows: the number of rows, the mean and median
//!   of `score`, the medians of `rmse256` and `gap`, the means of `ssim128`,
//!   `ssim1024` and `svg_bytes`, and the sum of `search_s` (and of
//!   `refine_s` with `--refine`).
//! - Per shape kind × checkpoint, over that kind's rows (one per image),
//!   kinds in the order `any`, `triangle`, `rectangle`, `ellipse`, `circle`,
//!   `rotated-rectangle`, `quadratic`, `rotated-ellipse`, `polygon`: the
//!   medians of `score`, `rmse256`, `gap` and `ssim128`, the mean of
//!   `svg_bytes` and the sum of `search_s`.
//!
//! Times vary between runs. Every other column is deterministic for a given
//! commit and platform, whatever the thread count (the search runs one
//! worker per logical core, but its result does not depend on how many
//! there are).

mod common;

use common::{ALL_SHAPES, BoxError, SEED, rgb_rmse};
use image::{ImageFormat, RgbImage, imageops};
use primeval_core::{Drawing, Model, ModelOptions};
use primeval_render::{OutputFormat, RenderOptions, ShapeKind, lab};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_CHECKPOINTS: [u32; 4] = [50, 100, 200, 500];
const QUICK_CHECKPOINTS: [u32; 2] = [10, 20];
/// Output size of the `ssim128` column.
const SMALL_SIZE: u32 = 128;

struct Config {
    photos: Vec<PathBuf>,
    synthetic: bool,
    shapes: Vec<ShapeKind>,
    checkpoints: Vec<u32>,
    refine: Refine,
    /// The joint optimisation's iteration count with `--refine final`.
    iterations: Option<u32>,
}

/// When the search runs refit passes; see the doc comment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refine {
    None,
    /// Passes on a clone at each checkpoint.
    End(u32),
    /// One pass after every this many steps.
    Every(u32),
    /// `approximate`'s final stage on a clone at each checkpoint.
    Final,
}

impl Refine {
    fn parse(value: &str) -> Result<Self, BoxError> {
        let invalid = || format!("--refine: expected end:P, every:K or final, got {value}");
        if value == "final" {
            return Ok(Self::Final);
        }
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
            Self::Final => "final (approximate's final stage at each checkpoint)".to_owned(),
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
    rmse256: f64,
    gap: f64,
    ssim128: f64,
    ssim1024: f64,
    svg_bytes: usize,
}

fn main() -> Result<(), BoxError> {
    let config = parse_args(std::env::args().skip(1))?;
    let inputs = common::load_inputs(&config.photos, config.synthetic)?;

    let mut rows = Vec::new();
    for input in &inputs {
        let original = image::load_from_memory(&input.bytes)?.to_rgb8();
        let mut small_reference: Option<RgbImage> = None;
        let mut output_reference: Option<RgbImage> = None;
        for &shape in &config.shapes {
            eprintln!("{} {}", input.name, shape.as_str());
            for checkpoint in search(
                &input.bytes,
                shape,
                &config.checkpoints,
                config.refine,
                config.iterations,
            )? {
                let small_reference =
                    small_reference.get_or_insert_with(|| resampled(&original, &checkpoint.small));
                let output_reference = output_reference
                    .get_or_insert_with(|| resampled(&original, &checkpoint.output));
                rows.push(Row {
                    image: input.name.clone(),
                    shape: shape.as_str(),
                    steps: checkpoint.steps,
                    search: checkpoint.search,
                    refine: checkpoint.refine,
                    score: checkpoint.score,
                    rmse256: checkpoint.rmse256,
                    gap: checkpoint.rmse256 / checkpoint.score - 1.0,
                    ssim128: lab::ssim(&checkpoint.small, small_reference),
                    ssim1024: lab::ssim(&checkpoint.output, output_reference),
                    svg_bytes: checkpoint.svg_bytes,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        (&left.image, left.shape, left.steps).cmp(&(&right.image, right.shape, right.steps))
    });

    let refined = config.refine != Refine::None;
    print_header(&config.checkpoints, config.refine, config.iterations);
    let (refine_head, refine_rule) = if refined {
        (" refine_s |", " ---: |")
    } else {
        ("", "")
    };
    println!(
        "| image | shape | steps | search_s |{refine_head} score | rmse256 | gap | ssim128 | \
         ssim1024 | svg_bytes |"
    );
    println!("| --- | --- | ---: | ---: |{refine_rule} ---: | ---: | ---: | ---: | ---: | ---: |");
    for row in &rows {
        let refine = if refined {
            format!(" {:.3} |", row.refine.as_secs_f64())
        } else {
            String::new()
        };
        println!(
            "| {} | {} | {} | {:.3} |{refine} {:.6} | {:.6} | {:+.4} | {:.6} | {:.6} | {} |",
            row.image,
            row.shape,
            row.steps,
            row.search.as_secs_f64(),
            row.score,
            row.rmse256,
            row.gap,
            row.ssim128,
            row.ssim1024,
            row.svg_bytes
        );
    }
    println!();
    print_summary(&rows, &config.checkpoints, refined);
    println!();
    print_kind_summary(&rows, &config.checkpoints);
    Ok(())
}

/// `original` resampled with Catmull-Rom to the dimensions of `rendered`.
fn resampled(original: &RgbImage, rendered: &RgbImage) -> RgbImage {
    imageops::resize(
        original,
        rendered.width(),
        rendered.height(),
        imageops::FilterType::CatmullRom,
    )
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Config, BoxError> {
    let mut photos = Vec::new();
    let mut synthetic = true;
    let mut quick = false;
    let mut shapes = None;
    let mut checkpoints: Option<Vec<u32>> = None;
    let mut refine = Refine::None;
    let mut iterations = None;
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
            "--iterations" => iterations = Some(value()?.parse()?),
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
    if iterations.is_some() && refine != Refine::Final {
        return Err("--iterations needs --refine final".into());
    }
    Ok(Config {
        photos,
        synthetic,
        shapes,
        checkpoints,
        refine,
        iterations,
    })
}

/// The state of one search at one checkpoint.
struct Checkpoint {
    steps: u32,
    search: Duration,
    refine: Duration,
    score: f64,
    rmse256: f64,
    /// The PNG at [`SMALL_SIZE`].
    small: RgbImage,
    /// The PNG at the default output size.
    output: RgbImage,
    svg_bytes: usize,
}

/// Runs one search of `shape` to the last of `checkpoints` (sorted, unique
/// and positive) exactly as `approximate` would, with the refit passes or
/// final stage of `refine` (`iterations` overriding the joint
/// optimisation's), and records each checkpoint.
fn search(
    input: &[u8],
    shape: ShapeKind,
    checkpoints: &[u32],
    refine: Refine,
    iterations: Option<u32>,
) -> Result<Vec<Checkpoint>, BoxError> {
    let last = *checkpoints.last().ok_or("--steps: no checkpoints")?;
    let mut render = RenderOptions::default();
    render.count = last;
    render.shape = shape;
    render.seed = Some(SEED);

    let (target, background) = lab::working_target(input, &render)?;
    let working = lab::working_image(input, &render)?;
    let working_size = working.width().max(working.height());
    let mut options = ModelOptions::default();
    options.seed = render.seed;
    let mut model = Model::new(target.clone(), background, options);

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
        let (drawing, score, search, refined) = match refine {
            Refine::End(passes) => {
                let mut clone = model.clone();
                let start = Instant::now();
                for _ in 0..passes {
                    clone.refine(render.alpha);
                }
                let elapsed = start.elapsed();
                (
                    clone.drawing(),
                    clone.score_f64(),
                    search + elapsed,
                    elapsed,
                )
            }
            Refine::Final => {
                let mut clone = model.clone();
                let start = Instant::now();
                let (drawing, score) = lab::final_stage(&mut clone, &target, &render, iterations);
                let elapsed = start.elapsed();
                (drawing, score, search + elapsed, elapsed)
            }
            Refine::None | Refine::Every(_) => {
                (model.drawing(), model.score_f64(), search, refined)
            }
        };
        let exported = png(&drawing, working_size)?;
        assert_eq!(
            exported.dimensions(),
            working.dimensions(),
            "the PNG at the working size must have the working target's dimensions"
        );
        let svg_bytes = lab::encode(&drawing, render.output_size, OutputFormat::Svg)?
            .into_bytes()
            .len();
        recorded.push(Checkpoint {
            steps: step,
            search,
            refine: refined,
            score,
            rmse256: rgb_rmse(&exported, &working),
            small: png(&drawing, SMALL_SIZE)?,
            output: png(&drawing, render.output_size)?,
            svg_bytes,
        });
    }
    Ok(recorded)
}

/// `drawing` encoded as a PNG at `output_size` and decoded again.
fn png(drawing: &Drawing, output_size: u32) -> Result<RgbImage, BoxError> {
    let bytes = lab::encode(drawing, output_size, OutputFormat::Png)?.into_bytes();
    Ok(image::load_from_memory_with_format(&bytes, ImageFormat::Png)?.to_rgb8())
}

fn print_header(checkpoints: &[u32], refine: Refine, iterations: Option<u32>) {
    let defaults = RenderOptions::default();
    println!("# primeval engine run");
    println!();
    common::print_commit_and_machine();
    println!(
        "- options: seed {SEED}, resize_input {}, output_size {} (ssim128 at {SMALL_SIZE}), \
         alpha auto, background auto, {} threads",
        defaults.resize_input,
        defaults.output_size,
        rayon::current_num_threads()
    );
    let checkpoints: Vec<String> = checkpoints.iter().map(u32::to_string).collect();
    println!("- checkpoints: {} steps", checkpoints.join(", "));
    if refine != Refine::None {
        println!("- refine: {}", refine.describe());
    }
    if let Some(iterations) = iterations {
        println!("- joint optimisation: {iterations} iterations");
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
        "| steps | rows | mean score | median score | median rmse256 | median gap | mean ssim128 \
         | mean ssim1024 | mean svg_bytes | total search_s |{refine_head}"
    );
    println!(
        "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |{refine_rule}"
    );
    for &steps in checkpoints {
        let group: Vec<&Row> = rows.iter().filter(|row| row.steps == steps).collect();
        if group.is_empty() {
            continue;
        }
        let search: Duration = group.iter().map(|row| row.search).sum();
        let refine = if refined {
            let refine: Duration = group.iter().map(|row| row.refine).sum();
            format!(" {:.3} |", refine.as_secs_f64())
        } else {
            String::new()
        };
        println!(
            "| {steps} | {} | {:.6} | {:.6} | {:.6} | {:+.4} | {:.6} | {:.6} | {:.1} | {:.3} |{refine}",
            group.len(),
            mean(&group, |row| row.score),
            median(&group, |row| row.score),
            median(&group, |row| row.rmse256),
            median(&group, |row| row.gap),
            mean(&group, |row| row.ssim128),
            mean(&group, |row| row.ssim1024),
            mean(&group, |row| row.svg_bytes as f64),
            search.as_secs_f64(),
        );
    }
}

fn print_kind_summary(rows: &[Row], checkpoints: &[u32]) {
    println!("Summary per shape kind and checkpoint:");
    println!();
    println!(
        "| shape | steps | median score | median rmse256 | median gap | median ssim128 \
         | mean svg_bytes | total search_s |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for shape in ALL_SHAPES.map(ShapeKind::as_str) {
        for &steps in checkpoints {
            let group: Vec<&Row> = rows
                .iter()
                .filter(|row| row.shape == shape && row.steps == steps)
                .collect();
            if group.is_empty() {
                continue;
            }
            let search: Duration = group.iter().map(|row| row.search).sum();
            println!(
                "| {shape} | {steps} | {:.6} | {:.6} | {:+.4} | {:.6} | {:.1} | {:.3} |",
                median(&group, |row| row.score),
                median(&group, |row| row.rmse256),
                median(&group, |row| row.gap),
                median(&group, |row| row.ssim128),
                mean(&group, |row| row.svg_bytes as f64),
                search.as_secs_f64(),
            );
        }
    }
}

/// The mean of `metric` over `group`, which is not empty.
fn mean(group: &[&Row], metric: fn(&Row) -> f64) -> f64 {
    group.iter().map(|row| metric(row)).sum::<f64>() / group.len() as f64
}

/// The median of `metric` over `group`, which is not empty: the middle
/// value, or the mean of the two middle values for an even count.
fn median(group: &[&Row], metric: fn(&Row) -> f64) -> f64 {
    let mut values: Vec<f64> = group.iter().map(|row| metric(row)).collect();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}
