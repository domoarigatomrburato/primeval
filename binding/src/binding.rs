use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, JsError};
use napi_derive::napi;
use primeval_render::{
    Alpha, ApproximateError, ApproximateRequest, ApproximateResult, BackgroundOption,
    CancellationToken, Execution, OutputFormat, PartialRenderOptions, ProgressInfo, RenderOption,
    RenderOptions, ShapeKind, approximate,
};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[napi(object, object_to_js = false)]
pub struct NativeRenderOptions {
    /// JavaScript numbers arrive as `f64`; Rust checks they are integers in
    /// range, so nothing wraps or truncates at this boundary.
    pub count: Option<f64>,
    pub shape: Option<String>,
    /// `"auto"` or a number; Rust validates both.
    pub alpha: Option<Either<f64, String>>,
    /// A safe-integer number or a `bigint` in `0..=2^64 - 1`.
    pub seed: Option<Either<f64, BigInt>>,
    pub background: Option<String>,
    #[napi(js_name = "resizeInput")]
    pub resize_input: Option<f64>,
    #[napi(js_name = "outputSize")]
    pub output_size: Option<f64>,
}

#[napi(object, object_to_js = false)]
pub struct NativeExecutionOptions {
    #[napi(js_name = "onProgress")]
    pub on_progress: Option<ThreadsafeFunction<NativeProgressInfo>>,
}

#[napi(object, object_to_js = false)]
pub struct NativeApproximateRequest {
    pub input: Buffer,
    pub output: String,
    pub render: NativeRenderOptions,
    pub execution: Option<NativeExecutionOptions>,
}

#[napi(object)]
pub struct NativeProgressInfo {
    pub step: u32,
    pub total: u32,
    pub score: f64,
}

#[napi(object)]
pub struct NativeApproximateResult {
    pub format: String,
    pub data: Buffer,
    #[napi(js_name = "mimeType")]
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

/// Handle to one running render. Dropping it does not cancel the render.
#[napi]
pub struct NativeTask {
    token: CancellationToken,
}

#[napi]
impl NativeTask {
    /// Requests cancellation; the render stops before its next step.
    #[napi]
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// Starts a render on the tokio blocking pool and returns
/// `{ promise, task }`.
///
/// Errors carry the stable code from [`ApproximateError::code`] as `err.code`.
#[napi(js_name = "startApproximate")]
pub fn start_approximate(env: &Env, request: NativeApproximateRequest) -> Result<Object<'_>> {
    let NativeApproximateRequest {
        input,
        output,
        render,
        execution,
    } = request;
    let progress = execution.and_then(|execution| execution.on_progress);
    let request =
        normalize_request(input, output, render).map_err(|error| js_error(env, &error))?;
    let token = CancellationToken::new();
    let task_token = token.clone();

    let promise = env.spawn_future_with_callback(
        async move {
            // The render is CPU-bound: run it on the blocking pool, not on a
            // tokio worker.
            let joined =
                spawn_blocking(move || catch_panic(|| render_request(request, &token, progress)))
                    .await;
            Ok(joined.unwrap_or_else(|join_error| {
                Err(ApproximateError::internal(format!(
                    "render task failed: {join_error}"
                )))
            }))
        },
        |env, outcome| {
            outcome
                .map(NativeApproximateResult::from)
                .map_err(|error| js_error(env, &error))
        },
    )?;

