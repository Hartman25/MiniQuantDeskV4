//! Kraken public OHLC parser/provider adapter (default-disabled, fixture-first).
//!
//! Parses the verified `/0/public/OHLC` response shape confirmed by
//! `CRYPTO-DATA-01S-T` (see
//! `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.md`):
//!
//! ```text
//! {"error": [], "result": {"<pair_key>": [[time, open, high, low, close, vwap, volume, count], ...], "last": <ts>}}
//! ```
//!
//! Kraken documents (and this repo's own live verification confirmed) that
//! the final row in the array is always the current, not-yet-committed
//! candle. This module never treats that row as complete: completion is
//! derived structurally from `row.time <= result.last`, never from a
//! provider-supplied flag (Kraken sends none) and never from wall-clock
//! guessing.
//!
//! # Volume semantics (CRYPTO-DATA-01V)
//!
//! Kraken's `volume` field is a decimal string in **base-asset units**
//! (e.g. BTC, ETH) — not whole coins in the sense of an integer, and not
//! quote-currency (USD) volume. [`ProviderBar::volume`]/[`RawBar::volume`]
//! (`crate::ProviderBar`) is typed `i64`, so this module scales the decimal
//! string by [`KRAKEN_VOLUME_SCALE`] (1e8, "atomic-volume units") before
//! conversion. This is an explicit, documented, tested convention:
//!
//! - `"1317.15941434"` (BTC) -> `131715941434`
//! - `"15588.01759742"` (ETH) -> `1558801759742`
//!
//! This is **not** whole-coin volume and **not** quote-currency (USD)
//! volume. A decimal string with more than [`KRAKEN_VOLUME_SCALE_DECIMALS`]
//! fractional digits is rejected (`KrakenOhlcParseError::VolumeTooManyDecimals`)
//! rather than silently truncated, and integer overflow during scaling is
//! rejected (`KrakenOhlcParseError::VolumeOverflow`) rather than wrapping.
//! This scaled value must not be used for trading/sizing decisions in this
//! patch — no strategy/risk/execution code reads it.
//!
//! # Scope
//!
//! This module supports exactly two canonical symbols (`BTC/USD`,
//! `ETH/USD`) and exactly one timeframe (`1D` / Kraken `interval=1440`,
//! 86 400 seconds), matching what `CRYPTO-DATA-01S-T` live-verified. It does
//! **not** implement recurring ingestion, scheduling, or any DB write path
//! itself — see [`crate::provider_registry`] for how a caller wires this
//! into the disabled-by-default provider factory.

use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;

use crate::{FetchBarsRequest, ProviderBar};

/// Stable provider identifier used throughout this module and
/// `config/providers/providers.json`.
pub const KRAKEN_PROVIDER_ID: &str = "kraken";

/// Default Kraken public REST base URL.
pub const KRAKEN_BASE_URL: &str = "https://api.kraken.com";

/// Bar-end-timestamp correction: Kraken's `time` field is the bar's
/// **start**, confirmed live by `CRYPTO-DATA-01S-T` (`result.last` equals
/// the last committed row's own `time`, and the next row is exactly one
/// interval later). `RawBar.end_ts` must be `row.time + interval_seconds`.
const KRAKEN_INTERVAL_1D_SECONDS: i64 = 86_400;

/// Scale factor applied to Kraken's fractional base-asset `volume` string to
/// produce an integer `i64` for [`ProviderBar::volume`]. See the module-level
/// doc comment ("Volume semantics") for the full rationale and worked
/// examples. This is base-asset volume scaled by 1e8 ("atomic-volume
/// units") — not whole coins, not quote-currency (USD) volume.
pub const KRAKEN_VOLUME_SCALE: i64 = 100_000_000;

/// Maximum number of fractional decimal digits [`KRAKEN_VOLUME_SCALE`] can
/// represent exactly. A volume string with more fractional digits than this
/// is rejected rather than truncated.
pub const KRAKEN_VOLUME_SCALE_DECIMALS: usize = 8;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Parse failures for a Kraken `/0/public/OHLC` response body. Every variant
/// is a rejection, never a silent default or fabrication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KrakenOhlcParseError {
    /// JSON decode failed outright.
    Decode { message: String },
    /// The response's top-level `error` array was non-empty.
    ApiError { messages: Vec<String> },
    /// The response had no `result` object.
    MissingResult,
    /// `result` had no `last` field, or it was not an integer.
    MissingLast,
    /// `result` had zero pair keys (besides `last`).
    NoPairKey,
    /// `result` had more than one pair key (besides `last`); this module
    /// only supports a single-pair response per call.
    UnexpectedPairKeyCount { found: Vec<String> },
    /// A row was not a JSON array, or did not have exactly 8 elements.
    MalformedRow { index: usize, message: String },
    /// Two or more rows shared the same `time` value.
    DuplicateTimestamp { time: i64 },
    /// `row.time + interval_seconds` would overflow `i64`.
    EndTsOverflow { time: i64 },
    /// The caller requested an interval this module does not support.
    UnsupportedInterval { interval_seconds: i64 },
    /// The `volume` string could not be parsed as a non-negative decimal.
    VolumeMalformed { time: i64, raw_volume: String },
    /// The `volume` string had more fractional digits than
    /// [`KRAKEN_VOLUME_SCALE_DECIMALS`] can represent exactly.
    VolumeTooManyDecimals {
        time: i64,
        raw_volume: String,
        max_decimals: usize,
    },
    /// Scaling the volume by [`KRAKEN_VOLUME_SCALE`] overflowed `i64`.
    VolumeOverflow { time: i64, raw_volume: String },
    /// The canonical symbol requested has no known Kraken query-pair mapping.
    UnsupportedSymbol { symbol: String },
    /// A conversion to [`ProviderBar`] was requested for a row that is not
    /// complete (`row.time > result.last`). This must never happen through
    /// the normal API (`KrakenPairOhlc::completed_provider_bars` filters
    /// these out itself) — it exists as a defense-in-depth guard.
    IncompleteBarConversionRefused { time: i64 },
}

