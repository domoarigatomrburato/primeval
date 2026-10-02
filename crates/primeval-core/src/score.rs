//! Scoring and blending routines for the energy minimization loop.
//!
//! Blending and scoring keep the Go original's integer arithmetic,
//! truncation semantics, and accumulator widths; the colour fit weights
//! pixels by coverage instead. Buffers are opaque RGB, so every kernel
//! works on the three colour channels only. On aarch64, hot paths use NEON
//! intrinsics to process 8 pixels per iteration.
//!
//! The search scores every candidate against the same target and canvas, so
//! [`fit`] reads each line's sums from the step's [`PrefixSums`] and the
//! energy only blends; [`energy_below`] stops a candidate as soon as it
//! cannot beat the best energy so far.

use crate::buffer::{BYTES_PER_PIXEL, Buffer};
use crate::color::Color;
use crate::prefix::{MAX_CHANNEL_SPAN, PrefixSums, SpanSums};
use crate::scanline::{Scanline, clamp_line};

const M: u32 = 0xFFFF;

/// Converts a raw score, the sum of squared channel differences over every
/// pixel, into the normalised RMSE over the RGB channels:
/// `sqrt(raw / (w · h · 3)) / 255`, from `0.0` (identical) to `1.0`.
#[inline]
pub(crate) fn raw_score_to_normalized(raw: u64, width: u32, height: u32) -> f64 {
    (raw as f64 / channel_count(width, height)).sqrt() / 255.0
}

#[cfg(test)]
#[inline]
fn normalized_to_raw_score(score: f64, width: u32, height: u32) -> u64 {
    let s = score * 255.0;
    (s * s * channel_count(width, height)).round() as u64
}

/// The number of channel values a `width × height` buffer holds.
#[inline]
fn channel_count(width: u32, height: u32) -> f64 {
    f64::from(width) * f64::from(height) * BYTES_PER_PIXEL as f64
}

/// Exclusive upper bound of every blend value `v` passed to [`div_by_m`]:
/// `M * (M + 1) = 0xFFFF_0000`, which fits in `u32`.
const BLEND_BOUND: u32 = M * (M + 1);

/// Returns `value / M` without a division.
///
/// Exact for every `value < M * (M + 1)`. Write `value = q * M + r` with
/// `0 <= r < M` and `q <= M`. Since `M = 2^16 - 1`, `value >> 16` is `q - 1`
/// when `r < q` and `q` otherwise, so `value + 1 + (value >> 16)` is
/// `q * 2^16 + r` or `q * 2^16 + r + 1`, both below `(q + 1) * 2^16`; shifting
/// right by 16 gives `q`. The sum stays below `2^32`, so it cannot overflow.
#[inline]
fn div_by_m(value: u32) -> u32 {
    debug_assert!(value < BLEND_BOUND, "blend value {value} out of range");
    (value + 1 + (value >> 16)) >> 16
}

/// Blends one 8-bit channel: `current` scaled by `a`, plus the premultiplied
/// 16-bit `source` scaled by the coverage `ma`, back to 8 bits.
///
/// Overflow bound: with `sa` the colour's premultiplied alpha,
/// `a = (M - sa * ma / M) * 0x101`, `source <= sa` and `ma <= M`, and
/// `k = sa * ma / M` (rounded down), so `sa * ma < (k + 1) * M`. Then
/// `v = current * a + source * ma <= M * (M - k) + sa * ma
/// < M * (M - k) + M * (k + 1) = M * (M + 1)`, where `current * 0x101 <= M`.
/// The NEON blend computes the same `v` per lane, so the bound holds there too.
#[inline]
fn blend_channel_scalar(current: u8, source: u32, ma: u32, a: u32) -> u8 {
    debug_assert!(ma <= M && source <= M, "source={source} ma={ma}");
    let value = u32::from(current) * a + source * ma;
    (div_by_m(value) >> 8) as u8
}

/// Weighted least-squares fit of a shape's colour, accumulated one scanline
/// at a time.
///
/// Drawing channel value `s` with weight `w` turns `c` into
/// `c · (1 − w) + s · w`, so the `s` that brings the shape's pixels closest
/// to the target `t` minimises `Σ (t − c · (1 − w) − s · w)²`, which gives
/// `s* = Σ w · (t − (1 − w) · c) / Σ w²`. The weight is
/// `w = (alpha / 255) · (coverage / M)`, held in the blend's own fixed
/// point: [`draw_lines`] scales the canvas by `M − k` with
/// `k = (alpha · 0x101) · coverage / M` (rounded down), so `w = k / M`.
/// Multiplying through by `M²` keeps every sum an integer:
/// `s* = (M · Σ k · (t − c) + Σ k² · c) / Σ k²`. Coverage is constant along
/// a scanline, so each line contributes through its per-channel sums of `t`
/// and `c` alone.
///
/// Bounds: `k <= M < 2^16`, so a line of `n` pixels adds less than
/// `2^24 · n` to `Σ k · (t − c)`, `2^40 · n` to `Σ k² · c` and `2^32 · n` to
/// `Σ k²`. Lines are summed in 64-bit batches of fewer than
/// [`Self::BATCH_PIXELS`] pixels, which keeps every batch below `2^62`, and
/// each batch is added to `i128` totals that no pixel count can overflow.
struct ColorFit {
    batch: FitSums<i64, u64>,
    batch_pixels: u64,
    total: FitSums<i128, i128>,
}

/// `Σ k · (t − c)` and `Σ k² · c` per channel, and `Σ k²` over every pixel.
#[derive(Default)]
struct FitSums<S, U> {
    differences: [S; 3],
    currents: [U; 3],
    weights: U,
}

impl ColorFit {
    /// Pixels per 64-bit batch: `2^40 · 2^22 = 2^62`.
    const BATCH_PIXELS: u64 = 1 << 22;

    fn new() -> Self {
        Self {
            batch: FitSums::default(),
            batch_pixels: 0,
            total: FitSums::default(),
        }
    }

    /// The blend's fixed-point weight `k` of a scanline: zero for a line the
    /// blend leaves unchanged, which therefore does not affect the fit.
    #[inline]
    fn weight(alpha: i32, coverage: u32) -> u32 {
        // `alpha * 0x101 <= M` and `coverage <= M`, so the product is below
        // `M * (M + 1)`, where `div_by_m` is exact.
        div_by_m(alpha as u32 * 0x101 * coverage)
    }

    /// Adds a scanline of `pixels` pixels with weight `k` whose RGB channels
    /// sum to `target` in the target and `current` on the canvas.
    #[inline]
    fn add_line(&mut self, k: u32, pixels: usize, target: [u64; 3], current: [u64; 3]) {
        let pixels = pixels as u64;
        if self.batch_pixels + pixels >= Self::BATCH_PIXELS {
            self.flush();
            if pixels >= Self::BATCH_PIXELS {
                self.add_long_line(k, pixels, target, current);
                return;
            }
        }
        self.batch_pixels += pixels;
        let k = u64::from(k);
        let k2 = k * k;
        for channel in 0..3 {
            let difference = target[channel] as i64 - current[channel] as i64;
            self.batch.differences[channel] += k as i64 * difference;
            self.batch.currents[channel] += k2 * current[channel];
        }
        self.batch.weights += k2 * pixels;
    }

    /// [`Self::add_line`] in `i128` for a line too long for a batch.
    #[cold]
    fn add_long_line(&mut self, k: u32, pixels: u64, target: [u64; 3], current: [u64; 3]) {
        let k = i128::from(k);
        for channel in 0..3 {
            let (t, c) = (i128::from(target[channel]), i128::from(current[channel]));
            self.total.differences[channel] += k * (t - c);
            self.total.currents[channel] += k * k * c;
        }
        self.total.weights += k * k * i128::from(pixels);
    }

    /// Moves the batch into the `i128` totals.
    fn flush(&mut self) {
        for channel in 0..3 {
            self.total.differences[channel] += i128::from(self.batch.differences[channel]);
            self.total.currents[channel] += i128::from(self.batch.currents[channel]);
        }
        self.total.weights += i128::from(self.batch.weights);
        self.batch = FitSums::default();
        self.batch_pixels = 0;
    }

    /// The fitted colour, each channel rounded to the nearest integer and
    /// clamped to `0..=255`; the default colour when nothing had weight.
    fn color(mut self, alpha: i32) -> Color {
        self.flush();
        let total = self.total;
        let weights = total.weights;
        if weights == 0 {
            return Color::default();
        }
        let channel = |channel: usize| -> u8 {
            let numerator = i128::from(M) * total.differences[channel] + total.currents[channel];
            if numerator <= 0 {
                return 0;
            }
            // Round to nearest. `i128` division is a library call, so take
            // the exact `i64` path whenever both operands fit, as they do for
            // any shape below about `2^21` pixels.
            let (numerator, denominator) = (2 * numerator + weights, 2 * weights);
            let quotient = match (i64::try_from(numerator), i64::try_from(denominator)) {
                (Ok(numerator), Ok(denominator)) => i128::from(numerator / denominator),
                _ => numerator / denominator,
            };
            quotient.min(255) as u8
        };
        Color::new(channel(0), channel(1), channel(2), alpha as u8)
    }
}

/// Lines shorter than this, the one-pixel anti-aliased edge runs that make
/// up most lines of the polygon, rotated-ellipse and quadratic fills, are
/// summed from their pixels, which the energy reads next anyway; longer
/// lines read the step's [`PrefixSums`]. Measured with the Divan
/// `compute_color` and `energy_from_lines_raw` benches: looking up one-pixel
/// lines is slower, and summing two-pixel lines directly gains nothing.
const SHORT_LINE_PIXELS: usize = 2;

/// The per-scanline kernels behind [`fit`] and the energies, in
/// scalar code ([`Scalar`]) or NEON ([`Neon`]).
///
/// # Safety
///
/// For every method, `start + pixels * 3` must not exceed `t_pix.len()` or
/// `c_pix.len()`.
trait LineKernels {
    /// Per-channel RGB sums of `pixels` pixels from byte offset `start` in
    /// the target and in the canvas.
    unsafe fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]);

    /// The sum of squared channel differences between the target and the
    /// canvas over `pixels` pixels from byte offset `start`.
    unsafe fn line_error(t_pix: &[u8], c_pix: &[u8], start: usize, pixels: usize) -> u64;

    /// The same sum after blending premultiplied `source` channels onto the
    /// canvas with coverage `ma` and canvas factor `a`, as [`draw_lines`]
    /// would.
    unsafe fn line_after(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
        source: [u32; 3],
        ma: u32,
        a: u32,
    ) -> u64;
}

