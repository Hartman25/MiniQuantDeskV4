//! IR7 (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01): decimal-exact score
//! representation.
//!
//! The old approach (`raw_f64 * 1e6` then `round()`) is not an exact
//! representation of a JSON artifact's numeric token: two distinct decimal
//! values (e.g. `0.1234561` and `0.1234564`) can round to the same micros
//! integer and be silently treated as a tie, and a value with more than six
//! fractional digits loses precision the artifact actually contained.
//!
//! This module works entirely on the *exact textual token* the caller
//! (`mqk-daemon`, which alone touches the filesystem/JSON) extracted from
//! the review artifact — never a float. It never parses JSON itself and has
//! no I/O; it stays consistent with this crate's zero-dependency, pure
//! convention.
//!
//! [`canonicalize_decimal_token`] turns a raw JSON-number-grammar token into
//! a canonical decimal string (minimal exact form: no leading zeros, no
//! trailing fractional zeros, no exponent, no `+`). [`compare_canonical_decimal_strings`]
//! compares two such canonical strings without ever going through a float.
//! [`canonical_decimal_to_micros_if_exact`] is a convenience bridge back to
//! the legacy scaled-integer (1e-6) representation, returning `None` when
//! the exact value is not representable at that scale (more than six
//! fractional digits) rather than silently truncating it.

use std::cmp::Ordering;

/// Reject rather than build an unbounded string for a token with more
/// significant digits or decimal-point shift than this. Ordinary scanner
/// scores never approach this bound; it exists purely so a malformed or
/// adversarial artifact fails closed instead of allocating without limit.
const MAX_SIGNIFICANT_DIGITS: usize = 40;
const MAX_POINT_SHIFT: i64 = 1_000;

/// Parse a raw JSON-number-grammar token (`-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`)
/// into its canonical exact decimal string: sign only if negative and
/// non-zero, integer part with no leading zeros (`"0"` alone for zero),
/// optional `.` + fractional digits with no trailing zeros. Returns `None`
/// for a token that is not syntactically a valid JSON number, or whose
/// magnitude/precision this function declines to support (see
/// [`MAX_SIGNIFICANT_DIGITS`]/[`MAX_POINT_SHIFT`]) — never truncates or
/// approximates silently.
pub fn canonicalize_decimal_token(token: &str) -> Option<String> {
    let bytes = token.as_bytes();
    let mut i = 0usize;

    let negative = if bytes.first() == Some(&b'-') {
        i += 1;
        true
    } else {
        false
    };

    let int_start = i;
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    if bytes[i] == b'0' {
        i += 1;
    } else {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let int_part = &token[int_start..i];

    let mut frac_part = "";
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        if i >= bytes.len() || !bytes[i].is_ascii_digit() {
            return None;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        frac_part = &token[frac_start..i];
    }

    let mut exponent: i64 = 0;
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        let mut exp_neg = false;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            exp_neg = bytes[i] == b'-';
            i += 1;
        }
        let exp_start = i;
        if i >= bytes.len() || !bytes[i].is_ascii_digit() {
            return None;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let exp_val: i64 = token[exp_start..i].parse().ok()?;
        exponent = if exp_neg { -exp_val } else { exp_val };
    }

    if i != bytes.len() {
        // Trailing garbage after a syntactically complete number -- not a
        // valid JSON number token.
        return None;
    }

    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);

    // Decimal point sits `point_pos` digits from the left of `digits`,
    // before applying the exponent shift.
    let point_pos = int_part.len() as i64 + exponent;

    if digits.len() > MAX_SIGNIFICANT_DIGITS || point_pos.unsigned_abs() > MAX_POINT_SHIFT as u64 {
        return None;
    }

    let (out_int, out_frac) = if point_pos <= 0 {
        let zeros = (-point_pos) as usize;
        (
            String::from("0"),
            format!("{}{}", "0".repeat(zeros), digits),
        )
    } else {
        let p = point_pos as usize;
        if p >= digits.len() {
            (
                format!("{}{}", digits, "0".repeat(p - digits.len())),
                String::new(),
            )
        } else {
            (digits[..p].to_string(), digits[p..].to_string())
        }
    };

    let int_trimmed = out_int.trim_start_matches('0');
    let int_final = if int_trimmed.is_empty() {
        "0"
    } else {
        int_trimmed
    };
    let frac_final = out_frac.trim_end_matches('0');

    let is_zero = int_final == "0" && frac_final.is_empty();

    let mut result = String::with_capacity(int_final.len() + frac_final.len() + 2);
    if negative && !is_zero {
        result.push('-');
    }
    result.push_str(int_final);
    if !frac_final.is_empty() {
        result.push('.');
        result.push_str(frac_final);
    }
    Some(result)
}

