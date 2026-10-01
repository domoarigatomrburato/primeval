//! High-level render facade for `primeval`.
//!
//! This crate handles the full decode -> optimize -> encode path on top of
//! `primeval-core`. It accepts either image bytes or filesystem paths, runs the
//! approximation search, and returns SVG or raster output.
//!
//! The Node package and napi binding in this repository build on the same API.
//! If you want the canonical Rust-side defaults and validation behavior, this is
//! the crate to call.
//!
//! # Example
//!
//! ```no_run
//! use primeval_render::{
//!     approximate, ApproximateRequest, ApproximateResult, InputSource, OutputFormat,
//!     RenderOptions,
//! };
//! use std::sync::atomic::AtomicBool;
//!
//! let cancelled = AtomicBool::new(false);
//! let result = approximate(
//!     ApproximateRequest {
//!         input: InputSource::Path("photo.jpg".into()),
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

use image::DynamicImage;
use primeval_core::export::{average_background, encode_gif, encode_jpg, encode_png, thumbnail};
use primeval_core::shapes::ShapeKind;
use primeval_core::{Buffer, Color, Model, ModelOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub use primeval_core::OutputFormat;

/// Source image input for a render request.
#[derive(Clone, Debug, PartialEq)]
pub enum InputSource {
    /// Raw encoded image bytes, typically PNG or JPEG.
    Bytes(Vec<u8>),
    /// A filesystem path to an encoded image file.
    Path(PathBuf),
}

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
    /// Use an explicit RGBA color.
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
    /// Extra hill-climb repetitions after each accepted step.
    pub repeat: usize,
    /// Deterministic RNG seed. `None` chooses a non-deterministic seed.
    pub seed: Option<u64>,
    /// Background fill strategy.
    pub background: BackgroundOption,
    /// Working resolution used during optimization.
    pub resize_input: u32,
    /// Maximum dimension of the final output replay.
    pub output_size: u32,
    /// Worker override. `None` uses available parallelism.
    pub workers: Option<usize>,
    /// Keep every `gif_frame_step`th replay frame when exporting GIFs.
    pub gif_frame_step: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            count: 100,
            shape: ShapeKind::Any,
            alpha: AlphaOption::Auto,
            repeat: 0,
            seed: None,
            background: BackgroundOption::Auto,
            resize_input: 256,
            output_size: 1024,
            workers: None,
            gif_frame_step: 1,
        }
    }
}

/// Full request for a single rendered output.
#[derive(Clone, Debug, PartialEq)]
pub struct ApproximateRequest {
    pub input: InputSource,
    pub output: OutputFormat,
    pub render: RenderOptions,
}

/// Request used when preparing a reusable approximation run.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    pub input: InputSource,
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
    /// Encoded raster output such as PNG, JPG, or GIF.
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
    NotFound(PathBuf),
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
            Self::NotFound(path) => {
                write!(f, "input path not found or unreadable: {}", path.display())
            }
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

    let color = Color::from_hex(value).ok_or_else(|| "invalid background color".to_string())?;
    Ok(BackgroundOption::Color(color))
}

/// Parse a signed binding-layer seed into the Rust-side unsigned form.
pub fn parse_seed_i64(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| "seed must be a positive integer".to_string())
}

/// Reusable prepared run that can be exported in multiple formats without
/// rerunning optimization.
pub struct ApproximationRun {
    model: Model,
    gif_frame_step: usize,
    rendered: Option<Buffer>,
    svg: Option<String>,
    frames: Option<Vec<Buffer>>,
}

impl ApproximationRun {
    /// Final output width after replay.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.model.output_width
    }

    /// Final output height after replay.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.model.output_height
    }

    /// Encode the prepared run into a single output format.
    pub fn result(&mut self, output: OutputFormat) -> Result<ApproximateResult, ApproximateError> {
        let width = self.width();
        let height = self.height();

        match output {
            OutputFormat::Svg => Ok(ApproximateResult::Svg {
                data: self.svg().to_owned(),
                width,
                height,
            }),
            OutputFormat::Png => Ok(ApproximateResult::Raster {
                format: OutputFormat::Png,
                data: encode_png(self.rendered())
                    .map_err(|err| ApproximateError::internal(err.to_string()))?,
                width,
                height,
            }),
            OutputFormat::Jpg => Ok(ApproximateResult::Raster {
                format: OutputFormat::Jpg,
                data: encode_jpg(self.rendered(), 95)
                    .map_err(|err| ApproximateError::internal(err.to_string()))?,
                width,
                height,
            }),
            OutputFormat::Gif => Ok(ApproximateResult::Raster {
                format: OutputFormat::Gif,
                data: encode_gif(self.frames(), 50, 250)
                    .map_err(|err| ApproximateError::internal(err.to_string()))?,
                width,
                height,
            }),
        }
    }

    fn rendered(&mut self) -> &Buffer {
        self.rendered
            .get_or_insert_with(|| self.model.render_output())
    }

    fn svg(&mut self) -> &str {
        self.svg.get_or_insert_with(|| self.model.svg())
    }

    fn frames(&mut self) -> &[Buffer] {
        self.frames
            .get_or_insert_with(|| gif_frames(&self.model, self.gif_frame_step))
    }
}

