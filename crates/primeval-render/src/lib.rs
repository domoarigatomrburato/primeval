//! High-level render facade for `primeval`.
//!
//! This crate handles the full decode -> optimize -> encode path on top of
//! `primeval-core`. It takes encoded image bytes (JPEG, PNG, or WebP), runs the
//! approximation search, and returns SVG or PNG output. Reading files is up to
//! the caller.
//!
//! The Node package and napi binding in this repository build on the same API.
//! If you want the canonical Rust-side defaults and validation behavior, this is
//! the crate to call.
//!
//! # Example
//!
//! ```no_run
//! use primeval_render::{
//!     approximate, ApproximateRequest, ApproximateResult, Execution, OutputFormat,
//!     ProgressInfo, RenderOptions,
//! };
//!
//! let mut render = RenderOptions::default();
//! render.count = 100;
//! render.resize_input = 128;
//! render.output_size = 512;
//!
//! let mut steps = Vec::new();
//! let mut on_progress = |info: ProgressInfo| steps.push(info.step);
//! let result = approximate(
//!     ApproximateRequest {
//!         input: std::fs::read("photo.jpg")?,
//!         output: OutputFormat::Svg,
//!         render,
//!     },
//!     Execution::new().progress(&mut on_progress),
//! )?;
//!
//! match result {
//!     ApproximateResult::Svg { data, .. } => std::fs::write("out.svg", data)?,
//!     _ => unreachable!("requested svg output"),
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod input;
mod output;
mod raster;
mod svg;

use image::{DynamicImage, RgbaImage};
use input::{average_background, thumbnail};
use output::output_dimensions;
use primeval_core::{Buffer, Drawing, Model, ModelOptions};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use output::OutputFormat;
pub use primeval_core::{Alpha, Color, ParseError, ShapeKind};

/// Background color strategy for the initial canvas.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackgroundOption {
    /// Derive the background color from the input image.
    #[default]
    Auto,
    /// Use an explicit opaque color (alpha 255).
    Color(Color),
}

impl FromStr for BackgroundOption {
    type Err = ParseError;

    /// Parses `auto` (any ASCII case) or an opaque hex color (`RGB` or
    /// `RRGGBB`, optional leading `#`).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }

        Color::from_hex(value).map(Self::Color).ok_or_else(|| {
            ParseError::new("background must be auto or an opaque hex color (RGB or RRGGBB)")
        })
    }
}

/// Render-time knobs that control optimization and final export.
///
/// Start from [`RenderOptions::default`] and set the fields you need, or
/// apply a [`PartialRenderOptions`] with [`RenderOptions::merge`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    /// Number of optimization steps.
    pub count: u32,
    /// Shape family to search during each step.
    pub shape: ShapeKind,
    /// Alpha handling for new shapes.
    pub alpha: Alpha,
    /// Deterministic RNG seed. `None` chooses a non-deterministic seed.
    pub seed: Option<u64>,
    /// Background fill strategy.
    pub background: BackgroundOption,
    /// Working resolution used during optimization.
    pub resize_input: u32,
    /// Maximum dimension of the final output replay.
    pub output_size: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            count: 100,
            shape: ShapeKind::Any,
            alpha: Alpha::Auto,
            seed: None,
            background: BackgroundOption::Auto,
            resize_input: 256,
            output_size: 1024,
        }
    }
}

impl RenderOptions {
    /// Applies every field `partial` sets; fields it leaves `None` keep
    /// their value in `self`.
    #[must_use]
    pub fn merge(self, partial: PartialRenderOptions) -> Self {
        let PartialRenderOptions {
            count,
            shape,
            alpha,
            seed,
            background,
            resize_input,
            output_size,
        } = partial;
        Self {
            count: count.unwrap_or(self.count),
            shape: shape.unwrap_or(self.shape),
            alpha: alpha.unwrap_or(self.alpha),
            seed: seed.or(self.seed),
            background: background.unwrap_or(self.background),
            resize_input: resize_input.unwrap_or(self.resize_input),
            output_size: output_size.unwrap_or(self.output_size),
        }
    }
}