fn split_sign(s: &str) -> (bool, &str) {
    match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

fn split_point(s: &str) -> (&str, &str) {
    match s.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (s, ""),
    }
}

fn compare_magnitude(a: &str, b: &str) -> Ordering {
    let (a_int, a_frac) = split_point(a);
    let (b_int, b_frac) = split_point(b);

    match a_int.len().cmp(&b_int.len()) {
        Ordering::Equal => {}
        other => return other,
    }
    match a_int.cmp(b_int) {
        Ordering::Equal => {}
        other => return other,
    }

    let max_frac_len = a_frac.len().max(b_frac.len());
    let a_bytes = a_frac.as_bytes();
    let b_bytes = b_frac.as_bytes();
    for idx in 0..max_frac_len {
        let av = a_bytes.get(idx).copied().unwrap_or(b'0');
        let bv = b_bytes.get(idx).copied().unwrap_or(b'0');
        match av.cmp(&bv) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// Compare two canonical decimal strings (as produced by
/// [`canonicalize_decimal_token`] only — this is a caller contract, not a
/// re-validation) exactly, without ever converting through a float.
pub fn compare_canonical_decimal_strings(a: &str, b: &str) -> Ordering {
    let (a_neg, a_body) = split_sign(a);
    let (b_neg, b_body) = split_sign(b);
    if a_neg != b_neg {
        return if a_neg {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let mag = compare_magnitude(a_body, b_body);
    if a_neg {
        mag.reverse()
    } else {
        mag
    }
}

/// Bridge a canonical decimal string back to the legacy scaled-integer
/// (1e-6 / micros) representation, but only when doing so is exact —
/// i.e. the value has at most six fractional digits. Returns `None`
/// (never a truncated/rounded value) when the value has more precision
/// than the scale supports, or the scaled magnitude would overflow `i64`.
pub fn canonical_decimal_to_micros_if_exact(s: &str) -> Option<i64> {
    let (negative, body) = split_sign(s);
    let (int_part, frac_part) = split_point(body);
    if frac_part.len() > 6 {
        return None;
    }
    let mut digits = String::with_capacity(int_part.len() + 6);
    digits.push_str(int_part);
    digits.push_str(frac_part);
    digits.push_str(&"0".repeat(6 - frac_part.len()));
    let magnitude: i64 = digits.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_plain_integers_and_decimals() {
        assert_eq!(canonicalize_decimal_token("9").as_deref(), Some("9"));
        assert_eq!(canonicalize_decimal_token("9.0").as_deref(), Some("9"));
        assert_eq!(canonicalize_decimal_token("0.5").as_deref(), Some("0.5"));
        assert_eq!(
            canonicalize_decimal_token("-1.25").as_deref(),
            Some("-1.25")
        );
        assert_eq!(canonicalize_decimal_token("0").as_deref(), Some("0"));
        assert_eq!(canonicalize_decimal_token("-0").as_deref(), Some("0"));
        assert_eq!(canonicalize_decimal_token("0.0").as_deref(), Some("0"));
    }

    #[test]
    fn canonicalizes_trailing_and_leading_zeros() {
        assert_eq!(canonicalize_decimal_token("1.50").as_deref(), Some("1.5"));
        assert_eq!(
            canonicalize_decimal_token("1.500000").as_deref(),
            Some("1.5")
        );
        assert_eq!(canonicalize_decimal_token("100").as_deref(), Some("100"));
    }

    #[test]
    fn exponent_forms_normalize_to_the_same_canonical_value() {
        let a = canonicalize_decimal_token("150").unwrap();
        let b = canonicalize_decimal_token("1.5e2").unwrap();
        let c = canonicalize_decimal_token("1500e-1").unwrap();
        let d = canonicalize_decimal_token("15E1").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
        assert_eq!(a, "150");
    }

    #[test]
    fn negative_exponent_produces_fractional_canonical_form() {
        assert_eq!(
            canonicalize_decimal_token("1.23e-2").as_deref(),
            Some("0.0123")
        );
    }

    #[test]
    fn rejects_non_numeric_tokens() {
        assert_eq!(canonicalize_decimal_token(""), None);
        assert_eq!(canonicalize_decimal_token("abc"), None);
        assert_eq!(canonicalize_decimal_token("01"), None); // JSON forbids leading zeros
        assert_eq!(canonicalize_decimal_token("1."), None);
        assert_eq!(canonicalize_decimal_token(".5"), None);
        assert_eq!(canonicalize_decimal_token("1e"), None);
        assert_eq!(canonicalize_decimal_token("NaN"), None);
        assert_eq!(canonicalize_decimal_token("Infinity"), None);
        assert_eq!(canonicalize_decimal_token("1.5 "), None);
        assert_eq!(canonicalize_decimal_token("1,5"), None);
    }

    #[test]
    fn two_distinct_values_that_would_collapse_under_micros_rounding_stay_distinct() {
        // Both of these round to the same micros integer (500000) under the
        // old `f64 * 1e6 -> round()` scheme -- IR7's whole point is that the
        // canonical decimal path must never treat them as equal.
        let a = canonicalize_decimal_token("0.1234561").unwrap();
        let b = canonicalize_decimal_token("0.1234564").unwrap();
        assert_ne!(a, b);
        assert_eq!(compare_canonical_decimal_strings(&a, &b), Ordering::Less);
        assert_eq!(canonical_decimal_to_micros_if_exact(&a), None);
        assert_eq!(canonical_decimal_to_micros_if_exact(&b), None);
    }

    #[test]
    fn compare_matches_numeric_ordering_across_signs_and_magnitudes() {
        let cases: &[(&str, &str, Ordering)] = &[
            ("1", "2", Ordering::Less),
            ("2", "1", Ordering::Greater),
            ("1", "1", Ordering::Equal),
            ("-1", "1", Ordering::Less),
            ("1", "-1", Ordering::Greater),
            ("-2", "-1", Ordering::Less),
            ("-1", "-2", Ordering::Greater),
            ("0.5", "0.500", Ordering::Equal),
            ("0.5", "0.6", Ordering::Less),
            ("10", "9", Ordering::Greater),
            ("0.09", "0.1", Ordering::Less),
            ("-0.1", "-0.09", Ordering::Less),
        ];
        for (a_raw, b_raw, expected) in cases {
            let a = canonicalize_decimal_token(a_raw).unwrap();
            let b = canonicalize_decimal_token(b_raw).unwrap();
            assert_eq!(
                compare_canonical_decimal_strings(&a, &b),
                *expected,
                "comparing {a_raw} vs {b_raw}"
            );
        }
    }

    #[test]
    fn micros_bridge_matches_legacy_expectations_for_exact_values() {
        assert_eq!(
            canonical_decimal_to_micros_if_exact(&canonicalize_decimal_token("9.0").unwrap()),
            Some(9_000_000)
        );
        assert_eq!(
            canonical_decimal_to_micros_if_exact(&canonicalize_decimal_token("0.5").unwrap()),
            Some(500_000)
        );
        assert_eq!(
            canonical_decimal_to_micros_if_exact(&canonicalize_decimal_token("-1.25").unwrap()),
            Some(-1_250_000)
        );
        assert_eq!(
            canonical_decimal_to_micros_if_exact(&canonicalize_decimal_token("0").unwrap()),
            Some(0)
        );
    }

    #[test]
    fn micros_bridge_refuses_when_not_exactly_representable() {
        let seven_frac_digits = canonicalize_decimal_token("0.1234567").unwrap();
        assert_eq!(
            canonical_decimal_to_micros_if_exact(&seven_frac_digits),
            None
        );
    }

    #[test]
    fn rejects_absurdly_long_or_shifted_tokens() {
        let too_many_digits = "1".repeat(MAX_SIGNIFICANT_DIGITS + 1);
        assert_eq!(canonicalize_decimal_token(&too_many_digits), None);
        assert_eq!(canonicalize_decimal_token("1e100000"), None);
    }
}
