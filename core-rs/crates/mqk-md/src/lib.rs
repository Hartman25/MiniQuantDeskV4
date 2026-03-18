//! mqk-md
//!
//! PATCH C — Historical provider ingest (pluggable providers).
//!
//! This crate owns the provider abstraction and concrete historical providers.
//! It does **not** write to the DB; callers (CLI) fetch bars and hand them to mqk-db ingestion.

pub mod normalizer;
pub mod provider;
pub mod quality;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of retries after the first attempt when TwelveData returns a rate-limit
/// response (HTTP 429 or body `code=429`).  Total attempts = 1 + MAX_RETRIES = 5.
const TWELVEDATA_RATE_LIMIT_MAX_RETRIES: u32 = 4;

/// Fixed sleep duration between rate-limit retries.
/// 65 seconds comfortably clears TwelveData's 60-second per-minute reset window.
/// Set to 0 in `new_for_test` so unit tests complete instantly.
const TWELVEDATA_RATE_LIMIT_SLEEP_SECS: u64 = 65;

/// Supported timeframe identifiers for historical ingestion.
///
/// Canonical user-facing values are aligned with the backtest spec:
/// - `1D`
/// - `1m`
/// - `5m`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    D1,
    M1,
    M5,
}

impl Timeframe {
    pub fn as_str(&self) -> &'static str {
        match self {
            Timeframe::D1 => "1D",
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
        }
    }

    /// TwelveData interval string.
    pub fn as_twelvedata_interval(&self) -> &'static str {
        match self {
            Timeframe::D1 => "1day",
            Timeframe::M1 => "1min",
            Timeframe::M5 => "5min",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1d" => Ok(Timeframe::D1),
            "1m" | "1min" | "1minute" => Ok(Timeframe::M1),
            "5m" | "5min" | "5minute" => Ok(Timeframe::M5),
            other => Err(anyhow!(
                "invalid timeframe '{}'. expected one of: 1D | 1m | 5m",
                other
            )),
        }
    }
}

/// A raw OHLCV bar as returned by a historical provider.
///
/// IMPORTANT: Prices remain as decimal strings so callers can normalize deterministically
/// (no floats) using mqk-db canonical conversion rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBar {
    pub symbol: String,
    pub timeframe: String,
    /// Bar end timestamp (epoch seconds, UTC).
    pub end_ts: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: i64,
    pub is_complete: bool,
}

/// Fetch request for a provider.
#[derive(Debug, Clone)]
pub struct FetchBarsRequest {
    pub symbols: Vec<String>,
    pub timeframe: Timeframe,
    /// Inclusive start date (UTC). Providers that only accept dates should treat this as start-of-day.
    pub start: NaiveDate,
    /// Inclusive end date (UTC). Providers that only accept dates should treat this as end-of-day.
    pub end: NaiveDate,
}

/// Pluggable historical provider interface.
#[async_trait::async_trait]
pub trait HistoricalProvider: Send + Sync {
    fn source_name(&self) -> &'static str;

    async fn fetch_bars(&self, req: FetchBarsRequest) -> Result<Vec<ProviderBar>>;
}