/// The kernels for this target: NEON on aarch64, scalar elsewhere.
#[cfg(target_arch = "aarch64")]
type Native = Neon;
#[cfg(not(target_arch = "aarch64"))]
type Native = Scalar;

/// Asserts that `sums` were built for buffers of `width` x `height`.
#[inline]
fn assert_sums_match(sums: Option<&PrefixSums>, width: u32, height: u32) {
    if let Some(sums) = sums {
        assert!(
            sums.matches(width, height),
            "prefix sums must match the buffers"
        );
    }
}

/// A shape's fitted colour and the old error of the pixels it covers,
/// which its energy subtracts; see [`fit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    pub(crate) color: Color,
    /// The squared channel differences between the target and the canvas
    /// over every in-bounds pixel of every line, a pixel counted once per
    /// line that covers it.
    pub(crate) before: u64,
}

/// [`fit`] with the kernels `K`.
fn fit_with<K: LineKernels>(
    target: &Buffer,
    current: &Buffer,
    sums: Option<&PrefixSums>,
    lines: &[Scanline],
    alpha: i32,
) -> Fit {
    assert_same_dimensions(target, current);
    assert_sums_match(sums, target.width(), target.height());
    debug_assert!((1..=255).contains(&alpha), "alpha must be 1..=255");
    let w = target.width() as i32;
    let h = target.height() as i32;
    let t_pix = target.pixels();
    let c_pix = current.pixels();
    let mut fit = ColorFit::new();
    let mut before = 0_u64;

    for line in lines {
        let Some((x1, x2)) = clamp_line(line, w, h) else {
            continue;
        };
        let k = ColorFit::weight(alpha, line.alpha);
        let pixels = (x2 - x1 + 1) as usize;
        let start = target.pix_offset(x1, line.y);
        // `clamp_line` keeps `line.y` in `0..h` and `x1..=x2` in `0..w` of
        // `target`, so `start + pixels * 3` is at most `w * h * 3`, the
        // length of `target.pixels()` (the `Buffer` length invariant,
        // asserted by every constructor). `current` has the same dimensions
        // (asserted above), so the same length: every kernel call below
        // meets its contract.
        let span = if pixels < SHORT_LINE_PIXELS {
            // SAFETY: see above.
            Some(unsafe { scalar::span_sums(t_pix, c_pix, start, pixels) })
        } else {
            match sums {
                Some(sums) if pixels <= MAX_CHANNEL_SPAN => Some(sums.span(line.y, x1, x2)),
                _ => None,
            }
        };
        match span {
            Some(span) => {
                before = before.wrapping_add(span.error);
                if k != 0 {
                    fit.add_line(k, pixels, span.target, span.current);
                }
            }
            None => {
                let error = match sums {
                    Some(sums) => sums.error(line.y, x1, x2),
                    // SAFETY: see above.
                    None => unsafe { K::line_error(t_pix, c_pix, start, pixels) },
                };
                before = before.wrapping_add(error);
                if k != 0 {
                    // SAFETY: see above.
                    let (target_sums, current_sums) =
                        unsafe { K::line_sums(t_pix, c_pix, start, pixels) };
                    fit.add_line(k, pixels, target_sums, current_sums);
                }
            }
        }
    }

    Fit {
        color: fit.color(alpha),
        before,
    }
}

/// The energy of drawing `lines` in `fit.color`: `score` minus
/// `fit.before` plus the new error of every covered pixel, in the wrapping
/// arithmetic of [`energy`].
///
/// With a `limit`, stops and returns `None` as soon as the running total
/// reaches it. Every old error is subtracted first and the new errors are
/// non-negative, so from then on the total only grows, and stopping means
/// the energy is at least `limit`. That holds while no partial total
/// overflows `i64`, which needs a canvas of about `2^43` pixels.
fn energy_with<K: LineKernels>(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    fit: Fit,
    score: u64,
    limit: Option<i64>,
) -> Option<u64> {
    assert_same_dimensions(target, current);
    let [sr, sg, sb, sa] = fit.color.to_premultiplied_rgba();
    let w = target.width() as i32;
    let h = target.height() as i32;
    let t_pix = target.pixels();
    let c_pix = current.pixels();
    let reached = |total: i64| limit.is_some_and(|limit| total >= limit);

    let mut total = (score as i64).wrapping_sub(fit.before as i64);
    if reached(total) {
        return None;
    }
    for line in lines {
        let Some((x1, x2)) = clamp_line(line, w, h) else {
            continue;
        };
        let ma = line.alpha;
        let a = (M - sa * ma / M) * 0x101;
        let pixels = (x2 - x1 + 1) as usize;
        let start = target.pix_offset(x1, line.y);
        // SAFETY: as in `fit_with`, the clamped line lies inside `target`,
        // and `current` has the same dimensions.
        let after = unsafe { K::line_after(t_pix, c_pix, start, pixels, [sr, sg, sb], ma, a) };
        total = total.wrapping_add(after as i64);
        if reached(total) {
            return None;
        }
    }

    Some(total as u64)
}

/// The scalar kernels, for targets without NEON and as the reference the
/// NEON kernels are tested against.
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
struct Scalar;

impl LineKernels for Scalar {
    #[inline]
    unsafe fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        scalar::line_sums(t_pix, c_pix, start, pixels)
    }

    #[inline]
    unsafe fn line_error(t_pix: &[u8], c_pix: &[u8], start: usize, pixels: usize) -> u64 {
        let end = start + pixels * BYTES_PER_PIXEL;
        scalar::difference_full_raw_pixels(&t_pix[start..end], &c_pix[start..end])
    }

    #[inline]
    unsafe fn line_after(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
        source: [u32; 3],
        ma: u32,
        a: u32,
    ) -> u64 {
        scalar::line_after(t_pix, c_pix, start, pixels, source, ma, a)
    }
}

/// The NEON kernels.
#[cfg(target_arch = "aarch64")]
struct Neon;

#[cfg(target_arch = "aarch64")]
impl LineKernels for Neon {
    #[inline]
    unsafe fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        // SAFETY: this method's contract is `neon::line_sums`'s.
        unsafe { neon::line_sums(t_pix, c_pix, start, pixels) }
    }

    #[inline]
    unsafe fn line_error(t_pix: &[u8], c_pix: &[u8], start: usize, pixels: usize) -> u64 {
        let end = start + pixels * BYTES_PER_PIXEL;
        neon::difference_bytes(&t_pix[start..end], &c_pix[start..end])
    }

    #[inline]
    unsafe fn line_after(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
        source: [u32; 3],
        ma: u32,
        a: u32,
    ) -> u64 {
        // SAFETY: this method's contract is `neon::line_after`'s.
        unsafe { neon::line_after(t_pix, c_pix, start, pixels, source, ma, a) }
    }
}

mod scalar {
    use super::{BYTES_PER_PIXEL, Buffer, SpanSums, blend_channel_scalar};

    /// The sums of [`SpanSums`] over `pixels` pixels from byte offset
    /// `start`, in one pass over the pixels; for the short lines that are
    /// cheaper to read than to look up.
    ///
    /// # Safety
    ///
    /// `start + pixels * 3` must not exceed `t_pix.len()` or `c_pix.len()`.
    #[inline]
    pub(super) unsafe fn span_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> SpanSums {
        let mut span = SpanSums {
            target: [0; 3],
            current: [0; 3],
            error: 0,
        };
        let mut byte = start;
        for _ in 0..pixels {
            let mut error = 0_u32;
            for channel in 0..BYTES_PER_PIXEL {
                // SAFETY: `byte + channel` is below `start + pixels * 3`,
                // within both slices by the caller's contract.
                let (t, c) = unsafe {
                    (
                        *t_pix.get_unchecked(byte + channel),
                        *c_pix.get_unchecked(byte + channel),
                    )
                };
                span.target[channel] += u64::from(t);
                span.current[channel] += u64::from(c);
                let d = u32::from(t.abs_diff(c));
                error += d * d;
            }
            span.error += u64::from(error);
            byte += BYTES_PER_PIXEL;
        }
        span
    }

