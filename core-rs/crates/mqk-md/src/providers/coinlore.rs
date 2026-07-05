//! CoinLore ticker parser and latest-mark client (default-disabled).
//!
//! Parses the verified `/api/ticker/?id=<id1>,<id2>` response shape
//! confirmed by `CRYPTO-DATA-01I` (see
//! `docs/specs/crypto_data_01i_coinlore_network_verify.md`): a bare JSON
//! array of ticker objects, each carrying `id`, `symbol`, `price_usd`
//! (decimal string), and `volume24` (rolling 24h figure) -- no OHLCV, no
//! per-ticker timestamp.
//!
//! This module produces [`crate::latest_mark::LatestMark`] values only. It
//! does **not** implement `HistoricalProvider`/`MarketDataProvider` and does
//! **not** fabricate `open`/`high`/`low`/`end_ts`/`is_complete` from this
//! data. `providers.json`'s `coinlore` entry stays `enabled=false`
//! (`CRYPTO-DATA-01H` §14); nothing in this module reads that flag itself --
//! callers (CLI/operator surfaces) are responsible for checking it before
//! invoking [`fetch_coinlore_ticker_body`].

use std::collections::HashSet;

use serde::Deserialize;

use crate::latest_mark::{LatestMark, LatestMarkError, LatestMarkKind, LatestMarkTruthState};

/// Stable provider identifier used throughout latest-mark error/metadata
/// fields for this provider.
pub const COINLORE_PROVIDER_ID: &str = "coinlore";

/// One configured canonical-symbol -> CoinLore identity mapping, sourced
/// from `provider_symbols.coinlore_id` / `provider_symbols.coinlore_symbol`
/// on a registry-v2 instrument (see
/// `config/instruments/instruments_v2.crypto_local_marks.example.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinloreAlias {
    pub canonical_symbol: String,
    pub coinlore_id: String,
    pub coinlore_symbol: String,
}

/// Extract [`CoinloreAlias`] entries from a registry-v2 document for every
/// instrument that carries both `coinlore_id` and `coinlore_symbol` in its
/// `provider_symbols` map. Instruments missing either key are silently
/// skipped (they simply have no CoinLore alias yet) -- this is a read-only
/// projection, not a validator; enablement flags are untouched and
/// unread here.
pub fn coinlore_aliases_from_registry_v2(
    registry: &crate::instrument_registry_v2::InstrumentRegistryV2,
) -> Vec<CoinloreAlias> {
    registry
        .instruments
        .iter()
        .filter_map(|inst| {
            let coinlore_id = inst.provider_symbols.get("coinlore_id")?;
            let coinlore_symbol = inst.provider_symbols.get("coinlore_symbol")?;
            Some(CoinloreAlias {
                canonical_symbol: inst.symbol.clone(),
                coinlore_id: coinlore_id.clone(),
                coinlore_symbol: coinlore_symbol.clone(),
            })
        })
        .collect()
}

/// Raw CoinLore ticker object exactly as returned by
/// `/api/ticker/?id=...` (the endpoint returns a bare JSON array of these).
///
/// Only the fields this module actually uses are modeled; CoinLore returns
/// additional fields (`name`, `nameid`, `rank`, `percent_change_*`,
/// `price_btc`, `market_cap_usd`, `volume24a`, `csupply`, `tsupply`,
/// `msupply`) that this patch does not need and does not claim to model.
#[derive(Debug, Clone, Deserialize)]
pub struct CoinloreTickerRaw {
    pub id: String,
    pub symbol: String,
    pub price_usd: String,
    #[serde(default)]
    pub volume24: Option<serde_json::Value>,
}

