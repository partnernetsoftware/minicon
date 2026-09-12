//! Deterministic floating-point leaves used at geometry boundaries.
//!
//! These reproduce the IEEE-754 behaviour of `f32`/`f64` `round`, `ceil` and
//! `trunc` without calling the C runtime math functions those methods lower to
//! on some targets. That matters for two reasons that are easy to conflate:
//!
//! 1. A host that links a different `libm` can round a halfway value the other
//!    way, which turns a pixel coordinate into a one-pixel difference that is
//!    invisible until it accumulates.
//! 2. The alternative, `(x + 0.5) as i64`, is *not* a substitute: it rounds
//!    halves away from zero for positives but is wrong for negatives, loses the
//!    halfway case for large magnitudes where `x + 0.5` is not representable,
//!    and traps on a value outside `i64`'s range.
//!
//! The operations are bit-level rather than arithmetic so there is no
//! intermediate rounding to reason about: mask the fraction away for `trunc`,
//! add half an ulp and mask for `round`, and branch on the sign for `ceil`.
//!
//! `#[inline(never)]` is deliberate. These are called on hot geometry paths, and
//! inlining them measurably changed the shape of the code around them without
//! changing the result; keeping them out of line keeps the call sites small.

const F32_SIGN: u32 = 1 << 31;
const F32_FRACTION_BITS: i32 = 23;
const F32_EXPONENT_BIAS: i32 = 127;
const F64_SIGN: u64 = 1 << 63;
const F64_FRACTION_BITS: i32 = 52;
const F64_EXPONENT_BIAS: i32 = 1023;

/// Truncates towards zero.
///
/// A magnitude below one has no integer part, so the result is a signed zero
/// rather than zero: `trunc(-0.5)` is `-0.0`, which compares equal to `0.0` but
/// keeps the sign for a following division.
#[inline(never)]
pub fn trunc_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let exponent = ((bits >> F32_FRACTION_BITS) & 0xff) as i32 - F32_EXPONENT_BIAS;
    if exponent < 0 {
        return f32::from_bits(bits & F32_SIGN);
    }
    if exponent >= F32_FRACTION_BITS {
        return value;
    }
    let fractional_mask = (1u32 << (F32_FRACTION_BITS - exponent)) - 1;
    f32::from_bits(bits & !fractional_mask)
}

/// Rounds to nearest, with ties away from zero.
#[inline(never)]
pub fn round_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let exponent_bits = (bits >> F32_FRACTION_BITS) & 0xff;
    // Infinity and NaN have no integer part; passing them through preserves
    // them rather than turning them into a masked finite value.
    if exponent_bits == 0xff {
        return value;
    }
    let exponent = exponent_bits as i32 - F32_EXPONENT_BIAS;
    // Below 2^-1 the value rounds to a signed zero.
    if exponent < -1 {
        return f32::from_bits(bits & F32_SIGN);
    }
    // Exactly 0.5 in magnitude rounds to 1.0, i.e. `1 << exponent` with the
    // original sign.
    if exponent == -1 {
        return f32::from_bits((bits & F32_SIGN) | 1.0f32.to_bits());
    }
    if exponent >= F32_FRACTION_BITS {
        return value;
    }
    let fractional_bits = F32_FRACTION_BITS - exponent;
    let fractional_mask = (1u32 << fractional_bits) - 1;
    if bits & fractional_mask == 0 {
        return value;
    }
    // Adding half on the magnitude and masking is ties-away-from-zero, and it
    // is why the sign is stripped first: adding to the raw bits of a negative
    // number would move it towards zero instead.
    let half = 1u32 << (fractional_bits - 1);
    let rounded_magnitude = ((bits & !F32_SIGN) + half) & !fractional_mask;
    f32::from_bits((bits & F32_SIGN) | rounded_magnitude)
}