    /// Per-channel RGB sums of `pixels` pixels from byte offset `start` in
    /// the target and in the canvas.
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    #[inline]
    pub(super) fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        let end = start + pixels * BYTES_PER_PIXEL;
        let (t_px3, _) = t_pix[start..end].as_chunks::<3>();
        let (c_px3, _) = c_pix[start..end].as_chunks::<3>();
        let mut target_sums = [0_u64; 3];
        let mut current_sums = [0_u64; 3];
        for (t_px, c_px) in t_px3.iter().zip(c_px3) {
            for channel in 0..3 {
                target_sums[channel] += u64::from(t_px[channel]);
                current_sums[channel] += u64::from(c_px[channel]);
            }
        }
        (target_sums, current_sums)
    }

    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    pub(super) fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
        difference_full_raw_pixels(a.pixels(), b.pixels())
    }

    /// Sum of squared differences of every byte: every byte of an RGB
    /// buffer is a colour channel, so the pixel boundaries do not matter.
    pub(super) fn difference_full_raw_pixels(a_pix: &[u8], b_pix: &[u8]) -> u64 {
        a_pix
            .iter()
            .zip(b_pix)
            .map(|(&a, &b)| {
                let d = i32::from(a) - i32::from(b);
                (d * d) as u64
            })
            .sum()
    }

    /// The squared channel differences between the target and the canvas
    /// blended with `source`, over `pixels` pixels from byte offset `start`.
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    #[inline]
    pub(super) fn line_after(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
        source: [u32; 3],
        ma: u32,
        a: u32,
    ) -> u64 {
        let end = start + pixels * BYTES_PER_PIXEL;
        let (t_px3, _) = t_pix[start..end].as_chunks::<3>();
        let (c_px3, _) = c_pix[start..end].as_chunks::<3>();
        let mut total = 0_u64;
        for (t_px, c_px) in t_px3.iter().zip(c_px3) {
            let mut after = 0_u32;
            for channel in 0..3 {
                let blended = blend_channel_scalar(c_px[channel], source[channel], ma, a);
                let d = u32::from(t_px[channel].abs_diff(blended));
                after += d * d;
            }
            total += u64::from(after);
        }
        total
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::{BYTES_PER_PIXEL, Buffer, blend_channel_scalar, scalar};
    use std::arch::aarch64::*;

    /// Bytes in a chunk of 8 RGB pixels, the unit of every `vld3_u8`.
    const CHUNK_BYTES: usize = 8 * BYTES_PER_PIXEL;

    /// # Safety
    ///
    /// Requires NEON, which every aarch64 target provides.
    unsafe fn div_by_m_u32x4(value: uint32x4_t) -> uint32x4_t {
        // SAFETY: register-only NEON intrinsics with no memory access. NEON is a
        // baseline feature of every aarch64 target and this module only compiles
        // for aarch64, so the required target feature is always available.
        unsafe {
            let plus_one = vdupq_n_u32(1);
            let adjusted = vaddq_u32(vaddq_u32(value, plus_one), vshrq_n_u32(value, 16));
            vshrq_n_u32(adjusted, 16)
        }
    }

    /// # Safety
    ///
    /// Requires NEON, which every aarch64 target provides.
    unsafe fn blend_vector_u8x8(current: uint8x8_t, source: u32, ma: u32, a: u32) -> uint8x8_t {
        // SAFETY: register-only NEON intrinsics with no memory access. NEON is a
        // baseline feature of every aarch64 target and this module only compiles
        // for aarch64, so the required target feature is always available.
        unsafe {
            let current16 = vmovl_u8(current);
            let source_term = vdupq_n_u32(source * ma);

            let current_low = vmovl_u16(vget_low_u16(current16));
            let current_high = vmovl_u16(vget_high_u16(current16));

            let blended_low = div_by_m_u32x4(vaddq_u32(vmulq_n_u32(current_low, a), source_term));
            let blended_high = div_by_m_u32x4(vaddq_u32(vmulq_n_u32(current_high, a), source_term));

            let blended16 = vcombine_u16(
                vmovn_u32(vshrq_n_u32(blended_low, 8)),
                vmovn_u32(vshrq_n_u32(blended_high, 8)),
            );
            vmovn_u16(blended16)
        }
    }

    /// # Safety
    ///
    /// Requires NEON, which every aarch64 target provides.
    #[cfg(test)]
    pub(super) unsafe fn blend_chunk_u8x8(
        current: [u8; 8],
        source: u32,
        ma: u32,
        a: u32,
    ) -> [u8; 8] {
        let mut out = [0_u8; 8];
        // SAFETY: `vld1_u8` reads and `vst1_u8` writes exactly 8 bytes, and both
        // pointers come from local `[u8; 8]` arrays. The remaining calls are
        // register-only NEON operations; NEON is a baseline aarch64 feature.
        unsafe {
            let current_vec = vld1_u8(current.as_ptr());
            let blended = blend_vector_u8x8(current_vec, source, ma, a);
            vst1_u8(out.as_mut_ptr(), blended);
        }
        out
    }

    /// # Safety
    ///
    /// Requires NEON, which every aarch64 target provides.
    unsafe fn sum_squared_diff_u8x8(lhs: uint8x8_t, rhs: uint8x8_t) -> u64 {
        // SAFETY: register-only NEON intrinsics with no memory access. NEON is a
        // baseline feature of every aarch64 target and this module only compiles
        // for aarch64, so the required target feature is always available.
        unsafe {
            let lhs16 = vreinterpretq_s16_u16(vmovl_u8(lhs));
            let rhs16 = vreinterpretq_s16_u16(vmovl_u8(rhs));
            let diff = vsubq_s16(lhs16, rhs16);
            let low = vmull_s16(vget_low_s16(diff), vget_low_s16(diff));
            let high = vmull_s16(vget_high_s16(diff), vget_high_s16(diff));
            u64::from(vaddvq_u32(vreinterpretq_u32_s32(low)))
                + u64::from(vaddvq_u32(vreinterpretq_u32_s32(high)))
        }
    }

    /// All ones in the first `pixels` of 8 lanes, zero in the others.
    ///
    /// # Safety
    ///
    /// Requires NEON, which every aarch64 target provides.
    #[inline]
    unsafe fn tail_mask(pixels: usize) -> uint8x8_t {
        debug_assert!(pixels < 8, "a tail is shorter than a chunk");
        const LANES: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
        // SAFETY: `vld1_u8` reads the 8 bytes of `LANES`; the other calls are
        // register-only. NEON is a baseline aarch64 feature.
        unsafe { vclt_u8(vld1_u8(LANES.as_ptr()), vdup_n_u8(pixels as u8)) }
    }

    /// Blocks of 16 pixels summed pairwise into 16-bit lanes before
    /// widening: each lane gains at most `2 * 255 = 510` per block, and
    /// `510 * 128 = 65_280` fits in `u16`.
    const BLOCKS_PER_FLUSH: usize = 128;

    /// Per-channel RGB sums of `pixels` pixels from byte offset `start` in
    /// the target and in the canvas; matches [`scalar::line_sums`].
    ///
    /// # Safety
    ///
    /// `start + pixels * 3` must not exceed `t_pix.len()` or `c_pix.len()`.
    #[inline]
    pub(super) unsafe fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        let mut target_sums = [0_u64; 3];
        let mut current_sums = [0_u64; 3];
        let mut byte_index = start;
        let mut blocks = pixels / 16;

        while blocks > 0 {
            let flush = blocks.min(BLOCKS_PER_FLUSH);
            blocks -= flush;
            // SAFETY: each `vld3q_u8` reads 16 pixels (48 bytes) from
            // `byte_index`. The loops read `pixels / 16` blocks in total from
            // `start`, so the last byte read is below `start + pixels * 3`,
            // which the caller keeps within both slices. The remaining calls
            // are register-only NEON operations; NEON is a baseline aarch64
            // feature.
            unsafe {
                let mut target_acc = [vdupq_n_u16(0); 3];
                let mut current_acc = [vdupq_n_u16(0); 3];
                for _ in 0..flush {
                    let t = vld3q_u8(t_pix.as_ptr().add(byte_index));
                    let c = vld3q_u8(c_pix.as_ptr().add(byte_index));
                    target_acc[0] = vpadalq_u8(target_acc[0], t.0);
                    target_acc[1] = vpadalq_u8(target_acc[1], t.1);
                    target_acc[2] = vpadalq_u8(target_acc[2], t.2);
                    current_acc[0] = vpadalq_u8(current_acc[0], c.0);
                    current_acc[1] = vpadalq_u8(current_acc[1], c.1);
                    current_acc[2] = vpadalq_u8(current_acc[2], c.2);
                    byte_index += 2 * CHUNK_BYTES;
                }
                for channel in 0..3 {
                    target_sums[channel] += u64::from(vaddlvq_u16(target_acc[channel]));
                    current_sums[channel] += u64::from(vaddlvq_u16(current_acc[channel]));
                }
            }
        }

        if pixels % 16 >= 8 {
            // SAFETY: `vld3_u8` reads the next 8 pixels (24 bytes), which
            // are within the line, below `start + pixels * 3`. The remaining
            // calls are register-only NEON operations.
            unsafe {
                let t = vld3_u8(t_pix.as_ptr().add(byte_index));
                let c = vld3_u8(c_pix.as_ptr().add(byte_index));
                for (channel, (t, c)) in
                    [(t.0, c.0), (t.1, c.1), (t.2, c.2)].into_iter().enumerate()
                {
                    target_sums[channel] += u64::from(vaddlv_u8(t));
                    current_sums[channel] += u64::from(vaddlv_u8(c));
                }
            }
            byte_index += CHUNK_BYTES;
        }

        // Three or more tail pixels take one 8-pixel load with the lanes past
        // the line masked out, when the buffers extend that far; shorter
        // tails, common at anti-aliased edges, are cheaper in scalar code.
        let tail = pixels % 8;
        if tail > 2 && byte_index + CHUNK_BYTES <= t_pix.len().min(c_pix.len()) {
            // SAFETY: `vld3_u8` reads 24 bytes from `byte_index`, within both
            // slices by the check above. The remaining calls are
            // register-only NEON operations.
            unsafe {
                let mask = tail_mask(tail);
                let t = vld3_u8(t_pix.as_ptr().add(byte_index));
                let c = vld3_u8(c_pix.as_ptr().add(byte_index));
                for (channel, (t, c)) in
                    [(t.0, c.0), (t.1, c.1), (t.2, c.2)].into_iter().enumerate()
                {
                    target_sums[channel] += u64::from(vaddlv_u8(vand_u8(t, mask)));
                    current_sums[channel] += u64::from(vaddlv_u8(vand_u8(c, mask)));
                }
            }
        } else {
            // SAFETY: the tail reads the last `tail` pixels of the line, which
            // end at `start + pixels * 3`, within both slices by the
            // caller's contract.
            unsafe {
                let (t_ptr, c_ptr) = (t_pix.as_ptr(), c_pix.as_ptr());
                for _ in 0..tail {
                    for channel in 0..3 {
                        target_sums[channel] += u64::from(*t_ptr.add(byte_index + channel));
                        current_sums[channel] += u64::from(*c_ptr.add(byte_index + channel));
                    }
                    byte_index += BYTES_PER_PIXEL;
                }
            }
        }
        (target_sums, current_sums)
    }

    /// Chunks of 16 bytes whose squared differences are summed in 32-bit
    /// lanes before widening: each chunk adds at most `4 * 255²` to a lane,
    /// and `4096 * 4 * 65_025 < 2^32`.
    const SQUARES_PER_BLOCK: usize = 4096;

    /// Sum of squared differences of every byte; matches
    /// [`scalar::difference_full_raw`]. Every byte of an RGB buffer is a
    /// colour channel, so plain 16-byte loads need no deinterleaving.
    pub(super) fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
        difference_bytes(a.pixels(), b.pixels())
    }

    /// Sum of squared differences of every pair of bytes; matches
    /// [`scalar::difference_full_raw_pixels`].
    ///
    /// # Panics
    ///
    /// Panics if the slices have different lengths.
    pub(super) fn difference_bytes(a_pix: &[u8], b_pix: &[u8]) -> u64 {
        assert_eq!(a_pix.len(), b_pix.len(), "byte slices of different lengths");
        let mut chunks = a_pix.len() / 16;
        let chunk_bytes = chunks * 16;
        let mut total = 0_u64;
        let mut byte_index = 0_usize;

        while chunks > 0 {
            let block = chunks.min(SQUARES_PER_BLOCK);
            chunks -= block;
            // SAFETY: each `vld1q_u8` reads 16 bytes from `byte_index`, and
            // the loops read `a_pix.len() / 16` chunks in total from 0, so
            // every read ends at or below `chunk_bytes <= a_pix.len()`, and
            // `b_pix` has the same length (asserted above). The remaining
            // calls are register-only NEON operations; NEON is a baseline
            // aarch64 feature.
            unsafe {
                let mut acc = vdupq_n_u32(0);
                for _ in 0..block {
                    let lhs = vld1q_u8(a_pix.as_ptr().add(byte_index));
                    let rhs = vld1q_u8(b_pix.as_ptr().add(byte_index));
                    let diff = vabdq_u8(lhs, rhs);
                    let low = vmull_u8(vget_low_u8(diff), vget_low_u8(diff));
                    let high = vmull_high_u8(diff, diff);
                    acc = vpadalq_u16(acc, low);
                    acc = vpadalq_u16(acc, high);
                    byte_index += 16;
                }
                total += vaddlvq_u32(acc);
            }
        }

        total + scalar::difference_full_raw_pixels(&a_pix[chunk_bytes..], &b_pix[chunk_bytes..])
    }

    /// The squared channel differences between the target and the canvas
    /// blended with `source`; matches [`scalar::line_after`].
    ///
    /// # Safety
    ///
    /// `start + pixels * 3` must not exceed `t_pix.len()` or `c_pix.len()`.
    #[inline]
    pub(super) unsafe fn line_after(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
        source: [u32; 3],
        ma: u32,
        a: u32,
    ) -> u64 {
        let [sr, sg, sb] = source;
        let mut total = 0_u64;
        let mut byte_index = start;

        for _ in 0..pixels / 8 {
            // SAFETY: each `vld3_u8` reads 8 pixels (24 bytes) from
            // `byte_index`; the loop reads `pixels / 8` chunks from `start`,
            // which end at or below `start + pixels * 3`, within both slices
            // by the caller's contract. The remaining calls are
            // register-only NEON operations; NEON is a baseline aarch64
            // feature.
            unsafe {
                let t = vld3_u8(t_pix.as_ptr().add(byte_index));
                let c = vld3_u8(c_pix.as_ptr().add(byte_index));
                total += sum_squared_diff_u8x8(t.0, blend_vector_u8x8(c.0, sr, ma, a));
                total += sum_squared_diff_u8x8(t.1, blend_vector_u8x8(c.1, sg, ma, a));
                total += sum_squared_diff_u8x8(t.2, blend_vector_u8x8(c.2, sb, ma, a));
            }
            byte_index += CHUNK_BYTES;
        }

        // As in `line_sums`: a masked 8-pixel chunk for three or more tail
        // pixels when the buffers extend that far, scalar otherwise.
        let tail = pixels % 8;
        if tail > 2 && byte_index + CHUNK_BYTES <= t_pix.len().min(c_pix.len()) {
            // SAFETY: `vld3_u8` reads 24 bytes from `byte_index`, within
            // both slices by the check above. Lanes past the line take the
            // target's value, so they add nothing. The remaining calls are
            // register-only NEON operations.
            unsafe {
                let mask = tail_mask(tail);
                let t = vld3_u8(t_pix.as_ptr().add(byte_index));
                let c = vld3_u8(c_pix.as_ptr().add(byte_index));
                let after_r = vbsl_u8(mask, blend_vector_u8x8(c.0, sr, ma, a), t.0);
                let after_g = vbsl_u8(mask, blend_vector_u8x8(c.1, sg, ma, a), t.1);
                let after_b = vbsl_u8(mask, blend_vector_u8x8(c.2, sb, ma, a), t.2);
                total += sum_squared_diff_u8x8(t.0, after_r);
                total += sum_squared_diff_u8x8(t.1, after_g);
                total += sum_squared_diff_u8x8(t.2, after_b);
            }
        } else {
            for _ in 0..tail {
                for (channel, &source) in source.iter().enumerate() {
                    // SAFETY: the tail reads the last `tail` pixels of the
                    // line, which end at `start + pixels * 3`, within both
                    // slices by the caller's contract.
                    let (t, c) = unsafe {
                        (
                            *t_pix.as_ptr().add(byte_index + channel),
                            *c_pix.as_ptr().add(byte_index + channel),
                        )
                    };
                    let d = u32::from(t.abs_diff(blend_channel_scalar(c, source, ma, a)));
                    total += u64::from(d * d);
                }
                byte_index += BYTES_PER_PIXEL;
            }
        }

        total
    }
}