/// Normalize a TwelveData price string into a form compatible with the DB `price_to_micros`
/// function.
///
/// The DB layer rejects decimal strings with more than 6 fractional digits. TwelveData
/// sometimes returns extra precision (e.g. `"123.45678900"`). This function normalizes
/// provider-side using only string operations — no float parsing, no rounding ambiguity:
///
/// 1. Strips an optional leading `+`.
/// 2. Rejects negative values with `Err`.
/// 3. Validates that all characters are ASCII digits or a single `.`.
/// 4. Truncates fractional digits beyond 6 (truncation, never rounding).
/// 5. Trims trailing zeros from the fractional part.
/// 6. Removes the decimal point when the fractional part becomes empty.
///
/// # Examples
///
/// ```rust
/// use mqk_md::normalize_price_str;
/// assert_eq!(normalize_price_str("123.45678900").unwrap(), "123.456789");
/// assert_eq!(normalize_price_str("10.50000000").unwrap(),  "10.5");
/// assert_eq!(normalize_price_str("100.000000").unwrap(),   "100");
/// assert_eq!(normalize_price_str("42").unwrap(),           "42");
/// ```
pub fn normalize_price_str(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("normalize_price_str: empty input"));
    }
    let s = s.strip_prefix('+').unwrap_or(s);
    if s.starts_with('-') {
        return Err(anyhow!(
            "normalize_price_str: negative price not allowed: {}",
            s
        ));
    }

    match s.split_once('.') {
        None => {
            // Integer-only string.
            if !s.chars().all(|c| c.is_ascii_digit()) {
                return Err(anyhow!(
                    "normalize_price_str: non-digit chars in integer: {}",
                    s
                ));
            }
            Ok(s.to_string())
        }
        Some((int_part, frac_part)) => {
            if int_part.is_empty() {
                return Err(anyhow!(
                    "normalize_price_str: missing integer part in: {}",
                    s
                ));
            }
            if !int_part.chars().all(|c| c.is_ascii_digit()) {
                return Err(anyhow!(
                    "normalize_price_str: non-digit in integer part of: {}",
                    s
                ));
            }
            if !frac_part.chars().all(|c| c.is_ascii_digit()) {
                return Err(anyhow!(
                    "normalize_price_str: non-digit in fractional part of: {}",
                    s
                ));
            }
            // Truncate to at most 6 fractional digits.
            // Deterministic truncation — never round, never use floats.
            let frac = if frac_part.len() > 6 {
                &frac_part[..6]
            } else {
                frac_part
            };
            // Trim trailing zeros; remove the decimal point when nothing remains.
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                Ok(int_part.to_string())
            } else {
                Ok(format!("{}.{}", int_part, frac))
            }
        }
    }
}

/// TwelveData-backed historical provider.
///
/// API key is read by the caller (CLI) and passed in; do not log it.
#[derive(Debug, Clone)]
pub struct TwelveDataHistoricalProvider {
    api_key: String,
    http: reqwest::Client,
    base_url: String,
    /// Seconds to sleep between rate-limit retries.
    /// Production: `TWELVEDATA_RATE_LIMIT_SLEEP_SECS` (65).
    /// Tests: 0 (instant, no real sleep).
    retry_sleep_secs: u64,
}

impl TwelveDataHistoricalProvider {
    pub fn new(api_key: String) -> Self {
        Self::new_with_base_url(api_key, "https://api.twelvedata.com".to_string())
    }

