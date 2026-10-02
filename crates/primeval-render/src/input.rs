use image::{DynamicImage, GenericImageView, Pixel, RgbaImage, imageops};
use primeval_core::Color;

/// Resize an image so its longest side is at most `max_size`, preserving
/// aspect ratio. Consumes the image so the full-resolution pixels are freed
/// as soon as the thumbnail exists.
///
/// Neither side drops below 2 pixels: the engine needs a working canvas of
/// at least 2 x 2, and callers reject inputs smaller than that.
pub(crate) fn thumbnail(image: DynamicImage, max_size: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width <= max_size && height <= max_size {
        return image.into_rgba8();
    }

    // The quotient is at most `max_size`, so the casts back to u32 are exact.
    let scaled = |side: u32, longest: u32| {
        ((u64::from(max_size) * u64::from(side) / u64::from(longest)) as u32).max(2)
    };
    let (new_width, new_height) = if width >= height {
        (max_size, scaled(height, width))
    } else {
        (scaled(width, height), max_size)
    };

    imageops::resize(
        &image,
        new_width,
        new_height,
        imageops::FilterType::CatmullRom,
    )
}

/// Compute the alpha-weighted mean color of an image as an opaque background.
///
/// Each channel is `sum(a * c) / sum(a)`, rounded down. A fully transparent
/// image yields white. Reads the decoded pixels in place; 8-bit RGB and RGBA
/// skip the per-pixel conversion.
pub(crate) fn average_background(image: &DynamicImage) -> Color {
    let mut sums = [0u64; 4];
    let mut add = |[r, g, b, a]: [u8; 4]| {
        let a = u64::from(a);
        sums[0] += u64::from(r) * a;
        sums[1] += u64::from(g) * a;
        sums[2] += u64::from(b) * a;
        sums[3] += a;
    };
    match image {
        DynamicImage::ImageRgb8(pixels) => {
            pixels.pixels().for_each(|pixel| {
                let [r, g, b] = pixel.0;
                add([r, g, b, 255]);
            });
        }
        DynamicImage::ImageRgba8(pixels) => pixels.pixels().for_each(|pixel| add(pixel.0)),
        other => other
            .pixels()
            .for_each(|(_, _, pixel)| add(pixel.to_rgba().0)),
    }
    let [r, g, b, weight] = sums;
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
    use image::{Rgb, RgbImage};

    #[test]
    fn thumbnail_keeps_both_sides_at_least_two_pixels() {
        let banner = DynamicImage::ImageRgb8(RgbImage::from_pixel(2000, 5, Rgb([0, 0, 0])));
        let resized = thumbnail(banner, 256);
        assert_eq!(resized.dimensions(), (256, 2));

        let column = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 10_000, Rgb([0, 0, 0])));
        let resized = thumbnail(column, 100);
        assert_eq!(resized.dimensions(), (2, 100));
    }

    #[test]
    fn average_background_reads_rgb_and_rgba_without_conversion() {
        let rgb = DynamicImage::ImageRgb8(RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgb([10, 20, 30])
            } else {
                Rgb([30, 40, 50])
            }
        }));
        assert_eq!(average_background(&rgb), Color::new(20, 30, 40, 255));

        let luma = DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::LumaA([100, 255])
            } else {
                image::LumaA([0, 0])
            }
        }));
        assert_eq!(average_background(&luma), Color::new(100, 100, 100, 255));
    }
}