/// Computes the optimal color for drawing `lines` onto `current` to
/// best approximate `target` at the given `alpha` level, which must be
/// `1..=255`; [`fit`] without prefix sums.
///
/// # Panics
///
/// Panics if `current` and `target` have different dimensions.
#[cfg(test)]
#[must_use]
pub(crate) fn compute_color(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    alpha: i32,
) -> Color {
    fit(target, current, None, lines, alpha).color
}

/// Fits the colour for drawing `lines` onto `current` to best approximate
/// `target` at the given `alpha` level, which must be `1..=255`, and sums
/// the old error of the covered pixels for [`energy`].
///
/// Each pixel is weighted by its scanline's coverage, so anti-aliased edges
/// are fitted as partly covered (see [`ColorFit`]); Go's `primitive` treats
/// them as fully covered, which under-saturates the colour. The colour is
/// a zero [`Color`] if no in-bounds scanline pixel has coverage.
///
/// `sums`, the prefix sums of `target` against `current`, turn each line
/// into two lookups; without them every pixel is read.
///
/// # Panics
///
/// Panics if `current`, `target` and `sums` have different dimensions.
#[must_use]
pub(crate) fn fit(
    target: &Buffer,
    current: &Buffer,
    sums: Option<&PrefixSums>,
    lines: &[Scanline],
    alpha: i32,
) -> Fit {
    fit_with::<Native>(target, current, sums, lines, alpha)
}

/// Asserts the precondition of the NEON scanline kernels: `current` has
/// `target`'s width and height, so (with the `Buffer` length invariant) any
/// offset inside `target` is inside `current` too.
#[inline]
fn assert_same_dimensions(target: &Buffer, current: &Buffer) {
    assert!(
        target.width() == current.width() && target.height() == current.height(),
        "current must have target's dimensions: target {}x{}, current {}x{}",
        target.width(),
        target.height(),
        current.width(),
        current.height(),
    );
}

#[cfg(test)]
pub(crate) fn copy_and_draw_lines(dst: &mut Buffer, src: &Buffer, c: Color, lines: &[Scanline]) {
    let [sr, sg, sb, sa] = c.to_premultiplied_rgba();
    let w = dst.width() as i32;
    let h = dst.height() as i32;

    // Split borrows: we need immutable access to src and mutable to dst.
    let src_pix = src.pixels();
    let dst_pix = dst.pixels_mut();

    for line in lines {
        let (x1, x2) = match clamp_line(line, w, h) {
            Some(v) => v,
            None => continue,
        };
        let ma = line.alpha;
        let a = (M - sa * ma / M) * 0x101;
        let mut i = (line.y as usize * w as usize + x1 as usize) * BYTES_PER_PIXEL;
        for _ in x1..=x2 {
            dst_pix[i] = blend_channel_scalar(src_pix[i], sr, ma, a);
            dst_pix[i + 1] = blend_channel_scalar(src_pix[i + 1], sg, ma, a);
            dst_pix[i + 2] = blend_channel_scalar(src_pix[i + 2], sb, ma, a);
            i += BYTES_PER_PIXEL;
        }
    }
}

/// Blends color `c` onto the existing pixels of `im` along the given scanlines.
pub(crate) fn draw_lines(im: &mut Buffer, c: Color, lines: &[Scanline]) {
    let [sr, sg, sb, sa] = c.to_premultiplied_rgba();
    let w = im.width() as i32;
    let h = im.height() as i32;
    let pix = im.pixels_mut();

    for line in lines {
        let (x1, x2) = match clamp_line(line, w, h) {
            Some(v) => v,
            None => continue,
        };
        let ma = line.alpha;
        let a = (M - sa * ma / M) * 0x101;
        let mut i = (line.y as usize * w as usize + x1 as usize) * BYTES_PER_PIXEL;
        for _ in x1..=x2 {
            pix[i] = blend_channel_scalar(pix[i], sr, ma, a);
            pix[i + 1] = blend_channel_scalar(pix[i + 1], sg, ma, a);
            pix[i + 2] = blend_channel_scalar(pix[i + 2], sb, ma, a);
            i += BYTES_PER_PIXEL;
        }
    }
}

/// Computes the sum of squared channel differences between two buffers,
/// the raw form of the score; [`raw_score_to_normalized`] turns it into
/// the RGB RMSE in `[0, 1]`.
///
/// # Panics
///
/// Panics if the two buffers have different dimensions.
#[must_use]
pub(crate) fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
    assert_eq!(a.width(), b.width(), "difference_full: width mismatch");
    assert_eq!(a.height(), b.height(), "difference_full: height mismatch");

    #[cfg(target_arch = "aarch64")]
    {
        neon::difference_full_raw(a, b)
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        scalar::difference_full_raw(a, b)
    }
}

/// Computes the normalized root-mean-square difference between two buffers.
#[cfg(test)]
#[must_use]
pub(crate) fn difference_full(a: &Buffer, b: &Buffer) -> f64 {
    raw_score_to_normalized(difference_full_raw(a, b), a.width(), a.height())
}

#[cfg(test)]
#[must_use]
pub(crate) fn difference_partial_raw(
    target: &Buffer,
    before: &Buffer,
    after: &Buffer,
    score: u64,
    lines: &[Scanline],
) -> u64 {
    let w = target.width() as i32;
    let h = target.height() as i32;
    let mut total = score;

    let t_pix = target.pixels();
    let b_pix = before.pixels();
    let a_pix = after.pixels();

    for line in lines {
        let (x1, x2) = match clamp_line(line, w, h) {
            Some(v) => v,
            None => continue,
        };
        let start = target.pix_offset(x1, line.y);
        let end = target.pix_offset(x2 + 1, line.y);
        for i in start..end {
            let t = i32::from(t_pix[i]);
            let d1 = t - i32::from(b_pix[i]);
            let d2 = t - i32::from(a_pix[i]);
            total = total.wrapping_sub((d1 * d1) as u64);
            total = total.wrapping_add((d2 * d2) as u64);
        }
    }

    total
}

