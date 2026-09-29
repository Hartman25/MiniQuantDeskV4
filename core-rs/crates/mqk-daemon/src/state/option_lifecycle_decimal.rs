//! Exact provider-decimal parsing for options-lifecycle evidence.
//!
//! Provider quantities/prices/cash arrive as decimal strings. Every value is
//! converted to the repo's 1e-6 fixed-point scale exactly, or refused:
//! malformed text, more than six fractional digits, and `i64` overflow are all
//! errors -- never truncated, rounded, wrapped or defaulted to zero.

use mqk_portfolio::{canonical_decimal_to_micros_if_exact, canonicalize_decimal_token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalParseError {
    /// Not a valid decimal token.
    Malformed,
    /// Valid, but not exactly representable at 1e-6 scale in an `i64`.
    NotExactOrOverflow,
}

pub fn parse_exact_micros(raw: &str) -> Result<i64, DecimalParseError> {
    let canonical = canonicalize_decimal_token(raw.trim()).ok_or(DecimalParseError::Malformed)?;
    canonical_decimal_to_micros_if_exact(&canonical).ok_or(DecimalParseError::NotExactOrOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_round_trip() {
        assert_eq!(parse_exact_micros("200"), Ok(200_000_000));
        assert_eq!(parse_exact_micros("-30000"), Ok(-30_000_000_000));
        assert_eq!(parse_exact_micros("0.000001"), Ok(1));
        assert_eq!(parse_exact_micros(" 150.50 "), Ok(150_500_000));
        assert_eq!(parse_exact_micros("-0"), Ok(0));
    }

    #[test]
    fn inexact_malformed_and_overflowing_values_are_refused() {
        assert_eq!(
            parse_exact_micros("0.0000001"),
            Err(DecimalParseError::NotExactOrOverflow)
        );
        assert_eq!(
            parse_exact_micros("99999999999999999999"),
            Err(DecimalParseError::NotExactOrOverflow)
        );
        for bad in ["", "abc", "1..2", "--1", "1e", "NaN"] {
            assert_eq!(
                parse_exact_micros(bad),
                Err(DecimalParseError::Malformed),
                "{bad:?}"
            );
        }
    }
}
