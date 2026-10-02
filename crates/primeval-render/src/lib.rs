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
//!     approximate, ApproximateRequest, ApproximateResult, OutputFormat, RenderOptions,
//! };
//! use std::sync::atomic::AtomicBool;
//!
//! let cancelled = AtomicBool::new(false);
//! let result = approximate(
//!     ApproximateRequest {
//!         input: std::fs::read("photo.jpg")?,
//!         output: OutputFormat::Svg,
//!         render: RenderOptions {
//!             count: 100,
//!             resize_input: 128,
//!             output_size: 512,
//!             ..RenderOptions::default()
//!         },
//!     },
//!     None,
//!     &cancelled,
//! )?;
//!
//! match result {
//!     ApproximateResult::Svg { data, .. } => std::fs::write("out.svg", data)?,
//!     ApproximateResult::Raster { .. } => unreachable!("requested svg output"),
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use image::{DynamicImage, RgbaImage};
use primeval_core::export::{average_background, encode_png, thumbnail};
use primeval_core::shapes::ShapeKind;
use primeval_core::{Buffer, Color, Model, ModelOptions};
use std::sync::atomic::{AtomicBool, Ordering};

pub use primeval_core::OutputFormat;

/// Alpha strategy for new shapes during optimization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlphaOption {
    /// Let the optimizer choose alpha automatically.
    Auto,
    /// Use a fixed alpha value for every committed shape.
    Fixed(u8),
}

/// Background color strategy for the initial canvas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundOption {
    /// Derive the background color from the input image.
    Auto,
    /// Use an explicit opaque color (alpha 255).
    Color(Color),
}

/// Render-time knobs that control optimization and final export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    /// Number of optimization steps.
    pub count: u32,
    /// Shape family to search during each step.
    pub shape: ShapeKind,
    /// Alpha handling for new shapes.
    pub alpha: AlphaOption,
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
            alpha: AlphaOption::Auto,
            seed: None,
            background: BackgroundOption::Auto,
            resize_input: 256,
            output_size: 1024,
        }
    }
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressInfo {
    pub step: u32,
    pub total: u32,
    pub score: f64,
}

/// Final encoded render result.
#[derive(Clone, Debug, PartialEq)]
pub enum ApproximateResult {
    /// SVG output as UTF-8 text.
    Svg {
        data: String,
        width: u32,
        height: u32,
    },
    /// Encoded PNG raster output.
    Raster {
        format: OutputFormat,
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
            Self::Raster { format, .. } => *format,
        }
    }

    #[must_use]
    pub const fn mime_type(&self) -> &'static str {
        self.format().mime_type()
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        match self {
            Self::Svg { width, .. } | Self::Raster { width, .. } => *width,
        }
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        match self {
            Self::Svg { height, .. } | Self::Raster { height, .. } => *height,
        }
    }
}

/// Render-time failures from validation, decoding, cancellation, or encoding.
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

/// Parse alpha input shared across Rust and binding layers.
pub fn parse_alpha_str(value: &str) -> Result<AlphaOption, String> {
    if value.eq_ignore_ascii_case("auto") || value == "0" {
        return Ok(AlphaOption::Auto);
    }

    let parsed: i32 = value
        .parse()
        .map_err(|err| format!("invalid alpha: {err}"))?;
    if !(1..=255).contains(&parsed) {
        return Err("alpha must be 0..255 where 0 means auto".to_string());
    }
    Ok(AlphaOption::Fixed(parsed as u8))
}

/// Parse numeric alpha input where `None` and `0` both mean auto.
pub fn parse_alpha_u32(value: Option<u32>) -> Result<AlphaOption, String> {
    match value {
        None => Ok(AlphaOption::Auto),
        Some(0) => Ok(AlphaOption::Auto),
        Some(value) if (1..=255).contains(&value) => Ok(AlphaOption::Fixed(value as u8)),
        Some(_) => Err("alpha must be 0..255 where 0 means auto".to_string()),
    }
}

/// Parse a background color string or the special `auto` value.
pub fn parse_background_str(value: &str) -> Result<BackgroundOption, String> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(BackgroundOption::Auto);
    }

    let color = Color::from_hex(value).ok_or_else(|| {
        "background must be auto or an opaque hex color (RGB or RRGGBB)".to_string()
    })?;
    Ok(BackgroundOption::Color(color))
}

/// Parse a signed binding-layer seed into the Rust-side unsigned form.
pub fn parse_seed_i64(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| "seed must be a positive integer".to_string())
}

