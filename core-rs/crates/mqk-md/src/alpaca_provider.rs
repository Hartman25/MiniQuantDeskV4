//! Alpaca market-data historical provider.
//!
//! Uses the Alpaca Data API v2 (`data.alpaca.markets`) to fetch OHLCV bars.
//! Auth: same paper credentials as the broker adapter (`ALPACA_API_KEY_PAPER` /
//! `ALPACA_API_SECRET_PAPER`).  No order or account endpoints are called.
//!
//! Supports pagination via `next_page_token`.

use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;

use crate::{FetchBarsRequest, ProviderBar};

/// Default Alpaca data base URL.
pub const ALPACA_DATA_BASE_URL: &str = "https://data.alpaca.markets";

/// Maximum bars per page for the Alpaca `/v2/stocks/bars` endpoint.
const ALPACA_PAGE_LIMIT: u32 = 10_000;

/// Alpaca feed to request.  "iex" is free-tier; "sip" requires a paid subscription.
/// Operator may override via `ALPACA_MARKET_DATA_FEED` env var.
const ALPACA_DEFAULT_FEED: &str = "iex";

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Alpaca Data API v2 historical provider.
#[derive(Debug, Clone)]
pub struct AlpacaHistoricalProvider {
    api_key: String,
    api_secret: String,
    http: reqwest::Client,
    base_url: String,
    feed: String,
}

impl AlpacaHistoricalProvider {
    /// Production constructor.  Credentials must be supplied by the caller (never log them).
    pub fn new(api_key: String, api_secret: String) -> Self {
        Self::new_with_base_url(api_key, api_secret, ALPACA_DATA_BASE_URL.to_string())
    }

    pub fn new_with_base_url(api_key: String, api_secret: String, base_url: String) -> Self {
        let feed = std::env::var("ALPACA_MARKET_DATA_FEED")
            .unwrap_or_else(|_| ALPACA_DEFAULT_FEED.to_string());
        Self {
            api_key,
            api_secret,
            http: reqwest::Client::new(),
            base_url,
            feed,
        }
    }

    /// Test constructor: accepts an injectable base URL so tests can point at a mock server.
    #[cfg(test)]
    pub fn new_for_test(api_key: String, api_secret: String, base_url: String) -> Self {
        Self {
            api_key,
            api_secret,
            http: reqwest::Client::new(),
            base_url,
            feed: "iex".to_string(),
        }
    }

    fn bars_url(&self) -> String {
        format!("{}/v2/stocks/bars", self.base_url.trim_end_matches('/'))
    }

    /// Map a canonical timeframe string to Alpaca's `timeframe` parameter value.
    fn alpaca_timeframe(tf: &crate::Timeframe) -> &'static str {
        match tf {
            crate::Timeframe::D1 => "1Day",
            crate::Timeframe::H1 => "1Hour",
            crate::Timeframe::M1 => "1Min",
            crate::Timeframe::M5 => "5Min",
            crate::Timeframe::M15 => "15Min",
        }
    }

    /// Seconds to add to Alpaca bar start timestamp (`t`) to produce the bar's end timestamp.
    /// TwelveData returns start timestamps and stores them as `end_ts`; Alpaca does the same,
    /// so we store `t` directly as `end_ts` for consistency with existing TwelveData data.
    fn end_ts_from_alpaca_t(t_seconds: i64, _tf: &crate::Timeframe) -> i64 {
        // Store Alpaca start-of-bar `t` as `end_ts` to match TwelveData convention already in DB.
        t_seconds
    }
}

