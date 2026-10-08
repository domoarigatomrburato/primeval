//! Hooks for the engine evaluation runner (`examples/engine.rs`).
//!
//! This module exists only so that runner can drive [`primeval_core::Model`]
//! itself while reproducing [`crate::approximate`] exactly, and score the
//! result. It is compiled only with the `lab` feature (and in this crate's
//! tests), is hidden from the documentation, and is not part of the
//! supported API: it can change or disappear in any release.

use crate::{ApproximateError, ApproximateResult, OutputFormat, RenderOptions, ShapeKind};
use image::RgbImage;
use primeval_core::{Alpha, Buffer, Color, Drawing, Model, joint};

/// The working-resolution target and the resolved background for `input`:
/// option validation, decoding, background resolution, flattening and the
/// thumbnail, exactly as [`crate::approximate`] runs them before it creates
/// its [`primeval_core::Model`].
///
/// # Errors
///
/// The errors [`crate::approximate`] returns before its first step, other
/// than [`ApproximateError::Aborted`].
pub fn working_target(
    input: &[u8],
    render: &RenderOptions,
) -> Result<(Buffer, Color), ApproximateError> {
    crate::working_target(input, render, || Ok(()))
}

/// The pixels of the target [`working_target`] returns, as an image, from
/// the same steps: [`primeval_core::Buffer`] does not expose its pixels.
///
/// # Errors
///
/// The errors of [`working_target`].
pub fn working_image(input: &[u8], render: &RenderOptions) -> Result<RgbImage, ApproximateError> {
    crate::working_image(input, render, || Ok(())).map(|(image, _)| image)
}

/// The stages around the greedy steps of a search: what
/// [`crate::approximate`] runs for a shape kind ([`pipeline`]), or a
/// variant to measure. The same as the crate's own pipeline type, which is
/// not public.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pipeline {
    /// Refit passes of the model during the search.
    pub during: During,
    /// Refit passes of the model after the last step.
    pub refits: Refits,
    /// After the refit passes, the joint optimisation with this multiple of
    /// its default iteration count ([`joint::default_iterations`]), or
    /// `None` for none.
    pub joint: Option<u32>,
    /// The step sizes of every joint optimisation, in the search and in
    /// the final stage.
    pub tuning: joint::Tuning,
    /// Whether every joint optimisation, in the search and in the final
    /// stage, moves the ellipses, circles and rotated ellipses too
    /// ([`joint::Settings::curved`]); `true` in [`pipeline`].
    pub curved: bool,
}

/// When the search runs a refit pass of the model itself, after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum During {
    /// Never.
    Never,
    /// After every step whose number (from 1) is a multiple of this, which
    /// is positive.
    Every(u32),
    /// After step `interval`, then each `max(interval, s / divisor)` steps
    /// after the previous pass, at step `s`. Both are positive.
    Spaced {
        /// The smallest number of steps between two passes.
        interval: u32,
        /// The divisor of the step number.
        divisor: u32,
    },
    /// On [`During::Spaced`]'s schedule, the joint optimisation of the
    /// model in place of the refit pass, its result adopted into the model
    /// ([`Model::adopt`]) if `guard` keeps it. All three numbers are
    /// positive.
    Joint {
        /// The smallest number of steps between two passes.
        interval: u32,
        /// The divisor of the step number.
        divisor: u32,
        /// The Adam iterations of every pass.
        iterations: u32,
        /// What decides whether a pass's result is kept.
        guard: Guard,
    },
}

/// What keeps the result of a joint pass in the search ([`During::Joint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    /// The model's exact canvas, repainted with the adopted shapes, scores
    /// strictly lower.
    Canvas,
    /// The joint result's PNG export at the working size is strictly closer
    /// to the target than the model's drawing's, the final stage's rule.
    Export,
}

/// What [`after_step`] ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// No pass was due.
    Skipped,
    /// A refit pass.
    Refit,
    /// A joint pass ([`During::Joint`]), and whether its result was kept.
    Joint {
        /// Whether the model adopted the joint result.
        kept: bool,
    },
}

