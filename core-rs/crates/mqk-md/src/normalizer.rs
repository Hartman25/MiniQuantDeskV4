//! Canonical OHLCV normalization for market-data bars.
//!
//! This module converts raw provider bars (`provider::RawBar`) into
//! `NormalizedBar` values with integer-micro prices, validated OHLC
//! relationships, and deterministic sort order.
//!
//! It does **not**:
//! - fetch data (no providers)
//! - write to the database
//! - perform data-quality reporting (that is `quality.rs`)

use std::fmt;

use crate::provider::RawBar;

// ---------------------------------------------------------------------------
// Output type
// ---------------------------------------------------------------------------

/// A fully normalized OHLCV bar ready for downstream storage.
///
/// Prices are stored as integer micros (1 USD = 1_000_000 micros) to avoid
/// floating-point rounding at any later stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBar {
    pub symbol: String,
    /// Canonical timeframe string (e.g. `"1D"`, `"1m"`, `"5m"`).
    pub timeframe: String,
    /// Bar end timestamp as UTC epoch seconds.
    pub end_ts: i64,
    /// Opening price in micros (1/1_000_000 of the base currency unit).
    pub open_micros: i64,
    /// High price in micros.
    pub high_micros: i64,
    /// Low price in micros.
    pub low_micros: i64,
    /// Closing price in micros.
    pub close_micros: i64,
    /// Trade volume (integer shares / contracts).
    pub volume: i64,
    /// `true` when the bar period has fully closed.
    pub is_complete: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced during normalization.
#[derive(Debug, PartialEq, Eq)]
pub enum NormalizerError {
    /// A price string was empty.
    EmptyPrice { field: &'static str },
    /// A price string could not be parsed as a decimal number.
    InvalidPrice { field: &'static str, raw: String },
    /// A price had more than 6 decimal places (ambiguous micro conversion).
    TooManyDecimalPlaces { field: &'static str, raw: String },
    /// OHLC sanity check failed.
    OhlcViolation(String),
    /// Volume is negative.
    NegativeVolume(i64),
}

impl fmt::Display for NormalizerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NormalizerError::EmptyPrice { field } => {
                write!(f, "price field '{field}' is empty")
            }
            NormalizerError::InvalidPrice { field, raw } => {
                write!(f, "price field '{field}' could not be parsed: '{raw}'")
            }
            NormalizerError::TooManyDecimalPlaces { field, raw } => {
                write!(
                    f,
                    "price field '{field}' has more than 6 decimal places \
                     (ambiguous micro conversion): '{raw}'"
                )
            }
            NormalizerError::OhlcViolation(msg) => {
                write!(f, "OHLC sanity violation: {msg}")
            }
            NormalizerError::NegativeVolume(v) => {
                write!(f, "volume must be >= 0, got {v}")
            }
        }
    }
}

impl std::error::Error for NormalizerError {}

// ---------------------------------------------------------------------------
// Price conversion
// ---------------------------------------------------------------------------

