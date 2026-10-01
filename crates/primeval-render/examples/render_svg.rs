use primeval_render::{
    ApproximateRequest, ApproximateResult, InputSource, OutputFormat, RenderOptions, approximate,
};
use std::sync::atomic::AtomicBool;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cancelled = AtomicBool::new(false);
    let result = approximate(
        ApproximateRequest {
            input: InputSource::Path("photo.jpg".into()),
            output: OutputFormat::Svg,
            render: RenderOptions {
                count: 100,
                resize_input: 128,
                output_size: 512,
                ..RenderOptions::default()
            },
        },
        None,
        &cancelled,
    )?;

    match result {
        ApproximateResult::Svg { data, .. } => std::fs::write("out.svg", data)?,
        ApproximateResult::Raster { .. } => unreachable!("requested svg output"),
    }

    Ok(())
}