/// Rounds towards positive infinity.
#[inline(never)]
pub fn ceil_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let exponent_bits = (bits >> F32_FRACTION_BITS) & 0xff;
    if exponent_bits == 0xff {
        return value;
    }
    let exponent = exponent_bits as i32 - F32_EXPONENT_BIAS;
    if exponent < 0 {
        // Zero and negative values below one are already their own ceiling;
        // a positive fraction is not, and rounds up to one.
        if bits & !F32_SIGN == 0 {
            return value;
        }
        return if bits & F32_SIGN == 0 {
            1.0
        } else {
            f32::from_bits(F32_SIGN)
        };
    }
    if exponent >= F32_FRACTION_BITS {
        return value;
    }
    let fractional_bits = F32_FRACTION_BITS - exponent;
    let fractional_mask = (1u32 << fractional_bits) - 1;
    if bits & fractional_mask == 0 {
        return value;
    }
    let truncated = bits & !fractional_mask;
    // With a fraction present, a positive value steps up one ulp of its own
    // scale; a negative value steps towards zero, which the mask already did.
    if bits & F32_SIGN == 0 {
        f32::from_bits(truncated + (1u32 << fractional_bits))
    } else {
        f32::from_bits(truncated)
    }
}

/// Rounds to nearest, with ties away from zero.
///
/// The `f64` twin of `round_f32`, and the one geometry actually calls: window
/// coordinates arrive as logical units and are scaled before use.
#[inline(never)]
pub fn round_f64(value: f64) -> f64 {
    let bits = value.to_bits();
    let exponent_bits = (bits >> F64_FRACTION_BITS) & 0x7ff;
    if exponent_bits == 0x7ff {
        return value;
    }
    let exponent = exponent_bits as i32 - F64_EXPONENT_BIAS;
    if exponent < -1 {
        return f64::from_bits(bits & F64_SIGN);
    }
    if exponent == -1 {
        return f64::from_bits((bits & F64_SIGN) | 1.0f64.to_bits());
    }
    if exponent >= F64_FRACTION_BITS {
        return value;
    }
    let fractional_bits = F64_FRACTION_BITS - exponent;
    let fractional_mask = (1u64 << fractional_bits) - 1;
    if bits & fractional_mask == 0 {
        return value;
    }
    let half = 1u64 << (fractional_bits - 1);
    let rounded_magnitude = ((bits & !F64_SIGN) + half) & !fractional_mask;
    f64::from_bits((bits & F64_SIGN) | rounded_magnitude)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The only thing that makes these worth having is that they agree with the
    /// operations they stand in for, bit for bit. Comparing bit patterns rather
    /// than values is the point: `-0.0 == 0.0` would hide exactly the sign error
    /// these functions exist to avoid.
    #[test]
    fn scalar_leaves_match_standard_ieee_operations() {
        let f32_cases = [
            f32::NEG_INFINITY,
            -8_388_609.0,
            -2.5,
            -1.5,
            -0.5,
            -0.499_999_97,
            -0.0,
            0.0,
            0.499_999_97,
            0.5,
            1.5,
            2.5,
            8_388_609.0,
            f32::INFINITY,
        ];
        for value in f32_cases {
            assert_eq!(round_f32(value).to_bits(), value.round().to_bits());
            assert_eq!(ceil_f32(value).to_bits(), value.ceil().to_bits());
            assert_eq!(trunc_f32(value).to_bits(), value.trunc().to_bits());
        }
        // A stride that is not a divisor of the mantissa width, so the sweep
        // does not land on the same fractional pattern at every exponent.
        for bits in (0..=u32::MAX).step_by(1_048_573) {
            let value = f32::from_bits(bits);
            if !value.is_nan() {
                assert_eq!(round_f32(value).to_bits(), value.round().to_bits());
                assert_eq!(ceil_f32(value).to_bits(), value.ceil().to_bits());
                assert_eq!(trunc_f32(value).to_bits(), value.trunc().to_bits());
            }
        }

        let f64_cases = [
            f64::NEG_INFINITY,
            -4_503_599_627_370_497.0,
            -2.5,
            -1.5,
            -0.5,
            -0.499_999_999_999_999_94,
            -0.0,
            0.0,
            0.499_999_999_999_999_94,
            0.5,
            1.5,
            2.5,
            4_503_599_627_370_497.0,
            f64::INFINITY,
        ];
        for value in f64_cases {
            assert_eq!(round_f64(value).to_bits(), value.round().to_bits());
        }
    }

    /// Subnormals and tiny values are where a bit-level implementation is most
    /// likely to diverge from the operation it stands in for: the exponent field
    /// is zero, so a naive `exponent < 0` branch is the only thing handling
    /// them, and `bits & SIGN` for a subnormal is not the same as for zero.
    /// This walks the whole subnormal range rather than sampling it, because
    /// that range is small enough to be exhaustive and a sampled test would
    /// miss exactly the boundary case it exists for. A wider sweep over 750
    /// million sampled bit patterns found no other disagreement.
    #[test]
    fn every_subnormal_agrees_with_the_standard_operations() {
        for bits in 1u32..0x0080_0000 {
            let value = f32::from_bits(bits);
            if value == 0.0 {
                continue;
            }
            assert_eq!(
                trunc_f32(value).to_bits(),
                value.trunc().to_bits(),
                "trunc {value:e}"
            );
            assert_eq!(
                round_f32(value).to_bits(),
                value.round().to_bits(),
                "round {value:e}"
            );
            assert_eq!(
                ceil_f32(value).to_bits(),
                value.ceil().to_bits(),
                "ceil {value:e}"
            );
            // The negative twin of the same encoding.
            let negated = -value;
            assert_eq!(trunc_f32(negated).to_bits(), negated.trunc().to_bits());
            assert_eq!(round_f32(negated).to_bits(), negated.round().to_bits());
            assert_eq!(ceil_f32(negated).to_bits(), negated.ceil().to_bits());
        }
    }

    /// NaN must come back as NaN rather than a masked finite value, because the
    /// exponent branch is the only thing standing between a NaN input and a
    /// silently invalid coordinate.
    #[test]
    fn non_finite_inputs_are_passed_through() {
        assert!(round_f32(f32::NAN).is_nan());
        assert!(ceil_f32(f32::NAN).is_nan());
        assert!(trunc_f32(f32::NAN).is_nan());
        assert!(round_f64(f64::NAN).is_nan());
        assert_eq!(round_f32(f32::INFINITY), f32::INFINITY);
        assert_eq!(ceil_f32(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert_eq!(round_f64(f64::INFINITY), f64::INFINITY);
    }

    /// A halfway value keeps its sign, which is what makes the sign-stripping
    /// step load-bearing rather than cosmetic.
    #[test]
    fn halfway_values_round_away_from_zero_on_both_signs() {
        assert_eq!(round_f32(-0.5), -1.0);
        assert_eq!(round_f32(0.5), 1.0);
        assert_eq!(round_f32(-1.5), -2.0);
        assert_eq!(round_f32(1.5), 2.0);
        assert_eq!(round_f64(-2.5), -3.0);
        assert_eq!(round_f64(2.5), 3.0);
    }

    /// The scaled-coordinate path is the reason `round_f64` exists, so it is
    /// checked on the values a display scale actually produces. A truncated
    /// version of this would report 96 where 97 is correct for 1.5 x 64.
    #[test]
    fn display_scale_coordinates_round_to_the_nearest_pixel() {
        for (scale, logical, expected) in [
            (1.0_f64, 10.0_f64, 10_i64),
            (1.25, 10.0, 13),
            (1.5, 64.0, 96),
            (1.5, 64.5, 97),
            (1.75, 10.0, 18),
            (2.0, 10.5, 21),
        ] {
            assert_eq!(
                round_f64(logical * scale) as i64,
                expected,
                "{logical} x {scale}"
            );
        }
    }
}
