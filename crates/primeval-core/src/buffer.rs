/// A contiguous RGBA pixel buffer with no row padding.
///
/// Pixels are stored in row-major order as `[R, G, B, A]` quads.
/// The total byte length is always exactly `width * height * 4`.
#[derive(Clone, Debug)]
pub struct Buffer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Buffer {
    /// Creates a zero-filled buffer of the given dimensions.
    ///
    /// # Panics
    ///
    /// Panics if `width * height * 4` overflows `usize`.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let len = pixel_byte_len(width, height);
        Self {
            width,
            height,
            pixels: vec![0u8; len],
        }
    }

    /// Creates a buffer filled with a single color.
    ///
    /// # Panics
    ///
    /// Panics if `width * height * 4` overflows `usize`.
    #[must_use]
    pub fn new_from_color(width: u32, height: u32, color: crate::Color) -> Self {
        let len = pixel_byte_len(width, height);
        let mut pixels = Vec::with_capacity(len);
        let pixel = [color.r, color.g, color.b, color.a];
        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&pixel);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Returns the buffer width in pixels.
    #[must_use]
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the buffer height in pixels.
    #[must_use]
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns a shared reference to the raw pixel bytes.
    #[must_use]
    #[inline]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns a mutable reference to the raw pixel bytes.
    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Returns the byte offset of pixel `(x, y)` in the pixel slice.
    ///
    /// No bounds checking is performed — the caller is responsible for
    /// ensuring `x` and `y` are within the buffer dimensions.
    #[must_use]
    #[inline]
    pub fn pix_offset(&self, x: i32, y: i32) -> usize {
        (y as usize * self.width as usize + x as usize) * 4
    }

    /// Copies all pixels from `other` into `self`.
    ///
    /// # Panics
    ///
    /// Panics if the dimensions of `self` and `other` differ.
    pub fn copy_from(&mut self, other: &Buffer) {
        assert_eq!(
            self.width, other.width,
            "copy_from: width mismatch ({} vs {})",
            self.width, other.width
        );
        assert_eq!(
            self.height, other.height,
            "copy_from: height mismatch ({} vs {})",
            self.height, other.height
        );
        self.pixels.copy_from_slice(&other.pixels);
    }

    /// Creates a buffer from an `image::RgbaImage`.
    ///
    /// The resulting buffer has identical dimensions and pixel data.
    #[must_use]
    pub fn from_image(img: &image::RgbaImage) -> Self {
        let width = img.width();
        let height = img.height();
        let pixels = img.as_raw().clone();
        debug_assert_eq!(pixels.len(), pixel_byte_len(width, height));
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Converts this buffer to an `image::RgbaImage`.
    #[must_use]
    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .expect("pixel data length matches width * height * 4")
    }

    /// Computes the average color of all pixels in the buffer.
    ///
    /// Alpha is always set to 255 in the result, matching the Go
    /// `AverageImageColor` behavior.
    #[must_use]
    pub fn average_color(&self) -> crate::Color {
        let pixel_count = self.width as usize * self.height as usize;
        if pixel_count == 0 {
            return crate::Color::default();
        }

        let (mut r_sum, mut g_sum, mut b_sum) = (0u64, 0u64, 0u64);
        for chunk in self.pixels.as_chunks::<4>().0 {
            r_sum += u64::from(chunk[0]);
            g_sum += u64::from(chunk[1]);
            b_sum += u64::from(chunk[2]);
        }

        let n = pixel_count as u64;
        crate::Color {
            r: (r_sum / n) as u8,
            g: (g_sum / n) as u8,
            b: (b_sum / n) as u8,
            a: 255,
        }
    }
}

/// Computes the required byte length for a buffer, panicking on overflow.
fn pixel_byte_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .expect("buffer pixel byte length must not overflow usize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn new_creates_correct_size() {
        let buf = Buffer::new(10, 20);
        assert_eq!(buf.width(), 10);
        assert_eq!(buf.height(), 20);
        assert_eq!(buf.pixels().len(), 10 * 20 * 4);
    }

    #[test]
    fn new_is_zeroed() {
        let buf = Buffer::new(3, 3);
        assert!(buf.pixels().iter().all(|&b| b == 0));
    }

    #[test]
    fn new_from_color_fills_correctly() {
        let c = Color::new(10, 20, 30, 255);
        let buf = Buffer::new_from_color(2, 2, c);
        assert_eq!(buf.pixels().len(), 2 * 2 * 4);
        for chunk in buf.pixels().as_chunks::<4>().0 {
            assert_eq!(*chunk, [10, 20, 30, 255]);
        }
    }

    #[test]
    fn pix_offset_computes_correctly() {
        let buf = Buffer::new(10, 10);
        // Pixel (0, 0) -> offset 0
        assert_eq!(buf.pix_offset(0, 0), 0);
        // Pixel (1, 0) -> offset 4
        assert_eq!(buf.pix_offset(1, 0), 4);
        // Pixel (0, 1) -> offset 10*4 = 40
        assert_eq!(buf.pix_offset(0, 1), 40);
        // Pixel (3, 2) -> (2*10 + 3)*4 = 92
        assert_eq!(buf.pix_offset(3, 2), 92);
    }

    #[test]
    fn average_color_uniform_buffer() {
        let c = Color::new(100, 150, 200, 255);
        let buf = Buffer::new_from_color(4, 4, c);
        let avg = buf.average_color();
        assert_eq!(avg, Color::new(100, 150, 200, 255));
    }

    #[test]
    fn average_color_mixed() {
        let mut buf = Buffer::new(2, 1);
        // Pixel (0,0) = (10, 20, 30, 255)
        let pix = buf.pixels_mut();
        pix[0] = 10;
        pix[1] = 20;
        pix[2] = 30;
        pix[3] = 255;
        // Pixel (1,0) = (30, 40, 50, 255)
        pix[4] = 30;
        pix[5] = 40;
        pix[6] = 50;
        pix[7] = 255;

        let avg = buf.average_color();
        assert_eq!(avg, Color::new(20, 30, 40, 255));
    }

    #[test]
    fn copy_from_copies_pixels() {
        let c = Color::new(42, 84, 126, 255);
        let src = Buffer::new_from_color(3, 3, c);
        let mut dst = Buffer::new(3, 3);
        dst.copy_from(&src);
        assert_eq!(dst.pixels(), src.pixels());
    }

    #[test]
    #[should_panic(expected = "width mismatch")]
    fn copy_from_panics_on_dimension_mismatch() {
        let src = Buffer::new(3, 3);
        let mut dst = Buffer::new(4, 3);
        dst.copy_from(&src);
    }

    #[test]
    fn roundtrip_through_image() {
        let c = Color::new(10, 20, 30, 255);
        let buf = Buffer::new_from_color(5, 5, c);
        let img = buf.to_image();
        let buf2 = Buffer::from_image(&img);
        assert_eq!(buf.width(), buf2.width());
        assert_eq!(buf.height(), buf2.height());
        assert_eq!(buf.pixels(), buf2.pixels());
    }

    #[test]
    fn zero_dimension_buffer() {
        let buf = Buffer::new(0, 0);
        assert_eq!(buf.pixels().len(), 0);
        assert_eq!(buf.average_color(), Color::default());
    }
}