impl Pass {
    /// Whether a pass ran.
    #[must_use]
    pub fn ran(self) -> bool {
        self != Self::Skipped
    }
}

/// Which drawing [`final_stage`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// The refitted model's: the pipeline has no joint optimisation.
    Refitted,
    /// The joint optimisation's result, which exports closer to the target
    /// than its input.
    Joint,
    /// The refitted model's, the joint optimisation's input: its result
    /// exports no closer to the target, and the guard kept the input.
    Input,
}

/// The refit passes of the final stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refits {
    /// This many passes.
    Passes(u32),
    /// Passes until one lowers the model's score by less than `min_gain`
    /// ten-thousandths of its score before the pass, at most `cap`.
    Until {
        /// In ten-thousandths: 50 is 0.5%.
        min_gain: u32,
        /// The most passes.
        cap: u32,
    },
}

impl From<crate::pipeline::Pipeline> for Pipeline {
    fn from(pipeline: crate::pipeline::Pipeline) -> Self {
        use crate::pipeline::{During as D, Refits as R};
        Self {
            during: match pipeline.during {
                D::Never => During::Never,
                D::Every(every) => During::Every(every),
                D::Spaced { interval, divisor } => During::Spaced { interval, divisor },
                D::Joint {
                    interval,
                    divisor,
                    iterations,
                    guard,
                } => During::Joint {
                    interval,
                    divisor,
                    iterations,
                    guard: match guard {
                        crate::pipeline::Guard::Canvas => Guard::Canvas,
                        crate::pipeline::Guard::Export => Guard::Export,
                    },
                },
            },
            refits: match pipeline.refits {
                R::Passes(passes) => Refits::Passes(passes),
                R::Until { min_gain, cap } => Refits::Until { min_gain, cap },
            },
            joint: pipeline.joint,
            tuning: pipeline.tuning,
            curved: pipeline.curved,
        }
    }
}

impl From<Pipeline> for crate::pipeline::Pipeline {
    fn from(pipeline: Pipeline) -> Self {
        use crate::pipeline::{During as D, Refits as R};
        Self {
            during: match pipeline.during {
                During::Never => D::Never,
                During::Every(every) => D::Every(every),
                During::Spaced { interval, divisor } => D::Spaced { interval, divisor },
                During::Joint {
                    interval,
                    divisor,
                    iterations,
                    guard,
                } => D::Joint {
                    interval,
                    divisor,
                    iterations,
                    guard: match guard {
                        Guard::Canvas => crate::pipeline::Guard::Canvas,
                        Guard::Export => crate::pipeline::Guard::Export,
                    },
                },
            },
            refits: match pipeline.refits {
                Refits::Passes(passes) => R::Passes(passes),
                Refits::Until { min_gain, cap } => R::Until { min_gain, cap },
            },
            joint: pipeline.joint,
            tuning: pipeline.tuning,
            curved: pipeline.curved,
        }
    }
}

/// [`crate::approximate`]'s pipeline for `shape`.
#[must_use]
pub fn pipeline(shape: ShapeKind) -> Pipeline {
    crate::pipeline::pipeline(shape).into()
}

/// Runs the pass `pipeline` schedules after step `step` (from 1) of
/// `model`, if any, exactly as [`crate::approximate`] runs it, and returns
/// what ran.
pub fn after_step(model: &mut Model, pipeline: Pipeline, step: u32, alpha: Alpha) -> Pass {
    let pipeline = crate::pipeline::Pipeline::from(pipeline);
    let pass = crate::pipeline::after_step(
        model,
        pipeline.during,
        pipeline.tuning,
        pipeline.curved,
        step,
        alpha,
        || false,
    )
    .expect("a pass that is never cancelled finishes");
    match pass {
        crate::pipeline::Pass::Skipped => Pass::Skipped,
        crate::pipeline::Pass::Refit => Pass::Refit,
        crate::pipeline::Pass::Joint { kept } => Pass::Joint { kept },
    }
}