impl fmt::Display for KrakenOhlcParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KrakenOhlcParseError::Decode { message } => write!(f, "json decode failed: {message}"),
            KrakenOhlcParseError::ApiError { messages } => {
                write!(f, "kraken api error: {}", messages.join("; "))
            }
            KrakenOhlcParseError::MissingResult => write!(f, "response missing 'result'"),
            KrakenOhlcParseError::MissingLast => {
                write!(f, "response 'result' missing integer 'last'")
            }
            KrakenOhlcParseError::NoPairKey => write!(f, "response 'result' has no pair key"),
            KrakenOhlcParseError::UnexpectedPairKeyCount { found } => write!(
                f,
                "response 'result' has unexpected pair key count: {found:?}"
            ),
            KrakenOhlcParseError::MalformedRow { index, message } => {
                write!(f, "malformed row at index {index}: {message}")
            }
            KrakenOhlcParseError::DuplicateTimestamp { time } => {
                write!(f, "duplicate row timestamp: {time}")
            }
            KrakenOhlcParseError::EndTsOverflow { time } => {
                write!(f, "end_ts overflow for row.time={time}")
            }
            KrakenOhlcParseError::UnsupportedInterval { interval_seconds } => {
                write!(f, "unsupported interval_seconds={interval_seconds}")
            }
            KrakenOhlcParseError::VolumeMalformed { time, raw_volume } => write!(
                f,
                "malformed volume at time={time}: {raw_volume:?}"
            ),
            KrakenOhlcParseError::VolumeTooManyDecimals {
                time,
                raw_volume,
                max_decimals,
            } => write!(
                f,
                "volume at time={time} has more than {max_decimals} fractional digits: {raw_volume:?}"
            ),
            KrakenOhlcParseError::VolumeOverflow { time, raw_volume } => write!(
                f,
                "volume scaling overflow at time={time}: {raw_volume:?}"
            ),
            KrakenOhlcParseError::UnsupportedSymbol { symbol } => {
                write!(f, "unsupported kraken symbol: {symbol}")
            }
            KrakenOhlcParseError::IncompleteBarConversionRefused { time } => write!(
                f,
                "refused to convert incomplete (forming) bar at time={time} to ProviderBar"
            ),
        }
    }
}

impl std::error::Error for KrakenOhlcParseError {}

// ---------------------------------------------------------------------------
// Parsed model
// ---------------------------------------------------------------------------

/// One parsed Kraken OHLC row, with completion already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KrakenOhlcBar {
    /// Bar **start** timestamp exactly as returned by Kraken.
    pub time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub vwap: String,
    /// Raw fractional base-asset volume string exactly as returned.
    pub volume_raw: String,
    pub count: i64,
    /// Bar **end** timestamp: `time + interval_seconds`.
    pub end_ts: i64,
    /// `true` iff `time <= result.last` (never inferred any other way).
    pub is_complete: bool,
}

/// A fully parsed single-pair Kraken `/0/public/OHLC` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KrakenPairOhlc {
    /// The pair key Kraken used as the JSON object key (e.g. `"XXBTZUSD"`).
    pub provider_pair_key: String,
    /// The query pair the caller supplied (e.g. `"XBTUSD"`).
    pub query_pair: String,
    /// Kraken's own completion cursor.
    pub last: i64,
    /// All rows, in response order, including any forming/incomplete rows.
    pub bars: Vec<KrakenOhlcBar>,
}

impl KrakenPairOhlc {
    /// Rows with `is_complete == true`, in response order.
    pub fn completed_bars(&self) -> Vec<&KrakenOhlcBar> {
        self.bars.iter().filter(|b| b.is_complete).collect()
    }

    /// Rows with `is_complete == false` (in practice, today, only ever the
    /// final row) -- these are never converted to [`ProviderBar`].
    pub fn forming_bars(&self) -> Vec<&KrakenOhlcBar> {
        self.bars.iter().filter(|b| !b.is_complete).collect()
    }

    /// Convert every completed row to a [`ProviderBar`] for `canonical_symbol`
    /// (e.g. `"BTC/USD"`) at `timeframe` (e.g. `"1D"`). Forming rows are never
    /// converted -- this method only ever iterates [`Self::completed_bars`].
    pub fn completed_provider_bars(
        &self,
        canonical_symbol: &str,
        timeframe: &str,
    ) -> Result<Vec<ProviderBar>, KrakenOhlcParseError> {
        self.completed_bars()
            .into_iter()
            .map(|bar| kraken_bar_to_provider_bar(bar, canonical_symbol, timeframe))
            .collect()
    }
}

/// Convert one completed [`KrakenOhlcBar`] into a [`ProviderBar`]. Returns
/// [`KrakenOhlcParseError::IncompleteBarConversionRefused`] if called on a
/// forming bar -- a defense-in-depth guard; normal callers should only ever
/// pass rows from [`KrakenPairOhlc::completed_bars`].
pub fn kraken_bar_to_provider_bar(
    bar: &KrakenOhlcBar,
    canonical_symbol: &str,
    timeframe: &str,
) -> Result<ProviderBar, KrakenOhlcParseError> {
    if !bar.is_complete {
        return Err(KrakenOhlcParseError::IncompleteBarConversionRefused { time: bar.time });
    }
    let volume = kraken_volume_to_scaled_i64(&bar.volume_raw, bar.time)?;
    Ok(ProviderBar {
        symbol: canonical_symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts: bar.end_ts,
        open: bar.open.clone(),
        high: bar.high.clone(),
        low: bar.low.clone(),
        close: bar.close.clone(),
        volume,
        is_complete: true,
    })
}

// ---------------------------------------------------------------------------
// Volume conversion (CRYPTO-DATA-01V)
// ---------------------------------------------------------------------------

/// Scale a Kraken fractional base-asset volume decimal string (e.g.
/// `"1317.15941434"`) by [`KRAKEN_VOLUME_SCALE`] into an exact `i64`
/// (e.g. `131715941434`). Never rounds, never truncates silently, never
/// wraps on overflow -- every failure mode is a typed rejection.
pub fn kraken_volume_to_scaled_i64(raw: &str, time: i64) -> Result<i64, KrakenOhlcParseError> {
    let raw_trimmed = raw.trim();
    if raw_trimmed.is_empty() {
        return Err(KrakenOhlcParseError::VolumeMalformed {
            time,
            raw_volume: raw.to_string(),
        });
    }
    if raw_trimmed.starts_with('-') {
        return Err(KrakenOhlcParseError::VolumeMalformed {
            time,
            raw_volume: raw.to_string(),
        });
    }

    let (int_part, frac_part) = match raw_trimmed.split_once('.') {
        Some((i, f)) => (i, f),
        None => (raw_trimmed, ""),
    };

    if int_part.is_empty() || !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(KrakenOhlcParseError::VolumeMalformed {
            time,
            raw_volume: raw.to_string(),
        });
    }
    if !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(KrakenOhlcParseError::VolumeMalformed {
            time,
            raw_volume: raw.to_string(),
        });
    }
    if frac_part.len() > KRAKEN_VOLUME_SCALE_DECIMALS {
        return Err(KrakenOhlcParseError::VolumeTooManyDecimals {
            time,
            raw_volume: raw.to_string(),
            max_decimals: KRAKEN_VOLUME_SCALE_DECIMALS,
        });
    }

    let padded_frac = format!("{frac_part:0<width$}", width = KRAKEN_VOLUME_SCALE_DECIMALS);

    let int_val: i64 = int_part
        .parse()
        .map_err(|_| KrakenOhlcParseError::VolumeOverflow {
            time,
            raw_volume: raw.to_string(),
        })?;
    let frac_val: i64 = padded_frac
        .parse()
        .map_err(|_| KrakenOhlcParseError::VolumeOverflow {
            time,
            raw_volume: raw.to_string(),
        })?;

    let scaled_int = int_val.checked_mul(KRAKEN_VOLUME_SCALE).ok_or_else(|| {
        KrakenOhlcParseError::VolumeOverflow {
            time,
            raw_volume: raw.to_string(),
        }
    })?;
    scaled_int
        .checked_add(frac_val)
        .ok_or_else(|| KrakenOhlcParseError::VolumeOverflow {
            time,
            raw_volume: raw.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Symbol mapping
// ---------------------------------------------------------------------------

/// Map a canonical symbol to its verified Kraken query-pair alt-name.
/// Only the two symbols verified live by `CRYPTO-DATA-01S-T` are supported.
pub fn kraken_query_pair_for_canonical_symbol(symbol: &str) -> Option<&'static str> {
    match symbol {
        "BTC/USD" => Some("XBTUSD"),
        "ETH/USD" => Some("ETHUSD"),
        _ => None,
    }
}

/// One configured canonical-symbol -> Kraken identity mapping, sourced from
/// `provider_symbols.kraken_pair` (and, optionally,
/// `provider_symbols.kraken_result_key`) on a registry-v2 instrument. Mirrors
/// `providers::coinlore::coinlore_aliases_from_registry_v2`'s shape and
/// intent for a different provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KrakenAlias {
    pub canonical_symbol: String,
    pub kraken_pair: String,
    pub kraken_result_key: Option<String>,
}