/// Parse a raw CoinLore `/api/ticker/?id=...` JSON response body (a bare
/// array) into [`LatestMark`]s for exactly the configured `aliases`, at
/// `as_of_client_request_ts` (the caller's own wall-clock request time --
/// CoinLore supplies no per-ticker timestamp, confirmed by
/// `CRYPTO-DATA-01I`).
///
/// Rejects (returns `Err`, never silently drops or fabricates):
/// - an empty response array (`LatestMarkError::EmptyResponse`)
/// - malformed/non-array JSON (`LatestMarkError::MalformedResponse`)
/// - an entry with a missing/empty `id`, `symbol`, or `price_usd`
///   (`LatestMarkError::MalformedResponse`)
/// - a non-decimal `price_usd` (`LatestMarkError::MalformedResponse`)
/// - duplicate `id` values in the response (`LatestMarkError::DuplicateIdentity`)
/// - a response missing one of the configured `aliases`
///   (`LatestMarkError::MissingConfiguredAsset`)
/// - a response entry whose `symbol` does not match the configured alias
///   for its `id` (`LatestMarkError::MismatchedIdentity`)
pub fn parse_coinlore_ticker_response(
    body: &str,
    aliases: &[CoinloreAlias],
    as_of_client_request_ts: i64,
) -> Result<Vec<LatestMark>, LatestMarkError> {
    if aliases.is_empty() {
        return Err(LatestMarkError::MalformedResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: "no configured coinlore aliases supplied".to_string(),
        });
    }

    let raw: Vec<CoinloreTickerRaw> =
        serde_json::from_str(body).map_err(|err| LatestMarkError::MalformedResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: format!("json decode failed: {err}"),
        })?;

    if raw.is_empty() {
        return Err(LatestMarkError::EmptyResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: "coinlore ticker response array is empty".to_string(),
        });
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    for ticker in &raw {
        if ticker.id.trim().is_empty() {
            return Err(LatestMarkError::MalformedResponse {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                message: "ticker entry missing id".to_string(),
            });
        }
        if ticker.symbol.trim().is_empty() {
            return Err(LatestMarkError::MalformedResponse {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                message: format!("ticker id={} missing symbol", ticker.id),
            });
        }
        if crate::normalize_price_str(&ticker.price_usd).is_err() {
            return Err(LatestMarkError::MalformedResponse {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                message: format!(
                    "ticker id={} symbol={} has non-decimal price_usd={:?}",
                    ticker.id, ticker.symbol, ticker.price_usd
                ),
            });
        }
        if !seen_ids.insert(ticker.id.clone()) {
            return Err(LatestMarkError::DuplicateIdentity {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                message: format!("duplicate ticker id={}", ticker.id),
            });
        }
    }

    let mut marks = Vec::with_capacity(aliases.len());
    for alias in aliases {
        let ticker = raw
            .iter()
            .find(|t| t.id == alias.coinlore_id)
            .ok_or_else(|| LatestMarkError::MissingConfiguredAsset {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                canonical_symbol: alias.canonical_symbol.clone(),
            })?;

        if ticker.symbol != alias.coinlore_symbol {
            return Err(LatestMarkError::MismatchedIdentity {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                message: format!(
                    "coinlore id={} returned symbol={} but alias for {} expects symbol={}",
                    alias.coinlore_id, ticker.symbol, alias.canonical_symbol, alias.coinlore_symbol
                ),
            });
        }

        marks.push(LatestMark {
            canonical_symbol: alias.canonical_symbol.clone(),
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            provider_symbol: ticker.symbol.clone(),
            provider_coin_id: Some(ticker.id.clone()),
            price_usd: ticker.price_usd.clone(),
            volume24_usd: ticker.volume24.as_ref().map(|v| v.to_string()),
            as_of_client_request_ts,
            provider_ts: None,
            truth_state: LatestMarkTruthState::ClientObservedOnly,
            kind: LatestMarkKind::Ticker,
        });
    }

    Ok(marks)
}

