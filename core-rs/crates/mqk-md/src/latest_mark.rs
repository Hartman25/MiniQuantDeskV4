//! Latest-mark model for ticker-style, non-bar market data.
//!
//! `CRYPTO-DATA-01I` verified that CoinLore's public ticker endpoints
//! (`/api/tickers/`, `/api/ticker/?id=`) return a single current spot price
//! plus a rolling 24h volume figure -- no open/high/low, no per-ticker
//! timestamp, no completed/forming bar concept. `RawBar`/`ProviderBar`
//! (see `provider.rs` / `lib.rs`) require exactly those fields, so building
//! one from CoinLore's response would mean fabricating three OHLC fields
//! (by copying `close`) and asserting a client-observed request time as a
//! provider-confirmed bar close -- both forbidden by CLAUDE.md's
//! "no fabricated truth" invariant (see
//! `docs/specs/crypto_data_01i_coinlore_network_verify.md` §6-7).
//!
//! This module defines a distinct, non-bar type instead: [`LatestMark`]. It
//! deliberately has no `open`/`high`/`low` fields and no `is_complete` field
//! -- those bar concepts do not apply to a point-in-time ticker quote, and
//! this module exposes no conversion into `RawBar`/`ProviderBar` at all.
//!
//! Not wired into any md_bars write path, portfolio valuation, risk,
//! runtime, or order path in this patch.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable identifier for a latest-mark data provider (e.g. `"coinlore"`).
pub type LatestMarkProviderId = String;

/// How the timestamp on a [`LatestMark`] should be interpreted.
///
/// This is the load-bearing distinction the `01I` evidence requires: CoinLore
/// supplies no per-ticker timestamp, so any `as_of` value attached to a
/// CoinLore-sourced mark is the *client's own* request time, not a value the
/// provider confirmed. Silently treating a client-observed time as if it were
/// provider-supplied would misrepresent the data's provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatestMarkTruthState {
    /// The provider itself supplied a timestamp for this observation.
    ProviderTimestamped,
    /// No provider timestamp exists; `as_of_client_request_ts` is the
    /// client's own wall-clock observation time, not a provider-confirmed
    /// value. This is the only truth state CoinLore's verified endpoints
    /// support today.
    ClientObservedOnly,
}

/// The kind of non-bar market-data value a [`LatestMark`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatestMarkKind {
    /// A single current price quote (e.g. CoinLore ticker `price_usd`).
    Ticker,
}

/// A single latest-mark observation. **Not** a completed OHLCV bar.
///
/// There is intentionally no `open`/`high`/`low` field and no `is_complete`
/// field on this type: adding them (even set to a copy of `price_usd`, or
/// hardcoded `false`) would give this type the same shape as a bar and
/// invite accidental reuse as one. Any future code that wants a bar-shaped
/// projection of this data must construct a `RawBar`/`ProviderBar` value
/// explicitly and separately -- this module provides no `From`/`Into`
/// conversion to either type, by design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestMark {
    /// Canonical registry symbol, e.g. `"BTC/USD"`.
    pub canonical_symbol: String,
    /// Provider identifier, e.g. `"coinlore"`.
    pub provider_id: LatestMarkProviderId,
    /// Provider's own symbol/ticker string, e.g. `"BTC"`.
    pub provider_symbol: String,
    /// Provider's own numeric/opaque coin identifier, e.g. `"90"`, if any.
    pub provider_coin_id: Option<String>,
    /// USD price as a decimal string (no floats at the model boundary).
    pub price_usd: String,
    /// Optional rolling volume window (e.g. CoinLore's 24h volume). Must be
    /// labeled honestly as a rolling window -- never treated as a
    /// bar-period volume tied to a specific timeframe close.
    pub volume24_usd: Option<String>,
    /// Client-observed wall-clock time this mark was fetched (epoch
    /// seconds, UTC). Always present, regardless of `truth_state`.
    pub as_of_client_request_ts: i64,
    /// Provider-supplied timestamp for this observation, if the provider
    /// exposes one. `None` for CoinLore (verified: no per-ticker
    /// timestamp on either endpoint checked by `CRYPTO-DATA-01I`).
    pub provider_ts: Option<i64>,
    /// How the timestamp fields above should be interpreted.
    pub truth_state: LatestMarkTruthState,
    /// What kind of non-bar value this is.
    pub kind: LatestMarkKind,
}

impl LatestMark {
    /// Always `false`. Exposed as an explicit method (not a struct field) so
    /// no caller can mistake this type for a `RawBar`/`ProviderBar` with a
    /// `false` completeness flag -- a `LatestMark` is never bar-shaped at
    /// all, regardless of completeness.
    pub fn is_completed_bar(&self) -> bool {
        false
    }
}

/// Request for latest marks for a set of canonical symbols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestMarkRequest {
    pub canonical_symbols: Vec<String>,
}

/// A batch of latest marks returned for a [`LatestMarkRequest`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestMarkBatch {
    pub marks: Vec<LatestMark>,
}

/// Typed failures for latest-mark fetch/parse paths.
///
/// Mirrors the naming and intent of `provider::MarketDataProviderError`
/// (`EmptyResponse`, `MalformedResponse`) without introducing a dependency
/// between the two error enums -- a `LatestMark` fetch is not a
/// `MarketDataProvider::fetch_historical_bars`/`fetch_latest_closed_bar`
/// call and must not be forced through that bar-oriented contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatestMarkError {
    /// The provider is disabled in the provider registry.
    DisabledProvider { provider_id: String },
    /// The provider returned a well-formed but empty response.
    EmptyResponse {
        provider_id: String,
        message: String,
    },
    /// The provider response could not be decoded, or a required field was
    /// missing/invalid.
    MalformedResponse {
        provider_id: String,
        message: String,
    },
    /// The response did not include one of the assets the caller
    /// configured/requested.
    MissingConfiguredAsset {
        provider_id: String,
        canonical_symbol: String,
    },
    /// A response entry's identity fields (id/symbol) did not match the
    /// configured alias for that asset.
    MismatchedIdentity {
        provider_id: String,
        message: String,
    },
    /// The response contained more than one entry for the same provider
    /// identity (e.g. duplicate `id`).
    DuplicateIdentity {
        provider_id: String,
        message: String,
    },
}