/// Extract [`KrakenAlias`] entries from a registry-v2 document for every
/// instrument that carries `kraken_pair` in its `provider_symbols` map.
/// Instruments missing that key are silently skipped -- this is a read-only
/// projection, not a validator; enablement flags are untouched and unread
/// here.
pub fn kraken_aliases_from_registry_v2(
    registry: &crate::instrument_registry_v2::InstrumentRegistryV2,
) -> Vec<KrakenAlias> {
    registry
        .instruments
        .iter()
        .filter_map(|inst| {
            let kraken_pair = inst.provider_symbols.get("kraken_pair")?;
            Some(KrakenAlias {
                canonical_symbol: inst.symbol.clone(),
                kraken_pair: kraken_pair.clone(),
                kraken_result_key: inst.provider_symbols.get("kraken_result_key").cloned(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Minimal top-level shape: only what is needed to detect an API-level error
/// and to hand the rest to manual `serde_json::Value` validation (Kraken's
/// `result` object has a data-dependent key name, which does not map onto a
/// fixed struct field).
#[derive(Debug, Deserialize)]
struct KrakenOhlcResponseEnvelope {
    #[serde(default)]
    error: Vec<String>,
    #[serde(default)]
    result: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Parse a raw Kraken `/0/public/OHLC` JSON response body for a single query
/// pair into a [`KrakenPairOhlc`]. `query_pair` is the pair the caller
/// queried with (e.g. `"XBTUSD"`) and is carried through for provenance
/// only -- it is not used to select which key of `result` to read (there
/// must be exactly one, see [`KrakenOhlcParseError::NoPairKey`] /
/// [`KrakenOhlcParseError::UnexpectedPairKeyCount`]).
///
/// `interval_seconds` must be [`KRAKEN_INTERVAL_1D_SECONDS`] (86 400); any
/// other value is rejected with `UnsupportedInterval` -- this module
/// supports only the `1D` timeframe verified by `CRYPTO-DATA-01S-T`.
pub fn parse_kraken_ohlc_response(
    body: &str,
    query_pair: &str,
    interval_seconds: i64,
) -> Result<KrakenPairOhlc, KrakenOhlcParseError> {
    if interval_seconds != KRAKEN_INTERVAL_1D_SECONDS {
        return Err(KrakenOhlcParseError::UnsupportedInterval { interval_seconds });
    }

    let envelope: KrakenOhlcResponseEnvelope =
        serde_json::from_str(body).map_err(|err| KrakenOhlcParseError::Decode {
            message: err.to_string(),
        })?;

    if !envelope.error.is_empty() {
        return Err(KrakenOhlcParseError::ApiError {
            messages: envelope.error,
        });
    }

    let result = envelope.result.ok_or(KrakenOhlcParseError::MissingResult)?;

    let last = result
        .get("last")
        .and_then(|v| v.as_i64())
        .ok_or(KrakenOhlcParseError::MissingLast)?;

    let pair_keys: Vec<String> = result
        .keys()
        .filter(|k| k.as_str() != "last")
        .cloned()
        .collect();
    if pair_keys.is_empty() {
        return Err(KrakenOhlcParseError::NoPairKey);
    }
    if pair_keys.len() > 1 {
        return Err(KrakenOhlcParseError::UnexpectedPairKeyCount { found: pair_keys });
    }
    let provider_pair_key = pair_keys.into_iter().next().expect("checked len == 1");

    let rows = result
        .get(&provider_pair_key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
            index: 0,
            message: format!("pair key {provider_pair_key:?} is not an array"),
        })?;

    let mut seen_times: HashSet<i64> = HashSet::new();
    let mut bars = Vec::with_capacity(rows.len());

    for (index, row) in rows.iter().enumerate() {
        let elements = row
            .as_array()
            .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
                index,
                message: "row is not an array".to_string(),
            })?;
        if elements.len() != 8 {
            return Err(KrakenOhlcParseError::MalformedRow {
                index,
                message: format!("expected 8 elements, found {}", elements.len()),
            });
        }

        let time = elements[0]
            .as_i64()
            .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
                index,
                message: format!("time is not an integer: {:?}", elements[0]),
            })?;

        if !seen_times.insert(time) {
            return Err(KrakenOhlcParseError::DuplicateTimestamp { time });
        }

        let open = row_decimal_str(elements, 1, index)?;
        let high = row_decimal_str(elements, 2, index)?;
        let low = row_decimal_str(elements, 3, index)?;
        let close = row_decimal_str(elements, 4, index)?;
        let vwap = row_decimal_str(elements, 5, index)?;
        let volume_raw = elements[6]
            .as_str()
            .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
                index,
                message: format!("volume is not a string: {:?}", elements[6]),
            })?
            .to_string();
        let count = elements[7]
            .as_i64()
            .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
                index,
                message: format!("count is not an integer: {:?}", elements[7]),
            })?;

        let end_ts = time
            .checked_add(interval_seconds)
            .ok_or(KrakenOhlcParseError::EndTsOverflow { time })?;

        bars.push(KrakenOhlcBar {
            time,
            open,
            high,
            low,
            close,
            vwap,
            volume_raw,
            count,
            end_ts,
            is_complete: time <= last,
        });
    }

    Ok(KrakenPairOhlc {
        provider_pair_key,
        query_pair: query_pair.to_string(),
        last,
        bars,
    })
}

fn row_decimal_str(
    elements: &[serde_json::Value],
    position: usize,
    index: usize,
) -> Result<String, KrakenOhlcParseError> {
    let raw = elements[position]
        .as_str()
        .ok_or_else(|| KrakenOhlcParseError::MalformedRow {
            index,
            message: format!(
                "element[{position}] is not a string: {:?}",
                elements[position]
            ),
        })?;
    crate::normalize_price_str(raw).map_err(|err| KrakenOhlcParseError::MalformedRow {
        index,
        message: format!("element[{position}] is not a valid decimal ({raw:?}): {err}"),
    })
}