/// Runs the final stage of `pipeline` on `model`, a search after its last
/// greedy step, exactly as [`crate::approximate`] runs it for
/// [`pipeline`]`(render.shape)`: the refit passes, which change `model`,
/// then the joint optimisation, if any, which does not. `iterations`, if
/// set, overrides the joint optimisation's iteration count.
///
/// Returns the drawing [`crate::approximate`] would encode, its score and
/// which drawing it is. The score is, after the joint optimisation, its
/// model's RMSE of that drawing ([`joint::score`]), otherwise the refitted
/// model's [`Model::score_f64`].
pub fn final_stage(
    model: &mut Model,
    render: &RenderOptions,
    pipeline: Pipeline,
    iterations: Option<u32>,
) -> (Drawing, f64, Chosen) {
    let (drawing, chosen) =
        crate::pipeline::final_stage(model, pipeline.into(), render.alpha, iterations, || false)
            .expect("a stage that is never cancelled finishes");
    let chosen = match chosen {
        crate::pipeline::Chosen::Refitted => Chosen::Refitted,
        crate::pipeline::Chosen::Joint => Chosen::Joint,
        crate::pipeline::Chosen::Input => Chosen::Input,
    };
    let score = if pipeline.joint.is_some() {
        joint::score(model, &drawing)
    } else {
        model.score_f64()
    };
    (drawing, score, chosen)
}

/// Encodes `drawing` as [`crate::approximate`] encodes its final drawing.
///
/// # Errors
///
/// [`ApproximateError::Internal`] for allocation and encoding failures.
pub fn encode(
    drawing: &Drawing,
    output_size: u32,
    output: OutputFormat,
) -> Result<ApproximateResult, ApproximateError> {
    crate::encode_output(drawing, output_size, output)
}

/// Mean SSIM over the three RGB channels (Wang et al. 2004).
///
/// Each channel is compared on its own with the paper's parameters: an
/// 11 × 11 Gaussian window with σ = 1.5, applied separably and normalised to
/// sum 1, and `C1 = (0.01 · 255)²`, `C2 = (0.03 · 255)²`. The local means,
/// variances and covariance are computed in `f64` at every position where
/// the window fits inside the image (no padding), and the channel's score
/// is the mean of its SSIM map. The result is the mean of the three channel
/// scores: `1.0` for identical images, lower for less similar ones.
///
/// An image smaller than 11 px on either side is one window: the whole
/// image, uniformly weighted, gives a single SSIM value per channel.
///
/// # Panics
///
/// If the images differ in size.
#[must_use]
pub fn ssim(a: &RgbImage, b: &RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions(), "size mismatch");
    let (width, height) = (a.width() as usize, a.height() as usize);
    let total: f64 = (0..3)
        .map(|channel| {
            let plane = |image: &RgbImage| -> Vec<f64> {
                image
                    .pixels()
                    .map(|pixel| f64::from(pixel[channel]))
                    .collect()
            };
            channel_ssim(&plane(a), &plane(b), width, height)
        })
        .sum();
    total / 3.0
}

/// Side of the SSIM window.
const WINDOW: usize = 11;
/// Standard deviation of the SSIM window's Gaussian.
const SIGMA: f64 = 1.5;
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

/// SSIM of one window from its local statistics: the means, the two
/// second moments `E[x²]`, `E[y²]` and the cross moment `E[xy]`.
fn window_ssim(mx: f64, my: f64, xx: f64, yy: f64, xy: f64) -> f64 {
    let var_x = xx - mx * mx;
    let var_y = yy - my * my;
    let cov = xy - mx * my;
    ((2.0 * mx * my + C1) * (2.0 * cov + C2)) / ((mx * mx + my * my + C1) * (var_x + var_y + C2))
}

