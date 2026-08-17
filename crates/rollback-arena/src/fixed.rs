//! Fixed-point scalars.
//!
//! The arena has no floats anywhere. Not because integers are faster here --
//! they are not, at this scale -- but because `f32` addition is only
//! *mostly* reproducible across machines: compilers are free to contract
//! `a * b + c` into an FMA, x87 has excess precision, and `-ffast-math`-style
//! reassociation is legal in LLVM under some flags. A single one-ULP difference
//! on one peer is a desync. Integers have none of those degrees of freedom.
//!
//! The format is Q23.8: one `i32`, eight fractional bits, so the unit is 1/256
//! of a pixel and the range is roughly ±8.4 million pixels.

/// 1.0 in fixed point.
pub const ONE: i32 = 256;
/// Fractional bits.
pub const SHIFT: u32 = 8;

/// Convert whole pixels to fixed point.
pub const fn from_px(px: i32) -> i32 {
    px * ONE
}

/// Truncate fixed point to whole pixels, rounding toward negative infinity.
pub const fn to_px(v: i32) -> i32 {
    v >> SHIFT
}

/// Fixed-point multiply.
///
/// Goes through `i64` so the intermediate product cannot overflow, then shifts
/// back down. The shift is arithmetic, so it rounds toward negative infinity
/// for negative values -- consistently, on every platform, which is the only
/// property that matters.
pub const fn mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> SHIFT) as i32
}

/// Fixed-point divide. Dividing by zero yields 0 rather than trapping: a
/// desync is bad, but a peer that panics mid-session is worse.
pub const fn div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << SHIFT) / (b as i64)) as i32
}

/// Absolute value that cannot panic on `i32::MIN`.
pub const fn abs(v: i32) -> i32 {
    if v < 0 {
        v.saturating_neg()
    } else {
        v
    }
}

/// Clamp `v` into `[lo, hi]`.
pub const fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Sign of `v` as -1, 0 or 1.
pub const fn signum(v: i32) -> i32 {
    if v > 0 {
        1
    } else if v < 0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_conversions_round_trip_on_whole_pixels() {
        for px in -1000..1000 {
            assert_eq!(to_px(from_px(px)), px);
        }
    }

    #[test]
    fn mul_matches_rational_arithmetic() {
        assert_eq!(mul(from_px(3), from_px(4)), from_px(12));
        assert_eq!(mul(from_px(5), ONE / 2), from_px(5) / 2);
        assert_eq!(mul(-from_px(6), ONE / 2), -from_px(3));
    }

    #[test]
    fn mul_does_not_overflow_at_large_magnitudes() {
        // Would wrap if the intermediate product stayed in i32.
        let big = from_px(30_000);
        assert_eq!(mul(big, ONE), big);
    }

    #[test]
    fn div_is_the_inverse_of_mul_for_exact_values() {
        assert_eq!(div(from_px(12), from_px(4)), from_px(3));
        assert_eq!(div(from_px(1), 0), 0, "division by zero must not panic");
    }

    #[test]
    fn abs_survives_i32_min() {
        assert_eq!(abs(i32::MIN), i32::MAX);
        assert_eq!(abs(-5), 5);
        assert_eq!(abs(5), 5);
    }

    #[test]
    fn clamp_and_signum_behave() {
        assert_eq!(clamp(10, 0, 5), 5);
        assert_eq!(clamp(-10, 0, 5), 0);
        assert_eq!(clamp(3, 0, 5), 3);
        assert_eq!((signum(-9), signum(0), signum(9)), (-1, 0, 1));
    }
}
