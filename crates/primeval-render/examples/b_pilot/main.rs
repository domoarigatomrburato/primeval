//! B pilot: joint gradient-based refinement of greedy triangles, and the
//! ablation that separates its search from its model (S-A1).
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
//!   --passes P          at most P passes per time-matched arm (default 100)
//!   --lr-vertex X       Adam step of the vertices, px (default 1)
//!   --lr-alpha X        Adam step of alpha, levels (default 10)
//!   --filter-start X    initial filter width, annealed to 1 (default 1)
//!   --anneal X          share of the iterations the annealing takes
//!                       (default 0.5)
//!   --constraints LIST  B arms: none, project, penalty (default all three)
//!   --min-degrees X     the projection's bound, at least 15 (default 15.5)
//!   --penalty-threshold X  angle below which the penalty acts, degrees
//!                       (default 17)
//!   --penalty-weight X  the penalty's weight (default 1e7)
//!   --skip-search       skip the S-A1 arms
//!   --save DIR          write every drawing as DIR/<image>-<steps>-<variant>.png
//!                       at 512 px
//! ```
//!
//! For every corpus image it runs one greedy triangle search (seed 42,
//! default options, the `lab` path of the engine runner) to the last
//! checkpoint. At each checkpoint it records:
//!
//! - `greedy`: the greedy drawing;
//! - `B<K>@0.25`: `K` Adam iterations on every vertex and alpha jointly
//!   (`diff::optimise`), then vertices snapped to 0.25 px, alphas to
//!   integers, one colour refit and the colours rounded; the export takes
//!   these coordinates as they are, the engine cannot;
//! - `Bp<K>@0.25`, `Bb<K>@0.25`: the same with the engine's minimum angle
//!   kept (step 9, `angle.rs`): (a) projection after every step, (b) a
//!   penalty on angles near 15° and the projection; the snap then takes
//!   the closest valid lattice triangle where rounding breaks the rule;
//! - `end@Bp<K>`, `end@Bb<K>`: the engine's refit passes matched to their
//!   time;
//! - `end@B<K>`: the engine's refit passes (`Model::refine`), as many as
//!   fit `B<K>@0.25`'s time;
//! - `S-A1@B<K>`: the engine's refit search on B's forward model with
//!   continuous coordinates (`search::pass`), as many passes as fit the
//!   time of `B<K>`'s Adam iterations, then the snap of `B<K>@0.25`;
//! - `S-A1-int@B<K>`: the same with integer coordinates and a snap to whole
//!   pixels, at the smallest `K` only;
//! - `end:P`, `S-A1:P`, `S-A1-int:P`: the same arms after `P` passes, for
//!   `P` in `FIXED_PASSES`, whatever their time.
//!
//! Times are wall time: `time` is the greedy search to the checkpoint plus
//! the variant's own work (`extra`); `passes` is the refit arms' count.
//! Metrics are those of `examples/engine.rs`: `score` on the engine's
//! canvas (none for `@0.25` and `S-A1`), `rmse256` of the PNG at the working
//! size, `ssim128`, `svg_bytes` at the default output size.

mod angle;
#[path = "../common/mod.rs"]
mod common;
mod diff;
mod search;

use common::{ALL_SHAPES, BoxError, SEED, rgb_rmse};
use diff::{Constraint, Scene, Settings, Tri};
use image::{ImageFormat, RgbImage, imageops};
use primeval_core::{Drawing, DrawnShape, Geometry, Model, ModelOptions, Point};
use primeval_render::{Color, OutputFormat, RenderOptions, ShapeKind, lab};
use search::Search;
use std::time::{Duration, Instant};

/// Output size of the `ssim128` column.
const SMALL_SIZE: u32 = 128;
/// Pass counts recorded for the refit arms whatever the time.
const FIXED_PASSES: [usize; 3] = [1, 2, 4];
/// How far outside the canvas the engine keeps triangle vertices.
const MARGIN: f64 = 16.0;

struct Config {
    coverage: bool,
    only: Option<String>,
    checkpoints: Vec<u32>,
    iterations: Vec<usize>,
    passes: u32,
    settings: Settings,
    /// The B arms: unconstrained, (a) projection, (b) penalty and
    /// projection.
    constraints: Vec<Constraint>,
    /// Skip the S-A1 arms.
    skip_search: bool,
    /// Where to write every drawing as a PNG.
    save: Option<std::path::PathBuf>,
}