/// Convert a decimal price string to integer micros deterministically.
///
/// Rules:
/// - Accepts optional leading `+` or `-`.
/// - Accepts an optional fractional part separated by `.`.
/// - Rejects strings with more than 6 decimal places (would require rounding).
/// - Rejects empty strings, non-numeric characters, or multiple `.` separators.
/// - Does **not** use floating-point at any stage.
pub fn price_to_micros(s: &str, field: &'static str) -> Result<i64, NormalizerError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(NormalizerError::EmptyPrice { field });
    }

    // Handle optional sign.
    let (negative, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    if digits.is_empty() {
        return Err(NormalizerError::InvalidPrice {
            field,
            raw: s.to_string(),
        });
    }

    // Split on '.'.
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((i, f)) => (i, f),
        None => (digits, ""),
    };

    // Reject anything that is not pure ASCII digits in either part.
    let all_digits = |p: &str| p.chars().all(|c| c.is_ascii_digit());
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(NormalizerError::InvalidPrice {
            field,
            raw: s.to_string(),
        });
    }
    if !all_digits(int_part) || !all_digits(frac_part) {
        return Err(NormalizerError::InvalidPrice {
            field,
            raw: s.to_string(),
        });
    }

    // Reject more than 6 decimal places (would require rounding).
    if frac_part.len() > 6 {
        return Err(NormalizerError::TooManyDecimalPlaces {
            field,
            raw: s.to_string(),
        });
    }

    // Parse integer part.
    let int_val: i64 = if int_part.is_empty() {
        0
    } else {
        int_part
            .parse::<i64>()
            .map_err(|_| NormalizerError::InvalidPrice {
                field,
                raw: s.to_string(),
            })?
    };

    // Pad fractional part to exactly 6 digits, then parse.
    let mut frac_padded = frac_part.to_string();
    while frac_padded.len() < 6 {
        frac_padded.push('0');
    }
    let frac_val: i64 = frac_padded
        .parse::<i64>()
        .map_err(|_| NormalizerError::InvalidPrice {
            field,
            raw: s.to_string(),
        })?;

    let micros = int_val
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(frac_val))
        .ok_or_else(|| NormalizerError::InvalidPrice {
            field,
            raw: s.to_string(),
        })?;

    Ok(if negative { -micros } else { micros })
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Normalize a single [`RawBar`] into a [`NormalizedBar`].
///
/// Returns `Err` if any price cannot be converted deterministically or if
/// OHLC sanity checks fail.
pub fn normalize(bar: &RawBar) -> Result<NormalizedBar, NormalizerError> {
    let open_micros = price_to_micros(&bar.open, "open")?;
    let high_micros = price_to_micros(&bar.high, "high")?;
    let low_micros = price_to_micros(&bar.low, "low")?;
    let close_micros = price_to_micros(&bar.close, "close")?;

    if bar.volume < 0 {
        return Err(NormalizerError::NegativeVolume(bar.volume));
    }

    validate_ohlc(open_micros, high_micros, low_micros, close_micros)?;

    Ok(NormalizedBar {
        symbol: bar.symbol.clone(),
        timeframe: bar.timeframe.clone(),
        end_ts: bar.end_ts,
        open_micros,
        high_micros,
        low_micros,
        close_micros,
        volume: bar.volume,
        is_complete: bar.is_complete,
    })
}

/// Normalize a batch of [`RawBar`]s, collecting all errors.
///
/// Returns `Ok(Vec<NormalizedBar>)` only if every bar normalizes successfully.
/// On the first error, returns `Err`.  Use this when you need an all-or-nothing
/// result; iterate and call [`normalize`] individually for partial success.
pub fn normalize_all(bars: &[RawBar]) -> Result<Vec<NormalizedBar>, NormalizerError> {
    bars.iter().map(normalize).collect()
}

