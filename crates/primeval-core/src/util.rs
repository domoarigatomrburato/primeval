/// Converts degrees to radians.
#[must_use]
#[inline]
pub(crate) fn radians(degrees: f64) -> f64 {
    degrees * std::f64::consts::PI / 180.0
}

/// Converts radians to degrees.
#[must_use]
#[inline]
pub(crate) fn degrees(radians: f64) -> f64 {
    radians * 180.0 / std::f64::consts::PI
}

/// Rotates point `(x, y)` using pre-computed sine and cosine values.
///
/// Use this when the same angle is applied to many points to avoid
/// redundant trigonometric calls.
#[must_use]
#[inline]
pub(crate) fn rotate_sc(x: f64, y: f64, sin_t: f64, cos_t: f64) -> (f64, f64) {
    (x * cos_t - y * sin_t, x * sin_t + y * cos_t)
}

#[must_use]
pub(crate) fn system_clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radians_and_degrees_roundtrip() {
        let deg = 45.0;
        let rad = radians(deg);
        let back = degrees(rad);
        assert!((back - deg).abs() < 1e-12);
    }

    #[test]
    fn radians_known_values() {
        assert!((radians(180.0) - std::f64::consts::PI).abs() < 1e-12);
        assert!((radians(90.0) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn rotate_sc_identity() {
        let (sin_t, cos_t) = 0.0_f64.sin_cos();
        let (rx, ry) = rotate_sc(1.0, 0.0, sin_t, cos_t);
        assert!((rx - 1.0).abs() < 1e-12);
        assert!(ry.abs() < 1e-12);
    }

    #[test]
    fn rotate_sc_quarter_turn() {
        let (sin_t, cos_t) = std::f64::consts::FRAC_PI_2.sin_cos();
        let (rx, ry) = rotate_sc(1.0, 0.0, sin_t, cos_t);
        assert!(rx.abs() < 1e-12);
        assert!((ry - 1.0).abs() < 1e-12);
    }
}