/// Bounded retry attempts (beyond the initial attempt) for transient Kraken
/// OHLC fetch failures: HTTP 429, retryable 5xx (500/502/503/504), and
/// connect/timeout transport errors. Permanent 4xx and decode/parse failures
/// are never retried -- they are not proven transient.
const KRAKEN_FETCH_RETRY_MAX_ATTEMPTS: u32 = 3;

/// Fallback sleep between retries when no valid `Retry-After` response
/// header is present. Production default; tests inject 0 via
/// [`fetch_kraken_ohlc_body_from`] so no unit test sleeps in real time.
const KRAKEN_FETCH_RETRY_FALLBACK_SLEEP_SECS: u64 = 2;

/// `true` iff `status` is a Kraken OHLC fetch failure proven transient:
/// rate-limited (429) or a retryable server error (500/502/503/504).
/// Any other non-success status (auth/permission, bad request, not-found,
/// etc.) is a permanent failure and must never be retried.
fn kraken_fetch_status_is_retryable(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

/// `true` iff `err` is a connect or timeout failure -- the only transport
/// error classes treated as transient. Request-building or other transport
/// errors are not retried.
fn kraken_fetch_transport_error_is_retryable(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout()
}

/// Parse a `Retry-After` header value as a non-negative integer seconds
/// count. Kraken's public OHLC endpoint does not document this header, but
/// if a proxy/future response supplies one, honor it; any missing or
/// malformed value falls back to the caller's fixed backoff instead of
/// failing the request.
fn kraken_fetch_parse_retry_after_secs(value: Option<&str>) -> Option<u64> {
    value?.trim().parse::<u64>().ok()
}

/// `true` iff a provider-requested `Retry-After` of `secs` is within the
/// permitted in-call retry-delay budget `budget_secs` (the fixed fallback
/// backoff). A wait strictly greater than the budget must never be honored
/// by sleeping, nor clamped down and retried early -- the caller fails
/// closed instead.
fn kraken_fetch_retry_after_within_budget(secs: u64, budget_secs: u64) -> bool {
    secs <= budget_secs
}

/// Fetch the raw Kraken `/0/public/OHLC` response body for `query_pair` at
/// Kraken's `interval=1440` (1D), retrying only proven-transient failures
/// (see [`kraken_fetch_status_is_retryable`] /
/// [`kraken_fetch_transport_error_is_retryable`]) up to
/// [`KRAKEN_FETCH_RETRY_MAX_ATTEMPTS`] times with a fixed fallback backoff,
/// honoring a `Retry-After` header when present and parseable.
///
/// This function performs the network call unconditionally when invoked --
/// it does **not** check any environment variable itself. Callers
/// (CLI/operator surfaces) MUST gate this call behind an explicit operator
/// opt-in (e.g. `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`) before invoking it. No
/// credentials are sent; Kraken's public OHLC endpoint requires none.
pub async fn fetch_kraken_ohlc_body(query_pair: &str) -> anyhow::Result<String> {
    fetch_kraken_ohlc_body_from(
        KRAKEN_BASE_URL,
        query_pair,
        KRAKEN_FETCH_RETRY_MAX_ATTEMPTS,
        KRAKEN_FETCH_RETRY_FALLBACK_SLEEP_SECS,
    )
    .await
}

/// Narrow testable helper behind [`fetch_kraken_ohlc_body`]: parameterizes
/// base URL, max retries, and fallback sleep so tests can point at a mock
/// server and pass `fallback_sleep_secs: 0` (no real sleeps in unit tests).
async fn fetch_kraken_ohlc_body_from(
    base_url: &str,
    query_pair: &str,
    max_retries: u32,
    fallback_sleep_secs: u64,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/0/public/OHLC?pair={query_pair}&interval=1440",
        base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::new();
    let mut retries_remaining = max_retries;

    loop {
        let send_result = client.get(&url).send().await;

        let resp = match send_result {
            Ok(resp) => resp,
            Err(err) => {
                if kraken_fetch_transport_error_is_retryable(&err) && retries_remaining > 0 {
                    retries_remaining -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(fallback_sleep_secs)).await;
                    continue;
                }
                return Err(anyhow::anyhow!("kraken ohlc transport error: {err}"));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            if kraken_fetch_status_is_retryable(status) {
                if retries_remaining > 0 {
                    let retry_after = kraken_fetch_parse_retry_after_secs(
                        resp.headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok()),
                    );
                    // The fixed fallback backoff is also the in-call
                    // retry-delay budget authority: a provider-requested
                    // wait at or under that budget is honored exactly, but
                    // a wait beyond it must fail closed -- never sleep
                    // unboundedly, and never clamp a large requested wait
                    // downward and retry before the provider asked us to.
                    if let Some(secs) = retry_after {
                        if !kraken_fetch_retry_after_within_budget(secs, fallback_sleep_secs) {
                            let body = resp.text().await.unwrap_or_default();
                            return Err(anyhow::anyhow!(
                                "kraken ohlc http status={} requested retry-after={secs}s \
                                 exceeds the permitted in-call retry budget of \
                                 {fallback_sleep_secs}s; failing closed rather than \
                                 sleeping unboundedly or retrying early: {body}",
                                status.as_u16()
                            ));
                        }
                    }
                    retries_remaining -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(
                        retry_after.unwrap_or(fallback_sleep_secs),
                    ))
                    .await;
                    continue;
                }
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "kraken ohlc http status={} persisted after {max_retries} retries: {body}",
                    status.as_u16()
                ));
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "kraken ohlc http error status={}: {body}",
                status.as_u16()
            ));
        }

        return resp
            .text()
            .await
            .map_err(|err| anyhow::anyhow!("kraken ohlc body read error: {err}"));
    }
}

// ---------------------------------------------------------------------------
// Historical provider adapter (disabled-by-default via provider_registry)
// ---------------------------------------------------------------------------

/// Kraken public OHLC historical provider. Implements [`crate::HistoricalProvider`]
/// so it can be wrapped by [`crate::HistoricalProviderMarketDataAdapter`], the
/// same pattern used by Alpaca/TwelveData. This struct performs a real HTTP
/// call inside `fetch_bars` when invoked -- exactly like the existing Alpaca
/// and TwelveData providers -- but is never invoked by default: the
/// `providers.json` `kraken` entry is `enabled: false`, no CLI/ingest command
/// wires it in, and no test in this crate calls `fetch_bars` against a live
/// host (tests use `httpmock`).
#[derive(Debug, Clone)]
pub struct KrakenHistoricalProvider {
    http: reqwest::Client,
    base_url: String,
}

impl Default for KrakenHistoricalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl KrakenHistoricalProvider {
    pub fn new() -> Self {
        Self::new_with_base_url(KRAKEN_BASE_URL.to_string())
    }