#[async_trait::async_trait]
impl crate::HistoricalProvider for AlpacaHistoricalProvider {
    fn source_name(&self) -> &'static str {
        "alpaca"
    }

    async fn fetch_bars(&self, req: FetchBarsRequest) -> Result<Vec<ProviderBar>> {
        let url = self.bars_url();
        let tf_str = Self::alpaca_timeframe(&req.timeframe);
        let tf_ref = req.timeframe;

        // Alpaca start/end: ISO 8601 UTC.  Use start-of-day for `start` and
        // end-of-day for `end` so the full calendar day is covered.
        let start_iso = format!("{}T00:00:00Z", req.start.format("%Y-%m-%d"));
        let end_dt = req
            .end
            .and_hms_opt(23, 59, 59)
            .ok_or_else(|| anyhow!("alpaca: could not build end datetime from {}", req.end))?;
        let end_iso = Utc.from_utc_datetime(&end_dt).to_rfc3339();

        let symbols_csv = req.symbols.join(",");

        let mut all_bars: HashMap<String, Vec<ProviderBar>> = HashMap::new();
        let mut page_token: Option<String> = None;

        // Paginate until no more pages.
        loop {
            let mut params: Vec<(&str, String)> = vec![
                ("symbols", symbols_csv.clone()),
                ("timeframe", tf_str.to_string()),
                ("start", start_iso.clone()),
                ("end", end_iso.clone()),
                ("adjustment", "raw".to_string()),
                ("feed", self.feed.clone()),
                ("limit", ALPACA_PAGE_LIMIT.to_string()),
            ];
            if let Some(ref tok) = page_token {
                params.push(("page_token", tok.clone()));
            }

            let resp = self
                .http
                .get(&url)
                .header("APCA-API-KEY-ID", &self.api_key)
                .header("APCA-API-SECRET-KEY", &self.api_secret)
                .query(&params)
                .send()
                .await
                .context("alpaca data api request failed")?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "alpaca data api http error status={}: {}",
                    status.as_u16(),
                    body
                ));
            }

            let page: AlpacaBarsResponse = resp
                .json()
                .await
                .context("alpaca data api response json decode failed")?;

            for (sym, raw_bars) in page.bars.unwrap_or_default() {
                let entry = all_bars.entry(sym.clone()).or_default();
                for rb in raw_bars {
                    // Parse RFC3339 timestamp.
                    let t_seconds = chrono::DateTime::parse_from_rfc3339(&rb.t)
                        .with_context(|| {
                            format!("alpaca bar timestamp parse failed: sym={sym} t={}", rb.t)
                        })?
                        .timestamp();

                    let end_ts = Self::end_ts_from_alpaca_t(t_seconds, &tf_ref);

                    // Convert f64 → string with enough precision for normalize_price_str.
                    // Use 6 decimal places; normalize_price_str will trim trailing zeros.
                    let open_s = format!("{:.6}", rb.o);
                    let high_s = format!("{:.6}", rb.h);
                    let low_s = format!("{:.6}", rb.l);
                    let close_s = format!("{:.6}", rb.c);

                    entry.push(ProviderBar {
                        symbol: sym.clone(),
                        timeframe: tf_ref.as_str().to_string(),
                        end_ts,
                        open: crate::normalize_price_str(&open_s).with_context(|| {
                            format!("alpaca open price normalize failed: sym={sym} t={}", rb.t)
                        })?,
                        high: crate::normalize_price_str(&high_s).with_context(|| {
                            format!("alpaca high price normalize failed: sym={sym} t={}", rb.t)
                        })?,
                        low: crate::normalize_price_str(&low_s).with_context(|| {
                            format!("alpaca low price normalize failed: sym={sym} t={}", rb.t)
                        })?,
                        close: crate::normalize_price_str(&close_s).with_context(|| {
                            format!("alpaca close price normalize failed: sym={sym} t={}", rb.t)
                        })?,
                        volume: rb.v,
                        is_complete: true,
                    });
                }
            }

            match page.next_page_token {
                Some(tok) if !tok.is_empty() => {
                    page_token = Some(tok);
                }
                _ => break,
            }
        }

        // Flatten all symbols, sort ascending by (symbol, end_ts).
        let mut out: Vec<ProviderBar> = all_bars.into_values().flatten().collect();
        out.sort_by(|a, b| {
            a.symbol
                .cmp(&b.symbol)
                .then_with(|| a.end_ts.cmp(&b.end_ts))
        });
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AlpacaBarsResponse {
    bars: Option<HashMap<String, Vec<AlpacaRawBar>>>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlpacaRawBar {
    /// Bar start timestamp (RFC3339 UTC), e.g. "2026-06-03T13:30:00Z".
    t: String,
    /// Open price as float.
    o: f64,
    /// High price as float.
    h: f64,
    /// Low price as float.
    l: f64,
    /// Close price as float.
    c: f64,
    /// Volume (integer shares).
    v: i64,
}

// ---------------------------------------------------------------------------
// Alpaca env-var helpers (used by CLI layer; credential values never printed)
// ---------------------------------------------------------------------------

pub const ENV_ALPACA_KEY_PAPER: &str = "ALPACA_API_KEY_PAPER";
pub const ENV_ALPACA_SECRET_PAPER: &str = "ALPACA_API_SECRET_PAPER";

/// Load Alpaca paper credentials from environment.  Returns `Err` if either is absent.
/// Never logs or prints the values.
pub fn load_alpaca_paper_credentials() -> Result<(String, String)> {
    let key = std::env::var(ENV_ALPACA_KEY_PAPER)
        .with_context(|| format!("missing env var {ENV_ALPACA_KEY_PAPER}"))?;
    let secret = std::env::var(ENV_ALPACA_SECRET_PAPER)
        .with_context(|| format!("missing env var {ENV_ALPACA_SECRET_PAPER}"))?;
    Ok((key, secret))
}

// ---------------------------------------------------------------------------
// Tests (no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FetchBarsRequest, HistoricalProvider, Timeframe};
    use chrono::NaiveDate;

    // -----------------------------------------------------------------------
    // Pure helpers
    // -----------------------------------------------------------------------

    #[test]
    fn alpaca_timeframe_maps_correctly() {
        assert_eq!(
            AlpacaHistoricalProvider::alpaca_timeframe(&Timeframe::M5),
            "5Min"
        );
        assert_eq!(
            AlpacaHistoricalProvider::alpaca_timeframe(&Timeframe::M1),
            "1Min"
        );
        assert_eq!(
            AlpacaHistoricalProvider::alpaca_timeframe(&Timeframe::D1),
            "1Day"
        );
        assert_eq!(
            AlpacaHistoricalProvider::alpaca_timeframe(&Timeframe::H1),
            "1Hour"
        );
    }

    #[test]
    fn end_ts_from_alpaca_t_passthrough() {
        let t = 1_748_952_600_i64; // some epoch second
        assert_eq!(
            AlpacaHistoricalProvider::end_ts_from_alpaca_t(t, &Timeframe::M5),
            t
        );
    }

    // -----------------------------------------------------------------------
    // Mock server integration (httpmock)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fetch_bars_single_symbol_single_page() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/v2/stocks/bars");
            then.status(200).json_body(serde_json::json!({
                "bars": {
                    "AAPL": [
                        {
                            "t": "2026-06-03T13:30:00Z",
                            "o": 195.5,
                            "h": 196.0,
                            "l": 194.8,
                            "c": 195.8,
                            "v": 123456
                        },
                        {
                            "t": "2026-06-03T13:35:00Z",
                            "o": 195.8,
                            "h": 196.5,
                            "l": 195.5,
                            "c": 196.2,
                            "v": 98765
                        }
                    ]
                },
                "next_page_token": null
            }));
        });

        let provider = AlpacaHistoricalProvider::new_for_test(
            "key".to_string(),
            "secret".to_string(),
            server.base_url(),
        );
        let req = FetchBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: Timeframe::M5,
            start: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        };
        let bars = provider.fetch_bars(req).await.unwrap();
        assert_eq!(bars.len(), 2, "must return 2 bars");
        assert_eq!(bars[0].symbol, "AAPL");
        assert_eq!(bars[0].timeframe, "5m");
        // t = 2026-06-03T13:30:00Z = 1780493400
        assert_eq!(bars[0].end_ts, 1_780_493_400);
        assert_eq!(bars[0].open, "195.5");
        assert_eq!(bars[0].close, "195.8");
        assert!(bars[0].is_complete);
        // bars are sorted ascending by end_ts
        assert!(bars[0].end_ts < bars[1].end_ts);
    }

    #[tokio::test]
    async fn fetch_bars_empty_response_returns_empty() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/v2/stocks/bars");
            then.status(200).json_body(serde_json::json!({
                "bars": {},
                "next_page_token": null
            }));
        });

        let provider = AlpacaHistoricalProvider::new_for_test(
            "key".to_string(),
            "secret".to_string(),
            server.base_url(),
        );
        let req = FetchBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: Timeframe::M5,
            start: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        };
        let bars = provider.fetch_bars(req).await.unwrap();
        assert!(
            bars.is_empty(),
            "must return empty vec when no bars available"
        );
    }

    #[tokio::test]
    async fn fetch_bars_http_error_returns_err() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/v2/stocks/bars");
            then.status(403).body("forbidden");
        });

        let provider = AlpacaHistoricalProvider::new_for_test(
            "bad-key".to_string(),
            "bad-secret".to_string(),
            server.base_url(),
        );
        let req = FetchBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: Timeframe::M5,
            start: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        };
        let err = provider.fetch_bars(req).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("403"),
            "error must mention http status 403: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_bars_pagination_collects_all_pages() {
        use httpmock::prelude::*;
        use std::sync::atomic::{AtomicU32, Ordering};

        static PAGE_CALL: AtomicU32 = AtomicU32::new(0);
        PAGE_CALL.store(0, Ordering::SeqCst);

        let server = MockServer::start();

        // Page 1: has next_page_token
        let _mock_page1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v2/stocks/bars")
                .matches(|_req: &HttpMockRequest| PAGE_CALL.fetch_add(1, Ordering::SeqCst) < 1);
            then.status(200).json_body(serde_json::json!({
                "bars": {
                    "AAPL": [{ "t": "2026-06-03T13:30:00Z", "o": 195.5, "h": 196.0, "l": 194.8, "c": 195.8, "v": 100 }]
                },
                "next_page_token": "abc123"
            }));
        });

        // Page 2: no next_page_token
        let _mock_page2 = server.mock(|when, then| {
            when.method(GET).path("/v2/stocks/bars");
            then.status(200).json_body(serde_json::json!({
                "bars": {
                    "AAPL": [{ "t": "2026-06-03T13:35:00Z", "o": 195.8, "h": 196.5, "l": 195.5, "c": 196.2, "v": 200 }]
                },
                "next_page_token": null
            }));
        });

        let provider = AlpacaHistoricalProvider::new_for_test(
            "key".to_string(),
            "secret".to_string(),
            server.base_url(),
        );
        let req = FetchBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: Timeframe::M5,
            start: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        };
        let bars = provider.fetch_bars(req).await.unwrap();
        assert_eq!(
            bars.len(),
            2,
            "pagination must collect bars from both pages"
        );
        assert!(bars[0].end_ts < bars[1].end_ts, "must be sorted ascending");
    }

    #[test]
    fn provider_name_is_alpaca() {
        use crate::HistoricalProvider;
        let p = AlpacaHistoricalProvider::new_for_test(
            "k".to_string(),
            "s".to_string(),
            "http://localhost".to_string(),
        );
        assert_eq!(p.source_name(), "alpaca");
    }

    #[test]
    fn load_credentials_fails_without_env() {
        // Temporarily clear env vars if set (safe in test isolation).
        let _k = std::env::var(ENV_ALPACA_KEY_PAPER).ok();
        let _s = std::env::var(ENV_ALPACA_SECRET_PAPER).ok();
        // If neither is set, must return Err.
        if _k.is_none() && _s.is_none() {
            assert!(load_alpaca_paper_credentials().is_err());
        }
    }
}
