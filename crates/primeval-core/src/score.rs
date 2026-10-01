/// Scoring and blending routines for the energy minimization loop.
///
/// Integer arithmetic, truncation semantics, and accumulator widths match
/// the Go original for reproducibility. On aarch64, hot paths use NEON
/// intrinsics to process 8 pixels per iteration.
use crate::buffer::Buffer;
use crate::color::Color;
use crate::scanline::{clamp_line, Scanline};

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

#[inline]
fn div_by_m(value: u32) -> u32 {
    (value + 1 + (value >> 16)) >> 16
}

#[inline]
fn blend_channel_scalar(current: u8, source: u32, ma: u32, a: u32) -> u8 {
    let value = u32::from(current) * a + source * ma;
    (div_by_m(value) >> 8) as u8
}

mod scalar {
    use super::{blend_channel_scalar, clamp_line, Buffer, Color, Scanline, M};

    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    pub(super) fn compute_color(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        alpha: i32,
    ) -> Color {
        let w = target.width() as i32;
        let h = target.height() as i32;

        let mut rsum: i64 = 0;
        let mut gsum: i64 = 0;
        let mut bsum: i64 = 0;
        let mut count: i64 = 0;

        let a = 0x101_i64 * 255 / alpha as i64;

        let t_pix = target.pixels();
        let c_pix = current.pixels();

        for line in lines {
            let (x1, x2) = match clamp_line(line, w, h) {
                Some(v) => v,
                None => continue,
            };
            let mut i = target.pix_offset(x1, line.y);
            for _ in x1..=x2 {
                let tr = i64::from(t_pix[i]);
                let tg = i64::from(t_pix[i + 1]);
                let tb = i64::from(t_pix[i + 2]);
                let cr = i64::from(c_pix[i]);
                let cg = i64::from(c_pix[i + 1]);
                let cb = i64::from(c_pix[i + 2]);
                i += 4;
                rsum += (tr - cr) * a + cr * 0x101;
                gsum += (tg - cg) * a + cg * 0x101;
                bsum += (tb - cb) * a + cb * 0x101;
                count += 1;
            }
        }

        if count == 0 {
            return Color::default();
        }

        let r = ((rsum / count) as i32 >> 8).clamp(0, 255);
        let g = ((gsum / count) as i32 >> 8).clamp(0, 255);
        let b = ((bsum / count) as i32 >> 8).clamp(0, 255);

        Color::new(r as u8, g as u8, b as u8, alpha as u8)
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

                total -= (dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64;
                total += (dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64;
            }
        }

        total
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::{blend_channel_scalar, clamp_line, scalar, Buffer, Color, Scanline, M};
    use std::arch::aarch64::*;

    unsafe fn div_by_m_u32x4(value: uint32x4_t) -> uint32x4_t {
        let plus_one = vdupq_n_u32(1);
        let adjusted = vaddq_u32(vaddq_u32(value, plus_one), vshrq_n_u32(value, 16));
        vshrq_n_u32(adjusted, 16)
    }

    unsafe fn blend_vector_u8x8(current: uint8x8_t, source: u32, ma: u32, a: u32) -> uint8x8_t {
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

    #[cfg(test)]
    pub(super) unsafe fn blend_chunk_u8x8(
        current: [u8; 8],
        source: u32,
        ma: u32,
        a: u32,
    ) -> [u8; 8] {
        let current_vec = vld1_u8(current.as_ptr());
        let blended = blend_vector_u8x8(current_vec, source, ma, a);
        let mut out = [0_u8; 8];
        vst1_u8(out.as_mut_ptr(), blended);
        out
    }

    unsafe fn sum_squared_diff_u8x8(lhs: uint8x8_t, rhs: uint8x8_t) -> u64 {
        let lhs16 = vreinterpretq_s16_u16(vmovl_u8(lhs));
        let rhs16 = vreinterpretq_s16_u16(vmovl_u8(rhs));
        let diff = vsubq_s16(lhs16, rhs16);
        let low = vmull_s16(vget_low_s16(diff), vget_low_s16(diff));
        let high = vmull_s16(vget_high_s16(diff), vget_high_s16(diff));
        u64::from(vaddvq_u32(vreinterpretq_u32_s32(low)))
            + u64::from(vaddvq_u32(vreinterpretq_u32_s32(high)))
    }

    unsafe fn accumulate_color_channel(target: uint8x8_t, current: uint8x8_t, alpha: i32) -> i64 {
        let target16 = vreinterpretq_s16_u16(vmovl_u8(target));
        let current16_signed = vreinterpretq_s16_u16(vmovl_u8(current));
        let diff16 = vsubq_s16(target16, current16_signed);
        let current16 = vmovl_u8(current);

        let diff_low = vmovl_s16(vget_low_s16(diff16));
        let diff_high = vmovl_s16(vget_high_s16(diff16));
        let current_low = vmovl_u16(vget_low_u16(current16));
        let current_high = vmovl_u16(vget_high_u16(current16));

        let low = vaddq_s32(
            vmulq_n_s32(diff_low, alpha),
            vreinterpretq_s32_u32(vmulq_n_u32(current_low, 0x101)),
        );
        let high = vaddq_s32(
            vmulq_n_s32(diff_high, alpha),
            vreinterpretq_s32_u32(vmulq_n_u32(current_high, 0x101)),
        );

        i64::from(vaddvq_s32(low)) + i64::from(vaddvq_s32(high))
    }

    pub(super) unsafe fn compute_color(
        target: &Buffer,
        current: &Buffer,
        lines: &[Scanline],
        alpha: i32,
    ) -> Color {
        let w = target.width() as i32;
        let h = target.height() as i32;
        let mut rsum = 0_i64;
        let mut gsum = 0_i64;
        let mut bsum = 0_i64;
        let mut count = 0_i64;
        let weight = 0x101_i32 * 255 / alpha;

        let t_pix = target.pixels();
        let c_pix = current.pixels();

        for line in lines {
            let (x1, x2) = match clamp_line(line, w, h) {
                Some(v) => v,
                None => continue,
            };
            let pixel_count = (x2 - x1 + 1) as usize;
            let mut byte_index = target.pix_offset(x1, line.y);
            let chunk_pixels = pixel_count / 8;

            for _ in 0..chunk_pixels {
                let target_channels = vld4_u8(t_pix.as_ptr().add(byte_index));
                let current_channels = vld4_u8(c_pix.as_ptr().add(byte_index));

                rsum += accumulate_color_channel(target_channels.0, current_channels.0, weight);
                gsum += accumulate_color_channel(target_channels.1, current_channels.1, weight);
                bsum += accumulate_color_channel(target_channels.2, current_channels.2, weight);
                count += 8;
                byte_index += 32;
            }

            for _ in 0..(pixel_count % 8) {
                let tr = i64::from(t_pix[byte_index]);
                let tg = i64::from(t_pix[byte_index + 1]);
                let tb = i64::from(t_pix[byte_index + 2]);
                let cr = i64::from(c_pix[byte_index]);
                let cg = i64::from(c_pix[byte_index + 1]);
                let cb = i64::from(c_pix[byte_index + 2]);
                byte_index += 4;
                rsum += (tr - cr) * i64::from(weight) + cr * 0x101;
                gsum += (tg - cg) * i64::from(weight) + cg * 0x101;
                bsum += (tb - cb) * i64::from(weight) + cb * 0x101;
                count += 1;
            }
        }

        if count == 0 {
            return Color::default();
        }

        let r = ((rsum / count) as i32 >> 8).clamp(0, 255);
        let g = ((gsum / count) as i32 >> 8).clamp(0, 255);
        let b = ((bsum / count) as i32 >> 8).clamp(0, 255);

        Color::new(r as u8, g as u8, b as u8, alpha as u8)
    }

    pub(super) unsafe fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
        let a_pix = a.pixels();
        let b_pix = b.pixels();
        let chunk_bytes = a_pix.len() / 32 * 32;
        let mut total = 0_u64;
        let mut byte_index = 0_usize;

        while byte_index < chunk_bytes {
            let a_channels = vld4_u8(a_pix.as_ptr().add(byte_index));
            let b_channels = vld4_u8(b_pix.as_ptr().add(byte_index));
            total += sum_squared_diff_u8x8(a_channels.0, b_channels.0);
            total += sum_squared_diff_u8x8(a_channels.1, b_channels.1);
            total += sum_squared_diff_u8x8(a_channels.2, b_channels.2);
            total += sum_squared_diff_u8x8(a_channels.3, b_channels.3);
            byte_index += 32;
        }

        total + scalar::difference_full_raw_pixels(&a_pix[chunk_bytes..], &b_pix[chunk_bytes..])
    }

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
                let target_channels = vld4_u8(t_pix.as_ptr().add(byte_index));
                let current_channels = vld4_u8(c_pix.as_ptr().add(byte_index));

                total -= sum_squared_diff_u8x8(target_channels.0, current_channels.0);
                total -= sum_squared_diff_u8x8(target_channels.1, current_channels.1);
                total -= sum_squared_diff_u8x8(target_channels.2, current_channels.2);
                total -= sum_squared_diff_u8x8(target_channels.3, current_channels.3);

                let after_r = blend_vector_u8x8(current_channels.0, sr, ma, a);
                let after_g = blend_vector_u8x8(current_channels.1, sg, ma, a);
                let after_b = blend_vector_u8x8(current_channels.2, sb, ma, a);
                let after_a = blend_vector_u8x8(current_channels.3, sa, ma, a);

                total += sum_squared_diff_u8x8(target_channels.0, after_r);
                total += sum_squared_diff_u8x8(target_channels.1, after_g);
                total += sum_squared_diff_u8x8(target_channels.2, after_b);
                total += sum_squared_diff_u8x8(target_channels.3, after_a);
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

                total -= (dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64;
                total += (dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64;
            }
        }

        total
    }
}