/// Mean SSIM of one channel, given as two row-major planes.
fn channel_ssim(x: &[f64], y: &[f64], width: usize, height: usize) -> f64 {
    let products =
        |f: fn(f64, f64) -> f64| -> Vec<f64> { x.iter().zip(y).map(|(&a, &b)| f(a, b)).collect() };
    let planes = [
        x.to_vec(),
        y.to_vec(),
        products(|a, _| a * a),
        products(|_, b| b * b),
        products(|a, b| a * b),
    ];

    if width < WINDOW || height < WINDOW {
        let n = (width * height) as f64;
        let [mx, my, xx, yy, xy] = planes.map(|plane| plane.iter().sum::<f64>() / n);
        return window_ssim(mx, my, xx, yy, xy);
    }

    let kernel = gaussian_kernel();
    let [mx, my, xx, yy, xy] = planes.map(|plane| filter_valid(&plane, width, height, &kernel));
    let positions = mx.len();
    let sum: f64 = (0..positions)
        .map(|i| window_ssim(mx[i], my[i], xx[i], yy[i], xy[i]))
        .sum();
    sum / positions as f64
}

/// The 1-D Gaussian of the SSIM window, normalised to sum 1; its outer
/// product with itself is the 2-D window.
fn gaussian_kernel() -> [f64; WINDOW] {
    let center = (WINDOW / 2) as f64;
    let mut kernel: [f64; WINDOW] = std::array::from_fn(|i| {
        let d = i as f64 - center;
        (-(d * d) / (2.0 * SIGMA * SIGMA)).exp()
    });
    let sum: f64 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= sum;
    }
    kernel
}

