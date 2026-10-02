use crate::{Buffer, Color};
use image::{DynamicImage, GenericImageView, ImageEncoder, RgbaImage, imageops};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Svg,
    Png,
}

impl OutputFormat {
    #[must_use]
    pub const fn variants() -> &'static [&'static str] {
        &["svg", "png"]
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        self.as_str()
    }

    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Svg => "image/svg+xml",
            Self::Png => "image/png",
        }
    }
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "svg" => Ok(Self::Svg),
            "png" => Ok(Self::Png),
            other => Err(format!("unknown output format: {other}")),
        }
    }
}

/// Encode a buffer as a PNG image.
pub fn encode_png(buffer: &Buffer) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let image = buffer.to_image();
    let mut out = Cursor::new(Vec::new());
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder.write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ColorType::Rgba8.into(),
    )?;
    Ok(out.into_inner())
}

/// Resize an image so its longest side is at most `max_size`, preserving aspect ratio.
#[must_use]
pub fn thumbnail(image: &DynamicImage, max_size: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width <= max_size && height <= max_size {
        return image.to_rgba8();
    }

    let (new_width, new_height) = if width >= height {
        (max_size, (max_size * height / width).max(1))
    } else {
        ((max_size * width / height).max(1), max_size)
    };

    imageops::resize(
        image,
        new_width,
        new_height,
        imageops::FilterType::CatmullRom,
    )
}

/// Build output file paths from a base path and a list of formats.
///
/// When a single format is requested the base path is used as-is;
/// for multiple formats the extension is replaced per format.
#[must_use]
pub fn output_paths(base_output: &str, emits: &[OutputFormat]) -> Vec<(OutputFormat, PathBuf)> {
    let path = Path::new(base_output);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    if emits.len() == 1 {
        return vec![(emits[0], path.to_path_buf())];
    }

    emits
        .iter()
        .map(|emit| (*emit, parent.join(format!("{stem}.{}", emit.extension()))))
        .collect()
}

/// Compute the average color of an image, suitable as a background fill.
#[must_use]
pub fn average_background(image: &DynamicImage) -> Color {
    Buffer::from_image(&image.to_rgba8()).average_color()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};

    #[test]
    fn thumbnail_clamps_extreme_aspect_ratio_to_non_zero_dimensions() {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(10_000, 1, Rgba([0, 0, 0, 255])));

        let resized = thumbnail(&image, 100);

        assert_eq!(resized.width(), 100);
        assert_eq!(resized.height(), 1);
    }

    #[test]
    fn output_format_round_trips_public_names() {
        let cases = [(OutputFormat::Svg, "svg"), (OutputFormat::Png, "png")];

        for (format, value) in cases {
            assert_eq!(format.as_str(), value);
            assert_eq!(
                value.parse::<OutputFormat>().expect("output format"),
                format
            );
        }

        assert_eq!(OutputFormat::variants(), &["svg", "png"]);
    }

    #[test]
    fn output_format_rejects_unknown_name() {
        for value in ["bmp", "jpg", "jpeg", "gif"] {
            assert!(value.parse::<OutputFormat>().is_err(), "{value}");
        }
    }

    #[test]
    fn output_paths_use_format_extensions() {
        let paths = output_paths("out/base.gif", &[OutputFormat::Svg, OutputFormat::Png]);

        assert_eq!(
            paths,
            vec![
                (OutputFormat::Svg, PathBuf::from("out/base.svg")),
                (OutputFormat::Png, PathBuf::from("out/base.png")),
            ]
        );
    }
}