    pub fn new_with_base_url(base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn ohlc_url(&self) -> String {
        format!("{}/0/public/OHLC", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait::async_trait]
impl crate::HistoricalProvider for KrakenHistoricalProvider {
    fn source_name(&self) -> &'static str {
        KRAKEN_PROVIDER_ID
    }

    async fn fetch_bars(&self, req: FetchBarsRequest) -> anyhow::Result<Vec<ProviderBar>> {
        if req.timeframe != crate::Timeframe::D1 {
            anyhow::bail!(
                "kraken: only timeframe 1D is supported by this adapter, got {}",
                req.timeframe.as_str()
            );
        }

        let mut out: Vec<ProviderBar> = Vec::new();
        for symbol in &req.symbols {
            let query_pair = kraken_query_pair_for_canonical_symbol(symbol).ok_or_else(|| {
                anyhow::anyhow!(
                    "kraken: unsupported symbol {symbol}; only BTC/USD and ETH/USD are supported"
                )
            })?;

            let resp = self
                .http
                .get(self.ohlc_url())
                .query(&[("pair", query_pair), ("interval", "1440")])
                .send()
                .await
                .map_err(|err| anyhow::anyhow!("kraken ohlc request failed: {err}"))?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("kraken ohlc http error status={}: {body}", status.as_u16());
            }

            let body = resp
                .text()
                .await
                .map_err(|err| anyhow::anyhow!("kraken ohlc response read failed: {err}"))?;

            let parsed = parse_kraken_ohlc_response(&body, query_pair, KRAKEN_INTERVAL_1D_SECONDS)
                .map_err(|err| anyhow::anyhow!("kraken ohlc parse failed for {symbol}: {err}"))?;

            let bars = parsed
                .completed_provider_bars(symbol, req.timeframe.as_str())
                .map_err(|err| {
                    anyhow::anyhow!("kraken ohlc bar conversion failed for {symbol}: {err}")
                })?;
            out.extend(bars);
        }

        out.sort_by(|a, b| {
            a.symbol
                .cmp(&b.symbol)
                .then_with(|| a.end_ts.cmp(&b.end_ts))
        });
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const BTC_FIXTURE: &str = include_str!("../../tests/fixtures/kraken_ohlc_xbtusd_1d.json");
    const ETH_FIXTURE: &str = include_str!("../../tests/fixtures/kraken_ohlc_ethusd_1d.json");

    // -----------------------------------------------------------------------
    // Volume scaling (CRYPTO-DATA-01V)
    // -----------------------------------------------------------------------

    // KV-01: BTC's live-verified volume scales to the documented exact value.
    #[test]
    fn kv01_btc_volume_scales_to_documented_exact_value() {
        assert_eq!(
            kraken_volume_to_scaled_i64("1317.15941434", 0).unwrap(),
            131_715_941_434
        );
    }

    // KV-02: ETH's live-verified volume scales to the documented exact value.
    #[test]
    fn kv02_eth_volume_scales_to_documented_exact_value() {
        assert_eq!(
            kraken_volume_to_scaled_i64("15588.01759742", 0).unwrap(),
            1_558_801_759_742
        );
    }

    // KV-03: integer-only volume (no fractional part) scales cleanly.
    #[test]
    fn kv03_integer_only_volume_scales_cleanly() {
        assert_eq!(kraken_volume_to_scaled_i64("42", 0).unwrap(), 4_200_000_000);
    }

    // KV-04: fewer than 8 fractional digits are right-padded, not left-padded.
    #[test]
    fn kv04_short_fraction_is_right_padded() {
        // 0.5 * 1e8 == 50_000_000, not 5.
        assert_eq!(kraken_volume_to_scaled_i64("1.5", 0).unwrap(), 150_000_000);
    }

    // KV-05: more than 8 fractional digits is rejected, not truncated.
    #[test]
    fn kv05_too_many_fractional_digits_rejected() {
        let err = kraken_volume_to_scaled_i64("1.123456789", 42).unwrap_err();
        assert_eq!(
            err,
            KrakenOhlcParseError::VolumeTooManyDecimals {
                time: 42,
                raw_volume: "1.123456789".to_string(),
                max_decimals: KRAKEN_VOLUME_SCALE_DECIMALS,
            }
        );
    }

    // KV-06: negative volume is rejected.
    #[test]
    fn kv06_negative_volume_rejected() {
        let err = kraken_volume_to_scaled_i64("-1.5", 0).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::VolumeMalformed { .. }));
    }