/// Sort a slice of [`NormalizedBar`]s in-place by `(symbol, timeframe, end_ts)`.
///
/// This ordering is deterministic and matches the natural primary key of
/// `md_bars` in the database.
pub fn sort_normalized(bars: &mut [NormalizedBar]) {
    bars.sort_by(|a, b| {
        a.symbol
            .cmp(&b.symbol)
            .then_with(|| a.timeframe.cmp(&b.timeframe))
            .then_with(|| a.end_ts.cmp(&b.end_ts))
    });
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn validate_ohlc(open: i64, high: i64, low: i64, close: i64) -> Result<(), NormalizerError> {
    if low > high {
        return Err(NormalizerError::OhlcViolation(format!(
            "low ({low}) > high ({high})"
        )));
    }
    if low > open {
        return Err(NormalizerError::OhlcViolation(format!(
            "low ({low}) > open ({open})"
        )));
    }
    if low > close {
        return Err(NormalizerError::OhlcViolation(format!(
            "low ({low}) > close ({close})"
        )));
    }
    if high < open {
        return Err(NormalizerError::OhlcViolation(format!(
            "high ({high}) < open ({open})"
        )));
    }
    if high < close {
        return Err(NormalizerError::OhlcViolation(format!(
            "high ({high}) < close ({close})"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::RawBar;

    #[allow(clippy::too_many_arguments)]
    fn raw(
        symbol: &str,
        timeframe: &str,
        end_ts: i64,
        open: &str,
        high: &str,
        low: &str,
        close: &str,
        volume: i64,
    ) -> RawBar {
        RawBar {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            end_ts,
            open: open.to_string(),
            high: high.to_string(),
            low: low.to_string(),
            close: close.to_string(),
            volume,
            is_complete: true,
        }
    }

    // --- price_to_micros ---

    #[test]
    fn micros_whole_number() {
        assert_eq!(price_to_micros("100", "open").unwrap(), 100_000_000);
    }

    #[test]
    fn micros_two_decimal_places() {
        assert_eq!(price_to_micros("182.34", "open").unwrap(), 182_340_000);
    }

    #[test]
    fn micros_six_decimal_places() {
        assert_eq!(price_to_micros("1.123456", "open").unwrap(), 1_123_456);
    }

    #[test]
    fn micros_leading_dot() {
        // ".5" is not valid — int_part would be empty AND frac_part "5", which is ok actually
        // but let's confirm the value is correct: 0.5 = 500_000 micros
        assert_eq!(price_to_micros(".5", "open").unwrap(), 500_000);
    }

    #[test]
    fn micros_zero() {
        assert_eq!(price_to_micros("0.000000", "open").unwrap(), 0);
        assert_eq!(price_to_micros("0", "open").unwrap(), 0);
    }

    #[test]
    fn micros_large_value() {
        // 999999.999999 → 999_999_999_999 micros
        assert_eq!(
            price_to_micros("999999.999999", "open").unwrap(),
            999_999_999_999_i64
        );
    }

    #[test]
    fn micros_trailing_zeros_padded() {
        // "1.1" should equal "1.100000"
        assert_eq!(price_to_micros("1.1", "open").unwrap(), 1_100_000);
    }

    #[test]
    fn micros_rejects_seven_decimal_places() {
        let err = price_to_micros("1.1234567", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::TooManyDecimalPlaces { .. }));
    }

    #[test]
    fn micros_rejects_empty() {
        let err = price_to_micros("", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::EmptyPrice { .. }));
    }

    #[test]
    fn micros_rejects_whitespace_only() {
        let err = price_to_micros("   ", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::EmptyPrice { .. }));
    }

    #[test]
    fn micros_rejects_alpha() {
        let err = price_to_micros("abc", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::InvalidPrice { .. }));
    }

    #[test]
    fn micros_rejects_nan_string() {
        let err = price_to_micros("NaN", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::InvalidPrice { .. }));
    }

    #[test]
    fn micros_rejects_inf_string() {
        let err = price_to_micros("inf", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::InvalidPrice { .. }));
    }

    #[test]
    fn micros_rejects_multiple_dots() {
        // "1.2.3" — split_once gives int="1", frac="2.3"; "2.3" is not all digits
        let err = price_to_micros("1.2.3", "open").unwrap_err();
        assert!(matches!(err, NormalizerError::InvalidPrice { .. }));
    }

    // --- normalize ---

    #[test]
    fn normalize_happy_path() {
        let bar = raw(
            "AAPL",
            "1D",
            1_700_000_000,
            "100.00",
            "105.00",
            "99.00",
            "103.00",
            1_000_000,
        );
        let nb = normalize(&bar).unwrap();
        assert_eq!(nb.symbol, "AAPL");
        assert_eq!(nb.timeframe, "1D");
        assert_eq!(nb.end_ts, 1_700_000_000);
        assert_eq!(nb.open_micros, 100_000_000);
        assert_eq!(nb.high_micros, 105_000_000);
        assert_eq!(nb.low_micros, 99_000_000);
        assert_eq!(nb.close_micros, 103_000_000);
        assert_eq!(nb.volume, 1_000_000);
        assert!(nb.is_complete);
    }

    #[test]
    fn normalize_rejects_low_gt_high() {
        let bar = raw("SPY", "1D", 0, "100", "99", "101", "100", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::OhlcViolation(_)));
    }

    #[test]
    fn normalize_rejects_low_gt_open() {
        // low=105, open=100, high=110, close=107
        let bar = raw("SPY", "1D", 0, "100", "110", "105", "107", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::OhlcViolation(_)));
    }

    #[test]
    fn normalize_rejects_low_gt_close() {
        // low=108, open=100, high=110, close=107
        let bar = raw("SPY", "1D", 0, "100", "110", "108", "107", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::OhlcViolation(_)));
    }

    #[test]
    fn normalize_rejects_high_lt_open() {
        // high=95, open=100
        let bar = raw("SPY", "1D", 0, "100", "95", "90", "92", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::OhlcViolation(_)));
    }

    #[test]
    fn normalize_rejects_high_lt_close() {
        // high=95, close=98
        let bar = raw("SPY", "1D", 0, "90", "95", "88", "98", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::OhlcViolation(_)));
    }

    #[test]
    fn normalize_rejects_negative_volume() {
        let mut bar = raw("SPY", "1D", 0, "100", "105", "99", "103", -1);
        bar.volume = -1;
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(err, NormalizerError::NegativeVolume(-1)));
    }

    #[test]
    fn normalize_rejects_invalid_price_string() {
        let bar = raw("SPY", "1D", 0, "NaN", "105", "99", "103", 0);
        let err = normalize(&bar).unwrap_err();
        assert!(matches!(
            err,
            NormalizerError::InvalidPrice { field: "open", .. }
        ));
    }

    #[test]
    fn normalize_all_returns_all_on_success() {
        let bars = vec![
            raw("AAPL", "1D", 1, "100", "105", "99", "103", 100),
            raw("MSFT", "1D", 2, "200", "210", "198", "205", 200),
        ];
        let result = normalize_all(&bars).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn normalize_all_fails_on_first_bad_bar() {
        let bars = vec![
            raw("AAPL", "1D", 1, "100", "105", "99", "103", 100),
            raw("BAD", "1D", 2, "NaN", "105", "99", "103", 0),
        ];
        assert!(normalize_all(&bars).is_err());
    }

    // --- sort_normalized ---

    #[test]
    fn sort_by_symbol_then_timeframe_then_ts() {
        let bars = [
            raw("SPY", "1m", 300, "100", "105", "99", "103", 0),
            raw("AAPL", "1D", 200, "100", "105", "99", "103", 0),
            raw("AAPL", "1D", 100, "100", "105", "99", "103", 0),
            raw("AAPL", "1m", 50, "100", "105", "99", "103", 0),
        ];
        let mut normalized: Vec<NormalizedBar> =
            bars.iter().map(|b| normalize(b).unwrap()).collect();
        sort_normalized(&mut normalized);

        assert_eq!(normalized[0].symbol, "AAPL");
        assert_eq!(normalized[0].timeframe, "1D");
        assert_eq!(normalized[0].end_ts, 100);

        assert_eq!(normalized[1].symbol, "AAPL");
        assert_eq!(normalized[1].timeframe, "1D");
        assert_eq!(normalized[1].end_ts, 200);

        assert_eq!(normalized[2].symbol, "AAPL");
        assert_eq!(normalized[2].timeframe, "1m");

        assert_eq!(normalized[3].symbol, "SPY");
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        // Two bars with identical keys — order among them must not panic.
        let bars = [
            raw("AAPL", "1D", 100, "100", "105", "99", "103", 10),
            raw("AAPL", "1D", 100, "100", "105", "99", "103", 20),
        ];
        let mut normalized: Vec<NormalizedBar> =
            bars.iter().map(|b| normalize(b).unwrap()).collect();
        sort_normalized(&mut normalized); // must not panic
        assert_eq!(normalized.len(), 2);
    }

    // --- error Display ---

    #[test]
    fn error_display_empty_price() {
        let e = NormalizerError::EmptyPrice { field: "high" };
        assert_eq!(e.to_string(), "price field 'high' is empty");
    }

    #[test]
    fn error_display_invalid_price() {
        let e = NormalizerError::InvalidPrice {
            field: "low",
            raw: "abc".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "price field 'low' could not be parsed: 'abc'"
        );
    }

    #[test]
    fn error_display_too_many_decimal_places() {
        let e = NormalizerError::TooManyDecimalPlaces {
            field: "close",
            raw: "1.1234567".to_string(),
        };
        assert!(e.to_string().contains("ambiguous micro conversion"));
    }

    #[test]
    fn error_display_ohlc_violation() {
        let e = NormalizerError::OhlcViolation("low > high".to_string());
        assert_eq!(e.to_string(), "OHLC sanity violation: low > high");
    }

    #[test]
    fn error_display_negative_volume() {
        let e = NormalizerError::NegativeVolume(-5);
        assert_eq!(e.to_string(), "volume must be >= 0, got -5");
    }
}