/// Decode, optimize, and encode a single output in one call.
pub fn approximate(
    request: ApproximateRequest,
    on_progress: Option<&dyn Fn(ProgressInfo)>,
    cancelled: &AtomicBool,
) -> Result<ApproximateResult, ApproximateError> {
    let mut run = prepare(
        RenderRequest {
            input: request.input,
            render: request.render,
        },
        on_progress,
        cancelled,
    )?;
    run.result(request.output)
}

/// Decode and optimize once, then return a reusable run for later encoding.
pub fn prepare(
    request: RenderRequest,
    on_progress: Option<&dyn Fn(ProgressInfo)>,
    cancelled: &AtomicBool,
) -> Result<ApproximationRun, ApproximateError> {
    validate_options(&request.render)?;

    let image = decode_input(&request.input)?;
    let background = match request.render.background {
        BackgroundOption::Auto => average_background(&image),
        BackgroundOption::Color(color) => color,
    };
    let working = thumbnail(&image, request.render.resize_input);
    let mut model = Model::new(
        Buffer::from_image(&working),
        background,
        request.render.output_size,
        ModelOptions {
            seed: request.render.seed,
            workers: request.render.workers.unwrap_or_else(default_worker_count),
            profile_quadratic: false,
            ..ModelOptions::default()
        },
    );
    let alpha = match request.render.alpha {
        AlphaOption::Auto => 0,
        AlphaOption::Fixed(alpha) => i32::from(alpha),
    };

    for step in 0..request.render.count {
        if cancelled.load(Ordering::SeqCst) {
            return Err(ApproximateError::Aborted);
        }

        model
            .step(request.render.shape, alpha, request.render.repeat)
            .map_err(ApproximateError::internal)?;

        if let Some(callback) = on_progress {
            callback(ProgressInfo {
                step: step + 1,
                total: request.render.count,
                score: model.score_f64(),
            });
        }
    }

    Ok(ApproximationRun {
        model,
        gif_frame_step: request.render.gif_frame_step,
        rendered: None,
        svg: None,
        frames: None,
    })
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
    if render.workers == Some(0) {
        return Err(ApproximateError::validation("workers must be at least 1"));
    }
    if render.gif_frame_step == 0 {
        return Err(ApproximateError::validation(
            "gif_frame_step must be at least 1",
        ));
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

fn decode_input(input: &InputSource) -> Result<DynamicImage, ApproximateError> {
    match input {
        InputSource::Bytes(bytes) => image::load_from_memory(bytes)
            .map_err(|err| ApproximateError::validation(format!("invalid image data: {err}"))),
        InputSource::Path(path) => {
            let bytes =
                std::fs::read(path).map_err(|_| ApproximateError::NotFound(path.clone()))?;
            image::load_from_memory(&bytes).map_err(|err| {
                ApproximateError::validation(format!(
                    "invalid image data at {}: {err}",
                    path.display()
                ))
            })
        }
    }
}

/// Choose the default worker count from available system parallelism.
pub fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

fn gif_frames(model: &Model, gif_frame_step: usize) -> Vec<Buffer> {
    let mut frames = model.frames(0.001);
    if gif_frame_step > 1 {
        frames = frames
            .into_iter()
            .enumerate()
            .filter_map(|(index, frame)| (index % gif_frame_step == 0).then_some(frame))
            .collect();
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::fs;
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
            repeat: 0,
            seed: Some(7),
            background: BackgroundOption::Auto,
            resize_input: 8,
            output_size: 16,
            workers: Some(1),
            gif_frame_step: 1,
        }
    }

    fn request(input: InputSource, output: OutputFormat) -> ApproximateRequest {
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
            "invalid background color"
        );
        assert_eq!(
            parse_seed_i64(-1).expect_err("negative seed should fail"),
            "seed must be a positive integer"
        );
    }

    fn temp_path(ext: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "primeval-render-{}-{:?}.{ext}",
            std::process::id(),
            std::thread::current().id()
        ));
        path
    }

    #[test]
    fn bytes_and_path_inputs_produce_identical_svg() {
        let bytes = fixture_bytes();
        let path = temp_path("png");
        fs::write(&path, &bytes).expect("write fixture");

        let cancelled = AtomicBool::new(false);
        let from_bytes = approximate(
            request(InputSource::Bytes(bytes.clone()), OutputFormat::Svg),
            None,
            &cancelled,
        )
        .expect("bytes render");
        let from_path = approximate(
            request(InputSource::Path(path.clone()), OutputFormat::Svg),
            None,
            &cancelled,
        )
        .expect("path render");

        fs::remove_file(path).ok();

        match (from_bytes, from_path) {
            (
                ApproximateResult::Svg {
                    data: left,
                    width: left_width,
                    height: left_height,
                },
                ApproximateResult::Svg {
                    data: right,
                    width: right_width,
                    height: right_height,
                },
            ) => {
                assert_eq!(left, right);
                assert_eq!(left_width, right_width);
                assert_eq!(left_height, right_height);
            }
            other => panic!("unexpected results: {other:?}"),
        }
    }

    #[test]
    fn encoders_emit_expected_headers() {
        let image = Buffer::new_from_color(4, 4, Color::new(10, 20, 30, 255));
        let png = encode_png(&image).expect("png");
        let jpg = encode_jpg(&image, 90).expect("jpg");
        let gif = encode_gif(
            &[
                image.clone(),
                Buffer::new_from_color(4, 4, Color::new(30, 20, 10, 255)),
            ],
            50,
            250,
        )
        .expect("gif");

        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(jpg.starts_with(&[0xFF, 0xD8]));
        assert!(gif.starts_with(b"GIF89a"));
        assert!(!png.is_empty());
        assert!(!jpg.is_empty());
        assert!(!gif.is_empty());
    }

    #[test]
    fn invalid_bytes_and_missing_path_return_stable_errors() {
        let cancelled = AtomicBool::new(false);

        let invalid = approximate(
            request(InputSource::Bytes(vec![0, 1, 2, 3]), OutputFormat::Svg),
            None,
            &cancelled,
        );
        assert!(matches!(invalid, Err(ApproximateError::Validation(_))));

        let missing = approximate(
            request(
                InputSource::Path(std::path::Path::new("/definitely/not/here.png").to_path_buf()),
                OutputFormat::Svg,
            ),
            None,
            &cancelled,
        );
        assert!(matches!(missing, Err(ApproximateError::NotFound(_))));
    }

    #[test]
    fn invalid_options_are_rejected() {
        let cancelled = AtomicBool::new(false);
        let mut options = render_options();
        options.count = 0;

        let result = approximate(
            ApproximateRequest {
                input: InputSource::Bytes(fixture_bytes()),
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
                input: InputSource::Bytes(fixture_bytes()),
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
            request(InputSource::Bytes(fixture_bytes()), OutputFormat::Svg),
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
            request(InputSource::Bytes(fixture_bytes()), OutputFormat::Svg),
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
            request(InputSource::Bytes(fixture_bytes()), OutputFormat::Png),
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
            workers: Some(4),
            ..RenderOptions::default()
        };

        let result = approximate(
            ApproximateRequest {
                input: InputSource::Bytes(fixture_bytes()),
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
    fn prepare_can_emit_multiple_formats_without_rerunning() {
        let cancelled = AtomicBool::new(false);
        let mut run = prepare(
            RenderRequest {
                input: InputSource::Bytes(fixture_bytes()),
                render: render_options(),
            },
            None,
            &cancelled,
        )
        .expect("prepare should succeed");

        let svg = run.result(OutputFormat::Svg).expect("svg result");
        let png = run.result(OutputFormat::Png).expect("png result");
        let gif = run.result(OutputFormat::Gif).expect("gif result");

        assert!(matches!(svg, ApproximateResult::Svg { .. }));
        assert!(matches!(
            png,
            ApproximateResult::Raster {
                format: OutputFormat::Png,
                ..
            }
        ));
        assert!(matches!(
            gif,
            ApproximateResult::Raster {
                format: OutputFormat::Gif,
                ..
            }
        ));
    }
}
