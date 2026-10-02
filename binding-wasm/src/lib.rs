//! WebAssembly binding for browsers.
//!
//! It runs the same `primeval-render` code as the napi binding and shares its
//! request layer (`primeval-js`), so options, defaults, errors and output are
//! the same. It differs where the platform does: the render runs
//! synchronously on the calling thread (a Web Worker), and the threaded
//! build (feature `threads`) exports what the browser runtime starts the
//! rayon pool with (`PoolBuilder`, `startPoolWorker`, `wasmMemory`). A panic
//! aborts the instance; [`set_panic_reporter`] installs a hook that reports
//! its message first.
//! `scripts/build-wasm.mjs` builds both variants.
#![warn(missing_docs)]

mod panic;
#[cfg(feature = "threads")]
mod pool;
mod request;

use js_sys::{Error, Function, Object, Reflect, TypeError, Uint8Array};
use primeval_js::{JsRenderOptions, js_error_fields, js_option_name, normalize_request};
use primeval_render::{ApproximateError, ApproximateResult, CancellationToken, Execution};
use request::{JsField, render_options};
use wasm_bindgen::prelude::*;

#[doc(hidden)]
pub use panic::panic_for_tests;
pub use panic::set_panic_reporter;

#[cfg(feature = "threads")]
pub use pool::{PoolBuilder, memory, start_pool_worker};

/// Renders `input` (JPEG, PNG or WebP bytes) as `output` (`"svg"` or
/// `"png"`) and returns `{ format, data, mimeType, width, height }`, with
/// `data` a `Uint8Array`.
///
/// `render` holds the render options in their JavaScript spelling (`count`,
/// `shape`, `alpha`, `seed`, `background`, `resizeInput`, `outputSize`); an
/// `undefined` or missing field takes the Rust default. `on_progress`, if
/// given, is called with `{ step, total, score, shape }` after each step; if it
/// throws, the render stops and `approximate` throws that value.
///
/// # Errors
///
/// A render error is a JavaScript `Error` whose `code` is
/// [`ApproximateError::code`]; an invalid option also carries `option` (its
/// JavaScript name) and `requirement`. A `render` that is neither `undefined`
/// nor an object is a `TypeError`.
#[wasm_bindgen]
pub fn approximate(
    input: Vec<u8>,
    output: String,
    render: JsValue,
    on_progress: Option<Function>,
) -> Result<JsValue, JsValue> {
    let render = read_render(&render)?;
    let request = normalize_request(input, &output, render).map_err(|error| js_error(&error))?;

    let token = CancellationToken::new();
    let mut thrown = None;
    let mut on_step = |info: primeval_render::ProgressInfo| {
        let Some(callback) = on_progress.as_ref() else {
            return;
        };
        if thrown.is_some() {
            return;
        }
        let progress = Object::new();
        set(&progress, "step", info.step.into());
        set(&progress, "total", info.total.into());
        set(&progress, "score", info.score.into());
        set(&progress, "shape", info.shape.into());
        if let Err(error) = callback.call1(&JsValue::UNDEFINED, &progress) {
            thrown = Some(error);
            token.cancel();
        }
    };
    let execution = Execution::new().cancellation(&token);
    let execution = if on_progress.is_some() {
        execution.progress(&mut on_step)
    } else {
        execution
    };
    let result = primeval_render::approximate(request, execution);
    if let Some(error) = thrown {
        return Err(error);
    }
    result.map(result_object).map_err(|error| js_error(&error))
}

/// Reads and type-checks the render fields of `render`.
fn read_render(render: &JsValue) -> Result<JsRenderOptions, JsValue> {
    if render.is_undefined() {
        return Ok(JsRenderOptions::default());
    }
    if !render.is_object() {
        return Err(TypeError::new("render must be an object").into());
    }
    render_options(
        |option| Reflect::get(render, &JsValue::from_str(js_option_name(option))).map(js_field),
        |error| js_error(&error),
    )
}

/// The JavaScript type of one field value.
fn js_field(value: JsValue) -> JsField {
    if value.is_undefined() {
        JsField::Absent
    } else if let Some(number) = value.as_f64() {
        JsField::Number(number)
    } else if let Some(text) = value.as_string() {
        JsField::Text(text)
    } else if value.is_bigint() {
        // Fails for a bigint outside `0..=u64::MAX`.
        JsField::BigInt(u64::try_from(value).ok())
    } else {
        JsField::Other
    }
}

/// `{ format, data, mimeType, width, height }`, the fields of the napi
/// binding's `NativeApproximateResult`.
fn result_object(result: ApproximateResult) -> JsValue {
    let format = result.format();
    let object = Object::new();
    set(&object, "format", format.extension().into());
    set(&object, "mimeType", format.mime_type().into());
    set(&object, "width", result.width().into());
    set(&object, "height", result.height().into());
    set(
        &object,
        "data",
        Uint8Array::from(result.into_bytes().as_slice()).into(),
    );
    object.into()
}

/// A JavaScript `Error` whose `code` is the stable error code. An invalid
/// option also carries `option` (its JavaScript name) and `requirement`, as
/// in the napi binding.
fn js_error(error: &ApproximateError) -> JsValue {
    let fields = js_error_fields(error);
    let object = Error::new(&fields.message);
    set(&object, "code", fields.code.into());
    if let Some(invalid) = fields.invalid_option {
        set(&object, "option", invalid.option.into());
        set(&object, "requirement", invalid.requirement.into());
    }
    object.into()
}

/// Sets a property on an object this binding just created. That cannot fail
/// for a plain data property, and a missing property would still leave a
/// usable value, so the result is ignored.
fn set(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