/// [`RenderOptions`] where every field is optional, for callers that pass
/// user input through and leave omitted fields to the Rust defaults.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartialRenderOptions {
    pub count: Option<u32>,
    pub shape: Option<ShapeKind>,
    pub alpha: Option<Alpha>,
    pub seed: Option<u64>,
    pub background: Option<BackgroundOption>,
    pub resize_input: Option<u32>,
    pub output_size: Option<u32>,
}

/// Full request for a single rendered output.
#[derive(Clone, Debug, PartialEq)]
pub struct ApproximateRequest {
    /// Encoded image bytes in JPEG, PNG, or WebP format.
    pub input: Vec<u8>,
    pub output: OutputFormat,
    pub render: RenderOptions,
}

/// Per-step progress information emitted during optimization.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressInfo {
    pub step: u32,
    pub total: u32,
    pub score: f64,
}

/// A cheap-to-clone handle that cancels a running [`approximate`] call.
///
/// Clones share the same flag. The render checks it before every step.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A token that is not cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation of every render that holds a clone of this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether [`cancel`](Self::cancel) was called on this token or a clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// How [`approximate`] reports progress and learns about cancellation.
///
/// `Execution::new()` reports nothing and cannot be cancelled.
#[derive(Default)]
pub struct Execution<'a> {
    progress: Option<&'a mut dyn FnMut(ProgressInfo)>,
    cancel: Option<&'a CancellationToken>,
}

impl<'a> Execution<'a> {
    /// No progress reporting and no cancellation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Calls `progress` after every step.
    #[must_use]
    pub fn progress(mut self, progress: &'a mut dyn FnMut(ProgressInfo)) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Stops the render with [`ApproximateError::Aborted`] once `token` is
    /// cancelled.
    #[must_use]
    pub fn cancellation(mut self, token: &'a CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(CancellationToken::is_cancelled)
    }
}

/// Final encoded render result.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum ApproximateResult {
    /// SVG output as UTF-8 text.
    Svg {
        data: String,
        width: u32,
        height: u32,
    },
    /// Encoded PNG output.
    Png {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
}

impl ApproximateResult {
    #[must_use]
    pub const fn format(&self) -> OutputFormat {
        match self {
            Self::Svg { .. } => OutputFormat::Svg,
            Self::Png { .. } => OutputFormat::Png,
        }
    }

    #[must_use]
    pub const fn mime_type(&self) -> &'static str {
        self.format().mime_type()
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        match self {
            Self::Svg { width, .. } | Self::Png { width, .. } => *width,
        }
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        match self {
            Self::Svg { height, .. } | Self::Png { height, .. } => *height,
        }
    }

    /// The encoded output bytes (UTF-8 text for SVG).
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Svg { data, .. } => data.into_bytes(),
            Self::Png { data, .. } => data,
        }
    }
}

/// Render-time failures from validation, decoding, cancellation, or encoding.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum ApproximateError {
    Validation(String),
    Aborted,
    Internal(String),
}

impl ApproximateError {
    fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl std::fmt::Display for ApproximateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message) => write!(f, "validation error: {message}"),
            Self::Aborted => f.write_str("render aborted"),
            Self::Internal(message) => write!(f, "internal render error: {message}"),
        }
    }
}

impl std::error::Error for ApproximateError {}

/// Decode, optimize, and encode a single output in one call.
///
/// # Errors
///
/// Returns [`ApproximateError::Validation`] for invalid options or input
/// bytes, [`ApproximateError::Aborted`] when `execution`'s token is
/// cancelled, and [`ApproximateError::Internal`] for encoding failures.
pub fn approximate(
    request: ApproximateRequest,
    mut execution: Execution<'_>,
) -> Result<ApproximateResult, ApproximateError> {
    let ApproximateRequest {
        input,
        output,
        render,
    } = request;
    validate_options(&render)?;

    let image = decode_input(&input)?;
    drop(input);
    let (working, background) = prepare_target(image, render.background, render.resize_input);
    let target = Buffer::from_rgba(working.width(), working.height(), working.into_raw())
        .ok_or_else(|| ApproximateError::internal("working image has an invalid pixel length"))?;
    let mut options = ModelOptions::default();
    options.seed = render.seed;
    options.workers = default_worker_count();
    let mut model = Model::new(target, background, options);

    for step in 0..render.count {
        if execution.is_cancelled() {
            return Err(ApproximateError::Aborted);
        }

        model
            .step(render.shape, render.alpha)
            .map_err(ApproximateError::internal)?;

        if let Some(progress) = execution.progress.as_mut() {
            progress(ProgressInfo {
                step: step + 1,
                total: render.count,
                score: model.score_f64(),
            });
        }
    }

    encode_output(&model.drawing(), render.output_size, output)
}

