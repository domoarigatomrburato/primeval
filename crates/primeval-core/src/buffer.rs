/// The smallest width and height of a [`Buffer`] built through the public
/// API: the engine's search needs a canvas of at least 2 x 2 pixels, so a
/// [`Model`](crate::Model) cannot be created for anything smaller.
const MIN_PUBLIC_SIDE: u32 = 2;

/// A contiguous RGBA pixel buffer with no row padding.
///
/// Pixels are stored in row-major order as `[R, G, B, A]` quads.
/// The total byte length is always exactly `width * height * 4`; every
/// constructor checks this with a hard assertion or rejects the input, and the
/// unsafe NEON scoring kernels rely on it. Buffers built through the public API
/// ([`Buffer::from_rgba`]) are also at least 2 x 2 pixels.
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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let len = pixel_byte_len(width, height);
        Self::from_parts(width, height, vec![0u8; len])
    }

    /// Creates a buffer filled with a single color.
    ///
    /// # Panics
    ///
    /// Panics if `width * height * 4` overflows `usize`.
    #[must_use]
    pub(crate) fn new_from_color(width: u32, height: u32, color: crate::Color) -> Self {
        let len = pixel_byte_len(width, height);
        let mut pixels = Vec::with_capacity(len);
        let pixel = [color.r, color.g, color.b, color.a];
        for _ in 0..len / 4 {
            pixels.extend_from_slice(&pixel);
        }
        Self::from_parts(width, height, pixels)
    }

    /// Assembles a buffer, asserting the length invariant
    /// `pixels.len() == width * height * 4` that the NEON kernels rely on.
    fn from_parts(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(
            pixels.len(),
            pixel_byte_len(width, height),
            "buffer pixel length must be width * height * 4"
        );
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
    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns a mutable reference to the raw pixel bytes.
    #[inline]
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Returns the byte offset of pixel `(x, y)` in the pixel slice.
    ///
    /// No bounds checking is performed — the caller is responsible for
    /// ensuring `x` and `y` are within the buffer dimensions.
    #[must_use]
    #[inline]
    pub(crate) fn pix_offset(&self, x: i32, y: i32) -> usize {
        (y as usize * self.width as usize + x as usize) * 4
    }

    /// Creates a buffer from raw row-major RGBA bytes.
    ///
    /// Returns `None` unless both sides are at least 2 pixels (the smallest
    /// canvas the engine supports) and `pixels.len()` is exactly
    /// `width * height * 4`.
    #[must_use]
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        if width < MIN_PUBLIC_SIDE || height < MIN_PUBLIC_SIDE {
            return None;
        }
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (pixels.len() == len).then(|| Self::from_parts(width, height, pixels))
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
    fn from_rgba_keeps_dimensions_and_pixels() {
        let pixels: Vec<u8> = (0..24).collect();
        let buf = Buffer::from_rgba(3, 2, pixels.clone()).expect("valid length");
        assert_eq!((buf.width(), buf.height()), (3, 2));
        assert_eq!(buf.pixels(), pixels.as_slice());
    }

    #[test]
    fn from_rgba_rejects_wrong_length() {
        assert!(Buffer::from_rgba(3, 2, vec![0; 23]).is_none());
        assert!(Buffer::from_rgba(3, 2, vec![0; 25]).is_none());
        assert!(Buffer::from_rgba(u32::MAX, u32::MAX, Vec::new()).is_none());
    }

    #[test]
    fn from_rgba_rejects_sides_below_two() {
        assert!(Buffer::from_rgba(0, 0, Vec::new()).is_none());
        assert!(Buffer::from_rgba(0, 5, Vec::new()).is_none());
        assert!(Buffer::from_rgba(1, 1, vec![0; 4]).is_none());
        assert!(Buffer::from_rgba(1, 5, vec![0; 20]).is_none());
        assert!(Buffer::from_rgba(5, 1, vec![0; 20]).is_none());
        assert!(Buffer::from_rgba(2, 2, vec![0; 16]).is_some());
    }

    #[test]
    fn zero_dimension_buffer() {
        let buf = Buffer::new(0, 0);
        assert_eq!(buf.pixels().len(), 0);
    }
}