#[cfg(test)]
#[must_use]
pub(crate) fn difference_partial(
    target: &Buffer,
    before: &Buffer,
    after: &Buffer,
    score: f64,
    lines: &[Scanline],
) -> f64 {
    let raw = normalized_to_raw_score(score, target.width(), target.height());
    let total = difference_partial_raw(target, before, after, raw, lines);
    raw_score_to_normalized(total, target.width(), target.height())
}

/// The raw score after drawing `lines` with `fit`, which [`fit`] computed
/// for the same buffers and lines.
///
/// Fused replacement for `copy_and_draw_lines` + `difference_partial`:
/// `score` minus the old error of the covered pixels, from `fit.before`,
/// plus their error after blending, computed on the fly (no write to any
/// intermediate buffer).
///
/// The running total uses wrapping arithmetic: a scanline set that covers a
/// pixel twice (the quadratic stroke, ENG-2) subtracts that pixel's old
/// difference twice and can dip below zero part-way through a small canvas.
/// Wrapping keeps debug builds from panicking and matches release builds.
///
/// # Panics
///
/// Panics if `current` and `target` have different dimensions.
#[must_use]
pub(crate) fn energy(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    fit: Fit,
    score: u64,
) -> u64 {
    energy_with::<Native>(target, current, lines, fit, score, None)
        .expect("an evaluation without a limit runs to the end")
}

/// The [`energy`] if it is below `bound`, `None` otherwise.
///
/// The search only keeps a candidate whose energy is below the best so far,
/// so it passes that energy as the bound, and the evaluation stops as soon
/// as the candidate cannot beat it: once every old error is subtracted, the
/// running total only grows.
///
/// # Panics
///
/// Panics if `current` and `target` have different dimensions.
#[must_use]
pub(crate) fn energy_below(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    fit: Fit,
    score: u64,
    bound: u64,
) -> Option<u64> {
    // A bound beyond `i64::MAX` is never reached by a total that fits in
    // `i64`, so the evaluation runs to the end and the check below decides.
    let limit = i64::try_from(bound).ok();
    energy_with::<Native>(target, current, lines, fit, score, limit)
        .filter(|&energy| energy < bound)
}

/// [`energy`] for `color` instead of a fit: the old error comes from the
/// pixels.
#[cfg(test)]
#[must_use]
pub(crate) fn energy_from_lines_raw(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    color: Color,
    score: u64,
) -> u64 {
    let before = fit(target, current, None, lines, 255).before;
    energy(target, current, lines, Fit { color, before }, score)
}

#[cfg(test)]
#[must_use]
pub(crate) fn energy_from_lines(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    color: Color,
    score: f64,
) -> f64 {
    let raw = normalized_to_raw_score(score, target.width(), target.height());
    let total = energy_from_lines_raw(target, current, lines, color, raw);
    raw_score_to_normalized(total, target.width(), target.height())
}

#[cfg(test)]
mod fixtures {
    use super::M;
    use crate::buffer::Buffer;
    use crate::scanline::Scanline;
    use rand::RngExt;
    use rand_chacha::ChaCha8Rng;

    pub(super) fn random_buffer(rng: &mut ChaCha8Rng, width: u32, height: u32) -> Buffer {
        let mut buffer = Buffer::new(width, height);
        rng.fill(buffer.pixels_mut());
        buffer
    }

