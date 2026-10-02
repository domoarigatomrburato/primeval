use image::{DynamicImage, GenericImageView, RgbaImage, imageops};
use primeval_core::Color;

/// Resize an image so its longest side is at most `max_size`, preserving aspect ratio.
pub(crate) fn thumbnail(image: &DynamicImage, max_size: u32) -> RgbaImage {
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

/// Compute the alpha-weighted mean color of an image as an opaque background.
///
/// Each channel is `sum(a * c) / sum(a)`, rounded down. A fully transparent
/// image yields white.
pub(crate) fn average_background(image: &RgbaImage) -> Color {
    let (mut r, mut g, mut b, mut weight) = (0u64, 0u64, 0u64, 0u64);
    for pixel in image.pixels() {
        let [pr, pg, pb, pa] = pixel.0.map(u64::from);
        r += pr * pa;
        g += pg * pa;
        b += pb * pa;
        weight += pa;
    }
    if weight == 0 {
        return Color::new(255, 255, 255, 255);
    }
    // Each quotient is a weighted mean of u8 values, so it fits in u8.
    Color::new(
        (r / weight) as u8,
        (g / weight) as u8,
        (b / weight) as u8,
        255,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn thumbnail_clamps_extreme_aspect_ratio_to_non_zero_dimensions() {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(10_000, 1, Rgba([0, 0, 0, 255])));

        let resized = thumbnail(&image, 100);

        assert_eq!(resized.width(), 100);
        assert_eq!(resized.height(), 1);
    }
}