fn encode_output(
    drawing: &Drawing,
    output_size: u32,
    output: OutputFormat,
) -> Result<ApproximateResult, ApproximateError> {
    let (width, height) = output_dimensions(drawing.width, drawing.height, output_size);

    match output {
        OutputFormat::Svg => Ok(ApproximateResult::Svg {
            data: svg::write_svg(drawing, width, height),
            width,
            height,
        }),
        OutputFormat::Png => {
            let rgb = raster::render_rgb(drawing, width, height).ok_or_else(|| {
                ApproximateError::internal("could not allocate the output raster")
            })?;
            Ok(ApproximateResult::Png {
                data: raster::encode_png(width, height, &rgb)
                    .map_err(|err| ApproximateError::internal(err.to_string()))?,
                width,
                height,
            })
        }
    }
}

fn validate_options(render: &RenderOptions) -> Result<(), ApproximateError> {
    if render.count == 0 {
        return Err(ApproximateError::validation("count must be at least 1"));
    }
    if render.output_size == 0 {
        return Err(ApproximateError::validation(
            "output_size must be at least 1",
        ));
    }
    if render.resize_input == 0 {
        return Err(ApproximateError::validation(
            "resize_input must be at least 1",
        ));
    }
    if let BackgroundOption::Color(color) = render.background
        && color.a != 255
    {
        return Err(ApproximateError::validation("background must be opaque"));
    }
    Ok(())
}

fn decode_input(bytes: &[u8]) -> Result<DynamicImage, ApproximateError> {
    image::load_from_memory(bytes)
        .map_err(|err| ApproximateError::validation(format!("invalid image data: {err}")))
}

/// Resolve the background, flatten the image onto it, and build the
/// working-resolution target.
///
/// The target is always opaque: every pixel is composited onto the opaque
/// background, which leaves already-opaque pixels unchanged.
fn prepare_target(
    image: DynamicImage,
    background: BackgroundOption,
    resize_input: u32,
) -> (RgbaImage, Color) {
    let mut pixels = image.into_rgba8();
    let background = match background {
        BackgroundOption::Auto => average_background(&pixels),
        BackgroundOption::Color(color) => color,
    };
    flatten_onto(&mut pixels, background);
    let flattened = DynamicImage::ImageRgba8(pixels);
    (thumbnail(&flattened, resize_input), background)
}

/// Composite every pixel onto an opaque background color.
fn flatten_onto(image: &mut RgbaImage, background: Color) {
    let bg = [background.r, background.g, background.b];
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 255 {
            continue;
        }
        for (channel, bg) in pixel.0.iter_mut().zip(bg) {
            let blended = (u32::from(*channel) * alpha + u32::from(bg) * (255 - alpha) + 127) / 255;
            // A weighted mean of two u8 values fits in u8.
            *channel = blended as u8;
        }
        pixel[3] = 255;
    }
}