    /// Scanlines of every kind the kernels must handle: full rows touching
    /// both edges, lines sticking out on either side or off the canvas,
    /// lengths around the 8-pixel chunk size, and every coverage extreme.
    pub(super) fn random_lines(rng: &mut ChaCha8Rng, width: i32, height: i32) -> Vec<Scanline> {
        let alpha = |rng: &mut ChaCha8Rng| match rng.random_range(0..4) {
            0 => 0,
            1 => M,
            _ => rng.random_range(0..=M),
        };
        let mut lines = Vec::new();
        for y in -1..=height {
            lines.push(Scanline {
                y,
                x1: -rng.random_range(0..3),
                x2: width - 1 + rng.random_range(0..3),
                alpha: alpha(rng),
            });
        }
        for _ in 0..64 {
            let x1 = rng.random_range(-10..width + 2);
            let len = rng.random_range(0..2 * width + 20);
            lines.push(Scanline {
                y: rng.random_range(-1..=height),
                x1,
                x2: x1 + len - rng.random_range(0..2),
                alpha: alpha(rng),
            });
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::color::Color;
    use crate::scanline::Scanline;

    #[test]
    fn div_by_m_equivalence() {
        let cases = [
            0,
            1,
            2,
            0xFFFE,
            0xFFFF,
            0x1_0000,
            0x1_0001,
            0x00FF_0000,
            0x00FF_FFFE,
            0x00FF_FFFF,
            0x0100_0000,
            0x7FFF_0000,
            0xFFFD_FFFF,
            0xFFFE_0000,
            0xFFFE_0001,
        ];

        for value in cases {
            assert_eq!(div_by_m(value), value / 0xFFFF, "value={value}");
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn simd_blend_matches_scalar() {
        let mut current = [0_u8; 8];
        for (index, value) in current.iter_mut().enumerate() {
            *value = (index as u8).wrapping_mul(31).wrapping_add(7);
        }

        for source in 0_u32..=255 {
            let expanded = source | (source << 8);
            for alpha in [0_u32, 1, 127, 128, 192, 255] {
                let ma = alpha * 0x101;
                let a = (0xFFFF - ma) * 0x101;

                // SAFETY: only requires NEON, a baseline aarch64 feature; the
                // test is compiled for aarch64 only.
                let simd = unsafe { neon::blend_chunk_u8x8(current, expanded, ma, a) };
                for lane in 0..8 {
                    let scalar = blend_channel_scalar(current[lane], expanded, ma, a);
                    assert_eq!(simd[lane], scalar, "src={source} alpha={alpha} lane={lane}");
                }
            }
        }
    }

    #[test]
    fn difference_full_identical_is_zero() {
        let buf = Buffer::new(2, 2);
        assert_eq!(difference_full(&buf, &buf), 0.0);
    }

    #[test]
    fn difference_full_identical_nonzero_pixels() {
        let c = Color::new(100, 150, 200, 255);
        let buf = Buffer::new_from_color(4, 4, c);
        assert_eq!(difference_full(&buf, &buf), 0.0);
    }

    #[test]
    fn difference_full_known_value() {
        // 1x1 buffer: a = (255, 0, 0), b = (0, 0, 0)
        let mut a = Buffer::new(1, 1);
        let b = Buffer::new(1, 1);
        a.pixels_mut()[0] = 255;

        // dr=255, dg=0, db=0 => total = 255*255 = 65025
        // sqrt(65025 / 3) / 255 = 1 / sqrt(3)
        let diff = difference_full(&a, &b);
        assert!((diff - 3_f64.sqrt().recip()).abs() < 1e-9, "got {diff}");
    }

    #[test]
    fn difference_full_raw_matches_normalized() {
        let mut a = Buffer::new(2, 1);
        let mut b = Buffer::new(2, 1);

        a.pixels_mut().copy_from_slice(&[255, 32, 0, 10, 20, 30]);
        b.pixels_mut().copy_from_slice(&[0, 16, 64, 40, 50, 60]);

        let raw = difference_full_raw(&a, &b);
        let normalized = difference_full(&a, &b);
        let expected = (raw as f64 / (a.width() as f64 * a.height() as f64 * 3.0)).sqrt() / 255.0;

        assert_eq!(raw, 65025 + 256 + 4096 + 900 + 900 + 900);
        assert!((normalized - expected).abs() < 1e-12);
    }

    /// The normalised score is the RMSE over the three colour channels,
    /// computed here from scratch per pixel; black against white is `1.0`.
    #[test]
    fn normalized_score_is_the_rgb_rmse() {
        use rand::{RngExt, SeedableRng};
        use rand_chacha::ChaCha8Rng;

        let black = Buffer::new_from_color(5, 3, Color::new(0, 0, 0, 255));
        let white = Buffer::new_from_color(5, 3, Color::new(255, 255, 255, 255));
        assert_eq!(difference_full(&black, &white), 1.0);

        let mut rng = ChaCha8Rng::seed_from_u64(0x4b5e);
        for (width, height) in [(2, 2), (7, 5), (33, 9)] {
            let mut a = Buffer::new(width, height);
            let mut b = Buffer::new(width, height);
            rng.fill(a.pixels_mut());
            rng.fill(b.pixels_mut());
            let mut squares = 0.0;
            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    let i = a.pix_offset(x, y);
                    for channel in 0..3 {
                        let d =
                            f64::from(a.pixels()[i + channel]) - f64::from(b.pixels()[i + channel]);
                        squares += d * d;
                    }
                }
            }
            let expected = (squares / f64::from(width * height * 3)).sqrt() / 255.0;
            let actual = difference_full(&a, &b);
            assert!(
                (actual - expected).abs() < 1e-12,
                "{width}x{height}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn compute_color_empty_scanlines() {
        let target = Buffer::new(1, 1);
        let current = Buffer::new(1, 1);
        let got = compute_color(&target, &current, &[], 128);
        assert_eq!(got, Color::default());
    }

    #[test]
    fn compute_color_known_values() {
        // target pixel (0,0) = (255, 0, 0), current = (0, 0, 0)
        let mut target = Buffer::new(1, 1);
        target.pixels_mut()[0] = 255;

        let current = Buffer::new(1, 1);
        let lines = [Scanline {
            y: 0,
            x1: 0,
            x2: 0,
            alpha: 0xFFFF,
        }];

        let c = compute_color(&target, &current, &lines, 255);
        // With alpha=255: a = 0x101 * 255 / 255 = 0x101 = 257
        // rsum = (255 - 0) * 257 + 0 * 257 = 65535
        // count = 1
        // r = (65535 / 1) >> 8 = 65535 >> 8 = 255
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 0);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 255);
    }

    #[test]
    fn draw_lines_blends_correctly() {
        // Start with a 4x4 black buffer with opaque alpha
        let mut im = Buffer::new_from_color(4, 4, Color::new(0, 0, 0, 255));
        let c = Color::new(0, 255, 0, 128);
        let lines = [Scanline {
            y: 2,
            x1: 1,
            x2: 2,
            alpha: 0xFFFF,
        }];

        draw_lines(&mut im, c, &lines);

        // Verify the pixels at (1,2) and (2,2) have been blended
        let i1 = im.pix_offset(1, 2);
        let i2 = im.pix_offset(2, 2);

        // Green channel should be non-zero after blending green over black
        assert!(
            im.pixels()[i1 + 1] > 0,
            "green channel at (1,2) should be non-zero"
        );
        assert!(
            im.pixels()[i2 + 1] > 0,
            "green channel at (2,2) should be non-zero"
        );

        // Pixels outside scanline (e.g. (0,0) and (3,2)) should be unchanged
        assert_eq!(im.pixels()[..3], [0, 0, 0]);
        let i3 = im.pix_offset(3, 2);
        assert_eq!(im.pixels()[i3..i3 + 3], [0, 0, 0]);
    }

    #[test]
    fn copy_and_draw_lines_matches_draw_lines() {
        // Replicate the Go test: set up a source buffer, apply both methods,
        // verify they produce identical results.
        let mut src = Buffer::new_from_color(4, 4, Color::new(0, 0, 0, 0));

        // Set specific pixels at (1,2) and (2,2)
        let i1 = src.pix_offset(1, 2);
        let sp = src.pixels_mut();
        sp[i1] = 200;
        sp[i1 + 1] = 100;
        sp[i1 + 2] = 50;
        let i2 = (2 * 4 + 2) * 3; // pix_offset(2, 2) for w=4
        sp[i2] = 10;
        sp[i2 + 1] = 20;
        sp[i2 + 2] = 30;

        let c = Color::new(0, 255, 0, 128);
        let lines = [Scanline {
            y: 2,
            x1: 1,
            x2: 2,
            alpha: 0xFFFF,
        }];

        // Reference: draw_lines on a copy of src
        let mut reference = src.clone();
        draw_lines(&mut reference, c, &lines);

        // Got: copy_and_draw_lines into fresh buffer
        let mut got = Buffer::new(4, 4);
        copy_and_draw_lines(&mut got, &src, c, &lines);

        // Compare the scanline pixels
        for x in 1..=2 {
            let i = got.pix_offset(x, 2);
            assert_eq!(
                &got.pixels()[i..i + 3],
                &reference.pixels()[i..i + 3],
                "pixel ({x}, 2) mismatch"
            );
        }
    }

    #[test]
    fn difference_partial_matches_full_after_drawing() {
        // Port of the Go TestDifferencePartialMatchesFullAfterDrawingLines
        let mut target = Buffer::new(2, 2);
        let current = Buffer::new(2, 2);

        // target pixel (0,0) = bright red
        target.pixels_mut()[0] = 255;

        let before = current.clone();
        let lines = [Scanline {
            y: 0,
            x1: 0,
            x2: 0,
            alpha: 0xFFFF,
        }];

        let color = compute_color(&target, &current, &lines, 255);
        let mut after = current.clone();
        draw_lines(&mut after, color, &lines);

        let base_score = difference_full(&target, &before);
        let partial = difference_partial(&target, &before, &after, base_score, &lines);
        let full = difference_full(&target, &after);

        assert!(
            (partial - full).abs() < 1e-9,
            "partial={partial} full={full}"
        );
    }

    #[test]
    fn difference_partial_no_lines() {
        let target = Buffer::new(2, 2);
        let before = Buffer::new(2, 2);
        let after = Buffer::new(2, 2);
        let score = difference_full(&target, &before);
        let partial = difference_partial(&target, &before, &after, score, &[]);
        assert!((partial - score).abs() < 1e-9);
    }

    #[test]
    fn difference_partial_raw_matches_normalized() {
        let mut target = Buffer::new(2, 2);
        let mut before = Buffer::new(2, 2);
        let tp = target.pixels_mut();
        tp[..6].copy_from_slice(&[255, 0, 0, 20, 40, 60]);
        let bp = before.pixels_mut();
        bp[..6].copy_from_slice(&[0, 0, 0, 80, 60, 40]);

        let lines = [Scanline {
            y: 0,
            x1: 0,
            x2: 1,
            alpha: 0xFFFF,
        }];
        let color = compute_color(&target, &before, &lines, 192);
        let mut after = before.clone();
        draw_lines(&mut after, color, &lines);

        let base_raw = difference_full_raw(&target, &before);
        let partial_raw = difference_partial_raw(&target, &before, &after, base_raw, &lines);
        let partial = difference_partial(
            &target,
            &before,
            &after,
            difference_full(&target, &before),
            &lines,
        );
        let expected =
            (partial_raw as f64 / (target.width() as f64 * target.height() as f64 * 3.0)).sqrt()
                / 255.0;

        assert_eq!(partial_raw, difference_full_raw(&target, &after));
        assert!((partial - expected).abs() < 1e-12);
    }

    /// With full coverage every pixel has the same weight `w = alpha / 255`,
    /// so the fit is the plain least-squares colour
    /// `s = 255 · Σ(t − c) / (alpha · n) + Σc / n`, rounded and clamped.
    #[test]
    fn compute_color_full_coverage_is_the_unweighted_fit() {
        let mut target = Buffer::new(2, 1);
        target
            .pixels_mut()
            .copy_from_slice(&[200, 100, 50, 120, 180, 90]);
        let current = Buffer::new_from_color(2, 1, Color::new(40, 40, 40, 255));
        let lines = [full_row(0, 2)];

        // r: 255 · 240 / 384 + 40 = 199.375; g: 255 · 200 / 384 + 40 = 172.81;
        // b: 255 · 60 / 384 + 40 = 79.84.
        let c = compute_color(&target, &current, &lines, 192);
        assert_eq!(c, Color::new(199, 173, 80, 192));

        // r: 255 · 240 / 256 + 40 = 279.06 is clamped to 255; g: 255 · 200 /
        // 256 + 40 = 239.22; b: 255 · 60 / 256 + 40 = 99.77.
        let c = compute_color(&target, &current, &lines, 128);
        assert_eq!(c, Color::new(255, 239, 100, 128));
    }

    /// Batches that reach `BATCH_PIXELS` flush to the `i128` totals, and a
    /// line longer than a batch goes straight to them; either way the fit
    /// equals the formula evaluated in `i128` throughout.
    #[test]
    fn color_fit_batches_match_the_exact_formula() {
        use rand::{RngExt, SeedableRng};
        use rand_chacha::ChaCha8Rng;

        let mut rng = ChaCha8Rng::seed_from_u64(0xba7c);
        for case in 0..200 {
            let mut fit = ColorFit::new();
            let mut differences = [0_i128; 3];
            let mut currents = [0_i128; 3];
            let mut weights = 0_i128;
            for _ in 0..rng.random_range(1..12) {
                let pixels: u64 = match rng.random_range(0..4) {
                    0 => rng.random_range(1..=16),
                    1 => ColorFit::BATCH_PIXELS - rng.random_range(1..=2),
                    2 => ColorFit::BATCH_PIXELS + rng.random_range(0..=2),
                    _ => rng.random_range(1..=u64::from(u32::MAX)),
                };
                let k = rng.random_range(1..=M);
                let target: [u64; 3] = std::array::from_fn(|_| rng.random_range(0..=255 * pixels));
                let current: [u64; 3] = std::array::from_fn(|_| rng.random_range(0..=255 * pixels));
                fit.add_line(k, pixels as usize, target, current);
                let k = i128::from(k);
                for channel in 0..3 {
                    let (t, c) = (i128::from(target[channel]), i128::from(current[channel]));
                    differences[channel] += k * (t - c);
                    currents[channel] += k * k * c;
                }
                weights += k * k * i128::from(pixels);
            }
            let expected = std::array::from_fn::<u8, 3, _>(|channel| {
                let numerator = i128::from(M) * differences[channel] + currents[channel];
                ((2 * numerator + weights) / (2 * weights)).clamp(0, 255) as u8
            });
            let color = fit.color(77);
            assert_eq!([color.r, color.g, color.b], expected, "case {case}");
            assert_eq!(color.a, 77);
        }
    }

    /// A pixel covered by half draws half the colour, so matching the
    /// target needs twice the difference: the fit must not treat the
    /// anti-aliased pixel as fully covered (ENG-6).
    #[test]
    fn compute_color_compensates_partial_coverage() {
        let target = Buffer::new_from_color(1, 1, Color::new(100, 60, 20, 255));
        let current = Buffer::new_from_color(1, 1, Color::new(0, 0, 0, 255));
        let lines = [Scanline {
            y: 0,
            x1: 0,
            x2: 0,
            alpha: 0x8000,
        }];

        let c = compute_color(&target, &current, &lines, 255);
        assert_eq!(c, Color::new(200, 120, 40, 255));
    }

    /// Scanlines with zero coverage do not change the canvas, so they must
    /// not pull the fit either; with no weight at all there is nothing to fit.
    #[test]
    fn compute_color_ignores_uncovered_scanlines() {
        let mut target = Buffer::new(2, 1);
        target
            .pixels_mut()
            .copy_from_slice(&[100, 60, 20, 0, 255, 0]);
        let current = Buffer::new_from_color(2, 1, Color::new(0, 0, 0, 255));
        let covered = Scanline {
            y: 0,
            x1: 0,
            x2: 0,
            alpha: 0xFFFF,
        };
        let uncovered = Scanline {
            y: 0,
            x1: 1,
            x2: 1,
            alpha: 0,
        };

        let c = compute_color(&target, &current, &[covered, uncovered], 255);
        assert_eq!(c, Color::new(100, 60, 20, 255));
        let c = compute_color(&target, &current, &[uncovered], 255);
        assert_eq!(c, Color::default());
    }

    /// The fitted channel minimises the squared error of the blend model
    /// `c · (1 − w) + s · w`, `w = (alpha / 255) · (coverage / M)`, over
    /// the 8-bit values: neither neighbour of `s` does better. The test
    /// computes the error in `f64` from scratch, per pixel.
    #[test]
    fn compute_color_minimises_the_weighted_error() {
        use rand::{RngExt, SeedableRng};
        use rand_chacha::ChaCha8Rng;

        let mut rng = ChaCha8Rng::seed_from_u64(0xf17);
        for case in 0..400 {
            let (width, height) = (rng.random_range(1..40), rng.random_range(1..6));
            let mut target = Buffer::new(width, height);
            let mut current = Buffer::new(width, height);
            rng.fill(target.pixels_mut());
            rng.fill(current.pixels_mut());
            let alpha = rng.random_range(1..=255);
            let lines: Vec<Scanline> = (0..rng.random_range(1..8))
                .map(|_| {
                    let x1 = rng.random_range(-2..width as i32);
                    Scanline {
                        y: rng.random_range(-1..=height as i32),
                        x1,
                        x2: x1 + rng.random_range(0..width as i32 + 2),
                        alpha: match rng.random_range(0..4) {
                            0 => M,
                            _ => rng.random_range(0..=M),
                        },
                    }
                })
                .collect();

            let color = compute_color(&target, &current, &lines, alpha);
            // A line has weight when its fixed-point `k` (below) is positive:
            // a tiny coverage at a low alpha rounds to zero and draws nothing.
            let covered = lines.iter().any(|line| {
                alpha as u32 * 0x101 * line.alpha / M > 0
                    && clamp_line(line, width as i32, height as i32).is_some()
            });
            if !covered {
                assert_eq!(color, Color::default(), "case {case}");
                continue;
            }
            assert_eq!(color.a, alpha as u8, "case {case}");
            let fitted = [color.r, color.g, color.b];
            for (channel, &s) in fitted.iter().enumerate() {
                let error = |s: f64| {
                    let mut sum = 0.0;
                    for line in &lines {
                        let Some((x1, x2)) = clamp_line(line, width as i32, height as i32) else {
                            continue;
                        };
                        // `(alpha / 255) · (coverage / M)` in the blend's fixed point.
                        let k = alpha as u32 * 0x101 * line.alpha / M;
                        let w = f64::from(k) / f64::from(M);
                        for x in x1..=x2 {
                            let i = target.pix_offset(x, line.y) + channel;
                            let t = f64::from(target.pixels()[i]);
                            let c = f64::from(current.pixels()[i]);
                            sum += (t - c * (1.0 - w) - s * w).powi(2);
                        }
                    }
                    sum
                };
                let best = error(f64::from(s));
                let tolerance = 1e-9 * (1.0 + best);
                if s > 0 {
                    assert!(best <= error(f64::from(s) - 1.0) + tolerance, "case {case}");
                }
                if s < 255 {
                    assert!(best <= error(f64::from(s) + 1.0) + tolerance, "case {case}");
                }
            }
        }
    }

    #[test]
    fn energy_from_lines_matches_two_pass() {
        // Verify that energy_from_lines produces the same score as
        // copy_and_draw_lines + difference_partial.
        let mut target = Buffer::new(4, 4);
        target.pixels_mut()[..6].copy_from_slice(&[200, 100, 50, 10, 20, 30]);

        let mut current = Buffer::new(4, 4);
        current.pixels_mut()[..3].copy_from_slice(&[50, 60, 70]);

        let lines = [
            Scanline {
                y: 0,
                x1: 0,
                x2: 1,
                alpha: 0xFFFF,
            },
            Scanline {
                y: 1,
                x1: 0,
                x2: 2,
                alpha: 32768,
            },
        ];

        let color = compute_color(&target, &current, &lines, 128);
        let base_score = difference_full(&target, &current);

        // Two-pass reference:
        let mut scratch = Buffer::new(4, 4);
        copy_and_draw_lines(&mut scratch, &current, color, &lines);
        let two_pass = difference_partial(&target, &current, &scratch, base_score, &lines);

        // Fused single-pass:
        let fused = energy_from_lines(&target, &current, &lines, color, base_score);

        assert!(
            (two_pass - fused).abs() < 1e-12,
            "two_pass={two_pass} fused={fused}"
        );
    }

    #[test]
    fn energy_from_lines_raw_matches_normalized() {
        let mut target = Buffer::new(3, 2);
        let mut current = Buffer::new(3, 2);
        let tp = target.pixels_mut();
        tp[..9].copy_from_slice(&[200, 100, 50, 10, 20, 30, 90, 80, 70]);
        let cp = current.pixels_mut();
        cp[..9].copy_from_slice(&[50, 60, 70, 80, 70, 60, 0, 0, 0]);

        let lines = [
            Scanline {
                y: 0,
                x1: 0,
                x2: 2,
                alpha: 0xFFFF,
            },
            Scanline {
                y: 1,
                x1: 1,
                x2: 2,
                alpha: 32768,
            },
        ];

        let color = compute_color(&target, &current, &lines, 160);
        let base_raw = difference_full_raw(&target, &current);
        let raw = energy_from_lines_raw(&target, &current, &lines, color, base_raw);
        let normalized = energy_from_lines(
            &target,
            &current,
            &lines,
            color,
            difference_full(&target, &current),
        );
        let expected =
            (raw as f64 / (target.width() as f64 * target.height() as f64 * 3.0)).sqrt() / 255.0;

        assert_eq!(
            raw,
            difference_full_raw(&target, &{
                let mut after = current.clone();
                draw_lines(&mut after, color, &lines);
                after
            })
        );
        assert!((normalized - expected).abs() < 1e-12);
    }

    /// Builds prefix sums for `target` against `current`.
    fn prefix_sums(target: &Buffer, current: &Buffer) -> PrefixSums {
        let mut sums = PrefixSums::default();
        sums.compute(target, current);
        sums
    }

    /// The fused energy computed pixel by pixel and line by line, as the
    /// kernels did before prefix sums: each line subtracts the old error of
    /// its pixels and adds their error after blending that line alone.
    fn reference_energy(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        color: Color,
        score: u64,
    ) -> u64 {
        let [sr, sg, sb, sa] = color.to_premultiplied_rgba();
        let (w, h) = (target.width() as i32, target.height() as i32);
        let mut total = score;
        for line in lines {
            let Some((x1, x2)) = clamp_line(line, w, h) else {
                continue;
            };
            let ma = line.alpha;
            let a = (M - sa * ma / M) * 0x101;
            for x in x1..=x2 {
                let i = target.pix_offset(x, line.y);
                for (channel, source) in [sr, sg, sb].into_iter().enumerate() {
                    let t = i64::from(target.pixels()[i + channel]);
                    let c = current.pixels()[i + channel];
                    let d1 = t - i64::from(c);
                    let d2 = t - i64::from(blend_channel_scalar(c, source, ma, a));
                    total = total.wrapping_sub((d1 * d1) as u64);
                    total = total.wrapping_add((d2 * d2) as u64);
                }
            }
        }
        total
    }

    /// The prefix-sum colour fit and energy equal the per-pixel ones, and
    /// the bounded energy is the energy exactly when it is below the bound,
    /// on random buffers and scanlines that stick out, overlap and repeat.
    #[test]
    fn scoring_with_prefix_sums_matches_the_per_pixel_path() {
        use super::fixtures::{random_buffer, random_lines};
        use rand::{RngExt, SeedableRng};
        use rand_chacha::ChaCha8Rng;

        let mut rng = ChaCha8Rng::seed_from_u64(0x9e5);
        for (width, height) in [(1, 1), (2, 2), (9, 5), (17, 6), (33, 9), (65, 4)] {
            let target = random_buffer(&mut rng, width, height);
            let current = random_buffer(&mut rng, width, height);
            let sums = prefix_sums(&target, &current);
            let score = difference_full_raw(&target, &current);
            for alpha in [1, 37, 128, 200, 255] {
                let lines = random_lines(&mut rng, width as i32, height as i32);
                let color = compute_color(&target, &current, &lines, alpha);
                let fit_pixels = fit(&target, &current, None, &lines, alpha);
                let fit_sums = fit(&target, &current, Some(&sums), &lines, alpha);
                assert_eq!(fit_pixels.color, color);
                assert_eq!(fit_sums, fit_pixels);

                let expected = reference_energy(&target, &current, &lines, color, score);
                assert_eq!(
                    energy_from_lines_raw(&target, &current, &lines, color, score),
                    expected
                );
                assert_eq!(energy(&target, &current, &lines, fit_sums, score), expected);
                let bounds = [
                    0,
                    1,
                    expected.wrapping_sub(1),
                    expected,
                    expected.wrapping_add(1),
                    score,
                    u64::MAX,
                    rng.random_range(0..=score),
                ];
                for bound in bounds {
                    assert_eq!(
                        energy_below(&target, &current, &lines, fit_sums, score, bound),
                        (expected < bound).then_some(expected),
                        "{width}x{height} alpha={alpha} bound={bound} energy={expected}"
                    );
                }
            }
        }
    }

    /// A line listed twice subtracts its old error twice, so the running
    /// total dips below zero before the new errors come back in. The bounded
    /// energy must neither stop at the dip nor misread the wrapped result.
    #[test]
    fn energy_below_is_exact_when_repeated_lines_dip_below_zero() {
        let target = Buffer::new_from_color(2, 2, Color::new(255, 255, 255, 255));
        let current = Buffer::new(2, 2);
        let sums = prefix_sums(&target, &current);
        let score = difference_full_raw(&target, &current);
        let line = full_row(0, 2);
        let lines = [line, line, line];
        let shape_fit = fit(&target, &current, Some(&sums), &lines, 255);
        assert_eq!(shape_fit.color, Color::new(255, 255, 255, 255));
        let expected = energy(&target, &current, &lines, shape_fit, score);
        // The row's error is half the score, subtracted three times, and
        // the opaque white shape matches the target, so nothing comes back.
        assert_eq!(expected, (score as i64 - 3 * (score as i64 / 2)) as u64);
        for bound in [0, 1, score, u64::MAX - 1, u64::MAX] {
            assert_eq!(
                energy_below(&target, &current, &lines, shape_fit, score, bound),
                (expected < bound).then_some(expected),
                "bound={bound}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "prefix sums must match the buffers")]
    fn fit_rejects_prefix_sums_of_another_size() {
        let target = Buffer::new(16, 4);
        let sums = prefix_sums(&Buffer::new(16, 5), &Buffer::new(16, 5));
        let _ = fit(&target, &target, Some(&sums), &[full_row(3, 16)], 128);
    }

    fn full_row(y: i32, width: i32) -> Scanline {
        Scanline {
            y,
            x1: 0,
            x2: width - 1,
            alpha: 0xFFFF,
        }
    }

    // A wider `current` keeps every read in bounds even before the check, so
    // these tests never run the out-of-bounds read they guard against.
    #[test]
    #[should_panic(expected = "current must have target's dimensions")]
    fn compute_color_rejects_current_with_other_dimensions() {
        let target = Buffer::new(16, 4);
        let current = Buffer::new(17, 4);
        let _ = compute_color(&target, &current, &[full_row(3, 16)], 128);
    }

    #[test]
    #[should_panic(expected = "current must have target's dimensions")]
    fn energy_from_lines_raw_rejects_current_with_other_dimensions() {
        let target = Buffer::new(16, 4);
        let current = Buffer::new(16, 5);
        let color = Color::new(10, 20, 30, 128);
        let _ = energy_from_lines_raw(&target, &current, &[full_row(3, 16)], color, 0);
    }

    /// The blend computes `v = current * a + source * ma` with
    /// `a = (M - sa * ma / M) * 0x101`. `v` is largest for `current = 255`
    /// and a white source (`source = sa`); check the documented bound
    /// `v < M * (M + 1)` for every colour alpha and every coverage.
    #[test]
    fn blend_value_stays_below_the_overflow_bound() {
        let bound = u64::from(M) * u64::from(M + 1);
        for alpha in 0_u8..=255 {
            let [source, .., sa] = Color::new(255, 255, 255, alpha).to_premultiplied_rgba();
            assert_eq!(source, sa);
            for ma in 0..=M {
                let a = (M - sa * ma / M) * 0x101;
                let value = 255 * u64::from(a) + u64::from(source) * u64::from(ma);
                assert!(value < bound, "alpha={alpha} ma={ma} value={value}");
            }
        }
    }

    /// `div_by_m(v) == v / M` for every `v < M * (M + 1)`: checked at both
    /// ends of every quotient's range, where an off-by-one would show.
    #[test]
    fn div_by_m_is_exact_below_the_bound() {
        for q in 0..=M {
            for r in [0, 1, q.saturating_sub(1), q.min(M - 1), M - 2, M - 1] {
                let value = q * M + r;
                assert_eq!(div_by_m(value), q, "q={q} r={r}");
            }
        }
    }
}

/// NEON and scalar kernels must agree bit for bit; release builds on aarch64
/// only run the NEON ones, so CI covers them on the macOS arm64 leg.
#[cfg(all(test, target_arch = "aarch64"))]
mod neon_parity {
    use super::fixtures::{random_buffer, random_lines};
    use super::{
        Fit, LineKernels, M, Neon, Scalar, blend_channel_scalar, energy_with, fit_with, neon,
        scalar,
    };
    use crate::buffer::Buffer;
    use crate::color::Color;
    use crate::prefix::PrefixSums;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn prefix_sums(target: &Buffer, current: &Buffer) -> PrefixSums {
        let mut sums = PrefixSums::default();
        sums.compute(target, current);
        sums
    }

    fn sizes() -> impl Iterator<Item = (u32, u32)> {
        [
            (1, 1),
            (2, 2),
            (7, 3),
            (8, 2),
            (9, 5),
            (15, 4),
            (16, 3),
            (17, 6),
        ]
        .into_iter()
        .chain([(23, 7), (31, 2), (33, 9), (64, 3), (65, 4)])
    }

    #[test]
    fn compute_color_matches_scalar_for_every_alpha() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xc010);
        for (width, height) in sizes() {
            let target = random_buffer(&mut rng, width, height);
            let current = random_buffer(&mut rng, width, height);
            let sums = prefix_sums(&target, &current);
            for alpha in 1..=255 {
                let lines = random_lines(&mut rng, width as i32, height as i32);
                for sums in [None, Some(&sums)] {
                    let expected = fit_with::<Scalar>(&target, &current, sums, &lines, alpha);
                    let actual = fit_with::<Neon>(&target, &current, sums, &lines, alpha);
                    assert_eq!(actual, expected, "{width}x{height} alpha={alpha}");
                }
            }
        }
    }

    /// Lines longer than `BLOCKS_PER_FLUSH` blocks of 16 pixels, so the NEON
    /// line sums flush their 16-bit lanes at least once mid-line; bright
    /// buffers push every lane towards its overflow bound.
    #[test]
    fn compute_color_matches_scalar_on_long_lines() {
        let mut rng = ChaCha8Rng::seed_from_u64(0x1086);
        let (width, height) = (2 * 8 * 256 + 13, 3);
        let mut target = Buffer::new_from_color(width, height, Color::new(255, 255, 255, 255));
        let current = random_buffer(&mut rng, width, height);
        rng.fill(&mut target.pixels_mut()[..64]);
        let sums = prefix_sums(&target, &current);
        for alpha in [1, 128, 255] {
            let lines = random_lines(&mut rng, width as i32, height as i32);
            for sums in [None, Some(&sums)] {
                let expected = fit_with::<Scalar>(&target, &current, sums, &lines, alpha);
                let actual = fit_with::<Neon>(&target, &current, sums, &lines, alpha);
                assert_eq!(actual, expected, "alpha={alpha}");
            }
        }
    }

    #[test]
    fn energy_matches_scalar_for_every_alpha() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xe4e7);
        for (width, height) in sizes() {
            let target = random_buffer(&mut rng, width, height);
            let current = random_buffer(&mut rng, width, height);
            let score = scalar::difference_full_raw(&target, &current);
            for alpha in 1..=255 {
                let lines = random_lines(&mut rng, width as i32, height as i32);
                let color = Color::new(rng.random(), rng.random(), rng.random(), alpha);
                let before = fit_with::<Scalar>(&target, &current, None, &lines, 255).before;
                let fit = Fit { color, before };
                let limit = Some(rng.random_range(0..=score as i64));
                for limit in [None, limit] {
                    let expected =
                        energy_with::<Scalar>(&target, &current, &lines, fit, score, limit);
                    let actual = energy_with::<Neon>(&target, &current, &lines, fit, score, limit);
                    assert_eq!(actual, expected, "{width}x{height} {color:?} {limit:?}");
                }
            }
        }
    }

    /// Every line kernel on every start and length of a few rows, so the
    /// masked tails meet the end of the buffer and fall back to scalar code.
    #[test]
    fn line_kernels_match_scalar_on_every_span() {
        let mut rng = ChaCha8Rng::seed_from_u64(0x11e5);
        for (width, height) in [(1, 1), (9, 2), (23, 3)] {
            let target = random_buffer(&mut rng, width, height);
            let current = random_buffer(&mut rng, width, height);
            let (t_pix, c_pix) = (target.pixels(), current.pixels());
            let total = (width * height) as usize;
            for first in 0..total {
                for pixels in 0..=total - first {
                    let start = first * 3;
                    let color = Color::new(rng.random(), rng.random(), rng.random(), rng.random());
                    let [sr, sg, sb, sa] = color.to_premultiplied_rgba();
                    let ma = rng.random_range(0..=M);
                    let a = (M - sa * ma / M) * 0x101;
                    // SAFETY: `start + pixels * 3` is at most `total * 3`,
                    // the length of both buffers.
                    unsafe {
                        assert_eq!(
                            Neon::line_sums(t_pix, c_pix, start, pixels),
                            Scalar::line_sums(t_pix, c_pix, start, pixels),
                            "{first} {pixels}"
                        );
                        assert_eq!(
                            Neon::line_error(t_pix, c_pix, start, pixels),
                            Scalar::line_error(t_pix, c_pix, start, pixels),
                            "{first} {pixels}"
                        );
                        assert_eq!(
                            Neon::line_after(t_pix, c_pix, start, pixels, [sr, sg, sb], ma, a),
                            Scalar::line_after(t_pix, c_pix, start, pixels, [sr, sg, sb], ma, a),
                            "{first} {pixels} {color:?} {ma}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn difference_full_raw_matches_scalar() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xd1ff);
        for (width, height) in sizes() {
            let a = random_buffer(&mut rng, width, height);
            let b = random_buffer(&mut rng, width, height);
            let expected = scalar::difference_full_raw(&a, &b);
            let actual = neon::difference_full_raw(&a, &b);
            assert_eq!(actual, expected, "{width}x{height}");
        }
    }

    /// Buffers longer than `SQUARES_PER_BLOCK` 16-byte chunks, so the NEON
    /// sum flushes its 32-bit lanes mid-buffer; black against white puts
    /// every lane at its overflow bound.
    #[test]
    fn difference_full_raw_matches_scalar_on_large_extreme_buffers() {
        let (width, height) = (7_301, 9);
        let mut black = Buffer::new_from_color(width, height, Color::new(0, 0, 0, 255));
        let white = Buffer::new_from_color(width, height, Color::new(255, 255, 255, 255));
        let actual = neon::difference_full_raw(&black, &white);
        assert_eq!(actual, u64::from(width * height) * 3 * 255 * 255);

        let mut rng = ChaCha8Rng::seed_from_u64(0xb16);
        rng.fill(&mut black.pixels_mut()[..4096]);
        let expected = scalar::difference_full_raw(&black, &white);
        let actual = neon::difference_full_raw(&black, &white);
        assert_eq!(actual, expected);
    }

    #[test]
    fn blend_matches_scalar_for_every_alpha_and_channel_value() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xb1e4);
        for alpha in 1..=255 {
            let color = Color::new(rng.random(), rng.random(), rng.random(), alpha);
            let [sr, _, _, sa] = color.to_premultiplied_rgba();
            for ma in [0, 1, M / 2, M - 1, M, rng.random_range(0..=M)] {
                let a = (M - sa * ma / M) * 0x101;
                for chunk in 0_u8..32 {
                    let current: [u8; 8] = std::array::from_fn(|lane| chunk * 8 + lane as u8);
                    // SAFETY: only requires NEON, a baseline aarch64 feature.
                    let simd = unsafe { neon::blend_chunk_u8x8(current, sr, ma, a) };
                    for lane in 0..8 {
                        let expected = blend_channel_scalar(current[lane], sr, ma, a);
                        assert_eq!(simd[lane], expected, "alpha={alpha} ma={ma} lane={lane}");
                    }
                }
            }
        }
    }
}