    pub fn new_with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
            base_url,
            retry_sleep_secs: TWELVEDATA_RATE_LIMIT_SLEEP_SECS,
        }
    }

    /// Test-only constructor: zero sleep so rate-limit retry tests complete instantly.
    #[cfg(test)]
    fn new_for_test(api_key: String, base_url: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
            base_url,
            retry_sleep_secs: 0,
        }
    }

    fn build_time_series_url(&self) -> String {
        format!("{}/time_series", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait::async_trait]
impl HistoricalProvider for TwelveDataHistoricalProvider {
    fn source_name(&self) -> &'static str {
        "twelvedata"
    }

    async fn fetch_bars(&self, req: FetchBarsRequest) -> Result<Vec<ProviderBar>> {
        // NOTE: TwelveData supports multi-symbol queries, but response shapes vary.
        // For PATCH C we fetch per-symbol deterministically.
        let mut out: Vec<ProviderBar> = Vec::new();

        for sym in req.symbols.iter() {
            let url = self.build_time_series_url();

            // TwelveData expects date strings. We provide ISO dates.
            let start_s = req.start.format("%Y-%m-%d").to_string();
            let end_s = req.end.format("%Y-%m-%d").to_string();

            // Bounded retry loop: handles TwelveData 429 rate-limit responses at both
            // the HTTP level (status 429) and body level (status="error", code=429).
            // All other errors are fatal immediately (no retry).
            let mut retries_remaining = TWELVEDATA_RATE_LIMIT_MAX_RETRIES;
            let body: TwelveDataTimeSeriesResponse = loop {
                let resp = self
                    .http
                    .get(&url)
                    .query(&[
                        ("symbol", sym.as_str()),
                        ("interval", req.timeframe.as_twelvedata_interval()),
                        ("start_date", start_s.as_str()),
                        ("end_date", end_s.as_str()),
                        ("timezone", "UTC"),
                        ("format", "JSON"),
                        ("apikey", self.api_key.as_str()),
                    ])
                    .send()
                    .await
                    .context("twelvedata request failed")?;

                let http_status = resp.status();

                // HTTP-level 429: retried without decoding the body.
                if http_status.as_u16() == 429 {
                    if retries_remaining > 0 {
                        retries_remaining -= 1;
                        tokio::time::sleep(std::time::Duration::from_secs(
                            self.retry_sleep_secs,
                        ))
                        .await;
                        continue;
                    }
                    return Err(anyhow!(
                        "twelvedata http 429 rate limit persisted after {} retries: \
                         symbol={} chunk={}..{}",
                        TWELVEDATA_RATE_LIMIT_MAX_RETRIES,
                        sym,
                        start_s,
                        end_s
                    ));
                }

                let b: TwelveDataTimeSeriesResponse = resp
                    .json()
                    .await
                    .context("twelvedata response json decode failed")?;

                // Body-level 429: TwelveData commonly returns HTTP 200 with a JSON
                // error envelope rather than a true HTTP 429 status.
                if b.is_rate_limit_error() {
                    if retries_remaining > 0 {
                        retries_remaining -= 1;
                        tokio::time::sleep(std::time::Duration::from_secs(
                            self.retry_sleep_secs,
                        ))
                        .await;
                        continue;
                    }
                    return Err(anyhow!(
                        "twelvedata rate limit persisted after {} retries: \
                         symbol={} chunk={}..{}: {}",
                        TWELVEDATA_RATE_LIMIT_MAX_RETRIES,
                        sym,
                        start_s,
                        end_s,
                        b.status_message()
                    ));
                }

                if !http_status.is_success() {
                    return Err(anyhow!(
                        "twelvedata http error status={} message={}",
                        http_status.as_u16(),
                        b.status_message()
                    ));
                }

                break b;
            };

            // A code=400 "no data available" response means this symbol/chunk window
            // predates the symbol's inception date.  This is non-fatal: emit zero bars
            // for this symbol/chunk and continue to the next one.
            // All other error responses (auth, malformed, etc.) remain fatal.
            if body.is_no_data_error() {
                continue;
            }

            if let Some(err) = body.error_message() {
                return Err(anyhow!("twelvedata error: {}", err));
            }

            let values = body.values.unwrap_or_default();
            let mut symbol_bars = Vec::with_capacity(values.len());

            for v in values {
                // TwelveData may return:
                // - RFC3339 timestamps
                // - naive datetime strings like "2024-01-02 15:30:00"
                // - date-only strings like "2000-12-29" for daily bars
                let end_ts = if let Ok(dt) = DateTime::parse_from_rfc3339(&v.datetime) {
                    dt.with_timezone(&Utc).timestamp()
                } else if let Ok(dt) =
                    NaiveDateTime::parse_from_str(&v.datetime, "%Y-%m-%d %H:%M:%S")
                {
                    dt.and_utc().timestamp()
                } else if let Ok(d) = NaiveDate::parse_from_str(&v.datetime, "%Y-%m-%d") {
                    d.and_hms_opt(0, 0, 0)
                        .ok_or_else(|| {
                            anyhow!(
                                "twelvedata date at midnight conversion failed: {}",
                                v.datetime
                            )
                        })?
                        .and_utc()
                        .timestamp()
                } else {
                    return Err(anyhow!("twelvedata datetime parse failed: {}", v.datetime));
                };

                // Normalize prices provider-side: truncate to ≤6 fractional digits,
                // trim trailing zeros, no float parsing. Adapts TwelveData output to
                // the DB-side `price_to_micros` precision requirement without relaxing
                // DB-side rules.
                symbol_bars.push(ProviderBar {
                    symbol: sym.to_string(),
                    timeframe: req.timeframe.as_str().to_string(),
                    end_ts,
                    open: normalize_price_str(&v.open).with_context(|| {
                        format!(
                            "open price normalize failed: symbol={sym} datetime={}",
                            v.datetime
                        )
                    })?,
                    high: normalize_price_str(&v.high).with_context(|| {
                        format!(
                            "high price normalize failed: symbol={sym} datetime={}",
                            v.datetime
                        )
                    })?,
                    low: normalize_price_str(&v.low).with_context(|| {
                        format!(
                            "low price normalize failed: symbol={sym} datetime={}",
                            v.datetime
                        )
                    })?,
                    close: normalize_price_str(&v.close).with_context(|| {
                        format!(
                            "close price normalize failed: symbol={sym} datetime={}",
                            v.datetime
                        )
                    })?,
                    volume: v.volume.parse::<i64>().unwrap_or(0),
                    is_complete: true,
                });
            }

            // Sort ascending by end_ts — TwelveData commonly returns newest-first.
            symbol_bars.sort_by_key(|bar| bar.end_ts);
            out.extend(symbol_bars);
        }

        Ok(out)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TwelveDataTimeSeriesResponse {
    status: Option<String>,
    message: Option<String>,
    code: Option<i64>,
    values: Option<Vec<TwelveDataBarValue>>,
}

impl TwelveDataTimeSeriesResponse {
    /// Returns `true` iff this is the specific TwelveData
    /// "No data is available on the specified dates" error (status=error, code=400).
    ///
    /// This is the only provider error treated as non-fatal by `fetch_bars`.
    /// It occurs when a symbol/chunk window predates the symbol's inception date.
    /// The caller continues with zero bars for that symbol/chunk.
    ///
    /// All three conditions must hold simultaneously:
    /// - `status == "error"`
    /// - `code == 400`
    /// - `message` contains the literal substring "No data is available on the specified dates"
    ///
    /// Any other error (auth failure, rate limit, malformed response, etc.)
    /// does NOT match and remains fatal.
    fn is_no_data_error(&self) -> bool {
        matches!(self.status.as_deref(), Some("error"))
            && self.code == Some(400)
            && self
                .message
                .as_deref()
                .map(|m| m.contains("No data is available on the specified dates"))
                .unwrap_or(false)
    }

    /// Returns `true` iff this is a TwelveData rate-limit error (status=error, code=429).
    ///
    /// This is the only provider error that triggers a bounded retry.
    /// Both conditions must hold simultaneously:
    /// - `status == "error"`
    /// - `code == 429`
    ///
    /// No message matching is required — code 429 is unambiguous.
    /// Other errors (no-data 400, auth 401, etc.) do NOT match.
    fn is_rate_limit_error(&self) -> bool {
        matches!(self.status.as_deref(), Some("error")) && self.code == Some(429)
    }

    fn error_message(&self) -> Option<String> {
        // TwelveData uses either a "status":"error" or a "code" and "message" fields.
        // We treat any message with non-success status as an error hint.
        match self.status.as_deref() {
            Some("error") => Some(self.status_message()),
            _ => None,
        }
    }

    fn status_message(&self) -> String {
        match (&self.code, &self.message) {
            (Some(c), Some(m)) => format!("code={} {}", c, m),
            (_, Some(m)) => m.clone(),
            _ => "unknown".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TwelveDataBarValue {
    datetime: String,
    open: String,
    high: String,
    low: String,
    close: String,
    #[serde(default)]
    volume: String,
}

// -----------------
// Tests (no network)
// -----------------

#[cfg(test)]
mod tests {
    use super::*;
    // Private struct accessible in child module via explicit path.
    use super::TwelveDataTimeSeriesResponse;

    #[test]
    fn timeframe_parse() {
        assert_eq!(Timeframe::parse("1D").unwrap(), Timeframe::D1);
        assert_eq!(Timeframe::parse("1m").unwrap(), Timeframe::M1);
        assert_eq!(Timeframe::parse("5m").unwrap(), Timeframe::M5);
        assert!(Timeframe::parse("15m").is_err());
    }

    // -----------------------------------------------------------------------
    // normalize_price_str — deterministic string truncation, no floats
    // -----------------------------------------------------------------------

    #[test]
    fn normalize_truncates_beyond_six_decimals() {
        // 7 decimal digits → first 6 kept, 7th dropped (not rounded)
        assert_eq!(normalize_price_str("100.1234567").unwrap(), "100.123456");
        // 8 decimal digits
        assert_eq!(normalize_price_str("123.45678900").unwrap(), "123.456789");
        // 9 decimal digits
        assert_eq!(normalize_price_str("1.123456789").unwrap(), "1.123456");
    }

    #[test]
    fn normalize_trims_trailing_zeros() {
        assert_eq!(normalize_price_str("10.50000000").unwrap(), "10.5");
        assert_eq!(normalize_price_str("10.500000").unwrap(), "10.5");
        assert_eq!(normalize_price_str("10.123400").unwrap(), "10.1234");
    }

    #[test]
    fn normalize_removes_dot_when_fraction_all_zeros() {
        assert_eq!(normalize_price_str("100.000000").unwrap(), "100");
        assert_eq!(normalize_price_str("1.0000000").unwrap(), "1");
        assert_eq!(normalize_price_str("0.000000").unwrap(), "0");
    }

    #[test]
    fn normalize_preserves_integer_input() {
        assert_eq!(normalize_price_str("42").unwrap(), "42");
        assert_eq!(normalize_price_str("0").unwrap(), "0");
        assert_eq!(normalize_price_str("999999").unwrap(), "999999");
    }

    #[test]
    fn normalize_handles_trailing_dot() {
        // "42." — empty fractional part treated as zero fractional digits
        assert_eq!(normalize_price_str("42.").unwrap(), "42");
    }

    #[test]
    fn normalize_handles_exactly_six_decimals() {
        // 6 decimal digits — no truncation needed
        assert_eq!(normalize_price_str("100.123456").unwrap(), "100.123456");
        // Trailing zeros within 6 decimals are trimmed
        assert_eq!(normalize_price_str("100.120000").unwrap(), "100.12");
    }

    #[test]
    fn normalize_strips_leading_plus() {
        assert_eq!(normalize_price_str("+10.5").unwrap(), "10.5");
        assert_eq!(normalize_price_str("+100").unwrap(), "100");
    }

    #[test]
    fn normalize_rejects_negative() {
        assert!(normalize_price_str("-10.5").is_err());
        assert!(normalize_price_str("-0").is_err());
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_price_str("").is_err());
        assert!(normalize_price_str("   ").is_err());
    }

    #[test]
    fn normalize_rejects_malformed_chars() {
        assert!(normalize_price_str("10.5e2").is_err()); // scientific notation not accepted
        assert!(normalize_price_str("1,000").is_err()); // comma separator not accepted
        assert!(normalize_price_str(".5").is_err()); // missing integer part
        assert!(normalize_price_str("NaN").is_err());
        assert!(normalize_price_str("inf").is_err());
    }

    #[test]
    fn normalize_output_has_at_most_six_decimal_places() {
        // Invariant: for any valid input, output has ≤6 fractional digits.
        // This is the DB compatibility contract.
        let inputs = [
            "1.23456789",
            "99.9999999",
            "0.10000000",
            "42",
            "42.",
            "100.000001",
            "100.0000001",
        ];
        for inp in inputs {
            let norm = normalize_price_str(inp)
                .unwrap_or_else(|e| panic!("normalize_price_str({inp:?}) failed: {e}"));
            let frac_len = if let Some(dot) = norm.find('.') {
                norm.len() - dot - 1
            } else {
                0
            };
            assert!(
                frac_len <= 6,
                "normalize({inp:?}) = {norm:?}: frac len {frac_len} exceeds 6"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Date-only timestamp parsing — TwelveData 1D daily bars
    // -----------------------------------------------------------------------

    #[test]
    fn date_only_string_parses_to_midnight_utc_epoch() {
        // TwelveData 1D bars return date strings like "2000-12-29".
        // The parse branch must convert to 00:00:00 UTC of that date.
        //
        // Verification:
        //   1970-01-01 → 2000-01-01 = 10957 days (23 regular + 7 leap years)
        //   2000-01-01 → 2000-12-29 = 363 days (2000 is a leap year)
        //   total = 11320 days × 86400 s/day = 978_048_000
        let d = NaiveDate::parse_from_str("2000-12-29", "%Y-%m-%d").unwrap();
        let ts = d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        assert_eq!(ts, 978_048_000);

        // The naive-datetime path for the same date must produce the same epoch.
        let dt = NaiveDateTime::parse_from_str("2000-12-29 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(dt.and_utc().timestamp(), 978_048_000);
    }

    // -----------------------------------------------------------------------
    // Ascending sort — fetch_bars sorts per-symbol regardless of API order
    // -----------------------------------------------------------------------

    #[test]
    fn fetch_bars_ascending_sort_correct_and_idempotent() {
        // Simulate the sort applied per-symbol inside fetch_bars.
        // Reverse-ordered input must become ascending after sort.
        let mut bars = [
            ProviderBar {
                symbol: "SPY".into(),
                timeframe: "1D".into(),
                end_ts: 3_000,
                open: "10".into(),
                high: "11".into(),
                low: "9".into(),
                close: "10".into(),
                volume: 100,
                is_complete: true,
            },
            ProviderBar {
                symbol: "SPY".into(),
                timeframe: "1D".into(),
                end_ts: 1_000,
                open: "8".into(),
                high: "9".into(),
                low: "7".into(),
                close: "8".into(),
                volume: 90,
                is_complete: true,
            },
            ProviderBar {
                symbol: "SPY".into(),
                timeframe: "1D".into(),
                end_ts: 2_000,
                open: "9".into(),
                high: "10".into(),
                low: "8".into(),
                close: "9".into(),
                volume: 95,
                is_complete: true,
            },
        ];
        bars.sort_by_key(|b| b.end_ts);
        assert_eq!(bars[0].end_ts, 1_000, "first bar must be earliest");
        assert_eq!(bars[1].end_ts, 2_000);
        assert_eq!(bars[2].end_ts, 3_000, "last bar must be latest");

        // Re-sorting an already-sorted slice must produce the same order.
        bars.sort_by_key(|b| b.end_ts);
        assert_eq!(bars[0].end_ts, 1_000);
        assert_eq!(bars[2].end_ts, 3_000);
    }

    // -----------------------------------------------------------------------
    // is_no_data_error — narrow classification of the pre-inception 400 error
    // -----------------------------------------------------------------------

    /// The exact TwelveData "no data available" payload seen in production for
    /// symbol/chunk windows that predate the symbol's inception date.
    /// Must be classified as a no-data error (non-fatal: zero bars, continue).
    #[test]
    fn no_data_error_exact_production_payload_is_recognized() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(400),
            message: Some(
                "No data is available on the specified dates. Try setting different start/end dates."
                    .to_string(),
            ),
            values: None,
        };
        assert!(
            resp.is_no_data_error(),
            "exact production payload must be recognized as no-data error"
        );
        // error_message() still returns Some — the is_no_data_error check happens
        // BEFORE the error_message check in fetch_bars, so the continue fires first.
        assert!(
            resp.error_message().is_some(),
            "error_message is still populated — the continue in fetch_bars fires before it"
        );
    }

    /// Message contains the required substring but with different surrounding text.
    /// Must still be recognized (substring match, not exact match).
    #[test]
    fn no_data_error_recognized_with_extra_surrounding_text() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(400),
            message: Some("No data is available on the specified dates.".to_string()),
            values: None,
        };
        assert!(resp.is_no_data_error());
    }

    /// code=401 (auth failure) must NOT be recognized as a no-data error.
    /// Auth failures must remain fatal.
    #[test]
    fn auth_failure_is_not_no_data_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(401),
            message: Some("Your API key is invalid.".to_string()),
            values: None,
        };
        assert!(
            !resp.is_no_data_error(),
            "auth failure must NOT be recognized as no-data error"
        );
        assert!(
            resp.error_message().is_some(),
            "auth failure must still be a fatal error"
        );
    }

    /// code=429 (rate limit) must NOT be recognized as a no-data error.
    /// Rate limit failures must remain fatal.
    #[test]
    fn rate_limit_is_not_no_data_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(429),
            message: Some("You have run out of API credits for the current minute.".to_string()),
            values: None,
        };
        assert!(!resp.is_no_data_error());
        assert!(resp.error_message().is_some());
    }

    /// code=400 but message does NOT contain the required substring.
    /// Must NOT be recognized as a no-data error (message matching is mandatory).
    #[test]
    fn code_400_wrong_message_is_not_no_data_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(400),
            message: Some("Invalid parameter: symbol.".to_string()),
            values: None,
        };
        assert!(
            !resp.is_no_data_error(),
            "code=400 with unrelated message must NOT be recognized as no-data error"
        );
    }

    /// code=400 with the right message but status != "error".
    /// All three conditions must hold simultaneously.
    #[test]
    fn no_data_message_without_error_status_is_not_recognized() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("ok".to_string()),
            code: Some(400),
            message: Some("No data is available on the specified dates.".to_string()),
            values: None,
        };
        assert!(!resp.is_no_data_error());
        // ok status → no error_message either.
        assert!(resp.error_message().is_none());
    }

    /// A fully successful response must not be treated as a no-data error.
    #[test]
    fn successful_response_is_not_no_data_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("ok".to_string()),
            code: None,
            message: None,
            values: Some(vec![]),
        };
        assert!(!resp.is_no_data_error());
        assert!(resp.error_message().is_none());
    }

    // -----------------------------------------------------------------------
    // is_rate_limit_error — pure unit classification (no HTTP)
    // -----------------------------------------------------------------------

    /// The exact TwelveData rate-limit payload (status=error, code=429).
    /// Must be classified as a rate-limit error (retriable).
    #[test]
    fn rate_limit_error_body_is_recognized() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(429),
            message: Some(
                "You have run out of API credits for the current minute.".to_string(),
            ),
            values: None,
        };
        assert!(
            resp.is_rate_limit_error(),
            "code=429 must be recognized as a rate-limit error"
        );
        // Also still surfaces as a generic error message — the retry loop fires first.
        assert!(resp.error_message().is_some());
    }

    /// code=400 "no data available" must NOT be recognized as a rate-limit error.
    /// No-data errors are non-fatal/non-retriable (emit zero bars, continue).
    #[test]
    fn no_data_error_is_not_rate_limit_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(400),
            message: Some(
                "No data is available on the specified dates. Try setting different start/end dates."
                    .to_string(),
            ),
            values: None,
        };
        assert!(
            !resp.is_rate_limit_error(),
            "no-data error (code=400) must NOT be a rate-limit error"
        );
        // It is a no-data error — must not be confused with rate-limit.
        assert!(
            resp.is_no_data_error(),
            "code=400 no-data message must still be recognized as a no-data error"
        );
    }

    /// code=401 (auth failure) must NOT be recognized as a rate-limit error.
    /// Auth failures are fatal and must never be retried.
    #[test]
    fn auth_error_is_not_rate_limit_error() {
        let resp = TwelveDataTimeSeriesResponse {
            status: Some("error".to_string()),
            code: Some(401),
            message: Some("Your API key is invalid.".to_string()),
            values: None,
        };
        assert!(
            !resp.is_rate_limit_error(),
            "auth error (code=401) must NOT be a rate-limit error"
        );
        assert!(
            resp.error_message().is_some(),
            "auth error must still surface as a fatal error"
        );
    }

    // -----------------------------------------------------------------------
    // Rate-limit retry integration (httpmock, no real network)
    // -----------------------------------------------------------------------

    fn sample_request_for_retry() -> FetchBarsRequest {
        FetchBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: Timeframe::D1,
            start: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            end: NaiveDate::from_ymd_opt(2024, 1, 10).unwrap(),
        }
    }

    /// One body-level 429 on the first attempt, success on the second.
    /// `fetch_bars` must return the bar from the successful response.
    #[tokio::test]
    async fn rate_limit_retry_succeeds_after_one_body_429() {
        use httpmock::prelude::*;
        use std::sync::{Arc, Mutex};

        let server = MockServer::start();
        let call_count = Arc::new(Mutex::new(0u32));
        let call_count_clone = Arc::clone(&call_count);

        // Registered first → lower priority (httpmock is LIFO for tie-breaking).
        // Matches all requests; returns a good bar.
        let _mock_ok = server.mock(|when, then| {
            when.method(GET).path("/time_series");
            then.status(200).json_body(serde_json::json!({
                "status": "ok",
                "values": [{
                    "datetime": "2024-01-02",
                    "open": "185.0",
                    "high": "186.0",
                    "low": "184.0",
                    "close": "185.5",
                    "volume": "50000000"
                }]
            }));
        });

        // Registered second → higher priority.
        // Matches only the first call (counter guard); returns body-level 429.
        let _mock_rl = server.mock(|when, then| {
            when.method(GET)
                .path("/time_series")
                .matches(move |_req| {
                    let mut c = call_count_clone.lock().unwrap();
                    if *c < 1 {
                        *c += 1;
                        true
                    } else {
                        false
                    }
                });
            then.status(200).json_body(serde_json::json!({
                "status": "error",
                "code": 429,
                "message": "You have run out of API credits for the current minute."
            }));
        });

        let provider =
            TwelveDataHistoricalProvider::new_for_test("test-key".to_string(), server.base_url());
        let bars = provider
            .fetch_bars(sample_request_for_retry())
            .await
            .unwrap();
        assert_eq!(bars.len(), 1, "must return bar from successful retry attempt");
        assert_eq!(bars[0].symbol, "AAPL");
    }

    /// Every attempt returns body-level 429.
    /// After exhausting all retries, `fetch_bars` must return an `Err` that mentions
    /// "rate limit" and "retries".
    #[tokio::test]
    async fn rate_limit_exhaustion_returns_error() {
        use httpmock::prelude::*;

        let server = MockServer::start();

        // Always return body-level 429.
        let _mock_rl = server.mock(|when, then| {
            when.method(GET).path("/time_series");
            then.status(200).json_body(serde_json::json!({
                "status": "error",
                "code": 429,
                "message": "You have run out of API credits for the current minute."
            }));
        });

        let provider =
            TwelveDataHistoricalProvider::new_for_test("test-key".to_string(), server.base_url());
        let err = provider
            .fetch_bars(sample_request_for_retry())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("rate limit"),
            "exhaustion error must mention rate limit: {msg}"
        );
        assert!(
            msg.contains("retries"),
            "exhaustion error must mention retries: {msg}"
        );
        assert!(
            msg.contains("AAPL"),
            "exhaustion error must identify the symbol: {msg}"
        );
    }
}
