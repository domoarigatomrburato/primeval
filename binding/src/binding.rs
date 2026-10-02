use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, Status};
use napi_derive::napi;
use primeval_render::{
    Alpha, ApproximateError, ApproximateRequest, ApproximateResult, BackgroundOption,
    CancellationToken, Execution, OutputFormat, PartialRenderOptions, ProgressInfo, RenderOptions,
    ShapeKind, approximate,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_TASK_ID: AtomicU32 = AtomicU32::new(1);
static TASKS: OnceLock<Mutex<HashMap<u32, CancellationToken>>> = OnceLock::new();

fn tasks() -> &'static Mutex<HashMap<u32, CancellationToken>> {
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_task_registry<R>(
    registry: &Mutex<HashMap<u32, CancellationToken>>,
    f: impl FnOnce(&HashMap<u32, CancellationToken>) -> R,
) -> R {
    match registry.lock() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(&poisoned.into_inner()),
    }
}

fn with_task_registry_mut<R>(
    registry: &Mutex<HashMap<u32, CancellationToken>>,
    f: impl FnOnce(&mut HashMap<u32, CancellationToken>) -> R,
) -> R {
    match registry.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            f(&mut guard)
        }
    }
}

fn with_tasks<R>(f: impl FnOnce(&HashMap<u32, CancellationToken>) -> R) -> R {
    with_task_registry(tasks(), f)
}

fn with_tasks_mut<R>(f: impl FnOnce(&mut HashMap<u32, CancellationToken>) -> R) -> R {
    with_task_registry_mut(tasks(), f)
}

fn with_registered_task<T, E>(
    registry: &Mutex<HashMap<u32, CancellationToken>>,
    task_id: u32,
    token: CancellationToken,
    setup: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    with_task_registry_mut(registry, |tasks| {
        tasks.insert(task_id, token);
    });

    match setup() {
        Ok(value) => Ok(value),
        Err(error) => {
            with_task_registry_mut(registry, |tasks| {
                tasks.remove(&task_id);
            });
            Err(error)
        }
    }
}

#[napi(object, object_to_js = false)]
pub struct NativeRenderOptions {
    pub count: Option<u32>,
    pub shape: Option<String>,
    /// `"auto"` or an integer; Rust validates both.
    pub alpha: Option<Either<u32, String>>,
    pub seed: Option<i64>,
    pub background: Option<String>,
    #[napi(js_name = "resizeInput")]
    pub resize_input: Option<u32>,
    #[napi(js_name = "outputSize")]
    pub output_size: Option<u32>,
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

#[napi(js_name = "startApproximate")]
pub fn start_approximate(env: &Env, request: NativeApproximateRequest) -> Result<Object<'_>> {
    let NativeApproximateRequest {
        input,
        output,
        render,
        execution,
    } = request;
    let progress = execution.and_then(|execution| execution.on_progress);
    let request = normalize_request(input, output, render)?;
    let token = CancellationToken::new();
    let token_for_future = token.clone();
    let task_id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);

    with_registered_task(tasks(), task_id, token, || {
        let promise = env.spawn_future_with_callback(
            async move {
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
                let execution = Execution::new().cancellation(&token_for_future);
                let execution = if progress.is_some() {
                    execution.progress(&mut on_progress)
                } else {
                    execution
                };
                let result = approximate(request, execution);
                with_tasks_mut(|tasks| {
                    tasks.remove(&task_id);
                });

                result.map(NativeApproximateResult::from).map_err(map_error)
            },
            |_, result| Ok(result),
        )?;

        let mut handle = Object::new(env)?;
        handle.set("promise", promise)?;
        handle.set("taskId", task_id)?;
        Ok(handle)
    })
}

#[napi(js_name = "cancelApproximate")]
pub fn cancel_approximate(task_id: u32) {
    if let Some(token) = with_tasks(|tasks| tasks.get(&task_id).cloned()) {
        token.cancel();
    }
}

/// Parses the explicit fields and leaves omitted ones to the Rust defaults.
fn normalize_request(
    input: Buffer,
    output: String,
    render: NativeRenderOptions,
) -> Result<ApproximateRequest> {
    let output = output.parse::<OutputFormat>().map_err(validation_error)?;

    let mut partial = PartialRenderOptions::default();
    partial.count = render.count;
    partial.shape = render
        .shape
        .as_deref()
        .map(str::parse::<ShapeKind>)
        .transpose()
        .map_err(validation_error)?;
    partial.alpha = render
        .alpha
        .map(|alpha| match alpha {
            Either::A(value) => Alpha::try_from(value),
            Either::B(value) => value.parse::<Alpha>(),
        })
        .transpose()
        .map_err(validation_error)?;
    partial.background = render
        .background
        .as_deref()
        .map(str::parse::<BackgroundOption>)
        .transpose()
        .map_err(validation_error)?;
    partial.seed = render.seed.map(parse_seed).transpose()?;
    partial.resize_input = render.resize_input;
    partial.output_size = render.output_size;

    Ok(ApproximateRequest {
        input: input.into(),
        output,
        render: RenderOptions::default().merge(partial),
    })
}

/// JavaScript numbers arrive as `i64`; the seed is unsigned.
fn parse_seed(seed: i64) -> Result<u64> {
    u64::try_from(seed)
        .map_err(|_| napi_error("ValidationError", "seed must be a positive integer"))
}

