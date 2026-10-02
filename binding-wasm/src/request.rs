//! The host-testable part of the request: turning classified JavaScript
//! field values into [`JsRenderOptions`], and filling an absent seed.

use primeval_js::{JsAlpha, JsRenderOptions, JsSeed};
use primeval_render::{ApproximateError, RenderOption, RenderOptions};

/// The render fields the binding reads, in the order it reads them.
pub(crate) const RENDER_FIELDS: [RenderOption; 7] = [
    RenderOption::Count,
    RenderOption::Shape,
    RenderOption::Alpha,
    RenderOption::Seed,
    RenderOption::Background,
    RenderOption::ResizeInput,
    RenderOption::OutputSize,
];

/// The JavaScript type of one render field, with its value where the request
/// can use it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsField {
    /// `undefined`, or a missing property.
    Absent,
    Number(f64),
    Text(String),
    /// A `bigint`: `Some` if it is in `0..=u64::MAX`, `None` otherwise.
    BigInt(Option<u64>),
    /// Any other type, including `null`.
    Other,
}

/// Builds the render options from each field's value. A field of the wrong
/// type is an invalid value for that option.
pub(crate) fn render_options(
    mut field: impl FnMut(RenderOption) -> JsField,
) -> Result<JsRenderOptions, ApproximateError> {
    let mut read = |option: RenderOption| (option, field(option));
    Ok(JsRenderOptions {
        count: number(read(RenderOption::Count))?,
        shape: text(read(RenderOption::Shape))?,
        alpha: match read(RenderOption::Alpha) {
            (_, JsField::Absent) => None,
            (_, JsField::Number(alpha)) => Some(JsAlpha::Number(alpha)),
            (_, JsField::Text(alpha)) => Some(JsAlpha::Text(alpha)),
            (option, _) => return Err(ApproximateError::invalid_option(option)),
        },
        seed: match read(RenderOption::Seed) {
            (_, JsField::Absent) => None,
            (_, JsField::Number(seed)) => Some(JsSeed::Number(seed)),
            (_, JsField::BigInt(Some(seed))) => Some(JsSeed::BigInt(seed)),
            (_, JsField::BigInt(None)) => Some(JsSeed::BigIntOutOfRange),
            (option, _) => return Err(ApproximateError::invalid_option(option)),
        },
        background: text(read(RenderOption::Background))?,
        resize_input: number(read(RenderOption::ResizeInput))?,
        output_size: number(read(RenderOption::OutputSize))?,
    })
}

fn number((option, value): (RenderOption, JsField)) -> Result<Option<f64>, ApproximateError> {
    match value {
        JsField::Absent => Ok(None),
        JsField::Number(number) => Ok(Some(number)),
        _ => Err(ApproximateError::invalid_option(option)),
    }
}

fn text((option, value): (RenderOption, JsField)) -> Result<Option<String>, ApproximateError> {
    match value {
        JsField::Absent => Ok(None),
        JsField::Text(text) => Ok(Some(text)),
        _ => Err(ApproximateError::invalid_option(option)),
    }
}