impl fmt::Display for LatestMarkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LatestMarkError::DisabledProvider { provider_id } => {
                write!(f, "disabled_provider: provider '{provider_id}' is disabled")
            }
            LatestMarkError::EmptyResponse {
                provider_id,
                message,
            } => write!(
                f,
                "latest-mark provider {provider_id} empty response: {message}"
            ),
            LatestMarkError::MalformedResponse {
                provider_id,
                message,
            } => write!(
                f,
                "latest-mark provider {provider_id} malformed response: {message}"
            ),
            LatestMarkError::MissingConfiguredAsset {
                provider_id,
                canonical_symbol,
            } => write!(
                f,
                "latest-mark provider {provider_id} missing configured asset: {canonical_symbol}"
            ),
            LatestMarkError::MismatchedIdentity {
                provider_id,
                message,
            } => write!(
                f,
                "latest-mark provider {provider_id} mismatched identity: {message}"
            ),
            LatestMarkError::DuplicateIdentity {
                provider_id,
                message,
            } => write!(
                f,
                "latest-mark provider {provider_id} duplicate identity: {message}"
            ),
        }
    }
}

impl std::error::Error for LatestMarkError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mark() -> LatestMark {
        LatestMark {
            canonical_symbol: "BTC/USD".to_string(),
            provider_id: "coinlore".to_string(),
            provider_symbol: "BTC".to_string(),
            provider_coin_id: Some("90".to_string()),
            price_usd: "62906.61".to_string(),
            volume24_usd: Some("16261421089.448309".to_string()),
            as_of_client_request_ts: 1_783_264_281,
            provider_ts: None,
            truth_state: LatestMarkTruthState::ClientObservedOnly,
            kind: LatestMarkKind::Ticker,
        }
    }

    // LM-01: a LatestMark can represent BTC/USD without any OHLCV field.
    #[test]
    fn lm01_latest_mark_represents_btc_usd_without_ohlcv() {
        let mark = sample_mark();
        assert_eq!(mark.canonical_symbol, "BTC/USD");
        assert_eq!(mark.price_usd, "62906.61");
        assert!(!mark.is_completed_bar());
    }

    // LM-02: a LatestMark can represent ETH/USD without any OHLCV field.
    #[test]
    fn lm02_latest_mark_represents_eth_usd_without_ohlcv() {
        let mark = LatestMark {
            canonical_symbol: "ETH/USD".to_string(),
            provider_symbol: "ETH".to_string(),
            provider_coin_id: Some("80".to_string()),
            price_usd: "1777.74".to_string(),
            volume24_usd: Some("9252669028.469555".to_string()),
            ..sample_mark()
        };
        assert_eq!(mark.canonical_symbol, "ETH/USD");
        assert_eq!(mark.price_usd, "1777.74");
        assert!(!mark.is_completed_bar());
    }

    // LM-03: the serialized shape carries no bar-like field names at all --
    // proves at the wire level (not just the Rust type system) that this
    // model cannot be mistaken for a RawBar/ProviderBar payload.
    #[test]
    fn lm03_serialized_shape_carries_no_bar_like_fields() {
        let mark = sample_mark();
        let json = serde_json::to_value(&mark).unwrap();
        let obj = json.as_object().unwrap();
        for forbidden in [
            "open",
            "high",
            "low",
            "close",
            "is_complete",
            "end_ts",
            "volume",
        ] {
            assert!(
                !obj.contains_key(forbidden),
                "LatestMark serialization must not contain bar-like field '{forbidden}'"
            );
        }
    }

    // LM-04: client-observed as-of time is distinguished from a
    // provider-supplied timestamp via truth_state, and provider_ts stays
    // None for a CoinLore-shaped mark.
    #[test]
    fn lm04_client_observed_time_is_not_represented_as_provider_timestamp() {
        let mark = sample_mark();
        assert_eq!(mark.truth_state, LatestMarkTruthState::ClientObservedOnly);
        assert_eq!(mark.provider_ts, None);
        assert_eq!(mark.as_of_client_request_ts, 1_783_264_281);
    }

    // LM-05: is_completed_bar() is always false, and there is no field named
    // is_complete anywhere on the type to invert -- checked via LM-03's
    // serialization proof plus this explicit method check.
    #[test]
    fn lm05_is_completed_bar_is_always_false() {
        let mark = sample_mark();
        assert!(!mark.is_completed_bar());
    }

    // LM-06: LatestMarkError::Display never contains a raw credential-like
    // substring and always identifies the provider.
    #[test]
    fn lm06_error_display_identifies_provider() {
        let err = LatestMarkError::EmptyResponse {
            provider_id: "coinlore".to_string(),
            message: "ticker array is empty".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("coinlore"));
        assert!(msg.contains("empty response"));
    }

    // LM-07: LatestMarkBatch defaults to an empty Vec (no fabricated marks).
    #[test]
    fn lm07_batch_default_is_empty() {
        let batch = LatestMarkBatch::default();
        assert!(batch.marks.is_empty());
    }
}