/// Filters a row-major plane with the separable window at every position
/// where it fits: the result is `(width - 10) × (height - 10)`, row-major.
fn filter_valid(plane: &[f64], width: usize, height: usize, kernel: &[f64; WINDOW]) -> Vec<f64> {
    let out_width = width - WINDOW + 1;
    let out_height = height - WINDOW + 1;
    let mut rows = Vec::with_capacity(out_width * height);
    for row in plane.chunks_exact(width) {
        rows.extend(
            row.windows(WINDOW)
                .map(|window| window.iter().zip(kernel).map(|(v, k)| v * k).sum::<f64>()),
        );
    }
    let mut out = Vec::with_capacity(out_width * out_height);
    for top in 0..out_height {
        out.extend((0..out_width).map(|col| {
            kernel
                .iter()
                .enumerate()
                .map(|(k, weight)| rows[(top + k) * out_width + col] * weight)
                .sum::<f64>()
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Alpha, ApproximateRequest, BackgroundOption, Execution, approximate};
    use image::{DynamicImage, ImageFormat, Rgb};
    use primeval_core::ModelOptions;
    use std::io::Cursor;

    fn png_bytes(image: &RgbImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image.clone())
            .write_to(&mut out, ImageFormat::Png)
            .expect("fixture png");
        out.into_inner()
    }

    fn fixture() -> RgbImage {
        RgbImage::from_fn(40, 30, |x, y| {
            let noise = (x * 7919 + y * 104_729) % 97;
            Rgb([(x * 6) as u8, (y * 8) as u8, (noise * 2) as u8])
        })
    }

    /// The lab path with `approximate`'s pipeline for `render`: the encoded
    /// output, and the number of refit passes that ran during the search.
    fn lab_render(input: &[u8], render: &RenderOptions, output: OutputFormat) -> (Vec<u8>, u32) {
        let (target, background) = working_target(input, render).expect("working target");
        let mut options = ModelOptions::default();
        options.seed = render.seed;
        let mut model = Model::new(target, background, options);
        let stages = pipeline(render.shape);
        let mut passes = 0;
        for step in 1..=render.count {
            model.step(render.shape, render.alpha);
            passes += u32::from(after_step(&mut model, stages, step, render.alpha).ran());
        }
        let (drawing, ..) = final_stage(&mut model, render, stages, None);
        let bytes = encode(&drawing, render.output_size, output)
            .expect("encode")
            .into_bytes();
        (bytes, passes)
    }

    /// The final stage's score is the joint optimisation's model RMSE of
    /// its drawing for the kinds that end with it, and the refitted
    /// model's score for the other kinds; an iteration count overrides the
    /// joint optimisation's scaled default. It says which drawing it
    /// returned: the refitted model's for a kind without the joint
    /// optimisation, otherwise the joint result or, if the guard rejected
    /// it, the refitted model's.
    #[test]
    fn the_final_stage_scores_its_drawing() {
        let input = png_bytes(&fixture());
        for shape in [
            ShapeKind::Triangle,
            ShapeKind::Polygon,
            ShapeKind::Rectangle,
            ShapeKind::RotatedRectangle,
            ShapeKind::RotatedEllipse,
        ] {
            let render = RenderOptions {
                count: 5,
                seed: Some(11),
                resize_input: 32,
                shape,
                ..RenderOptions::default()
            };
            let (target, background) = working_target(&input, &render).expect("working target");
            let mut options = ModelOptions::default();
            options.seed = render.seed;
            let mut model = Model::new(target, background, options);
            for _ in 0..render.count {
                model.step(render.shape, render.alpha);
            }
            let greedy = model.clone();
            let stages = pipeline(shape);
            let (drawing, score, chosen) = final_stage(&mut model, &render, stages, None);
            if let Some(scale) = stages.joint {
                assert_ne!(shape, ShapeKind::RotatedEllipse);
                assert_eq!(score, primeval_core::joint::score(&model, &drawing));
                match chosen {
                    Chosen::Joint => assert_ne!(drawing, model.drawing(), "{shape:?}"),
                    Chosen::Input => assert_eq!(drawing, model.drawing(), "{shape:?}"),
                    Chosen::Refitted => panic!("{shape:?}: the joint optimisation ran"),
                }
                let (fewer, ..) = final_stage(&mut greedy.clone(), &render, stages, Some(1));
                assert_ne!(fewer, drawing);
                // Without an override, 5 shapes run the rule's 80
                // iterations, times the kind's multiple.
                let iterations = 80 * scale;
                let (rule, ..) =
                    final_stage(&mut greedy.clone(), &render, stages, Some(iterations));
                assert_eq!(rule, drawing);
                let (fewer, ..) =
                    final_stage(&mut greedy.clone(), &render, stages, Some(iterations - 30));
                assert_ne!(fewer, drawing);
            } else {
                assert_eq!(shape, ShapeKind::RotatedEllipse);
                assert_eq!(chosen, Chosen::Refitted);
                assert_eq!(score, model.score_f64());
                assert_eq!(drawing, model.drawing());
                assert!(score <= greedy.score_f64());
            }
        }
    }

    /// For every kind, the lab path with `approximate`'s pipeline and the
    /// model's default search effort gives `approximate`'s exact output.
    /// With 21 shapes every kind's search runs at least one refit pass.
    #[test]
    fn the_lab_path_reproduces_approximate() {
        let input = png_bytes(&fixture());
        let base = RenderOptions {
            count: 21,
            seed: Some(11),
            resize_input: 24,
            output_size: 64,
            ..RenderOptions::default()
        };
        let option_sets = [
            RenderOptions {
                shape: ShapeKind::Triangle,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Polygon,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::RotatedRectangle,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Any,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Circle,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Rectangle,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Quadratic,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::RotatedEllipse,
                ..base
            },
            RenderOptions {
                shape: ShapeKind::Ellipse,
                alpha: Alpha::Fixed(std::num::NonZeroU8::new(160).expect("non-zero")),
                background: BackgroundOption::Color(Color::new(10, 20, 30, 255)),
                seed: Some(12),
                ..base
            },
        ];
        let mut kinds: Vec<_> = option_sets.iter().map(|render| render.shape).collect();
        kinds.sort_by_key(|kind| format!("{kind:?}"));
        kinds.dedup();
        assert_eq!(kinds.len(), 9, "every kind once");
        for render in option_sets {
            // The encoding is shared; one set also checks the PNG.
            let outputs: &[OutputFormat] = if render.shape == ShapeKind::Ellipse {
                &[OutputFormat::Svg, OutputFormat::Png]
            } else {
                &[OutputFormat::Svg]
            };
            for &output in outputs {
                let expected = approximate(
                    ApproximateRequest {
                        input: input.clone(),
                        output,
                        render,
                    },
                    Execution::new(),
                )
                .expect("approximate")
                .into_bytes();
                let (actual, passes) = lab_render(&input, &render, output);
                assert!(passes > 0, "{render:?}: no refit pass in the search");
                assert_eq!(actual, expected, "{render:?} {output:?}");
            }
        }
    }

    #[test]
    fn working_target_validates_options() {
        let input = png_bytes(&fixture());
        let render = RenderOptions {
            count: 0,
            ..RenderOptions::default()
        };
        assert!(matches!(
            working_target(&input, &render),
            Err(ApproximateError::InvalidOption { .. })
        ));
    }

    fn constant(width: u32, height: u32, value: u8) -> RgbImage {
        RgbImage::from_pixel(width, height, Rgb([value; 3]))
    }

    /// `image` with every channel offset by `+amplitude` or `-amplitude`
    /// in a fixed pattern.
    fn perturbed(image: &RgbImage, amplitude: i32) -> RgbImage {
        RgbImage::from_fn(image.width(), image.height(), |x, y| {
            let pixel = image.get_pixel(x, y);
            let offset = if (x * 3 + y * 5) % 7 < 3 {
                amplitude
            } else {
                -amplitude
            };
            Rgb(pixel.0.map(|c| (i32::from(c) + offset).clamp(0, 255) as u8))
        })
    }

    #[test]
    fn ssim_of_identical_images_is_one() {
        let image = fixture();
        assert!((ssim(&image, &image) - 1.0).abs() < 1e-12);
        let small = constant(4, 4, 77);
        assert!((ssim(&small, &small) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn ssim_is_symmetric_and_orders_perturbations() {
        let image = fixture();
        let slight = perturbed(&image, 4);
        let heavy = perturbed(&image, 40);
        let slight_score = ssim(&image, &slight);
        let heavy_score = ssim(&image, &heavy);
        assert_eq!(slight_score, ssim(&slight, &image));
        assert_eq!(heavy_score, ssim(&heavy, &image));
        assert!(slight_score < 1.0, "{slight_score}");
        assert!(
            heavy_score < slight_score,
            "{heavy_score} vs {slight_score}"
        );
        assert!(heavy_score > -1.0, "{heavy_score}");
    }

    /// For two constant images both variances and the covariance are zero,
    /// so SSIM is the luminance term `(2 μx μy + C1) / (μx² + μy² + C1)`.
    #[test]
    fn ssim_of_constant_images_is_the_luminance_term() {
        let expected = (2.0 * 100.0 * 110.0 + C1) / (100.0_f64.powi(2) + 110.0_f64.powi(2) + C1);
        let score = ssim(&constant(16, 13, 100), &constant(16, 13, 110));
        assert!((score - expected).abs() < 1e-9, "{score} vs {expected}");
        // The ratio is far enough from 1 that the test pins the formula.
        assert!(expected < 0.996, "{expected}");
    }

    /// Below 11 px on a side the whole image is one uniformly weighted
    /// window. Against a constant 100, an image whose halves are 100 and
    /// 120 has μy = 110, σy² = 100 and σxy = 0.
    #[test]
    fn ssim_of_a_small_image_uses_one_whole_window() {
        let x = constant(4, 4, 100);
        let y = RgbImage::from_fn(4, 4, |px, _| Rgb([if px < 2 { 100 } else { 120 }; 3]));
        let expected = ((2.0 * 100.0 * 110.0 + C1) * C2)
            / ((100.0_f64.powi(2) + 110.0_f64.powi(2) + C1) * (100.0 + C2));
        let score = ssim(&x, &y);
        assert!((score - expected).abs() < 1e-12, "{score} vs {expected}");
        assert_eq!(score, ssim(&y, &x));
    }

    #[test]
    #[should_panic(expected = "size mismatch")]
    fn ssim_rejects_a_size_mismatch() {
        let _ = ssim(&constant(12, 12, 0), &constant(12, 13, 0));
    }
}