/// Fills an absent seed with `random()`, because the engine's clock seed
/// cannot run on `wasm32-unknown-unknown`. An explicit seed is kept, and
/// `random` is not called.
pub(crate) fn fill_seed<E>(
    render: &mut RenderOptions,
    random: impl FnOnce() -> Result<u64, E>,
) -> Result<(), E> {
    if render.seed.is_none() {
        render.seed = Some(random()?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use primeval_js::js_message;

    fn fields(values: &[(RenderOption, JsField)]) -> impl FnMut(RenderOption) -> JsField {
        let values = values.to_vec();
        move |option| {
            values
                .iter()
                .find(|(name, _)| *name == option)
                .map_or(JsField::Absent, |(_, value)| value.clone())
        }
    }

    #[test]
    fn absent_fields_stay_absent() {
        assert_eq!(
            render_options(fields(&[])).expect("no fields"),
            JsRenderOptions::default()
        );
    }

    #[test]
    fn fields_of_the_accepted_types_convert() {
        let render = render_options(fields(&[
            (RenderOption::Count, JsField::Number(10.0)),
            (RenderOption::Shape, JsField::Text("circle".into())),
            (RenderOption::Alpha, JsField::Text("auto".into())),
            (RenderOption::Seed, JsField::BigInt(Some(u64::MAX))),
            (RenderOption::Background, JsField::Text("#fff".into())),
            (RenderOption::ResizeInput, JsField::Number(64.0)),
            (RenderOption::OutputSize, JsField::Number(128.0)),
        ]))
        .expect("valid fields");

        assert_eq!(
            render,
            JsRenderOptions {
                count: Some(10.0),
                shape: Some("circle".into()),
                alpha: Some(JsAlpha::Text("auto".into())),
                seed: Some(JsSeed::BigInt(u64::MAX)),
                background: Some("#fff".into()),
                resize_input: Some(64.0),
                output_size: Some(128.0),
            }
        );

        let render = render_options(fields(&[
            (RenderOption::Alpha, JsField::Number(9.0)),
            (RenderOption::Seed, JsField::Number(3.0)),
        ]))
        .expect("number alpha and seed");
        assert_eq!(render.alpha, Some(JsAlpha::Number(9.0)));
        assert_eq!(render.seed, Some(JsSeed::Number(3.0)));

        let render = render_options(fields(&[(RenderOption::Seed, JsField::BigInt(None))]))
            .expect("the range is checked when the request is normalized");
        assert_eq!(render.seed, Some(JsSeed::BigIntOutOfRange));
    }

    #[test]
    fn a_field_of_the_wrong_type_is_an_invalid_option() {
        let wrong: [(RenderOption, &[JsField]); 7] = [
            (
                RenderOption::Count,
                &[
                    JsField::Text("1".into()),
                    JsField::BigInt(Some(1)),
                    JsField::Other,
                ],
            ),
            (
                RenderOption::Shape,
                &[
                    JsField::Number(1.0),
                    JsField::BigInt(Some(1)),
                    JsField::Other,
                ],
            ),
            (
                RenderOption::Alpha,
                &[JsField::BigInt(Some(1)), JsField::Other],
            ),
            (
                RenderOption::Seed,
                &[JsField::Text("1".into()), JsField::Other],
            ),
            (
                RenderOption::Background,
                &[JsField::Number(1.0), JsField::Other],
            ),
            (
                RenderOption::ResizeInput,
                &[JsField::Text("1".into()), JsField::Other],
            ),
            (
                RenderOption::OutputSize,
                &[JsField::Text("1".into()), JsField::Other],
            ),
        ];
        for (option, values) in wrong {
            for value in values {
                let error = render_options(fields(&[(option, value.clone())]))
                    .expect_err("wrong type should fail");

                assert!(
                    matches!(error, ApproximateError::InvalidOption { option: o } if o == option),
                    "{option:?} = {value:?}: {}",
                    js_message(&error)
                );
            }
        }
    }

    #[test]
    fn every_render_field_is_read() {
        let mut read = Vec::new();
        render_options(|option| {
            read.push(option);
            JsField::Absent
        })
        .expect("no fields");

        assert_eq!(read, RENDER_FIELDS);
    }

    #[test]
    fn fill_seed_fills_only_an_absent_seed() {
        let mut render = RenderOptions::default();
        assert_eq!(render.seed, None);
        fill_seed(&mut render, || Ok::<_, ()>(42)).expect("random seed");
        assert_eq!(render.seed, Some(42));

        let mut render = RenderOptions::default();
        render.seed = Some(7);
        fill_seed(&mut render, || -> Result<u64, ()> {
            panic!("an explicit seed needs no random value")
        })
        .expect("explicit seed");
        assert_eq!(render.seed, Some(7));

        let mut render = RenderOptions::default();
        assert_eq!(
            fill_seed(&mut render, || Err("no crypto")),
            Err("no crypto")
        );
        assert_eq!(render.seed, None);
    }
}
