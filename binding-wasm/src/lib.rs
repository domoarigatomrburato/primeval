//! WebAssembly binding for browsers.
//!
//! It runs the same `primeval-render` code as the napi binding and shares its
//! request layer (`primeval-js`), so options, defaults, errors and output are
//! the same. It differs where the platform does: the render runs
//! synchronously on the calling thread (a Web Worker), an absent seed comes
//! from `crypto.getRandomValues` because the engine's clock seed cannot run on
//! `wasm32-unknown-unknown`, and the threaded build (feature `threads`)
//! exports `initThreadPool` for the rayon pool. `scripts/build-wasm.mjs` builds
//! both variants.
#![warn(missing_docs)]

mod request;

use js_sys::{Error, Function, Object, Reflect, TypeError, Uint8Array};
use primeval_js::{JsRenderOptions, js_message, js_option_name, normalize_request};
use primeval_render::{ApproximateError, ApproximateResult, CancellationToken, Execution};
use request::{JsField, RENDER_FIELDS, fill_seed, render_options};
use wasm_bindgen::prelude::*;

#[cfg(all(target_arch = "wasm32", feature = "threads"))]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Renders `input` (JPEG, PNG or WebP bytes) as `output` (`"svg"` or
/// `"png"`) and returns `{ format, data, mimeType, width, height }`, with
/// `data` a `Uint8Array`.
///
/// `render` holds the render options in their JavaScript spelling (`count`,
/// `shape`, `alpha`, `seed`, `background`, `resizeInput`, `outputSize`); an
/// `undefined` or missing field takes the Rust default. `on_progress`, if
/// given, is called with `{ step, total, score }` after each step; if it
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
    input: &[u8],
    output: String,
    render: JsValue,
    on_progress: Option<Function>,
) -> Result<JsValue, JsValue> {
    let render = read_render(&render)?;
    let mut request =
        normalize_request(input.to_vec(), &output, render).map_err(|error| js_error(&error))?;
    fill_seed(&mut request.render, random_seed)?;

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
    let mut fields = Vec::with_capacity(RENDER_FIELDS.len());
    for option in RENDER_FIELDS {
        let value = Reflect::get(render, &JsValue::from_str(js_option_name(option)))?;
        fields.push((option, js_field(value)));
    }
    render_options(|option| {
        fields
            .iter()
            .find(|(field, _)| *field == option)
            .map_or(JsField::Absent, |(_, value)| value.clone())
    })
    .map_err(|error| js_error(&error))
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

/// A seed from `crypto.getRandomValues`, read into a JavaScript-owned buffer
/// because the API rejects views of a shared (threaded) wasm memory.
fn random_seed() -> Result<u64, JsValue> {
    let random = || -> Result<u64, JsValue> {
        let crypto = Reflect::get(&js_sys::global(), &JsValue::from_str("crypto"))?;
        let get_random_values: Function =
            Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))?.dyn_into()?;
        let bytes = Uint8Array::new_with_length(8);
        get_random_values.call1(&crypto, &bytes)?;
        let mut seed = [0_u8; 8];
        bytes.copy_to(&mut seed);
        Ok(u64::from_le_bytes(seed))
    };
    random().map_err(|_| {
        js_error(&ApproximateError::internal(
            "crypto.getRandomValues is unavailable for a random seed",
        ))
    })
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
    let object = Error::new(&js_message(error));
    set(&object, "code", error.code().into());
    if let ApproximateError::InvalidOption { option } = error {
        set(&object, "option", js_option_name(*option).into());
        set(&object, "requirement", option.requirement().into());
    }
    object.into()
}

/// Sets a property on an object this binding just created. That cannot fail
/// for a plain data property, and a missing property would still leave a
/// usable value, so the result is ignored.
fn set(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
