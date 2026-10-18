//! Compatibility exports for core-owned deterministic numeric rules.
//!
//! Native geometry keeps its established API without a second math kernel.
pub use minicon_core::numeric::{ceil_f32, round_f32, round_f64, trunc_f32};

#[cfg(test)]
mod tests {
    use super::*;

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
}