/// Fetch the raw CoinLore ticker response body for the given comma-joined
/// `coin_ids` via exactly one HTTP GET to
/// `https://api.coinlore.net/api/ticker/?id=<ids>`.
///
/// This function performs the network call unconditionally when invoked --
/// it does **not** check `providers.json`'s `enabled` flag or any
/// environment variable itself. Callers (CLI/operator surfaces) MUST check
/// `providers.json`'s `coinlore.enabled` flag and gate this call behind an
/// explicit operator opt-in (e.g. `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1|true`)
/// before invoking it. No credentials are sent; CoinLore's public ticker
/// endpoint requires none.
pub async fn fetch_coinlore_ticker_body(coin_ids: &[String]) -> Result<String, LatestMarkError> {
    if coin_ids.is_empty() {
        return Err(LatestMarkError::MalformedResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: "no coin ids supplied for coinlore fetch".to_string(),
        });
    }
    let url = format!(
        "https://api.coinlore.net/api/ticker/?id={}",
        coin_ids.join(",")
    );
    let resp = reqwest::get(&url)
        .await
        .map_err(|err| LatestMarkError::MalformedResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: format!("transport error: {err}"),
        })?;
    resp.text()
        .await
        .map_err(|err| LatestMarkError::MalformedResponse {
            provider_id: COINLORE_PROVIDER_ID.to_string(),
            message: format!("body read error: {err}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btc_eth_aliases() -> Vec<CoinloreAlias> {
        vec![
            CoinloreAlias {
                canonical_symbol: "BTC/USD".to_string(),
                coinlore_id: "90".to_string(),
                coinlore_symbol: "BTC".to_string(),
            },
            CoinloreAlias {
                canonical_symbol: "ETH/USD".to_string(),
                coinlore_id: "80".to_string(),
                coinlore_symbol: "ETH".to_string(),
            },
        ]
    }

    /// Shaped exactly like the real `/api/ticker/?id=90,80` body captured by
    /// `CRYPTO-DATA-01I` (extra fields present on the wire but not modeled
    /// by `CoinloreTickerRaw` are intentionally included here too, proving
    /// `#[serde(default)]`/ignored-field tolerance).
    const VERIFIED_SHAPE_BODY: &str = r#"[
        {
            "id": "90",
            "symbol": "BTC",
            "name": "Bitcoin",
            "nameid": "bitcoin",
            "rank": 1,
            "price_usd": "62906.61",
            "percent_change_24h": "0.5",
            "percent_change_1h": "0.1",
            "percent_change_7d": "-1.2",
            "price_btc": "1",
            "market_cap_usd": "1239876543210",
            "volume24": 16261421089.448309,
            "volume24a": 16000000000.0,
            "csupply": "19700000",
            "tsupply": "19700000",
            "msupply": "21000000"
        },
        {
            "id": "80",
            "symbol": "ETH",
            "name": "Ethereum",
            "nameid": "ethereum",
            "rank": 2,
            "price_usd": "1777.74",
            "percent_change_24h": "0.3",
            "percent_change_1h": "0.05",
            "percent_change_7d": "-2.1",
            "price_btc": "0.02825",
            "market_cap_usd": "213900000000",
            "volume24": 9252669028.469555,
            "volume24a": 9000000000.0,
            "csupply": "120280000",
            "tsupply": "120280000",
            "msupply": null
        }
    ]"#;

    const AS_OF_TS: i64 = 1_783_264_281;

    // CL-01: parser handles the exact 01I-verified targeted response shape.
    #[test]
    fn cl01_parses_verified_targeted_response_shape() {
        let marks =
            parse_coinlore_ticker_response(VERIFIED_SHAPE_BODY, &btc_eth_aliases(), AS_OF_TS)
                .expect("verified shape must parse");
        assert_eq!(marks.len(), 2);
    }

    // CL-02: CoinLore id 90/BTC maps to canonical BTC/USD.
    #[test]
    fn cl02_id_90_btc_maps_to_canonical_btc_usd() {
        let marks =
            parse_coinlore_ticker_response(VERIFIED_SHAPE_BODY, &btc_eth_aliases(), AS_OF_TS)
                .unwrap();
        let btc = marks
            .iter()
            .find(|m| m.canonical_symbol == "BTC/USD")
            .expect("BTC/USD mark must be present");
        assert_eq!(btc.provider_coin_id.as_deref(), Some("90"));
        assert_eq!(btc.provider_symbol, "BTC");
        assert_eq!(btc.price_usd, "62906.61");
        assert_eq!(btc.provider_id, COINLORE_PROVIDER_ID);
        assert!(!btc.is_completed_bar());
    }

    // CL-03: CoinLore id 80/ETH maps to canonical ETH/USD.
    #[test]
    fn cl03_id_80_eth_maps_to_canonical_eth_usd() {
        let marks =
            parse_coinlore_ticker_response(VERIFIED_SHAPE_BODY, &btc_eth_aliases(), AS_OF_TS)
                .unwrap();
        let eth = marks
            .iter()
            .find(|m| m.canonical_symbol == "ETH/USD")
            .expect("ETH/USD mark must be present");
        assert_eq!(eth.provider_coin_id.as_deref(), Some("80"));
        assert_eq!(eth.provider_symbol, "ETH");
        assert_eq!(eth.price_usd, "1777.74");
    }

    // CL-04: parser rejects an empty response array.
    #[test]
    fn cl04_rejects_empty_array() {
        let err = parse_coinlore_ticker_response("[]", &btc_eth_aliases(), AS_OF_TS).unwrap_err();
        assert!(matches!(err, LatestMarkError::EmptyResponse { .. }));
    }

    // CL-05: parser rejects malformed (non-JSON) input.
    #[test]
    fn cl05_rejects_malformed_json() {
        let err =
            parse_coinlore_ticker_response("{ not json", &btc_eth_aliases(), AS_OF_TS).unwrap_err();
        assert!(matches!(err, LatestMarkError::MalformedResponse { .. }));
    }

    // CL-06: parser rejects a missing configured asset (response has BTC
    // only; ETH/USD alias configured but absent from the response).
    #[test]
    fn cl06_rejects_missing_configured_asset() {
        let btc_only = r#"[{"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":123.0}]"#;
        let err = parse_coinlore_ticker_response(btc_only, &btc_eth_aliases(), AS_OF_TS)
            .unwrap_err();
        assert_eq!(
            err,
            LatestMarkError::MissingConfiguredAsset {
                provider_id: COINLORE_PROVIDER_ID.to_string(),
                canonical_symbol: "ETH/USD".to_string(),
            }
        );
    }

    // CL-07: parser rejects missing price_usd (absent field -> serde decode failure).
    #[test]
    fn cl07_rejects_missing_price_usd() {
        let missing_price = r#"[
            {"id":"90","symbol":"BTC","volume24":123.0},
            {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":456.0}
        ]"#;
        let err = parse_coinlore_ticker_response(missing_price, &btc_eth_aliases(), AS_OF_TS)
            .unwrap_err();
        assert!(matches!(err, LatestMarkError::MalformedResponse { .. }));
    }

    // CL-08: parser rejects a non-decimal price_usd (e.g. scientific notation).
    #[test]
    fn cl08_rejects_non_decimal_price_usd() {
        let bad_price = r#"[
            {"id":"90","symbol":"BTC","price_usd":"6.29e4","volume24":123.0},
            {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":456.0}
        ]"#;
        let err = parse_coinlore_ticker_response(bad_price, &btc_eth_aliases(), AS_OF_TS)
            .unwrap_err();
        assert!(matches!(err, LatestMarkError::MalformedResponse { .. }));
    }

    // CL-09: parser rejects duplicate ids in the response.
    #[test]
    fn cl09_rejects_duplicate_ids() {
        let dup = r#"[
            {"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":123.0},
            {"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":123.0}
        ]"#;
        let err = parse_coinlore_ticker_response(dup, &btc_eth_aliases(), AS_OF_TS).unwrap_err();
        assert!(matches!(err, LatestMarkError::DuplicateIdentity { .. }));
    }

    // CL-10: parser rejects a mismatched id/symbol pair (id=90 must be BTC,
    // not some other symbol).
    #[test]
    fn cl10_rejects_mismatched_id_symbol_pair() {
        let mismatched = r#"[
            {"id":"90","symbol":"NOTBTC","price_usd":"62906.61","volume24":123.0},
            {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":456.0}
        ]"#;
        let err = parse_coinlore_ticker_response(mismatched, &btc_eth_aliases(), AS_OF_TS)
            .unwrap_err();
        assert!(matches!(err, LatestMarkError::MismatchedIdentity { .. }));
    }

    // CL-11: parser rejects a missing id field.
    #[test]
    fn cl11_rejects_missing_id() {
        let missing_id = r#"[
            {"symbol":"BTC","price_usd":"62906.61","volume24":123.0},
            {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":456.0}
        ]"#;
        let err = parse_coinlore_ticker_response(missing_id, &btc_eth_aliases(), AS_OF_TS)
            .unwrap_err();
        assert!(matches!(err, LatestMarkError::MalformedResponse { .. }));
    }

    // CL-12: parser rejects an empty aliases list rather than silently
    // returning zero marks.
    #[test]
    fn cl12_rejects_empty_aliases_list() {
        let err =
            parse_coinlore_ticker_response(VERIFIED_SHAPE_BODY, &[], AS_OF_TS).unwrap_err();
        assert!(matches!(err, LatestMarkError::MalformedResponse { .. }));
    }

    // CL-13: no mark produced carries any provider_ts (CoinLore supplies
    // none); as_of_client_request_ts is threaded through unchanged.
    #[test]
    fn cl13_no_mark_carries_a_provider_timestamp() {
        let marks =
            parse_coinlore_ticker_response(VERIFIED_SHAPE_BODY, &btc_eth_aliases(), AS_OF_TS)
                .unwrap();
        for mark in &marks {
            assert_eq!(mark.provider_ts, None);
            assert_eq!(mark.as_of_client_request_ts, AS_OF_TS);
            assert_eq!(mark.truth_state, LatestMarkTruthState::ClientObservedOnly);
        }
    }

    // CL-14: coinlore_aliases_from_registry_v2 extracts both aliases from
    // the real committed fixture (proven again, end-to-end, by the
    // scenario_coinlore_latest_mark_01jkl.rs integration test).
    #[test]
    fn cl14_alias_extraction_skips_instruments_without_both_keys() {
        use crate::instrument_registry_v2::{
            ContractDefinitionV2, InstrumentDefinitionV2, InstrumentRegistryV2,
        };
        use std::collections::BTreeMap;

        let mut with_alias_symbols = BTreeMap::new();
        with_alias_symbols.insert("local_csv".to_string(), "BTC/USD".to_string());
        with_alias_symbols.insert("coinlore_id".to_string(), "90".to_string());
        with_alias_symbols.insert("coinlore_symbol".to_string(), "BTC".to_string());

        let mut without_alias_symbols = BTreeMap::new();
        without_alias_symbols.insert("local_csv".to_string(), "ETH/USD".to_string());

        let registry = InstrumentRegistryV2 {
            schema_version: 1,
            instruments: vec![
                InstrumentDefinitionV2 {
                    instrument_id: "crypto:GLOBAL:BTCUSD".to_string(),
                    symbol: "BTC/USD".to_string(),
                    asset_class: "crypto".to_string(),
                    instrument_kind: None,
                    venue: None,
                    currency: "USD".to_string(),
                    quote_currency: Some("USD".to_string()),
                    provider_symbols: with_alias_symbols,
                    enabled: false,
                    paper_trading_enabled: false,
                    live_trading_enabled: false,
                    timeframes: vec!["1D".to_string()],
                    contract: Some(ContractDefinitionV2::CryptoPair {
                        base: "BTC".to_string(),
                        quote: "USD".to_string(),
                    }),
                    metadata: Default::default(),
                    notes: None,
                    allow_enabled_non_equity_for_testing: false,
                    economics: None,
                },
                InstrumentDefinitionV2 {
                    instrument_id: "crypto:GLOBAL:ETHUSD".to_string(),
                    symbol: "ETH/USD".to_string(),
                    asset_class: "crypto".to_string(),
                    instrument_kind: None,
                    venue: None,
                    currency: "USD".to_string(),
                    quote_currency: Some("USD".to_string()),
                    provider_symbols: without_alias_symbols,
                    enabled: false,
                    paper_trading_enabled: false,
                    live_trading_enabled: false,
                    timeframes: vec!["1D".to_string()],
                    contract: Some(ContractDefinitionV2::CryptoPair {
                        base: "ETH".to_string(),
                        quote: "USD".to_string(),
                    }),
                    metadata: Default::default(),
                    notes: None,
                    allow_enabled_non_equity_for_testing: false,
                    economics: None,
                },
            ],
        };

        let aliases = coinlore_aliases_from_registry_v2(&registry);
        assert_eq!(aliases.len(), 1, "only BTC/USD carries both alias keys");
        assert_eq!(aliases[0].canonical_symbol, "BTC/USD");
        assert_eq!(aliases[0].coinlore_id, "90");
        assert_eq!(aliases[0].coinlore_symbol, "BTC");
    }
}