fn validation_error(error: impl std::fmt::Display) -> Error {
    napi_error("ValidationError", error.to_string())
}

fn map_error(error: ApproximateError) -> Error {
    match error {
        ApproximateError::Validation(message) => napi_error("ValidationError", message),
        ApproximateError::Aborted => napi_error("AbortError", "operation aborted"),
        ApproximateError::Internal(message) => napi_error("Error", message),
        other => napi_error("Error", other.to_string()),
    }
}

fn napi_error(name: &str, message: impl Into<String>) -> Error {
    Error::new(
        Status::GenericFailure,
        format!("[{name}] {}", message.into()),
    )
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn render_options(shape: &str) -> NativeRenderOptions {
        NativeRenderOptions {
            count: Some(1),
            shape: Some(shape.to_string()),
            alpha: Some(Either::A(128)),
            seed: Some(7),
            background: Some("auto".to_string()),
            resize_input: Some(32),
            output_size: Some(32),
        }
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
                error.reason,
                format!("[ValidationError] unknown output format: {output}")
            );
        }
    }

    #[test]
    fn normalize_request_rejects_unknown_shape() {
        let error = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            render_options("hexagon"),
        )
        .expect_err("shape should fail");

        assert_eq!(error.reason, "[ValidationError] unknown shape: hexagon");
    }

    #[test]
    fn normalize_request_uses_rust_defaults_for_omitted_render_fields() {
        let request = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            NativeRenderOptions {
                count: None,
                shape: None,
                alpha: None,
                seed: None,
                background: None,
                resize_input: None,
                output_size: None,
            },
        )
        .expect("request should normalize");

        assert_eq!(request.render, RenderOptions::default());
    }

    fn normalize_alpha(alpha: Either<u32, String>) -> Result<ApproximateRequest> {
        normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            NativeRenderOptions {
                alpha: Some(alpha),
                ..render_options("any")
            },
        )
    }

    #[test]
    fn normalize_request_accepts_auto_alpha_string() {
        let request = normalize_alpha(Either::B("auto".to_string())).expect("auto alpha");

        assert_eq!(request.render.alpha, Alpha::Auto);
    }

    #[test]
    fn normalize_request_accepts_integer_alpha() {
        let request = normalize_alpha(Either::A(200)).expect("integer alpha");

        assert_eq!(
            request.render.alpha,
            Alpha::Fixed(NonZeroU8::new(200).expect("non-zero"))
        );
    }

    #[test]
    fn normalize_request_rejects_zero_and_out_of_range_alpha() {
        for alpha in [
            Either::A(0),
            Either::A(256),
            Either::B("0".to_string()),
            Either::B("half".to_string()),
        ] {
            let error = normalize_alpha(alpha).expect_err("alpha should fail");

            assert_eq!(
                error.reason,
                "[ValidationError] alpha must be auto or an integer 1..255"
            );
        }
    }

    #[test]
    fn normalize_request_rejects_negative_seed() {
        let error = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            NativeRenderOptions {
                seed: Some(-1),
                ..render_options("any")
            },
        )
        .expect_err("negative seed should fail");

        assert_eq!(
            error.reason,
            "[ValidationError] seed must be a positive integer"
        );
    }

    #[test]
    fn normalize_request_rejects_invalid_background() {
        let error = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            NativeRenderOptions {
                background: Some("#1234".to_string()),
                ..render_options("any")
            },
        )
        .expect_err("background should fail");

        assert_eq!(
            error.reason,
            "[ValidationError] background must be auto or an opaque hex color (RGB or RRGGBB)"
        );
    }

    #[test]
    fn task_registry_helpers_recover_from_poisoned_locks() {
        let registry = Mutex::new(HashMap::new());

        let _ = catch_unwind(AssertUnwindSafe(|| {
            with_task_registry_mut(&registry, |tasks| {
                tasks.insert(1, CancellationToken::new());
                panic!("poison the mutex");
            });
        }));

        with_task_registry_mut(&registry, |tasks| {
            tasks.insert(2, CancellationToken::new());
        });

        let task_ids =
            with_task_registry(&registry, |tasks| tasks.keys().copied().collect::<Vec<_>>());
        assert!(task_ids.contains(&1));
        assert!(task_ids.contains(&2));
    }

    #[test]
    fn registered_task_is_removed_when_setup_fails() {
        let registry = Mutex::new(HashMap::new());
        let result = with_registered_task(
            &registry,
            7,
            CancellationToken::new(),
            || -> std::result::Result<(), &'static str> { Err("boom") },
        );

        assert_eq!(result, Err("boom"));
        assert!(!with_task_registry(&registry, |tasks| tasks.contains_key(&7)));
    }

    #[test]
    fn registered_task_stays_present_when_setup_succeeds() {
        let registry = Mutex::new(HashMap::new());
        let token = CancellationToken::new();

        let result = with_registered_task(&registry, 7, token.clone(), || {
            Ok::<_, &'static str>(token.is_cancelled())
        });

        assert_eq!(result, Ok(false));
        assert!(with_task_registry(&registry, |tasks| tasks.contains_key(&7)));
    }
}