    // KV-07: non-decimal garbage is rejected.
    #[test]
    fn kv07_non_decimal_garbage_rejected() {
        let err = kraken_volume_to_scaled_i64("not-a-number", 0).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::VolumeMalformed { .. }));
    }

    // KV-08: overflow during scaling is rejected, not wrapped.
    #[test]
    fn kv08_overflow_rejected() {
        // i64::MAX ~= 9.2e18; scaling anything with an integer part near that
        // by 1e8 overflows.
        let huge = format!("{}", i64::MAX);
        let err = kraken_volume_to_scaled_i64(&huge, 0).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::VolumeOverflow { .. }));
    }

    // -----------------------------------------------------------------------
    // Fixture parsing — BTC/USD
    // -----------------------------------------------------------------------

    // KP-01: BTC fixture parses; exactly one forming row is excluded.
    #[test]
    fn kp01_btc_fixture_parses_and_excludes_forming_row() {
        let parsed =
            parse_kraken_ohlc_response(BTC_FIXTURE, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        assert_eq!(parsed.provider_pair_key, "XXBTZUSD");
        assert_eq!(parsed.query_pair, "XBTUSD");
        assert_eq!(
            parsed.bars.len(),
            3,
            "fixture carries 2 committed + 1 forming row"
        );
        assert_eq!(parsed.completed_bars().len(), 2);
        assert_eq!(parsed.forming_bars().len(), 1);
    }

    // KP-02: the last committed BTC row matches CRYPTO-DATA-01S-T's exact
    // live-verified values.
    #[test]
    fn kp02_btc_last_committed_row_matches_live_verified_values() {
        let parsed =
            parse_kraken_ohlc_response(BTC_FIXTURE, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        let last_committed = parsed
            .completed_bars()
            .into_iter()
            .max_by_key(|b| b.time)
            .unwrap();
        assert_eq!(last_committed.time, 1_783_123_200);
        assert_eq!(last_committed.end_ts, 1_783_209_600);
        assert_eq!(last_committed.open, "62539");
        assert_eq!(last_committed.close, "63085.8");
        assert_eq!(last_committed.volume_raw, "1317.15941434");
        assert!(last_committed.is_complete);
    }

    // KP-03: the forming BTC row is excluded from completed_provider_bars.
    #[test]
    fn kp03_btc_forming_row_never_appears_in_provider_bars() {
        let parsed =
            parse_kraken_ohlc_response(BTC_FIXTURE, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        let bars = parsed.completed_provider_bars("BTC/USD", "1D").unwrap();
        assert_eq!(bars.len(), 2);
        assert!(
            bars.iter().all(|b| b.end_ts != 1_783_296_000),
            "forming row's end_ts must never appear in converted ProviderBars"
        );
        assert!(bars.iter().all(|b| b.is_complete));
    }

    // KP-04: ProviderBar conversion applies the documented volume scale.
    #[test]
    fn kp04_btc_provider_bar_volume_is_scaled() {
        let parsed =
            parse_kraken_ohlc_response(BTC_FIXTURE, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        let bars = parsed.completed_provider_bars("BTC/USD", "1D").unwrap();
        let last = bars.iter().max_by_key(|b| b.end_ts).unwrap();
        assert_eq!(last.volume, 131_715_941_434);
        assert_eq!(last.symbol, "BTC/USD");
        assert_eq!(last.timeframe, "1D");
    }

    // -----------------------------------------------------------------------
    // Fixture parsing — ETH/USD
    // -----------------------------------------------------------------------

    // KP-05: ETH fixture parses with the same shape/semantics as BTC.
    #[test]
    fn kp05_eth_fixture_parses_and_excludes_forming_row() {
        let parsed =
            parse_kraken_ohlc_response(ETH_FIXTURE, "ETHUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        assert_eq!(parsed.provider_pair_key, "XETHZUSD");
        assert_eq!(parsed.completed_bars().len(), 2);
        assert_eq!(parsed.forming_bars().len(), 1);
    }

    // KP-06: ETH's last committed row matches CRYPTO-DATA-01S-T's exact
    // live-verified values and scaled volume.
    #[test]
    fn kp06_eth_last_committed_row_matches_live_verified_values() {
        let parsed =
            parse_kraken_ohlc_response(ETH_FIXTURE, "ETHUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap();
        let bars = parsed.completed_provider_bars("ETH/USD", "1D").unwrap();
        let last = bars.iter().max_by_key(|b| b.end_ts).unwrap();
        assert_eq!(last.end_ts, 1_783_209_600);
        assert_eq!(last.close, "1778.72");
        assert_eq!(last.volume, 1_558_801_759_742);
    }

    // -----------------------------------------------------------------------
    // Rejection behavior
    // -----------------------------------------------------------------------

    // KP-07: non-empty top-level `error` is rejected.
    #[test]
    fn kp07_non_empty_error_array_rejected() {
        let body = r#"{"error": ["EQuery:Unknown asset pair"], "result": {}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::ApiError { .. }));
    }

    // KP-08: missing `result` is rejected.
    #[test]
    fn kp08_missing_result_rejected() {
        let body = r#"{"error": []}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert_eq!(err, KrakenOhlcParseError::MissingResult);
    }

    // KP-09: missing `last` is rejected.
    #[test]
    fn kp09_missing_last_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": []}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert_eq!(err, KrakenOhlcParseError::MissingLast);
    }

    // KP-10: zero pair keys (besides `last`) is rejected.
    #[test]
    fn kp10_zero_pair_keys_rejected() {
        let body = r#"{"error": [], "result": {"last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert_eq!(err, KrakenOhlcParseError::NoPairKey);
    }

    // KP-11: multiple unexpected pair keys is rejected.
    #[test]
    fn kp11_multiple_pair_keys_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [], "XETHZUSD": [], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(
            err,
            KrakenOhlcParseError::UnexpectedPairKeyCount { .. }
        ));
    }

    // KP-12: a row with != 8 elements is rejected.
    #[test]
    fn kp12_malformed_row_length_rejected() {
        let body =
            r#"{"error": [], "result": {"XXBTZUSD": [[1000, "1", "2", "3", "4"]], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::MalformedRow { .. }));
    }

    // KP-13: a non-integer time is rejected.
    #[test]
    fn kp13_non_integer_time_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [["not-a-time", "1", "2", "3", "4", "5", "6", 7]], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::MalformedRow { .. }));
    }

    // KP-14: a non-decimal OHLC field is rejected.
    #[test]
    fn kp14_non_decimal_ohlc_field_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [[1000, "not-decimal", "2", "3", "4", "5", "6", 7]], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::MalformedRow { .. }));
    }

    // KP-15: missing/non-string volume is rejected.
    #[test]
    fn kp15_non_string_volume_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [[1000, "1", "2", "3", "4", "5", 6, 7]], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::MalformedRow { .. }));
    }

    // KP-16: non-integer count is rejected.
    #[test]
    fn kp16_non_integer_count_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [[1000, "1", "2", "3", "4", "5", "6", "not-a-count"]], "last": 1000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::MalformedRow { .. }));
    }

    // KP-17: duplicate timestamps are rejected.
    #[test]
    fn kp17_duplicate_timestamps_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [
            [1000, "1", "2", "0.5", "1.5", "1", "10.0", 1],
            [1000, "1", "2", "0.5", "1.5", "1", "10.0", 1]
        ], "last": 2000}}"#;
        let err =
            parse_kraken_ohlc_response(body, "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS).unwrap_err();
        assert_eq!(err, KrakenOhlcParseError::DuplicateTimestamp { time: 1000 });
    }

    // KP-18: unsupported interval is rejected.
    #[test]
    fn kp18_unsupported_interval_rejected() {
        let body = r#"{"error": [], "result": {"XXBTZUSD": [], "last": 1000}}"#;
        let err = parse_kraken_ohlc_response(body, "XBTUSD", 3_600).unwrap_err();
        assert_eq!(
            err,
            KrakenOhlcParseError::UnsupportedInterval {
                interval_seconds: 3_600
            }
        );
    }

    // KP-19: malformed top-level JSON is rejected.
    #[test]
    fn kp19_malformed_json_rejected() {
        let err = parse_kraken_ohlc_response("{ not json", "XBTUSD", KRAKEN_INTERVAL_1D_SECONDS)
            .unwrap_err();
        assert!(matches!(err, KrakenOhlcParseError::Decode { .. }));
    }

    // KP-20: converting a forming (incomplete) bar directly is refused.
    #[test]
    fn kp20_direct_conversion_of_forming_bar_refused() {
        let forming = KrakenOhlcBar {
            time: 5000,
            open: "1".to_string(),
            high: "2".to_string(),
            low: "0.5".to_string(),
            close: "1.5".to_string(),
            vwap: "1".to_string(),
            volume_raw: "10.0".to_string(),
            count: 1,
            end_ts: 5000 + KRAKEN_INTERVAL_1D_SECONDS,
            is_complete: false,
        };
        let err = kraken_bar_to_provider_bar(&forming, "BTC/USD", "1D").unwrap_err();
        assert_eq!(
            err,
            KrakenOhlcParseError::IncompleteBarConversionRefused { time: 5000 }
        );
    }

    // -----------------------------------------------------------------------
    // Symbol mapping
    // -----------------------------------------------------------------------

    // KS-01: canonical BTC/USD and ETH/USD map to their verified query pairs.
    #[test]
    fn ks01_canonical_symbols_map_to_verified_query_pairs() {
        assert_eq!(
            kraken_query_pair_for_canonical_symbol("BTC/USD"),
            Some("XBTUSD")
        );
        assert_eq!(
            kraken_query_pair_for_canonical_symbol("ETH/USD"),
            Some("ETHUSD")
        );
    }

    // KS-02: an unrecognized symbol maps to None, not a fabricated pair.
    #[test]
    fn ks02_unrecognized_symbol_maps_to_none() {
        assert_eq!(kraken_query_pair_for_canonical_symbol("DOGE/USD"), None);
    }

    // -----------------------------------------------------------------------
    // Registry alias extraction
    // -----------------------------------------------------------------------

    // KR-01: alias extraction skips instruments without kraken_pair, and
    // reads kraken_result_key when present.
    #[test]
    fn kr01_alias_extraction_reads_both_symbols_from_registry() {
        use crate::instrument_registry_v2::load_instrument_registry_v2;
        use std::path::PathBuf;

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = PathBuf::from(manifest_dir)
            .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json");
        let registry = load_instrument_registry_v2(&path).expect("registry must load");
        let aliases = kraken_aliases_from_registry_v2(&registry);

        assert_eq!(
            aliases.len(),
            2,
            "both BTC/USD and ETH/USD carry kraken_pair"
        );
        let btc = aliases
            .iter()
            .find(|a| a.canonical_symbol == "BTC/USD")
            .expect("BTC/USD alias must be present");
        assert_eq!(btc.kraken_pair, "XBTUSD");
        assert_eq!(btc.kraken_result_key.as_deref(), Some("XXBTZUSD"));

        let eth = aliases
            .iter()
            .find(|a| a.canonical_symbol == "ETH/USD")
            .expect("ETH/USD alias must be present");
        assert_eq!(eth.kraken_pair, "ETHUSD");
        assert_eq!(eth.kraken_result_key.as_deref(), Some("XETHZUSD"));
    }

    // -----------------------------------------------------------------------
    // HistoricalProvider adapter (httpmock — no live network)
    // -----------------------------------------------------------------------

    // KH-01: fetch_bars against a mocked Kraken response returns only
    // completed bars with scaled volume.
    #[tokio::test]
    async fn kh01_fetch_bars_returns_completed_bars_only() {
        use crate::HistoricalProvider;
        use httpmock::prelude::*;

        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/0/public/OHLC");
            then.status(200).body(BTC_FIXTURE);
        });

        let provider = KrakenHistoricalProvider::new_with_base_url(server.base_url());
        let req = FetchBarsRequest {
            symbols: vec!["BTC/USD".to_string()],
            timeframe: crate::Timeframe::D1,
            start: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap(),
        };

        let bars = provider.fetch_bars(req).await.unwrap();
        assert_eq!(bars.len(), 2, "only the 2 completed rows must be returned");
        assert!(bars.iter().all(|b| b.is_complete));
        assert!(bars.iter().all(|b| b.symbol == "BTC/USD"));
    }

    // KH-02: fetch_bars rejects an unsupported timeframe before any network call.
    #[tokio::test]
    async fn kh02_fetch_bars_rejects_unsupported_timeframe() {
        use crate::HistoricalProvider;

        let provider =
            KrakenHistoricalProvider::new_with_base_url("http://localhost:1".to_string());
        let req = FetchBarsRequest {
            symbols: vec!["BTC/USD".to_string()],
            timeframe: crate::Timeframe::M5,
            start: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap(),
        };

        let err = provider.fetch_bars(req).await.unwrap_err();
        assert!(err.to_string().contains("only timeframe 1D is supported"));
    }

    // KH-03: fetch_bars rejects an unsupported symbol before any network call
    // succeeds meaningfully (the mock never registers a matching pair).
    #[tokio::test]
    async fn kh03_fetch_bars_rejects_unsupported_symbol() {
        use crate::HistoricalProvider;

        let provider =
            KrakenHistoricalProvider::new_with_base_url("http://localhost:1".to_string());
        let req = FetchBarsRequest {
            symbols: vec!["DOGE/USD".to_string()],
            timeframe: crate::Timeframe::D1,
            start: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap(),
        };

        let err = provider.fetch_bars(req).await.unwrap_err();
        assert!(err.to_string().contains("unsupported symbol"));
    }

    // KH-04: provider name is "kraken".
    #[test]
    fn kh04_provider_name_is_kraken() {
        use crate::HistoricalProvider;
        let provider = KrakenHistoricalProvider::new();
        assert_eq!(provider.source_name(), "kraken");
    }

    // -----------------------------------------------------------------------
    // fetch_kraken_ohlc_body retry/backoff (MD-KRAKEN-FETCH-RETRY-BACKOFF-01)
    // -----------------------------------------------------------------------

    // Per-test call counters for the "fails N times then succeeds" httpmock
    // pattern (custom `.matches()` closures must be non-capturing fn ptrs in
    // httpmock 0.7, so each test needing this shape gets its own static).
    static KF01_CALL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static KF02_CALL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    // KF-01: a single transient 429 followed by success returns the body
    // from the successful retry, having actually retried (2 total hits).
    #[tokio::test]
    async fn kf01_transient_429_then_success_returns_body() {
        use httpmock::prelude::*;
        use std::sync::atomic::Ordering;

        KF01_CALL.store(0, Ordering::SeqCst);
        let server = MockServer::start();

        // httpmock's internal mock table is a BTreeMap keyed by an
        // auto-incrementing id, so `find_mock` checks mocks in registration
        // order (first-registered wins ties) -- register the conditional
        // mock first so it gets first refusal, then the unconditional
        // catch-all second.
        let mock_429 = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD")
                .matches(|_req: &HttpMockRequest| KF01_CALL.fetch_add(1, Ordering::SeqCst) < 1);
            then.status(429).body("rate limited");
        });
        let mock_ok = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(200).body("ok-body-after-retry");
        });

        let body = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 1, 0)
            .await
            .unwrap();
        assert_eq!(body, "ok-body-after-retry");
        // httpmock's own assert_hits() is unreliable with a side-effecting
        // `.matches()` closure on this platform/version, so the retry is
        // proven via the counter driving the matcher directly: exactly 2
        // real requests occurred (429 then a fresh retry that only mock_ok
        // could have served, since mock_429 no longer matches call #2).
        assert_eq!(KF01_CALL.load(Ordering::SeqCst), 2);
        let _ = (&mock_ok, &mock_429);
    }

    // KF-02: a retryable 503 that recovers on the next attempt succeeds and
    // returns the successful body, proving the loop actually retries a
    // 5xx rather than failing fast.
    #[tokio::test]
    async fn kf02_retryable_5xx_then_success_returns_body() {
        use httpmock::prelude::*;
        use std::sync::atomic::Ordering;

        KF02_CALL.store(0, Ordering::SeqCst);
        let server = MockServer::start();

        // See kf01 above: register the conditional mock first so it is
        // checked before the unconditional catch-all (registration order,
        // not LIFO).
        let mock_503 = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD")
                .matches(|_req: &HttpMockRequest| KF02_CALL.fetch_add(1, Ordering::SeqCst) < 1);
            then.status(503).body("service unavailable");
        });
        let mock_ok = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(200).body("ok-body-after-5xx-retry");
        });

        let body = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 1, 0)
            .await
            .unwrap();
        assert_eq!(body, "ok-body-after-5xx-retry");
        assert_eq!(KF02_CALL.load(Ordering::SeqCst), 2);
        let _ = (&mock_ok, &mock_503);
    }

    // KF-03: an always-retryable 503 exhausts the bounded retry budget and
    // returns a truthful "persisted after N retries" error, with the mock
    // hit exactly max_retries + 1 times (bounded, not unbounded).
    #[tokio::test]
    async fn kf03_retryable_5xx_exhaustion_is_bounded_and_truthful() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(503).body("service unavailable");
        });

        let err = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 2, 0)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("persisted after 2 retries"),
            "error must truthfully state retry exhaustion: {err}"
        );
        mock.assert_hits(3); // 1 initial attempt + 2 retries, never more.
    }

    // KF-04: a permanent 4xx (404) is never retried -- exactly one attempt.
    #[tokio::test]
    async fn kf04_permanent_4xx_is_never_retried() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(404).body("unknown pair");
        });

        let err = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 5, 0)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("http error status=404"));
        mock.assert_hits(1);
    }

    // KF-05: a successful (200) response with a malformed body is returned
    // as-is -- decode/parse failures happen downstream and must never
    // trigger a fetch-layer retry.
    #[tokio::test]
    async fn kf05_malformed_200_body_is_not_retried() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(200).body("{ not json");
        });

        let body = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 5, 0)
            .await
            .unwrap();
        assert_eq!(body, "{ not json");
        mock.assert_hits(1);
    }

    // KF-06: attempts are strictly bounded -- max_retries=0 means exactly
    // one attempt, never a retry, on a retryable status.
    #[tokio::test]
    async fn kf06_zero_max_retries_means_exactly_one_attempt() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(429).body("rate limited");
        });

        let err = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 0, 0)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("persisted after 0 retries"));
        mock.assert_hits(1);
    }

    // KF-07: a connect failure (nothing listening) is retried up to the
    // bound and then fails with a truthful transport-error message -- no
    // hang, no unbounded loop.
    #[tokio::test]
    async fn kf07_connect_failure_is_bounded_and_truthful() {
        // Port 1 is a reserved/unassigned port; nothing listens there, so
        // this reliably produces a connect error without any real network
        // dependency or flakiness.
        let err = fetch_kraken_ohlc_body_from("http://127.0.0.1:1", "XBTUSD", 2, 0)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("kraken ohlc transport error"),
            "must surface a truthful transport error, got: {err}"
        );
    }

    // KF-08: Retry-After header value, when present and a valid integer, is
    // parsed; a malformed Retry-After falls back to the fixed backoff rather
    // than failing the request outright.
    #[test]
    fn kf08_retry_after_parsing_accepts_valid_and_falls_back_on_malformed() {
        assert_eq!(kraken_fetch_parse_retry_after_secs(Some("5")), Some(5));
        assert_eq!(kraken_fetch_parse_retry_after_secs(Some(" 5 ")), Some(5));
        assert_eq!(kraken_fetch_parse_retry_after_secs(Some("not-a-number")), None);
        assert_eq!(kraken_fetch_parse_retry_after_secs(Some("")), None);
        assert_eq!(kraken_fetch_parse_retry_after_secs(None), None);
    }

    // KF-09: status classification matches exactly the documented transient
    // set -- 429 and 500/502/503/504 -- and nothing else.
    #[test]
    fn kf09_status_retryable_classification_is_exact() {
        for code in [429u16, 500, 502, 503, 504] {
            assert!(
                kraken_fetch_status_is_retryable(reqwest::StatusCode::from_u16(code).unwrap()),
                "status {code} must be classified retryable"
            );
        }
        for code in [400u16, 401, 403, 404, 405, 501, 505] {
            assert!(
                !kraken_fetch_status_is_retryable(reqwest::StatusCode::from_u16(code).unwrap()),
                "status {code} must NOT be classified retryable"
            );
        }
    }

    // KF-10: the retry-after/budget comparison is an exact boundary -- equal
    // to the budget is within it, one second over is not.
    #[test]
    fn kf10_retry_after_budget_boundary_is_exact() {
        assert!(kraken_fetch_retry_after_within_budget(0, 2));
        assert!(kraken_fetch_retry_after_within_budget(2, 2));
        assert!(!kraken_fetch_retry_after_within_budget(3, 2));
        assert!(kraken_fetch_retry_after_within_budget(0, 0));
        assert!(!kraken_fetch_retry_after_within_budget(1, 0));
    }

    // KF-11: a numeric Retry-After at or under the in-call retry budget
    // (the fixed fallback backoff) is honored -- the retry actually
    // happens and returns the successful body.
    #[tokio::test]
    async fn kf11_retry_after_within_budget_is_honored() {
        use httpmock::prelude::*;

        let server = MockServer::start();

        static KF11_CALL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        KF11_CALL.store(0, std::sync::atomic::Ordering::SeqCst);

        let mock_first = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD")
                .matches(|_req: &HttpMockRequest| {
                    KF11_CALL.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 1
                });
            then.status(429).header("Retry-After", "0").body("rate limited");
        });
        let mock_ok = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(200).body("ok-body-after-budgeted-retry-after");
        });

        // Budget (fallback_sleep_secs) is 0, matching the header's requested
        // Retry-After of 0s exactly -- within budget, so the retry happens.
        let body = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 1, 0)
            .await
            .unwrap();
        assert_eq!(body, "ok-body-after-budgeted-retry-after");
        assert_eq!(KF11_CALL.load(std::sync::atomic::Ordering::SeqCst), 2);
        let _ = (&mock_first, &mock_ok);
    }

    // KF-12: a numeric Retry-After that exceeds the in-call retry budget
    // fails closed immediately -- no sleep, no retry attempted, and the
    // error names the exceeded budget rather than silently clamping the
    // wait down and retrying early.
    #[tokio::test]
    async fn kf12_retry_after_above_budget_fails_closed_without_retry() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/0/public/OHLC")
                .query_param("pair", "XBTUSD");
            then.status(429)
                .header("Retry-After", "3600")
                .body("rate limited");
        });

        // Budget (fallback_sleep_secs) is 0; the provider's requested
        // 3600s wait is far beyond it, so this must fail closed on the
        // very first response rather than sleeping for an hour.
        let err = fetch_kraken_ohlc_body_from(&server.base_url(), "XBTUSD", 5, 0)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeds the permitted in-call retry budget"),
            "must fail closed naming the exceeded budget, got: {err}"
        );
        mock.assert_hits(1);
    }
}