/// Decode, optimize, and encode a single output in one call.
pub fn approximate(
    request: ApproximateRequest,
    on_progress: Option<&dyn Fn(ProgressInfo)>,
    cancelled: &AtomicBool,
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
    let mut model = Model::new(
        Buffer::from_image(&working),
        background,
        render.output_size,
        ModelOptions {
            seed: render.seed,
            workers: default_worker_count(),
            profile_quadratic: false,
            ..ModelOptions::default()
        },
    );
    let alpha = match render.alpha {
        AlphaOption::Auto => 0,
        AlphaOption::Fixed(alpha) => i32::from(alpha),
    };

    for step in 0..render.count {
        if cancelled.load(Ordering::SeqCst) {
            return Err(ApproximateError::Aborted);
        }

        model
            .step(render.shape, alpha)
            .map_err(ApproximateError::internal)?;

        if let Some(callback) = on_progress {
            callback(ProgressInfo {
                step: step + 1,
                total: render.count,
                score: model.score_f64(),
            });
        }
    }

    encode_output(&model, output)
}

fn encode_output(
    model: &Model,
    output: OutputFormat,
) -> Result<ApproximateResult, ApproximateError> {
    let width = model.output_width;
    let height = model.output_height;

    match output {
        OutputFormat::Svg => Ok(ApproximateResult::Svg {
            data: model.svg(),
            width,
            height,
        }),
        OutputFormat::Png => Ok(ApproximateResult::Raster {
            format: OutputFormat::Png,
            data: encode_png(&model.render_output())
                .map_err(|err| ApproximateError::internal(err.to_string()))?,
            width,
            height,
        }),
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
    if let AlphaOption::Fixed(alpha) = render.alpha
        && alpha == 0
    {
        return Err(ApproximateError::validation(
            "alpha must be 0..255 where 0 means auto",
        ));
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

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

    fn render_options() -> RenderOptions {
        RenderOptions {
            count: 3,
            shape: ShapeKind::Triangle,
            alpha: AlphaOption::Fixed(128),
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
    fn parse_alpha_helpers_cover_string_and_numeric_inputs() {
        assert_eq!(parse_alpha_str("auto"), Ok(AlphaOption::Auto));
        assert_eq!(parse_alpha_str("0"), Ok(AlphaOption::Auto));
        assert_eq!(parse_alpha_str("128"), Ok(AlphaOption::Fixed(128)));
        assert_eq!(parse_alpha_u32(None), Ok(AlphaOption::Auto));
        assert_eq!(parse_alpha_u32(Some(0)), Ok(AlphaOption::Auto));
        assert_eq!(parse_alpha_u32(Some(128)), Ok(AlphaOption::Fixed(128)));

        assert_eq!(
            parse_alpha_str("256").expect_err("alpha >255 should fail"),
            "alpha must be 0..255 where 0 means auto"
        );
        assert_eq!(
            parse_alpha_u32(Some(256)).expect_err("alpha >255 should fail"),
            "alpha must be 0..255 where 0 means auto"
        );
        assert_eq!(
            parse_alpha_str("-1").expect_err("negative alpha should fail"),
            "alpha must be 0..255 where 0 means auto"
        );
    }

    #[test]
    fn parse_background_and_seed_helpers_validate_shared_inputs() {
        assert_eq!(parse_background_str("auto"), Ok(BackgroundOption::Auto));
        assert_eq!(
            parse_background_str("#112233"),
            Ok(BackgroundOption::Color(Color::new(0x11, 0x22, 0x33, 0xFF)))
        );
        assert_eq!(parse_seed_i64(7), Ok(7));

        assert_eq!(
            parse_background_str("not-a-color").expect_err("invalid color should fail"),
            "background must be auto or an opaque hex color (RGB or RRGGBB)"
        );
        assert_eq!(
            parse_seed_i64(-1).expect_err("negative seed should fail"),
            "seed must be a positive integer"
        );
    }

    #[test]
    fn same_seed_renders_are_deterministic() {
        let cancelled = AtomicBool::new(false);
        let first = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            None,
            &cancelled,
        )
        .expect("first render");
        let second = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            None,
            &cancelled,
        )
        .expect("second render");

        assert_eq!(first, second);
    }

    #[test]
    fn png_encoder_emits_png_header() {
        let image = Buffer::new_from_color(4, 4, Color::new(10, 20, 30, 255));
        let png = encode_png(&image).expect("png");

        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    fn encoded_fixture(format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(fixture_image().to_rgb8());
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode fixture");
        out.into_inner()
    }

    fn assert_renders_svg(bytes: Vec<u8>) {
        let cancelled = AtomicBool::new(false);
        let result = approximate(request(bytes, OutputFormat::Svg), None, &cancelled)
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
        let cancelled = AtomicBool::new(false);

        let result = approximate(request(GIF.to_vec(), OutputFormat::Svg), None, &cancelled);

        match result {
            Err(ApproximateError::Validation(message)) => {
                assert!(message.starts_with("invalid image data"), "{message}");
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_bytes_return_validation_error() {
        let cancelled = AtomicBool::new(false);

        let invalid = approximate(
            request(vec![0, 1, 2, 3], OutputFormat::Svg),
            None,
            &cancelled,
        );
        assert!(matches!(invalid, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn invalid_options_are_rejected() {
        let cancelled = AtomicBool::new(false);
        let mut options = render_options();
        options.count = 0;

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            None,
            &cancelled,
        );
        assert!(matches!(result, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn resize_input_zero_is_rejected() {
        let cancelled = AtomicBool::new(false);
        let mut options = render_options();
        options.resize_input = 0;

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            None,
            &cancelled,
        );

        assert!(matches!(result, Err(ApproximateError::Validation(_))));
    }

    #[test]
    fn progress_fires_once_per_step_and_steps_increase() {
        let cancelled = AtomicBool::new(false);
        let steps = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&steps);
        let callback = move |info: ProgressInfo| {
            captured.lock().expect("lock").push((info.step, info.total));
        };

        let result = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Some(&callback),
            &cancelled,
        );

        assert!(result.is_ok());
        let steps = steps.lock().expect("lock");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps.as_slice(), &[(1, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn cancellation_between_steps_returns_abort_error() {
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let fired = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&fired);
        let cancelled_for_callback = Arc::clone(&cancelled);
        let callback = move |info: ProgressInfo| {
            captured.lock().expect("lock").push(info.step);
            if info.step == 1 {
                cancelled_for_callback.store(true, Ordering::SeqCst);
            }
        };

        let result = approximate(
            request(fixture_bytes(), OutputFormat::Svg),
            Some(&callback),
            cancelled.as_ref(),
        );

        assert!(matches!(result, Err(ApproximateError::Aborted)));
        let fired = fired.lock().expect("lock");
        assert_eq!(fired.as_slice(), &[1]);
    }

    #[test]
    fn raster_outputs_report_rendered_dimensions() {
        let cancelled = AtomicBool::new(false);
        let result = approximate(
            request(fixture_bytes(), OutputFormat::Png),
            None,
            &cancelled,
        )
        .expect("png render");

        match result {
            ApproximateResult::Raster {
                format,
                data,
                width,
                height,
            } => {
                assert_eq!(format, OutputFormat::Png);
                assert!(!data.is_empty());
                assert!(width > 0);
                assert!(height > 0);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn any_shape_render_does_not_panic_in_debug() {
        let cancelled = AtomicBool::new(false);
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
            None,
            &cancelled,
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
                parse_background_str(value),
                Err("background must be auto or an opaque hex color (RGB or RRGGBB)".to_string()),
                "{value}"
            );
        }
        assert_eq!(
            parse_background_str("#abc"),
            Ok(BackgroundOption::Color(Color::new(0xAA, 0xBB, 0xCC, 0xFF)))
        );
    }

    #[test]
    fn parse_background_rejects_multibyte_input_without_panicking() {
        for value in ["a€bc", "#a€bc", "€", "aé"] {
            assert!(parse_background_str(value).is_err(), "{value}");
        }
    }

    #[test]
    fn translucent_explicit_background_is_rejected() {
        let cancelled = AtomicBool::new(false);
        let mut options = render_options();
        options.background = BackgroundOption::Color(Color::new(10, 20, 30, 128));

        let result = approximate(
            ApproximateRequest {
                input: fixture_bytes(),
                output: OutputFormat::Svg,
                render: options,
            },
            None,
            &cancelled,
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
        let expected_background = Buffer::from_image(&image.to_rgba8()).average_color();

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
        let cancelled = AtomicBool::new(false);
        let input = png_bytes(RgbaImage::from_fn(12, 8, |x, y| {
            Rgba([(x * 20) as u8, (y * 30) as u8, 90, ((x + y) * 12) as u8])
        }));
        let mut options = render_options();
        options.background = parse_background_str("#336699").expect("background");

        let result = approximate(
            ApproximateRequest {
                input,
                output: OutputFormat::Png,
                render: options,
            },
            None,
            &cancelled,
        )
        .expect("png render");

        let ApproximateResult::Raster { data, .. } = result else {
            panic!("expected raster output");
        };
        let decoded = image::load_from_memory(&data)
            .expect("decode png")
            .to_rgba8();
        assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
    }
}