    let mut handle = Object::new(env)?;
    handle.set("promise", promise)?;
    handle.set("task", NativeTask { token: task_token })?;
    Ok(handle)
}

fn render_request(
    request: ApproximateRequest,
    token: &CancellationToken,
    progress: Option<ThreadsafeFunction<NativeProgressInfo>>,
) -> std::result::Result<ApproximateResult, ApproximateError> {
    let mut on_progress = |info: ProgressInfo| {
        if let Some(tsfn) = progress.as_ref() {
            let _ = tsfn.call(
                Ok(NativeProgressInfo {
                    step: info.step,
                    total: info.total,
                    score: info.score,
                }),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        }
    };
    let execution = Execution::new().cancellation(token);
    let execution = if progress.is_some() {
        execution.progress(&mut on_progress)
    } else {
        execution
    };
    approximate(request, execution)
}

/// Runs `f`, turning a panic into an internal error that carries the panic
/// message. The release profile unwinds, and rayon re-raises worker panics
/// on the calling thread, so this catches panics anywhere in the render.
fn catch_panic<T>(
    f: impl FnOnce() -> std::result::Result<T, ApproximateError>,
) -> std::result::Result<T, ApproximateError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        Err(ApproximateError::internal(format!(
            "render panicked: {}",
            panic_message(payload.as_ref())
        )))
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

/// Parses the explicit fields and leaves omitted ones to the Rust defaults.
fn normalize_request(
    input: Buffer,
    output: String,
    render: NativeRenderOptions,
) -> std::result::Result<ApproximateRequest, ApproximateError> {
    let invalid = ApproximateError::invalid_option;
    let output = output
        .parse::<OutputFormat>()
        .map_err(|_| invalid(RenderOption::Output))?;

    let mut partial = PartialRenderOptions::default();
    if let Some(count) = render.count {
        partial.set_number(RenderOption::Count, count)?;
    }
    partial.shape = render
        .shape
        .as_deref()
        .map(str::parse::<ShapeKind>)
        .transpose()
        .map_err(|_| invalid(RenderOption::Shape))?;
    match render.alpha {
        Some(Either::A(alpha)) => partial.set_number(RenderOption::Alpha, alpha)?,
        Some(Either::B(alpha)) => {
            partial.alpha = Some(
                alpha
                    .parse::<Alpha>()
                    .map_err(|_| invalid(RenderOption::Alpha))?,
            );
        }
        None => {}
    }
    partial.background = render
        .background
        .as_deref()
        .map(str::parse::<BackgroundOption>)
        .transpose()
        .map_err(|_| invalid(RenderOption::Background))?;
    match render.seed {
        Some(Either::A(seed)) => partial.set_number(RenderOption::Seed, seed)?,
        Some(Either::B(seed)) => partial.seed = Some(bigint_seed(&seed)?),
        None => {}
    }
    if let Some(resize_input) = render.resize_input {
        partial.set_number(RenderOption::ResizeInput, resize_input)?;
    }
    if let Some(output_size) = render.output_size {
        partial.set_number(RenderOption::OutputSize, output_size)?;
    }

    Ok(ApproximateRequest {
        input: input.into(),
        output,
        render: RenderOptions::default().merge(partial),
    })
}

/// A `bigint` seed must be in `0..=u64::MAX`.
fn bigint_seed(seed: &BigInt) -> std::result::Result<u64, ApproximateError> {
    let (negative, value, lossless) = seed.get_u64();
    if negative || !lossless {
        return Err(ApproximateError::invalid_option(RenderOption::Seed));
    }
    Ok(value)
}

/// The Node spelling of an option name. This is the only place that maps
/// Rust option names to Node ones.
fn node_option_name(option: RenderOption) -> &'static str {
    match option {
        RenderOption::ResizeInput => "resizeInput",
        RenderOption::OutputSize => "outputSize",
        other => other.name(),
    }
}

/// The message Node callers see: option names in Node spelling, followed by
/// the chain of error sources.
fn node_message(error: &ApproximateError) -> String {
    let mut message = match error {
        ApproximateError::InvalidOption { option } => {
            format!("{} {}", node_option_name(*option), option.requirement())
        }
        other => other.to_string(),
    };
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// A JavaScript `Error` whose `code` is the stable error code.
///
/// napi-rs only sets a string `code` on errors it creates from an
/// `Error<S: AsRef<str>>`; the promise path takes `Error<Status>`, so the
/// error object is created here and passed through as a reference.
fn js_error(env: &Env, error: &ApproximateError) -> Error {
    let unknown = JsError::from(Error::new(error.code(), node_message(error))).into_unknown(*env);
    Error::from(unknown)
}

impl From<ApproximateResult> for NativeApproximateResult {
    fn from(value: ApproximateResult) -> Self {
        let fmt = value.format();
        let format = fmt.extension().to_string();
        let mime_type = fmt.mime_type().to_string();
        let width = value.width();
        let height = value.height();
        let data = Buffer::from(value.into_bytes());
        Self {
            format,
            data,
            mime_type,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU8;

    fn render_options(shape: &str) -> NativeRenderOptions {
        NativeRenderOptions {
            count: Some(1.0),
            shape: Some(shape.to_string()),
            alpha: Some(Either::A(128.0)),
            seed: Some(Either::A(7.0)),
            background: Some("auto".to_string()),
            resize_input: Some(32.0),
            output_size: Some(32.0),
        }
    }

    fn normalize(render: NativeRenderOptions) -> std::result::Result<ApproximateRequest, String> {
        normalize_request(Buffer::from(vec![0_u8; 4]), "svg".to_string(), render)
            .map_err(|error| format!("{}: {}", error.code(), node_message(&error)))
    }

    #[test]
    fn normalize_request_uses_shared_shape_and_output_parsers() {
        let request = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "png".to_string(),
            render_options("rotated-rectangle"),
        )
        .expect("request should normalize");

        assert_eq!(request.output, OutputFormat::Png);
        assert_eq!(request.render.shape, ShapeKind::RotatedRectangle);
    }

    #[test]
    fn normalize_request_rejects_removed_output_formats() {
        for output in ["jpeg", "jpg", "gif"] {
            let error = normalize_request(
                Buffer::from(vec![0_u8; 4]),
                output.to_string(),
                render_options("triangle"),
            )
            .expect_err("removed output format should fail");

            assert_eq!(
                node_message(&error),
                "output must be one of: svg, png",
                "{output}"
            );
        }
    }

    #[test]
    fn normalize_request_rejects_unknown_shape() {
        let error = normalize(render_options("hexagon")).expect_err("shape should fail");

        assert_eq!(
            error,
            "INVALID_OPTION: shape must be one of: any, triangle, rectangle, ellipse, circle, \
             rotated-rectangle, quadratic, rotated-ellipse, polygon"
        );
    }

    #[test]
    fn normalize_request_uses_rust_defaults_for_omitted_render_fields() {
        let request = normalize(NativeRenderOptions {
            count: None,
            shape: None,
            alpha: None,
            seed: None,
            background: None,
            resize_input: None,
            output_size: None,
        })
        .expect("request should normalize");

        assert_eq!(request.render, RenderOptions::default());
    }

    fn normalize_alpha(
        alpha: Either<f64, String>,
    ) -> std::result::Result<ApproximateRequest, String> {
        normalize(NativeRenderOptions {
            alpha: Some(alpha),
            ..render_options("any")
        })
    }

    #[test]
    fn normalize_request_accepts_auto_alpha_string() {
        let request = normalize_alpha(Either::B("auto".to_string())).expect("auto alpha");

        assert_eq!(request.render.alpha, Alpha::Auto);
    }

    #[test]
    fn normalize_request_accepts_integer_alpha() {
        let request = normalize_alpha(Either::A(200.0)).expect("integer alpha");

        assert_eq!(
            request.render.alpha,
            Alpha::Fixed(NonZeroU8::new(200).expect("non-zero"))
        );
    }

    #[test]
    fn normalize_request_rejects_zero_and_out_of_range_alpha() {
        for alpha in [
            Either::A(0.0),
            Either::A(256.0),
            Either::A(2_f64.powi(32) + 128.0),
            Either::A(1.5),
            Either::B("0".to_string()),
            Either::B("half".to_string()),
        ] {
            let error = normalize_alpha(alpha).expect_err("alpha should fail");

            assert_eq!(
                error,
                "INVALID_OPTION: alpha must be auto or an integer 1..255"
            );
        }
    }

    #[test]
    fn numeric_options_do_not_wrap_and_use_node_names() {
        let wrapping = 2_f64.powi(32) + 1.0;
        type Setter = fn(&mut NativeRenderOptions, f64);
        let cases: [(Setter, &str); 3] = [
            (|r, v| r.count = Some(v), "count"),
            (|r, v| r.resize_input = Some(v), "resizeInput"),
            (|r, v| r.output_size = Some(v), "outputSize"),
        ];
        for (set, name) in cases {
            for value in [wrapping, 1e20, -1.0, 1.5, f64::NAN] {
                let mut render = render_options("any");
                set(&mut render, value);

                let error = normalize(render).expect_err("value should fail");

                assert!(
                    error.starts_with(&format!("INVALID_OPTION: {name} must be an integer from ")),
                    "{name} = {value}: {error}"
                );
            }
        }
    }

    #[test]
    fn number_seeds_must_be_safe_non_negative_integers() {
        for seed in [-1.0, 1.5, f64::NAN, 2_f64.powi(53), 1e20] {
            let error = normalize(NativeRenderOptions {
                seed: Some(Either::A(seed)),
                ..render_options("any")
            })
            .expect_err("seed should fail");

            assert!(
                error.starts_with("INVALID_OPTION: seed must be an integer from 0 to 2^64 - 1"),
                "{seed}: {error}"
            );
        }

        let request = normalize(NativeRenderOptions {
            seed: Some(Either::A(2_f64.powi(53) - 1.0)),
            ..render_options("any")
        })
        .expect("max safe integer seed");
        assert_eq!(request.render.seed, Some(9_007_199_254_740_991));
    }

    fn bigint(sign_bit: bool, words: Vec<u64>) -> BigInt {
        BigInt { sign_bit, words }
    }

    #[test]
    fn bigint_seeds_cover_the_full_u64_range() {
        for value in [0, 7, u64::MAX] {
            assert_eq!(bigint_seed(&bigint(false, vec![value])).ok(), Some(value));
        }
        for seed in [bigint(true, vec![1]), bigint(false, vec![0, 1])] {
            assert!(matches!(
                bigint_seed(&seed),
                Err(ApproximateError::InvalidOption {
                    option: RenderOption::Seed
                })
            ));
        }
    }

    #[test]
    fn normalize_request_rejects_invalid_background() {
        let error = normalize(NativeRenderOptions {
            background: Some("#1234".to_string()),
            ..render_options("any")
        })
        .expect_err("background should fail");

        assert_eq!(
            error,
            "INVALID_OPTION: background must be auto or an opaque hex color (RGB or RRGGBB)"
        );
    }

    #[test]
    fn node_message_appends_the_source_chain() {
        let error = ApproximateError::Internal {
            reason: "PNG encoding failed".into(),
            source: Some(Box::new(std::io::Error::other("disk full"))),
        };

        assert_eq!(
            node_message(&error),
            "internal render error: PNG encoding failed: disk full"
        );
    }

    #[test]
    fn catch_panic_turns_a_panic_into_an_internal_error() {
        let result: std::result::Result<(), _> = catch_panic(|| panic!("boom {}", 7));

        match result {
            Err(error @ ApproximateError::Internal { .. }) => {
                assert_eq!(error.code(), "INTERNAL");
                assert_eq!(
                    node_message(&error),
                    "internal render error: render panicked: boom 7"
                );
            }
            other => panic!("expected internal error, got {other:?}"),
        }
    }

    #[test]
    fn catch_panic_passes_results_through() {
        assert_eq!(catch_panic(|| Ok(5)).ok(), Some(5));
        assert!(matches!(
            catch_panic::<()>(|| Err(ApproximateError::Aborted)),
            Err(ApproximateError::Aborted)
        ));
    }

    #[test]
    fn panic_message_reads_static_and_formatted_payloads() {
        assert_eq!(panic_message(&"static"), "static");
        assert_eq!(panic_message(&String::from("formatted")), "formatted");
        assert_eq!(panic_message(&42_u8), "non-string panic payload");
    }
}