/// Choose the worker count from available system parallelism.
fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use std::num::NonZeroU8;

    fn fixture_image() -> DynamicImage {
        let image = RgbaImage::from_fn(12, 8, |x, y| {
            let r = (x * 16) as u8;
            let g = (y * 24) as u8;
            let b = ((x + y) * 12) as u8;
            Rgba([r, g, b, 255])
        });
        DynamicImage::ImageRgba8(image)
    }

    fn fixture_bytes() -> Vec<u8> {
        let image = fixture_image();
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .expect("fixture png");
        out.into_inner()
    }

    fn fixed_alpha(alpha: u8) -> Alpha {
        Alpha::Fixed(NonZeroU8::new(alpha).expect("non-zero alpha"))
    }

    fn render_options() -> RenderOptions {
        RenderOptions {
            count: 3,
            shape: ShapeKind::Triangle,
            alpha: fixed_alpha(128),
            seed: Some(7),
            background: BackgroundOption::Auto,
            resize_input: 8,
            output_size: 16,
        }
    }

    fn request(input: Vec<u8>, output: OutputFormat) -> ApproximateRequest {
        ApproximateRequest {
            input,
            output,
            render: render_options(),
        }
    }

    #[test]
    fn background_parses_auto_and_hex_colors() {
        assert_eq!("auto".parse(), Ok(BackgroundOption::Auto));
        assert_eq!("AUTO".parse(), Ok(BackgroundOption::Auto));
        assert_eq!(
            "#112233".parse(),
            Ok(BackgroundOption::Color(Color::new(0x11, 0x22, 0x33, 0xFF)))
        );
        assert_eq!(
            "not-a-color".parse::<BackgroundOption>(),
            Err(ParseError::new(
                "background must be auto or an opaque hex color (RGB or RRGGBB)"
            ))
        );
    }

    #[test]
    fn merge_keeps_defaults_for_omitted_fields() {
        assert_eq!(
            RenderOptions::default().merge(PartialRenderOptions::default()),
            RenderOptions::default()
        );
    }

    #[test]
    fn merge_applies_every_set_field() {
        let partial = PartialRenderOptions {
            count: Some(3),
            shape: Some(ShapeKind::Triangle),
            alpha: Some(fixed_alpha(128)),
            seed: Some(7),
            background: Some(BackgroundOption::Auto),
            resize_input: Some(8),
            output_size: Some(16),
        };

        assert_eq!(RenderOptions::default().merge(partial), render_options());
    }

    #[test]
    fn merge_overrides_only_the_set_fields() {
        let partial = PartialRenderOptions {
            count: Some(5),
            alpha: Some(Alpha::Auto),
            ..PartialRenderOptions::default()
        };

        let merged = render_options().merge(partial);

        let mut expected = render_options();
        expected.count = 5;
        expected.alpha = Alpha::Auto;
        assert_eq!(merged, expected);
    }

    #[test]
    fn cancellation_token_clones_share_the_flag() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());

        clone.cancel();

        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());
    }

    #[test]
    fn same_seed_renders_are_deterministic() {
        let first = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new(),
        )
        .expect("first render");
        let second = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new(),
        )
        .expect("second render");

        assert_eq!(first, second);
    }

    fn encoded_fixture(format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(fixture_image().to_rgb8());
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode fixture");
        out.into_inner()
    }

    fn assert_renders_svg(bytes: Vec<u8>) {
        let result = approximate(request(bytes, OutputFormat::Svg), Execution::new())
            .expect("render should succeed");

        match result {
            ApproximateResult::Svg { width, height, .. } => {
                assert_eq!((width, height), (16, 10));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn webp_input_renders() {
        let bytes = encoded_fixture(ImageFormat::WebP);
        assert!(bytes.starts_with(b"RIFF"));
        assert_eq!(&bytes[8..12], b"WEBP");

        assert_renders_svg(bytes);
    }

    #[test]
    fn jpeg_input_renders() {
        let bytes = encoded_fixture(ImageFormat::Jpeg);
        assert!(bytes.starts_with(&[0xFF, 0xD8]));

        assert_renders_svg(bytes);
    }

    #[test]
    fn gif_input_is_rejected_as_invalid_image_data() {
        // Minimal valid 1x1 GIF89a.
        const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\
            !\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

        let result = approximate(request(GIF.to_vec(), OutputFormat::Svg), Execution::new());

        match result {
            Err(ApproximateError::Validation(message)) => {
                assert!(message.starts_with("invalid image data"), "{message}");
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_bytes_return_validation_error() {
        let invalid = approximate(
            request(vec![0, 1, 2, 3], OutputFormat::Svg),
            Execution::new(),
        );
        assert!(matches!(invalid, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn invalid_options_are_rejected() {
        let mut options = render_options();
        options.count = 0;

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            Execution::new(),
        );
        assert!(matches!(result, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn resize_input_zero_is_rejected() {
        let mut options = render_options();
        options.resize_input = 0;

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            Execution::new(),
        );

        assert!(matches!(result, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn progress_fires_once_per_step_and_steps_increase() {
        let mut steps = Vec::new();
        let mut on_progress = |info: ProgressInfo| steps.push((info.step, info.total));

        let result = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new().progress(&mut on_progress),
        );

        assert!(result.is_ok());
        assert_eq!(steps, [(1, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn cancellation_between_steps_returns_abort_error() {
        let token = CancellationToken::new();
        let canceller = token.clone();
        let mut fired = Vec::new();
        let mut on_progress = |info: ProgressInfo| {
            fired.push(info.step);
            if info.step == 1 {
                canceller.cancel();
            }
        };

        let result = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new()
                .progress(&mut on_progress)
                .cancellation(&token),
        );

        assert_eq!(result, Err(ApproximateError::Aborted));
        assert_eq!(fired, [1]);
    }

    #[test]
    fn cancelled_token_aborts_before_the_first_step() {
        let token = CancellationToken::new();
        token.cancel();
        let mut fired = Vec::new();
        let mut on_progress = |info: ProgressInfo| fired.push(info.step);

        let result = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new()
                .progress(&mut on_progress)
                .cancellation(&token),
        );

        assert_eq!(result, Err(ApproximateError::Aborted));
        assert!(fired.is_empty());
    }

    #[test]
    fn raster_outputs_report_rendered_dimensions() {
        let result = approximate(
            request(fixture_bytes(), OutputFormat::Png),
            Execution::new(),
        )
        .expect("png render");

        match result {
            ApproximateResult::Png {
                ref data,
                width,
                height,
            } => {
                assert_eq!(result.format(), OutputFormat::Png);
                assert_eq!(result.mime_type(), "image/png");
                assert!(data.starts_with(b"\x89PNG"));
                assert!(width > 0);
                assert!(height > 0);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn svg_and_png_map_the_same_canvas_onto_the_same_size() {
        let svg = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Execution::new(),
        )
        .expect("svg render");
        let png = approximate(
            request(fixture_bytes(), OutputFormat::Png),
            Execution::new(),
        )
        .expect("png render");

        // The 12 x 8 fixture works at 8 x 5 and exports at 16 x 10.
        let ApproximateResult::Svg { data, .. } = &svg else {
            panic!("expected svg output");
        };
        assert!(data.starts_with(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"10\" \
             viewBox=\"0 0 8 5\">"
        ));
        let ApproximateResult::Png { data, .. } = &png else {
            panic!("expected png output");
        };
        let decoded = image::load_from_memory(data).expect("decode png");
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        assert_eq!((decoded.width(), decoded.height()), (16, 10));
        assert_eq!((svg.width(), svg.height()), (16, 10));
        assert_eq!((png.width(), png.height()), (16, 10));
    }

    #[test]
    fn any_shape_render_does_not_panic_in_debug() {
        let render = RenderOptions {
            count: 1,
            seed: Some(1),
            resize_input: 32,
            output_size: 32,
            ..RenderOptions::default()
        };

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render,
            },
            Execution::new(),
        );

        assert!(
            result.is_ok(),
            "any-shape render should succeed: {result:?}"
        );
    }

    #[test]
    fn parse_background_rejects_alpha_forms() {
        for value in ["#1234", "1234", "#11223344", "11223344"] {
            assert_eq!(
                value.parse::<BackgroundOption>(),
                Err(ParseError::new(
                    "background must be auto or an opaque hex color (RGB or RRGGBB)"
                )),
                "{value}"
            );
        }
        assert_eq!(
            "#abc".parse(),
            Ok(BackgroundOption::Color(Color::new(0xAA, 0xBB, 0xCC, 0xFF)))
        );
    }

    #[test]
    fn parse_background_rejects_multibyte_input_without_panicking() {
        for value in ["a€bc", "#a€bc", "€", "aé"] {
            assert!(value.parse::<BackgroundOption>().is_err(), "{value}");
        }
    }

    #[test]
    fn translucent_explicit_background_is_rejected() {
        let mut options = render_options();
        options.background = BackgroundOption::Color(Color::new(10, 20, 30, 128));

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            Execution::new(),
        );

        assert_eq!(
            result,
            Err(ApproximateError::Validation(
                "background must be opaque".to_string()
            ))
        );
    }

    fn png_bytes(image: RgbaImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode png");
        out.into_inner()
    }

    #[test]
    fn opaque_input_target_and_background_are_unchanged() {
        // Larger than resize_input so the resampling path runs too.
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(40, 24, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 10) as u8, ((x * y) % 256) as u8])
        }));
        let expected_target = thumbnail(&image, 16);
        let pixels = image.to_rgba8();
        let pixel_count = u64::from(pixels.width()) * u64::from(pixels.height());
        let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
        for pixel in pixels.pixels() {
            r_sum += u64::from(pixel[0]);
            g_sum += u64::from(pixel[1]);
            b_sum += u64::from(pixel[2]);
        }
        let expected_background = Color::new(
            (r_sum / pixel_count) as u8,
            (g_sum / pixel_count) as u8,
            (b_sum / pixel_count) as u8,
            255,
        );

        let (target, background) = prepare_target(image, BackgroundOption::Auto, 16);

        assert_eq!(target, expected_target);
        assert_eq!(background, expected_background);
    }

    #[test]
    fn transparent_input_is_flattened_onto_explicit_background() {
        let image = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([200, 100, 0, 0])
            } else {
                Rgba([200, 100, 0, 128])
            }
        });
        let background = Color::new(0, 50, 255, 255);

        let (target, resolved) = prepare_target(
            DynamicImage::ImageRgba8(image),
            BackgroundOption::Color(background),
            16,
        );

        assert_eq!(resolved, background);
        assert_eq!(target.get_pixel(0, 0), &Rgba([0, 50, 255, 255]));
        // (c*a + bg*(255-a) + 127) / 255 with a = 128.
        assert_eq!(target.get_pixel(1, 0), &Rgba([100, 75, 127, 255]));
    }

    #[test]
    fn auto_background_is_alpha_weighted_mean() {
        // Half-transparent red next to fully transparent green.
        let image = RgbaImage::from_fn(2, 2, |x, _| {
            if x == 0 {
                Rgba([255, 0, 0, 128])
            } else {
                Rgba([0, 255, 0, 0])
            }
        });

        let (target, background) =
            prepare_target(DynamicImage::ImageRgba8(image), BackgroundOption::Auto, 16);

        assert_eq!(background, Color::new(255, 0, 0, 255));
        assert!(
            target
                .pixels()
                .all(|pixel| pixel == &Rgba([255, 0, 0, 255]))
        );
    }

    #[test]
    fn fully_transparent_input_uses_white_auto_background() {
        let image = RgbaImage::from_pixel(3, 3, Rgba([10, 20, 30, 0]));

        let (target, background) =
            prepare_target(DynamicImage::ImageRgba8(image), BackgroundOption::Auto, 16);

        assert_eq!(background, Color::new(255, 255, 255, 255));
        assert!(
            target
                .pixels()
                .all(|pixel| pixel == &Rgba([255, 255, 255, 255]))
        );
    }

    #[test]
    fn transparent_input_with_explicit_background_renders_opaque_png() {
        let input = png_bytes(RgbaImage::from_fn(12, 8, |x, y| {
            Rgba([(x * 20) as u8, (y * 30) as u8, 90, ((x + y) * 12) as u8])
        }));
        let mut options = render_options();
        options.background = "#336699".parse().expect("background");

        let result = approximate(
            ApproximateRequest {
                input,
                output: OutputFormat::Png,
                render: options,
            },
            Execution::new(),
        )
        .expect("png render");

        let ApproximateResult::Png { data, .. } = result else {
            panic!("expected png output");
        };
        let decoded = image::load_from_memory(&data)
            .expect("decode png")
            .to_rgba8();
        assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
    }
}
