use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, Status};
use napi_derive::napi;
use primeval_core::shapes::ShapeKind;
use primeval_render::{
    ApproximateError, ApproximateRequest, ApproximateResult, OutputFormat, ProgressInfo,
    RenderOptions, approximate, parse_alpha_str, parse_background_str, parse_seed_i64,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static NEXT_TASK_ID: AtomicU32 = AtomicU32::new(1);
static TASKS: OnceLock<Mutex<HashMap<u32, Arc<AtomicBool>>>> = OnceLock::new();

fn tasks() -> &'static Mutex<HashMap<u32, Arc<AtomicBool>>> {
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_task_registry<R>(
    registry: &Mutex<HashMap<u32, Arc<AtomicBool>>>,
    f: impl FnOnce(&HashMap<u32, Arc<AtomicBool>>) -> R,
) -> R {
    match registry.lock() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(&poisoned.into_inner()),
    }
}

fn with_task_registry_mut<R>(
    registry: &Mutex<HashMap<u32, Arc<AtomicBool>>>,
    f: impl FnOnce(&mut HashMap<u32, Arc<AtomicBool>>) -> R,
) -> R {
    match registry.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            f(&mut guard)
        }
    }
}

fn with_tasks<R>(f: impl FnOnce(&HashMap<u32, Arc<AtomicBool>>) -> R) -> R {
    with_task_registry(tasks(), f)
}

fn with_tasks_mut<R>(f: impl FnOnce(&mut HashMap<u32, Arc<AtomicBool>>) -> R) -> R {
    with_task_registry_mut(tasks(), f)
}

fn with_registered_task<T, E>(
    registry: &Mutex<HashMap<u32, Arc<AtomicBool>>>,
    task_id: u32,
    cancelled: Arc<AtomicBool>,
    setup: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    with_task_registry_mut(registry, |tasks| {
        tasks.insert(task_id, cancelled);
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
    pub alpha: Option<String>,
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
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_future = Arc::clone(&cancelled);
    let task_id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);

    with_registered_task(tasks(), task_id, cancelled, || {
        let promise = env.spawn_future_with_callback(
            async move {
                let result = if let Some(tsfn) = progress.as_ref() {
                    let on_progress = |info: ProgressInfo| {
                        let _ = tsfn.call(
                            Ok(NativeProgressInfo {
                                step: info.step,
                                total: info.total,
                                score: info.score,
                            }),
                            ThreadsafeFunctionCallMode::NonBlocking,
                        );
                    };
                    approximate(request, Some(&on_progress), cancelled_for_future.as_ref())
                } else {
                    approximate(request, None, cancelled_for_future.as_ref())
                };
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
    if let Some(cancelled) = with_tasks(|tasks| tasks.get(&task_id).cloned()) {
        cancelled.store(true, Ordering::SeqCst);
    }
}

fn normalize_request(
    input: Buffer,
    output: String,
    render: NativeRenderOptions,
) -> Result<ApproximateRequest> {
    let format = output
        .parse::<OutputFormat>()
        .map_err(|message| napi_error("ValidationError", message))?;

    let defaults = RenderOptions::default();

    let shape = render
        .shape
        .as_deref()
        .map(|shape| shape.parse::<ShapeKind>())
        .transpose()
        .map_err(|message| napi_error("ValidationError", message))?
        .unwrap_or(defaults.shape);

    let alpha = render
        .alpha
        .as_deref()
        .map(parse_alpha_str)
        .transpose()
        .map_err(|message| napi_error("ValidationError", message))?
        .unwrap_or(defaults.alpha);

    let background = render
        .background
        .as_deref()
        .map(parse_background_str)
        .transpose()
        .map_err(|message| napi_error("ValidationError", message))?
        .unwrap_or(defaults.background);

    let seed = render
        .seed
        .map(|seed| parse_seed_i64(seed).map_err(|message| napi_error("ValidationError", message)))
        .transpose()?;

    Ok(ApproximateRequest {
        input: input.into(),
        output: format,
        render: RenderOptions {
            count: render.count.unwrap_or(defaults.count),
            shape,
            alpha,
            seed,
            background,
            resize_input: render.resize_input.unwrap_or(defaults.resize_input),
            output_size: render.output_size.unwrap_or(defaults.output_size),
        },
    })
}

fn map_error(error: ApproximateError) -> Error {
    match error {
        ApproximateError::Validation(message) => napi_error("ValidationError", message),
        ApproximateError::Aborted => napi_error("AbortError", "operation aborted"),
        ApproximateError::Internal(message) => napi_error("Error", message),
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
        let data = match value {
            ApproximateResult::Svg { data, .. } => Buffer::from(data.into_bytes()),
            ApproximateResult::Raster { data, .. } => Buffer::from(data),
        };
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn render_options(shape: &str) -> NativeRenderOptions {
        NativeRenderOptions {
            count: Some(1),
            shape: Some(shape.to_string()),
            alpha: Some("128".to_string()),
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

    #[test]
    fn normalize_request_accepts_auto_alpha_string() {
        let request = normalize_request(
            Buffer::from(vec![0_u8; 4]),
            "svg".to_string(),
            NativeRenderOptions {
                alpha: Some("auto".to_string()),
                ..render_options("any")
            },
        )
        .expect("request should normalize");

        assert_eq!(request.render.alpha, primeval_render::AlphaOption::Auto);
    }

    #[test]
    fn task_registry_helpers_recover_from_poisoned_locks() {
        let registry = Mutex::new(HashMap::new());

        let _ = catch_unwind(AssertUnwindSafe(|| {
            with_task_registry_mut(&registry, |tasks| {
                tasks.insert(1, Arc::new(AtomicBool::new(false)));
                panic!("poison the mutex");
            });
        }));

        let cancelled = Arc::new(AtomicBool::new(false));
        with_task_registry_mut(&registry, |tasks| {
            tasks.insert(2, Arc::clone(&cancelled));
        });

        let task_ids =
            with_task_registry(&registry, |tasks| tasks.keys().copied().collect::<Vec<_>>());
        assert!(task_ids.contains(&1));
        assert!(task_ids.contains(&2));
    }

    #[test]
    fn registered_task_is_removed_when_setup_fails() {
        let registry = Mutex::new(HashMap::new());
        let cancelled = Arc::new(AtomicBool::new(false));

        let result = with_registered_task(
            &registry,
            7,
            cancelled,
            || -> std::result::Result<(), &'static str> { Err("boom") },
        );

        assert_eq!(result, Err("boom"));
        assert!(!with_task_registry(&registry, |tasks| tasks.contains_key(&7)));
    }

    #[test]
    fn registered_task_stays_present_when_setup_succeeds() {
        let registry = Mutex::new(HashMap::new());
        let cancelled = Arc::new(AtomicBool::new(false));

        let result = with_registered_task(&registry, 7, Arc::clone(&cancelled), || {
            Ok::<_, &'static str>(cancelled.load(Ordering::SeqCst))
        });

        assert_eq!(result, Ok(false));
        assert!(with_task_registry(&registry, |tasks| tasks.contains_key(&7)));
    }
}
