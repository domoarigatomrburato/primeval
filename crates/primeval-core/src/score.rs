//! Scoring and blending routines for the energy minimization loop.
//!
//! Blending and scoring keep the Go original's integer arithmetic,
//! truncation semantics, and accumulator widths; the colour fit weights
//! pixels by coverage instead. On aarch64, hot paths use NEON
//! intrinsics to process 8 pixels per iteration.

use crate::buffer::Buffer;
use crate::color::Color;
use crate::scanline::{Scanline, clamp_line};

const M: u32 = 0xFFFF;

#[inline]
pub(crate) fn raw_score_to_normalized(raw: u64, width: u32, height: u32) -> f64 {
    (raw as f64 / (width as f64 * height as f64 * 4.0)).sqrt() / 255.0
}

#[cfg(test)]
#[inline]
fn normalized_to_raw_score(score: f64, width: u32, height: u32) -> u64 {
    let s = score * 255.0;
    (s * s * (width as f64 * height as f64 * 4.0)).round() as u64
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

mod scalar {
    use super::{Buffer, Color, ColorFit, M, Scanline, blend_channel_scalar, clamp_line};

    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    pub(super) fn compute_color(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        alpha: i32,
    ) -> Color {
        let w = target.width() as i32;
        let h = target.height() as i32;
        let t_pix = target.pixels();
        let c_pix = current.pixels();
        let mut fit = ColorFit::new();

        for line in lines {
            let k = ColorFit::weight(alpha, line.alpha);
            if k == 0 {
                continue;
            }
            let Some((x1, x2)) = clamp_line(line, w, h) else {
                continue;
            };
            let pixels = (x2 - x1 + 1) as usize;
            let start = target.pix_offset(x1, line.y);
            let (target_sums, current_sums) = line_sums(t_pix, c_pix, start, pixels);
            fit.add_line(k, pixels, target_sums, current_sums);
        }

        fit.color(alpha)
    }

    /// Per-channel RGB sums of `pixels` pixels from byte offset `start` in
    /// the target and in the canvas.
    #[inline]
    pub(super) fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        let end = start + pixels * 4;
        let (t_px4, _) = t_pix[start..end].as_chunks::<4>();
        let (c_px4, _) = c_pix[start..end].as_chunks::<4>();
        let mut target_sums = [0_u64; 3];
        let mut current_sums = [0_u64; 3];
        for (t_px, c_px) in t_px4.iter().zip(c_px4) {
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

    pub(super) fn difference_full_raw_pixels(a_pix: &[u8], b_pix: &[u8]) -> u64 {
        let mut total = 0_u64;
        let (a_px4, _) = a_pix.as_chunks::<4>();
        let (b_px4, _) = b_pix.as_chunks::<4>();
        for (a_px, b_px) in a_px4.iter().zip(b_px4) {
            let dr = i32::from(a_px[0]) - i32::from(b_px[0]);
            let dg = i32::from(a_px[1]) - i32::from(b_px[1]);
            let db = i32::from(a_px[2]) - i32::from(b_px[2]);
            let da = i32::from(a_px[3]) - i32::from(b_px[3]);
            total += (dr * dr + dg * dg + db * db + da * da) as u64;
        }
        total
    }

    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    pub(super) fn energy_from_lines_raw(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        color: Color,
        score: u64,
    ) -> u64 {
        let [sr, sg, sb, sa] = color.to_premultiplied_rgba();
        let w = target.width() as i32;
        let h = target.height() as i32;
        let mut total = score;

        let t_pix = target.pixels();
        let c_pix = current.pixels();

        for line in lines {
            let (x1, x2) = match clamp_line(line, w, h) {
                Some(v) => v,
                None => continue,
            };
            let ma = line.alpha;
            let a = (M - sa * ma / M) * 0x101;
            let mut i = (line.y as usize * w as usize + x1 as usize) * 4;
            for _ in x1..=x2 {
                let b0 = c_pix[i];
                let b1 = c_pix[i + 1];
                let b2 = c_pix[i + 2];
                let b3 = c_pix[i + 3];

                let br = i32::from(b0);
                let bg = i32::from(b1);
                let bb = i32::from(b2);
                let ba = i32::from(b3);

                let ar = i32::from(blend_channel_scalar(b0, sr, ma, a));
                let ag = i32::from(blend_channel_scalar(b1, sg, ma, a));
                let ab = i32::from(blend_channel_scalar(b2, sb, ma, a));
                let aa = i32::from(blend_channel_scalar(b3, sa, ma, a));

                let tr = i32::from(t_pix[i]);
                let tg = i32::from(t_pix[i + 1]);
                let tb = i32::from(t_pix[i + 2]);
                let ta = i32::from(t_pix[i + 3]);
                i += 4;

                let dr1 = tr - br;
                let dg1 = tg - bg;
                let db1 = tb - bb;
                let da1 = ta - ba;

                let dr2 = tr - ar;
                let dg2 = tg - ag;
                let db2 = tb - ab;
                let da2 = ta - aa;

                total = total.wrapping_sub((dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64);
                total = total.wrapping_add((dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64);
            }
        }

        total
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::{Buffer, Color, ColorFit, M, Scanline, blend_channel_scalar, clamp_line, scalar};
    use std::arch::aarch64::*;

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

    /// Chunks of 8 pixels summed in 16-bit lanes before widening: each lane
    /// gains at most 255 per chunk, and `255 * 256 = 65_280` fits in `u16`.
    const CHUNKS_PER_BLOCK: usize = 256;

    /// Per-channel RGB sums of `pixels` pixels from byte offset `start` in
    /// the target and in the canvas; matches [`scalar::line_sums`].
    ///
    /// # Safety
    ///
    /// `start + pixels * 4` must not exceed `t_pix.len()` or `c_pix.len()`.
    #[inline]
    unsafe fn line_sums(
        t_pix: &[u8],
        c_pix: &[u8],
        start: usize,
        pixels: usize,
    ) -> ([u64; 3], [u64; 3]) {
        let mut target_sums = [0_u64; 3];
        let mut current_sums = [0_u64; 3];
        let mut byte_index = start;
        let mut chunks = pixels / 8;

        while chunks > 0 {
            let block = chunks.min(CHUNKS_PER_BLOCK);
            chunks -= block;
            // SAFETY: each `vld4_u8` reads 8 pixels (32 bytes) from
            // `byte_index`. The loops read `pixels / 8` chunks in total from
            // `start`, so the last byte read is below `start + pixels * 4`,
            // which the caller keeps within both slices. The remaining calls
            // are register-only NEON operations; NEON is a baseline aarch64
            // feature.
            unsafe {
                let mut target_acc = [vdupq_n_u16(0); 3];
                let mut current_acc = [vdupq_n_u16(0); 3];
                for _ in 0..block {
                    let t = vld4_u8(t_pix.as_ptr().add(byte_index));
                    let c = vld4_u8(c_pix.as_ptr().add(byte_index));
                    target_acc[0] = vaddw_u8(target_acc[0], t.0);
                    target_acc[1] = vaddw_u8(target_acc[1], t.1);
                    target_acc[2] = vaddw_u8(target_acc[2], t.2);
                    current_acc[0] = vaddw_u8(current_acc[0], c.0);
                    current_acc[1] = vaddw_u8(current_acc[1], c.1);
                    current_acc[2] = vaddw_u8(current_acc[2], c.2);
                    byte_index += 32;
                }
                for channel in 0..3 {
                    target_sums[channel] += u64::from(vaddlvq_u16(target_acc[channel]));
                    current_sums[channel] += u64::from(vaddlvq_u16(current_acc[channel]));
                }
            }
        }

        let (tail_target, tail_current) = scalar::line_sums(t_pix, c_pix, byte_index, pixels % 8);
        for channel in 0..3 {
            target_sums[channel] += tail_target[channel];
            current_sums[channel] += tail_current[channel];
        }
        (target_sums, current_sums)
    }

    /// # Safety
    ///
    /// `current` must have the same width and height as `target`. Scanlines are
    /// clipped against `target`, and the 8-pixel loads read both buffers at the
    /// same byte offsets. The safe wrapper [`super::compute_color`] checks this.
    pub(super) unsafe fn compute_color(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        alpha: i32,
    ) -> Color {
        let w = target.width() as i32;
        let h = target.height() as i32;
        let t_pix = target.pixels();
        let c_pix = current.pixels();
        let mut fit = ColorFit::new();

        for line in lines {
            let k = ColorFit::weight(alpha, line.alpha);
            if k == 0 {
                continue;
            }
            let Some((x1, x2)) = clamp_line(line, w, h) else {
                continue;
            };
            let pixels = (x2 - x1 + 1) as usize;
            let start = target.pix_offset(x1, line.y);
            // SAFETY: `clamp_line` keeps `line.y` in `0..h` and `x1..=x2` in
            // `0..w` of `target`, so `start + pixels * 4` is at most the
            // length of `target.pixels()`, `w * h * 4` (the `Buffer` length
            // invariant, asserted by every constructor). `current` has
            // `target`'s width and height (this function's contract, asserted
            // by the safe caller), so by the same invariant
            // `c_pix.len() == t_pix.len()`.
            let (target_sums, current_sums) = unsafe { line_sums(t_pix, c_pix, start, pixels) };
            fit.add_line(k, pixels, target_sums, current_sums);
        }

        fit.color(alpha)
    }

    /// # Safety
    ///
    /// `a` and `b` must have the same width and height.
    pub(super) unsafe fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
        let a_pix = a.pixels();
        let b_pix = b.pixels();
        let chunk_bytes = a_pix.len() / 32 * 32;
        let mut total = 0_u64;
        let mut byte_index = 0_usize;

        while byte_index < chunk_bytes {
            // SAFETY: `byte_index + 32 <= chunk_bytes <= a_pix.len()`. By this
            // function's contract `b` has `a`'s dimensions, so by the `Buffer`
            // invariant (`len == width * height * 4`) `b_pix.len() == a_pix.len()`
            // and the read from `b_pix` is in bounds too. The remaining calls
            // are register-only NEON operations; NEON is a baseline aarch64
            // feature.
            unsafe {
                let a_channels = vld4_u8(a_pix.as_ptr().add(byte_index));
                let b_channels = vld4_u8(b_pix.as_ptr().add(byte_index));
                total += sum_squared_diff_u8x8(a_channels.0, b_channels.0);
                total += sum_squared_diff_u8x8(a_channels.1, b_channels.1);
                total += sum_squared_diff_u8x8(a_channels.2, b_channels.2);
                total += sum_squared_diff_u8x8(a_channels.3, b_channels.3);
            }
            byte_index += 32;
        }

        total + scalar::difference_full_raw_pixels(&a_pix[chunk_bytes..], &b_pix[chunk_bytes..])
    }

    /// # Safety
    ///
    /// `current` must have the same width and height as `target`. Scanlines are
    /// clipped against `target`, and the 8-pixel loads read both buffers at the
    /// same byte offsets. The safe wrapper [`super::energy_from_lines_raw`]
    /// checks this.
    pub(super) unsafe fn energy_from_lines_raw(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        color: Color,
        score: u64,
    ) -> u64 {
        let [sr, sg, sb, sa] = color.to_premultiplied_rgba();
        let w = target.width() as i32;
        let h = target.height() as i32;
        let mut total = score;
        let t_pix = target.pixels();
        let c_pix = current.pixels();

        for line in lines {
            let (x1, x2) = match clamp_line(line, w, h) {
                Some(v) => v,
                None => continue,
            };

            let ma = line.alpha;
            let a = (M - sa * ma / M) * 0x101;
            let pixel_count = (x2 - x1 + 1) as usize;
            let mut byte_index = target.pix_offset(x1, line.y);
            let chunk_pixels = pixel_count / 8;

            for _ in 0..chunk_pixels {
                // SAFETY: same bounds argument as in `compute_color`: the loads
                // stay inside the clipped row of `target`, and `current` has
                // `target`'s dimensions (this function's contract, asserted by
                // the safe caller), so `c_pix.len() == t_pix.len()` by the
                // `Buffer` length invariant.
                let (target_channels, current_channels) = unsafe {
                    (
                        vld4_u8(t_pix.as_ptr().add(byte_index)),
                        vld4_u8(c_pix.as_ptr().add(byte_index)),
                    )
                };

                // SAFETY: these helpers only run register-only NEON operations
                // (no memory access); NEON is a baseline aarch64 feature.
                unsafe {
                    total = total
                        .wrapping_sub(sum_squared_diff_u8x8(target_channels.0, current_channels.0));
                    total = total
                        .wrapping_sub(sum_squared_diff_u8x8(target_channels.1, current_channels.1));
                    total = total
                        .wrapping_sub(sum_squared_diff_u8x8(target_channels.2, current_channels.2));
                    total = total
                        .wrapping_sub(sum_squared_diff_u8x8(target_channels.3, current_channels.3));

                    let after_r = blend_vector_u8x8(current_channels.0, sr, ma, a);
                    let after_g = blend_vector_u8x8(current_channels.1, sg, ma, a);
                    let after_b = blend_vector_u8x8(current_channels.2, sb, ma, a);
                    let after_a = blend_vector_u8x8(current_channels.3, sa, ma, a);

                    total = total.wrapping_add(sum_squared_diff_u8x8(target_channels.0, after_r));
                    total = total.wrapping_add(sum_squared_diff_u8x8(target_channels.1, after_g));
                    total = total.wrapping_add(sum_squared_diff_u8x8(target_channels.2, after_b));
                    total = total.wrapping_add(sum_squared_diff_u8x8(target_channels.3, after_a));
                }
                byte_index += 32;
            }

            for _ in 0..(pixel_count % 8) {
                let b0 = c_pix[byte_index];
                let b1 = c_pix[byte_index + 1];
                let b2 = c_pix[byte_index + 2];
                let b3 = c_pix[byte_index + 3];

                let br = i32::from(b0);
                let bg = i32::from(b1);
                let bb = i32::from(b2);
                let ba = i32::from(b3);

                let ar = i32::from(blend_channel_scalar(b0, sr, ma, a));
                let ag = i32::from(blend_channel_scalar(b1, sg, ma, a));
                let ab = i32::from(blend_channel_scalar(b2, sb, ma, a));
                let aa = i32::from(blend_channel_scalar(b3, sa, ma, a));

                let tr = i32::from(t_pix[byte_index]);
                let tg = i32::from(t_pix[byte_index + 1]);
                let tb = i32::from(t_pix[byte_index + 2]);
                let ta = i32::from(t_pix[byte_index + 3]);
                byte_index += 4;

                let dr1 = tr - br;
                let dg1 = tg - bg;
                let db1 = tb - bb;
                let da1 = ta - ba;
                let dr2 = tr - ar;
                let dg2 = tg - ag;
                let db2 = tb - ab;
                let da2 = ta - aa;

                total = total.wrapping_sub((dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64);
                total = total.wrapping_add((dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64);
            }
        }

        total
    }
}

/// Computes the optimal color for drawing `lines` onto `current` to
/// best approximate `target` at the given `alpha` level, which must be
/// `1..=255`.
///
/// Each pixel is weighted by its scanline's coverage, so anti-aliased edges
/// are fitted as partly covered (see [`ColorFit`]); Go's `primitive` treats
/// them as fully covered, which under-saturates the colour.
///
/// Returns a zero [`Color`] if no in-bounds scanline pixel has coverage.
///
/// # Panics
///
/// Panics if `current` and `target` have different dimensions.
#[must_use]
pub(crate) fn compute_color(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    alpha: i32,
) -> Color {
    assert_same_dimensions(target, current);
    debug_assert!((1..=255).contains(&alpha), "alpha must be 1..=255");
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: `assert_same_dimensions` above guarantees that `current` has
        // `target`'s width and height, the contract of `neon::compute_color`.
        unsafe { neon::compute_color(target, current, lines, alpha) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        scalar::compute_color(target, current, lines, alpha)
    }
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
        let mut i = (line.y as usize * w as usize + x1 as usize) * 4;
        for _ in x1..=x2 {
            dst_pix[i] = blend_channel_scalar(src_pix[i], sr, ma, a);
            dst_pix[i + 1] = blend_channel_scalar(src_pix[i + 1], sg, ma, a);
            dst_pix[i + 2] = blend_channel_scalar(src_pix[i + 2], sb, ma, a);
            dst_pix[i + 3] = blend_channel_scalar(src_pix[i + 3], sa, ma, a);
            i += 4;
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
        let mut i = (line.y as usize * w as usize + x1 as usize) * 4;
        for _ in x1..=x2 {
            pix[i] = blend_channel_scalar(pix[i], sr, ma, a);
            pix[i + 1] = blend_channel_scalar(pix[i + 1], sg, ma, a);
            pix[i + 2] = blend_channel_scalar(pix[i + 2], sb, ma, a);
            pix[i + 3] = blend_channel_scalar(pix[i + 3], sa, ma, a);
            i += 4;
        }
    }
}

/// Computes the root-mean-square difference between two buffers,
/// normalized to the `[0, 1]` range.
///
/// Identical buffers yield `0.0`. Maximum difference (all black vs.
/// all white with full alpha) yields a value close to `1.0`.
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
        // SAFETY: the `assert_eq!`s above guarantee `a` and `b` have the same
        // dimensions, which is the contract of `neon::difference_full_raw`.
        unsafe { neon::difference_full_raw(a, b) }
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
        let mut i = target.pix_offset(x1, line.y);
        for _ in x1..=x2 {
            let tr = t_pix[i] as i32;
            let tg = t_pix[i + 1] as i32;
            let tb = t_pix[i + 2] as i32;
            let ta = t_pix[i + 3] as i32;
            let br = b_pix[i] as i32;
            let bg = b_pix[i + 1] as i32;
            let bb = b_pix[i + 2] as i32;
            let ba = b_pix[i + 3] as i32;
            let ar = a_pix[i] as i32;
            let ag = a_pix[i + 1] as i32;
            let ab = a_pix[i + 2] as i32;
            let aa = a_pix[i + 3] as i32;
            i += 4;

            let dr1 = tr - br;
            let dg1 = tg - bg;
            let db1 = tb - bb;
            let da1 = ta - ba;
            let dr2 = tr - ar;
            let dg2 = tg - ag;
            let db2 = tb - ab;
            let da2 = ta - aa;

            total = total.wrapping_sub((dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64);
            total = total.wrapping_add((dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64);
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

/// Fused replacement for `copy_and_draw_lines` + `difference_partial`.
///
/// Computes the blended pixel for each covered scanline pixel on the fly
/// (no write to any intermediate buffer) and accumulates the squared-difference
/// update in a single pass. This halves memory traffic compared to the
/// two-pass approach used outside the hot energy-evaluation loop.
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
pub(crate) fn energy_from_lines_raw(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    color: Color,
    score: u64,
) -> u64 {
    assert_same_dimensions(target, current);
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: `assert_same_dimensions` above guarantees that `current` has
        // `target`'s width and height, the contract of
        // `neon::energy_from_lines_raw`.
        unsafe { neon::energy_from_lines_raw(target, current, lines, color, score) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        scalar::energy_from_lines_raw(target, current, lines, color, score)
    }
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
        // 1x1 buffer: a = (255,0,0,255), b = (0,0,0,255)
        let mut a = Buffer::new(1, 1);
        let mut b = Buffer::new(1, 1);
        let ap = a.pixels_mut();
        ap[0] = 255;
        ap[1] = 0;
        ap[2] = 0;
        ap[3] = 255;
        let bp = b.pixels_mut();
        bp[0] = 0;
        bp[1] = 0;
        bp[2] = 0;
        bp[3] = 255;

        // dr=255, dg=0, db=0, da=0 => total = 255*255 = 65025
        // sqrt(65025 / 4) / 255 = sqrt(16256.25) / 255 = 127.5 / 255 = 0.5
        let diff = difference_full(&a, &b);
        assert!((diff - 0.5).abs() < 1e-9, "got {diff}");
    }

    #[test]
    fn difference_full_raw_matches_normalized() {
        let mut a = Buffer::new(2, 1);
        let mut b = Buffer::new(2, 1);

        let ap = a.pixels_mut();
        ap[..8].copy_from_slice(&[255, 32, 0, 255, 10, 20, 30, 255]);

        let bp = b.pixels_mut();
        bp[..8].copy_from_slice(&[0, 16, 64, 255, 40, 50, 60, 255]);

        let raw = difference_full_raw(&a, &b);
        let normalized = difference_full(&a, &b);
        let expected = (raw as f64 / (a.width() as f64 * a.height() as f64 * 4.0)).sqrt() / 255.0;

        assert_eq!(raw, 65025 + 256 + 4096 + 2700);
        assert!((normalized - expected).abs() < 1e-12);
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
        // target pixel (0,0) = (255, 0, 0, 255), current = (0, 0, 0, 0)
        let mut target = Buffer::new(1, 1);
        let tp = target.pixels_mut();
        tp[0] = 255;
        tp[1] = 0;
        tp[2] = 0;
        tp[3] = 255;

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

        // Pixels outside scanline (e.g. (0,0)) should be unchanged
        assert_eq!(im.pixels()[0], 0);
        assert_eq!(im.pixels()[1], 0);
        assert_eq!(im.pixels()[2], 0);
        assert_eq!(im.pixels()[3], 255);
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
        sp[i1 + 3] = 255;
        let i2 = (2 * 4 + 2) * 4; // pix_offset(2, 2) for w=4
        sp[i2] = 10;
        sp[i2 + 1] = 20;
        sp[i2 + 2] = 30;
        sp[i2 + 3] = 255;

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
                &got.pixels()[i..i + 4],
                &reference.pixels()[i..i + 4],
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
        let tp = target.pixels_mut();
        tp[0] = 255;
        tp[1] = 0;
        tp[2] = 0;
        tp[3] = 255;

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
        tp[..8].copy_from_slice(&[255, 0, 0, 255, 20, 40, 60, 255]);
        let bp = before.pixels_mut();
        bp[..8].copy_from_slice(&[0, 0, 0, 255, 80, 60, 40, 255]);

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
            (partial_raw as f64 / (target.width() as f64 * target.height() as f64 * 4.0)).sqrt()
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
            .copy_from_slice(&[200, 100, 50, 255, 120, 180, 90, 255]);
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
            .copy_from_slice(&[100, 60, 20, 255, 0, 255, 0, 255]);
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
            let covered = lines.iter().any(|line| {
                line.alpha > 0 && clamp_line(line, width as i32, height as i32).is_some()
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
        let tp = target.pixels_mut();
        tp[0] = 200;
        tp[1] = 100;
        tp[2] = 50;
        tp[3] = 255;
        tp[4] = 10;
        tp[5] = 20;
        tp[6] = 30;
        tp[7] = 255;

        let mut current = Buffer::new(4, 4);
        let cp = current.pixels_mut();
        cp[0] = 50;
        cp[1] = 60;
        cp[2] = 70;
        cp[3] = 255;

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
        tp[..12].copy_from_slice(&[200, 100, 50, 255, 10, 20, 30, 255, 90, 80, 70, 255]);
        let cp = current.pixels_mut();
        cp[..12].copy_from_slice(&[50, 60, 70, 255, 80, 70, 60, 255, 0, 0, 0, 255]);

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
            (raw as f64 / (target.width() as f64 * target.height() as f64 * 4.0)).sqrt() / 255.0;

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
    use super::{M, blend_channel_scalar, neon, scalar};
    use crate::buffer::Buffer;
    use crate::color::Color;
    use crate::scanline::Scanline;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn random_buffer(rng: &mut ChaCha8Rng, width: u32, height: u32) -> Buffer {
        let mut buffer = Buffer::new(width, height);
        rng.fill(buffer.pixels_mut());
        buffer
    }

    /// Scanlines of every kind the kernels must handle: full rows touching
    /// both edges, lines sticking out on either side or off the canvas,
    /// lengths around the 8-pixel chunk size, and every coverage extreme.
    fn random_lines(rng: &mut ChaCha8Rng, width: i32, height: i32) -> Vec<Scanline> {
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
            for alpha in 1..=255 {
                let lines = random_lines(&mut rng, width as i32, height as i32);
                let expected = scalar::compute_color(&target, &current, &lines, alpha);
                // SAFETY: `target` and `current` have the same dimensions.
                let actual = unsafe { neon::compute_color(&target, &current, &lines, alpha) };
                assert_eq!(actual, expected, "{width}x{height} alpha={alpha}");
            }
        }
    }

    /// Lines longer than `CHUNKS_PER_BLOCK` chunks of 8 pixels, so the NEON
    /// line sums flush their 16-bit lanes at least once mid-line; bright
    /// buffers push every lane towards its overflow bound.
    #[test]
    fn compute_color_matches_scalar_on_long_lines() {
        let mut rng = ChaCha8Rng::seed_from_u64(0x1086);
        let (width, height) = (2 * 8 * 256 + 13, 3);
        let mut target = Buffer::new_from_color(width, height, Color::new(255, 255, 255, 255));
        let current = random_buffer(&mut rng, width, height);
        rng.fill(&mut target.pixels_mut()[..64]);
        for alpha in [1, 128, 255] {
            let lines = random_lines(&mut rng, width as i32, height as i32);
            let expected = scalar::compute_color(&target, &current, &lines, alpha);
            // SAFETY: `target` and `current` have the same dimensions.
            let actual = unsafe { neon::compute_color(&target, &current, &lines, alpha) };
            assert_eq!(actual, expected, "alpha={alpha}");
        }
    }

    #[test]
    fn energy_from_lines_raw_matches_scalar_for_every_alpha() {
        let mut rng = ChaCha8Rng::seed_from_u64(0xe4e7);
        for (width, height) in sizes() {
            let target = random_buffer(&mut rng, width, height);
            let current = random_buffer(&mut rng, width, height);
            let score = scalar::difference_full_raw(&target, &current);
            for alpha in 1..=255 {
                let lines = random_lines(&mut rng, width as i32, height as i32);
                let color = Color::new(rng.random(), rng.random(), rng.random(), alpha);
                let expected =
                    scalar::energy_from_lines_raw(&target, &current, &lines, color, score);
                // SAFETY: `target` and `current` have the same dimensions.
                let actual =
                    unsafe { neon::energy_from_lines_raw(&target, &current, &lines, color, score) };
                assert_eq!(actual, expected, "{width}x{height} {color:?}");
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
            // SAFETY: `a` and `b` have the same dimensions.
            let actual = unsafe { neon::difference_full_raw(&a, &b) };
            assert_eq!(actual, expected, "{width}x{height}");
        }
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
