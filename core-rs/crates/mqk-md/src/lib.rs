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
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

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

/// TwelveData-backed historical provider.
///
/// API key is read by the caller (CLI) and passed in; do not log it.
#[derive(Debug, Clone)]
pub struct TwelveDataHistoricalProvider {
    api_key: String,
    http: reqwest::Client,
    base_url: String,
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

            let resp = self
                .http
                .get(url)
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

            let status = resp.status();
            let body: TwelveDataTimeSeriesResponse = resp
                .json()
                .await
                .context("twelvedata response json decode failed")?;

            if !status.is_success() {
                return Err(anyhow!(
                    "twelvedata http error status={} message={}",
                    status.as_u16(),
                    body.status_message()
                ));
            }

            if let Some(err) = body.error_message() {
                return Err(anyhow!("twelvedata error: {}", err));
            }

            let values = body.values.unwrap_or_default();

            for v in values {
                // TwelveData timestamps are usually ISO strings.
                // We parse them as UTC and convert to epoch seconds.
                let dt = DateTime::parse_from_rfc3339(&v.datetime)
                    .or_else(|_| DateTime::parse_from_str(&v.datetime, "%Y-%m-%d %H:%M:%S"))
                    .or_else(|_| DateTime::parse_from_str(&v.datetime, "%Y-%m-%d"))
                    .with_context(|| format!("twelvedata datetime parse failed: {}", v.datetime))?;
                let end_ts = dt.with_timezone(&Utc).timestamp();

                out.push(ProviderBar {
                    symbol: sym.to_string(),
                    timeframe: req.timeframe.as_str().to_string(),
                    end_ts,
                    open: v.open,
                    high: v.high,
                    low: v.low,
                    close: v.close,
                    volume: v.volume.parse::<i64>().unwrap_or(0),
                    is_complete: true,
                });
            }
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

    #[test]
    fn timeframe_parse() {
        assert_eq!(Timeframe::parse("1D").unwrap(), Timeframe::D1);
        assert_eq!(Timeframe::parse("1m").unwrap(), Timeframe::M1);
        assert_eq!(Timeframe::parse("5m").unwrap(), Timeframe::M5);
        assert!(Timeframe::parse("15m").is_err());
    }
}