/// One B arm's time, the budget of its matched arms.
struct Budget {
    label: String,
    constrained: bool,
    iterations: usize,
    /// The Adam iterations alone.
    optimised: Duration,
    /// With the snap.
    total: Duration,
}

/// Upper ends, in degrees, of the bins of the smallest exported angle;
/// the last bin is everything above.
const ANGLE_BINS: [f64; 5] = [15.0, 16.0, 18.0, 20.0, 25.0];

/// The name of a B arm.
fn arm_label(constraint: Constraint) -> &'static str {
    match constraint {
        Constraint::Free => "B",
        Constraint::Project { .. } => "Bp",
        Constraint::Penalty { .. } => "Bb",
    }
}

struct Row {
    image: String,
    variant: String,
    steps: u32,
    time: Duration,
    extra: Duration,
    score: Option<f64>,
    /// Refit passes of the matched arms.
    passes: Option<usize>,
    /// Candidate evaluations of the S-A1 arms.
    evaluations: Option<u64>,
    rmse256: f64,
    ssim128: f64,
    svg_bytes: usize,
    /// Exported triangles that break the engine's minimum angle.
    violations: usize,
    /// Exported triangles per bin of their smallest angle
    /// ([`ANGLE_BINS`]).
    bins: [usize; 6],
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
         K {:?}; at most {} passes per matched arm",
        rayon::current_num_threads(),
        config.checkpoints,
        config.iterations,
        config.passes
    );
    println!("- Adam: {:?}", config.settings);
    println!("- B arms: {:?}", config.constraints);
    println!();
    let mut rows = Vec::new();
    for input in &inputs {
        eprintln!("{}", input.name);
        run_image(&input.name, &input.bytes, &config, &mut rows)?;
    }
    println!(
        "| image | variant | steps | passes | time_s | extra_s | score | rmse256 | ssim128 \
         | svg_bytes | evaluations | violations | min angle ≤15/≤16/≤18/≤20/≤25/>25 |"
    );
    println!(
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |"
    );
    for row in &rows {
        println!(
            "| {} | {} | {} | {} | {:.3} | {:.3} | {} | {:.6} | {:.6} | {} | {} | {} | {} |",
            row.image,
            row.variant,
            row.steps,
            row.passes
                .map_or("-".to_owned(), |passes| passes.to_string()),
            row.time.as_secs_f64(),
            row.extra.as_secs_f64(),
            row.score
                .map_or("-".to_owned(), |score| format!("{score:.6}")),
            row.rmse256,
            row.ssim128,
            row.svg_bytes,
            row.evaluations
                .map_or("-".to_owned(), |evaluations| evaluations.to_string()),
            row.violations,
            bins_text(&row.bins),
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
                      passes: Option<usize>,
                      evaluations: Option<u64>,
                      drawing: &Drawing|
     -> Result<(), BoxError> {
        let (rmse256, ssim128, svg_bytes) = measure.measure(drawing)?;
        if let Some(dir) = &config.save {
            let path = dir.join(format!("{name}-{steps}-{variant}.png"));
            std::fs::write(
                path,
                lab::encode(drawing, 512, OutputFormat::Png)?.into_bytes(),
            )?;
        }
        let angles = exported_angles(drawing);
        let mut bins = [0; 6];
        for &(smallest, _) in &angles {
            bins[ANGLE_BINS.partition_point(|&end| end < smallest)] += 1;
        }
        rows.push(Row {
            violations: angles.iter().filter(|(_, valid)| !valid).count(),
            bins,
            image: name.to_owned(),
            variant,
            steps,
            time: greedy + extra,
            extra,
            score,
            passes,
            evaluations,
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
        let drawing = model.drawing();
        record(
            "greedy".into(),
            step,
            greedy,
            Duration::ZERO,
            Some(model.score_f64()),
            None,
            None,
            &drawing,
        )?;
        let start_tris: Vec<Tri> = drawing.shapes.iter().map(to_tri).collect();

        // B at every K and constraint: its time, without and with the
        // snap, is the budget of the matched arms.
        let mut budgets: Vec<Budget> = Vec::new();
        for &constraint in &config.constraints {
            let constrained = constraint != Constraint::Free;
            for &iterations in &config.iterations {
                let settings = Settings {
                    iterations,
                    constraint,
                    ..config.settings
                };
                let label = format!("{}{iterations}", arm_label(constraint));
                let start = Instant::now();
                let mut tris = start_tris.clone();
                let result = diff::optimise(&mut scene, &mut tris, &settings, true, MARGIN);
                let optimised = start.elapsed();
                let start = Instant::now();
                let (quarter, repaired) = snap(&mut scene, &tris, 0.25, background, constrained);
                let snapped = start.elapsed();
                let losses = &result.losses;
                eprintln!(
                    "    {label}: model rmse {:.6} -> {:.6} in {:.3}s (+{:.3}s snap), \
                     {:.2} ms/iteration; {} of {} triangles with an angle <= 15° before the \
                     snap; {} projections, {} rebuilt, {} snaps repaired",
                    model_rmse(losses[0], &scene),
                    model_rmse(*losses.last().expect("a final loss"), &scene),
                    optimised.as_secs_f64(),
                    snapped.as_secs_f64(),
                    1e3 * optimised.as_secs_f64() / iterations as f64,
                    tris.iter()
                        .filter(|tri| search::min_angle(&tri.vertices) <= 15.0)
                        .count(),
                    tris.len(),
                    result.projections,
                    result.rebuilds,
                    repaired,
                );
                record(
                    format!("{label}@0.25"),
                    step,
                    greedy,
                    optimised + snapped,
                    None,
                    None,
                    None,
                    &quarter,
                )?;
                budgets.push(Budget {
                    label,
                    constrained,
                    iterations,
                    optimised,
                    total: optimised + snapped,
                });
            }
        }
        let longest = budgets
            .iter()
            .map(|budget| budget.total)
            .max()
            .ok_or("no K")?;

        // The engine's refit passes, matched to each B arm's time with its
        // snap.
        let mut refined = model.clone();
        let mut ends = vec![(Duration::ZERO, model.score_f64(), drawing.clone())];
        let mut extra = Duration::ZERO;
        let fixed = *FIXED_PASSES.last().expect("fixed pass counts");
        while (extra <= longest || ends.len() <= fixed) && ends.len() <= config.passes as usize {
            let start = Instant::now();
            refined.refine(render.alpha);
            extra += start.elapsed();
            ends.push((extra, refined.score_f64(), refined.drawing()));
        }
        let mut selected: Vec<(String, usize)> = budgets
            .iter()
            .map(|budget| {
                let passes = ends
                    .iter()
                    .rposition(|end| end.0 <= budget.total)
                    .expect("zero passes fit");
                (format!("end@{}", budget.label), passes)
            })
            .collect();
        selected.extend(
            FIXED_PASSES
                .iter()
                .filter(|&&p| p < ends.len())
                .map(|&p| (format!("end:{p}"), p)),
        );
        for (variant, passes) in selected {
            let (extra, score, drawing) = &ends[passes];
            record(
                variant,
                step,
                greedy,
                *extra,
                Some(*score),
                Some(passes),
                None,
                drawing,
            )?;
        }

        // S-A1, continuous and integer, matched to the unconstrained B's
        // time without the snap, since they pay their own.
        if config.skip_search {
            continue;
        }
        let budgets: Vec<(usize, Duration)> = budgets
            .iter()
            .filter(|budget| !budget.constrained)
            .map(|budget| (budget.iterations, budget.optimised))
            .collect();
        let smallest = *budgets.first().ok_or("S-A1 needs the unconstrained arm")?;
        for (arm, search, budgets) in [
            ("S-A1", Search::continuous(MARGIN, SEED), budgets.as_slice()),
            (
                "S-A1-int",
                Search::integer(MARGIN, SEED),
                std::slice::from_ref(&smallest),
            ),
        ] {
            let longest = budgets.iter().map(|budget| budget.1).max().ok_or("no K")?;
            let mut tris = start_tris.clone();
            let mut states = vec![(Duration::ZERO, 0_u64, tris.clone())];
            let (mut elapsed, mut evaluations) = (Duration::ZERO, 0);
            let fixed = *FIXED_PASSES.last().expect("fixed pass counts");
            while (elapsed <= longest || states.len() <= fixed)
                && states.len() <= config.passes as usize
            {
                let number = states.len() as u64 - 1;
                let start = Instant::now();
                let result = search::pass(&scene, &mut tris, &search, number);
                elapsed += start.elapsed();
                evaluations += result.evaluations;
                eprintln!(
                    "    {arm} pass {}: model rmse {:.6} -> {:.6}, {} layers changed, \
                     {} evaluations, {:.3}s, {:.2} µs/evaluation",
                    number + 1,
                    model_rmse(result.before, &scene),
                    model_rmse(result.after, &scene),
                    result.changed,
                    result.evaluations,
                    elapsed.as_secs_f64(),
                    1e6 * elapsed.as_secs_f64() / evaluations as f64,
                );
                states.push((elapsed, evaluations, tris.clone()));
            }
            let mut selected: Vec<(String, usize)> = budgets
                .iter()
                .map(|&(iterations, budget)| {
                    let passes = states
                        .iter()
                        .rposition(|state| state.0 <= budget)
                        .expect("zero passes fit");
                    (format!("{arm}@B{iterations}"), passes)
                })
                .collect();
            selected.extend(FIXED_PASSES.iter().map(|&p| (format!("{arm}:{p}"), p)));
            for (variant, passes) in selected {
                let (passes_time, evaluations, tris) = &states[passes];
                let start = Instant::now();
                let quantum = if search.integer { 1.0 } else { 0.25 };
                let (drawing, _) = snap(&mut scene, tris, quantum, background, false);
                let snapped = start.elapsed();
                let score = if search.integer {
                    Some(
                        Model::from_drawing(target.clone(), options, &drawing)
                            .ok_or("not integer triangles")?
                            .score_f64(),
                    )
                } else {
                    None
                };
                record(
                    variant,
                    step,
                    greedy,
                    *passes_time + snapped,
                    score,
                    Some(passes),
                    Some(*evaluations),
                    &drawing,
                )?;
            }
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
/// and rounded, as a drawing. With `constrained`, a triangle whose
/// rounding breaks the engine's minimum angle takes the closest valid
/// lattice triangle instead ([`angle::snap`]); the second value counts
/// them.
fn snap(
    scene: &mut Scene<f32>,
    tris: &[Tri],
    quantum: f64,
    background: Color,
    constrained: bool,
) -> (Drawing, usize) {
    let mut repaired = 0;
    let mut snapped: Vec<Tri> = tris
        .iter()
        .map(|tri| Tri {
            vertices: if constrained {
                let (vertices, replaced) = angle::snap(&tri.vertices, quantum);
                repaired += usize::from(replaced);
                vertices
            } else {
                tri.vertices.map(|v| (v / quantum).round() * quantum)
            },
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
    let drawing = Drawing {
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
    };
    (drawing, repaired)
}

/// The smallest angle, in degrees, of every triangle of `drawing` as it is
/// exported, and whether it keeps the engine's rule.
fn exported_angles(drawing: &Drawing) -> Vec<(f64, bool)> {
    drawing
        .shapes
        .iter()
        .map(|shape| {
            let Geometry::Polygon(points) = &shape.geometry else {
                panic!("triangles only");
            };
            let [a, b, c] = points.as_slice() else {
                panic!("triangles only");
            };
            let v = [a.x, a.y, b.x, b.y, c.x, c.y];
            (angle::min_angle(&v).to_degrees(), angle::is_valid(&v))
        })
        .collect()
}

/// `drawing` encoded as a PNG at `output_size` and decoded again.
fn png(drawing: &Drawing, output_size: u32) -> Result<RgbImage, BoxError> {
    let bytes = lab::encode(drawing, output_size, OutputFormat::Png)?.into_bytes();
    Ok(image::load_from_memory_with_format(&bytes, ImageFormat::Png)?.to_rgb8())
}

/// Angle bins as `a/b/c/d/e/f`.
fn bins_text(bins: &[usize; 6]) -> String {
    bins.map(|count| count.to_string()).join("/")
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
         | total time_s | total extra_s | passes | median vs greedy, points | mean Δrmse256 vs \
         greedy | violations | min angle ≤15/≤16/≤18/≤20/≤25/>25 |"
    );
    println!(
        "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |"
    );
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
            let passes: Vec<usize> = group.iter().filter_map(|row| row.passes).collect();
            let passes = match (passes.iter().min(), passes.iter().max()) {
                (Some(low), Some(high)) if low == high => low.to_string(),
                (Some(low), Some(high)) => format!("{low}–{high}"),
                _ => "-".to_owned(),
            };
            let rmse = median(group.iter().map(|row| row.rmse256).collect());
            let greedy = median(
                rows.iter()
                    .filter(|row| row.steps == steps && row.variant == "greedy")
                    .map(|row| row.rmse256)
                    .collect(),
            );
            let mut bins = [0; 6];
            for row in &group {
                for (sum, count) in bins.iter_mut().zip(row.bins) {
                    *sum += count;
                }
            }
            println!(
                "| {steps} | {variant} | {score} | {:.6} | {:.6} | {:.1} | {:.3} | {:.3} | {passes} \
                 | {:+.2} | {:+.2}% | {} | {} |",
                rmse,
                median(group.iter().map(|row| row.ssim128).collect()),
                group.iter().map(|row| row.svg_bytes as f64).sum::<f64>() / group.len() as f64,
                time.as_secs_f64(),
                extra.as_secs_f64(),
                100.0 * (rmse / greedy - 1.0),
                100.0 * change,
                group.iter().map(|row| row.violations).sum::<usize>(),
                bins_text(&bins),
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
        passes: 100,
        constraints: Vec::new(),
        skip_search: false,
        save: None,
        settings: Settings {
            iterations: 0,
            lr_vertex: 1.0,
            lr_alpha: 10.0,
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1e-8,
            filter_start: 1.0,
            anneal: 0.5,
            constraint: Constraint::Free,
        },
    };
    let (mut names, mut min_degrees, mut threshold, mut weight) = (
        vec![
            "none".to_owned(),
            "project".to_owned(),
            "penalty".to_owned(),
        ],
        15.5,
        17.0,
        1e7,
    );
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
            "--constraints" => names = value()?.split(',').map(str::to_owned).collect(),
            "--min-degrees" => min_degrees = value()?.parse()?,
            "--penalty-threshold" => threshold = value()?.parse()?,
            "--penalty-weight" => weight = value()?.parse()?,
            "--skip-search" => config.skip_search = true,
            "--save" => config.save = Some(value()?.into()),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    if !(search::MIN_DEGREES..60.0).contains(&min_degrees) {
        return Err(format!("--min-degrees {min_degrees} is outside 15..60").into());
    }
    for name in names {
        config.constraints.push(match name.as_str() {
            "none" => Constraint::Free,
            "project" => Constraint::Project { min_degrees },
            "penalty" => Constraint::Penalty {
                min_degrees,
                threshold,
                weight,
            },
            other => return Err(format!("unknown constraint {other}").into()),
        });
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// On a small noisy target, from valid triangles as greedy's are, the
    /// unconstrained arm exports slivers and both constrained arms export
    /// none, after the 0.25 px snap and the colour refit.
    #[test]
    fn constrained_arms_export_no_violations() {
        let mut rng = ChaCha8Rng::seed_from_u64(77);
        let (width, height) = (40, 32);
        let mut scene = Scene::<f32> {
            width,
            height,
            target: (0..3 * width * height)
                .map(|_| rng.random_range(0.0..255.0))
                .collect(),
            background: [90.0, 60.0, 30.0],
            filter: 1.0,
        };
        let mut start = Vec::new();
        while start.len() < 24 {
            let vertices: [f64; 6] = std::array::from_fn(|_| rng.random_range(-2.0..42.0));
            if angle::is_valid(&vertices) && angle::min_angle(&vertices) < 25_f64.to_radians() {
                start.push(Tri {
                    vertices,
                    alpha: rng.random_range(60.0..250.0),
                    color: [128.0; 3],
                });
            }
        }
        let background = Color::new(90, 60, 30, 255);
        // No margin: the snap alone has to keep the rule.
        let min_degrees = 15.0 + 1e-9;
        for constraint in [
            Constraint::Free,
            Constraint::Project { min_degrees },
            Constraint::Penalty {
                min_degrees,
                threshold: 20.0,
                weight: 1e6,
            },
        ] {
            let settings = Settings {
                iterations: 40,
                lr_vertex: 1.0,
                lr_alpha: 10.0,
                beta1: 0.9,
                beta2: 0.999,
                epsilon: 1e-8,
                filter_start: 1.0,
                anneal: 0.5,
                constraint,
            };
            let mut tris = start.clone();
            diff::optimise(&mut scene, &mut tris, &settings, true, MARGIN);
            let free = constraint == Constraint::Free;
            let (drawing, repaired) = snap(&mut scene, &tris, 0.25, background, !free);
            let violations = exported_angles(&drawing)
                .iter()
                .filter(|(_, valid)| !valid)
                .count();
            eprintln!(
                "{constraint:?}: {violations} of {} violate, {repaired} snaps repaired",
                tris.len()
            );
            if free {
                assert!(violations > 0);
            } else {
                // Projected triangles sit at the boundary, so some
                // roundings break the rule and need the repair; the
                // penalty keeps them away from it.
                if let Constraint::Project { .. } = constraint {
                    assert!(repaired > 0);
                }
                assert_eq!(violations, 0);
            }
        }
    }
}
