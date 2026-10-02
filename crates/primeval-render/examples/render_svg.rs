//! Render an image as SVG.
//!
//! Usage: `cargo run --example render_svg -- <input> [output.svg]`
//!
//! Writes the SVG to the output path, or to stdout when it is omitted.
//! Progress is reported on stderr.

use primeval_render::{
    ApproximateRequest, ApproximateResult, OutputFormat, ProgressInfo, RenderOptions, approximate,
};
use std::io::Write;
use std::sync::atomic::AtomicBool;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: render_svg <input> [output.svg]");
        std::process::exit(2);
    };
    let output = args.next();

    let on_progress = |info: ProgressInfo| eprintln!("step {}/{}", info.step, info.total);
    // Set this flag from another thread to abort the render between steps.
    let cancelled = AtomicBool::new(false);
    let result = approximate(
        ApproximateRequest {
            input: std::fs::read(&input)?,
            output: OutputFormat::Svg,
            render: RenderOptions {
                count: 100,
                resize_input: 128,
                output_size: 512,
                ..RenderOptions::default()
            },
        },
        Some(&on_progress),
        &cancelled,
    )?;

    let ApproximateResult::Svg { data, .. } = result else {
        unreachable!("requested svg output");
    };
    match output {
        Some(path) => std::fs::write(path, data)?,
        None => std::io::stdout().write_all(data.as_bytes())?,
    }

    Ok(())
}