/// Computes the optimal color for drawing `lines` onto `current` to
/// best approximate `target` at the given `alpha` level.
///
/// Returns a zero [`Color`] if no scanline pixels fall within bounds.
#[must_use]
pub fn compute_color(target: &Buffer, current: &Buffer, lines: &[Scanline], alpha: i32) -> Color {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { neon::compute_color(target, current, lines, alpha) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        scalar::compute_color(target, current, lines, alpha)
    }
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
pub fn draw_lines(im: &mut Buffer, c: Color, lines: &[Scanline]) {
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
pub fn difference_full_raw(a: &Buffer, b: &Buffer) -> u64 {
    assert_eq!(a.width(), b.width(), "difference_full: width mismatch");
    assert_eq!(a.height(), b.height(), "difference_full: height mismatch");

    #[cfg(target_arch = "aarch64")]
    {
        unsafe { neon::difference_full_raw(a, b) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        scalar::difference_full_raw(a, b)
    }
}

/// Computes the normalized root-mean-square difference between two buffers.
#[must_use]
pub fn difference_full(a: &Buffer, b: &Buffer) -> f64 {
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

            total -= (dr1 * dr1 + dg1 * dg1 + db1 * db1 + da1 * da1) as u64;
            total += (dr2 * dr2 + dg2 * dg2 + db2 * db2 + da2 * da2) as u64;
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

/// Fused replacement for [`copy_and_draw_lines`] + [`difference_partial`].
///
/// Computes the blended pixel for each covered scanline pixel on the fly
/// (no write to any intermediate buffer) and accumulates the squared-difference
/// update in a single pass. This halves memory traffic compared to the
/// two-pass approach used outside the hot energy-evaluation loop.
#[must_use]
pub fn energy_from_lines_raw(
    target: &Buffer,
    current: &Buffer,
    lines: &[Scanline],
    color: Color,
    score: u64,
) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
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

    #[test]
    fn compute_color_partial_alpha() {
        // Test with alpha < 255 to exercise the weighting
        let mut target = Buffer::new(2, 1);
        let tp = target.pixels_mut();
        // pixel (0,0) = (200, 100, 50, 255)
        tp[0] = 200;
        tp[1] = 100;
        tp[2] = 50;
        tp[3] = 255;
        // pixel (1,0) = (100, 200, 150, 255)
        tp[4] = 100;
        tp[5] = 200;
        tp[6] = 150;
        tp[7] = 255;

        let current = Buffer::new(2, 1);
        let lines = [Scanline {
            y: 0,
            x1: 0,
            x2: 1,
            alpha: 0xFFFF,
        }];

        let c = compute_color(&target, &current, &lines, 128);
        assert_eq!(c.a, 128);
        // The computed RGB values should be nonzero and within bounds
        assert!(c.r > 0);
        assert!(c.g > 0);
        assert!(c.b > 0);
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
}
