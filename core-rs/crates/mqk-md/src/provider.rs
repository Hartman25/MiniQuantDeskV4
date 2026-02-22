//! Provider boundary for OHLCV market-data ingestion.
//!
//! This module defines **only** the raw bar type and provider trait.
//! No concrete provider implementations, no DB logic, no CSV logic,
//! no normalization to micros, and no data-quality logic belong here.
//!
//! # Wiring
//! `lib.rs` must add `pub mod provider;` (or `mod provider; pub use provider::*;`)
//! for this file to be compiled as part of the crate.

use std::fmt;

// ---------------------------------------------------------------------------
// Raw bar
// ---------------------------------------------------------------------------

/// A single OHLCV bar as returned verbatim by an upstream data provider.
///
/// Prices are kept as decimal strings so downstream callers can normalise
/// deterministically (e.g. convert to integer micros) without floating-point
/// rounding being introduced at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBar {
    /// Ticker symbol exactly as given to the provider (e.g. `"AAPL"`).
    pub symbol: String,
    /// Canonical timeframe string (e.g. `"1D"`, `"1m"`, `"5m"`).
    pub timeframe: String,
    /// Bar end timestamp as UTC epoch seconds.
    pub end_ts: i64,
    /// Opening price as a decimal string (e.g. `"182.34"`).
    pub open: String,
    /// High price as a decimal string.
    pub high: String,
    /// Low price as a decimal string.
    pub low: String,
    /// Closing price as a decimal string.
    pub close: String,
    /// Trade volume (integer shares / contracts).
    pub volume: i64,
    /// `true` when the bar period has fully closed; `false` for a live/partial bar.
    pub is_complete: bool,
}

// ---------------------------------------------------------------------------
// Fetch request
// ---------------------------------------------------------------------------

/// Parameters for a historical fetch request passed to a [`Provider`].
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// One or more ticker symbols to retrieve.
    pub symbols: Vec<String>,
    /// Canonical timeframe string (e.g. `"1D"`, `"1m"`, `"5m"`).
    pub timeframe: String,
    /// Inclusive start date as `YYYY-MM-DD`.
    pub start_date: String,
    /// Inclusive end date as `YYYY-MM-DD`.
    pub end_date: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`Provider`] implementation may return.
#[derive(Debug)]
pub enum ProviderError {
    /// Network or transport failure.
    Transport(String),
    /// The upstream API returned an application-level error.
    Api { code: Option<i64>, message: String },
    /// A response payload could not be decoded.
    Decode(String),
    /// A required configuration value (e.g. API key) is missing or invalid.
    Config(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Transport(msg) => write!(f, "transport error: {msg}"),
            ProviderError::Api {
                code: Some(c),
                message,
            } => {
                write!(f, "provider api error code={c}: {message}")
            }
            ProviderError::Api {
                code: None,
                message,
            } => {
                write!(f, "provider api error: {message}")
            }
            ProviderError::Decode(msg) => write!(f, "decode error: {msg}"),
            ProviderError::Config(msg) => write!(f, "config error: {msg}"),
        }
    }
}

impl std::error::Error for ProviderError {}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Upstream market-data provider contract.
///
/// Implementations must be object-safe so callers can hold a
/// `Box<dyn Provider>` without knowing the concrete type.
///
/// Implementations must be `Send + Sync` so they can be used across
/// async task boundaries (the crate already depends on `tokio`).
pub trait Provider: Send + Sync {
    /// Human-readable name identifying this provider (e.g. `"twelvedata"`).
    fn name(&self) -> &'static str;

    /// Fetch historical OHLCV bars for the symbols and date range in `req`.
    ///
    /// Returns bars in the order supplied by the upstream API; callers are
    /// responsible for sorting or deduplication.
    fn fetch_historical(&self, req: &FetchRequest) -> Result<Vec<RawBar>, ProviderError>;

    /// Fetch the most-recent (potentially incomplete) bar for each symbol.
    ///
    /// Implementations may return an empty `Vec` if real-time data is not
    /// supported by this provider; the default does exactly that.
    fn fetch_latest(&self, symbols: &[String]) -> Result<Vec<RawBar>, ProviderError> {
        let _ = symbols;
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-process mock that satisfies the trait for use in unit tests.
    struct MockProvider {
        bars: Vec<RawBar>,
    }

    impl Provider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn fetch_historical(&self, _req: &FetchRequest) -> Result<Vec<RawBar>, ProviderError> {
            Ok(self.bars.clone())
        }
    }

    fn sample_bar(symbol: &str) -> RawBar {
        RawBar {
            symbol: symbol.to_string(),
            timeframe: "1D".to_string(),
            end_ts: 1_700_000_000,
            open: "100.00".to_string(),
            high: "105.00".to_string(),
            low: "99.00".to_string(),
            close: "103.00".to_string(),
            volume: 1_000_000,
            is_complete: true,
        }
    }

    #[test]
    fn mock_provider_returns_configured_bars() {
        let bars = vec![sample_bar("AAPL"), sample_bar("MSFT")];
        let provider: Box<dyn Provider> = Box::new(MockProvider { bars: bars.clone() });

        let req = FetchRequest {
            symbols: vec!["AAPL".to_string(), "MSFT".to_string()],
            timeframe: "1D".to_string(),
            start_date: "2023-11-01".to_string(),
            end_date: "2023-11-14".to_string(),
        };

        let result = provider.fetch_historical(&req).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].symbol, "AAPL");
        assert_eq!(result[1].symbol, "MSFT");
    }

    #[test]
    fn fetch_latest_default_returns_empty() {
        let provider: Box<dyn Provider> = Box::new(MockProvider { bars: vec![] });
        let result = provider.fetch_latest(&["AAPL".to_string()]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn provider_error_display_api_with_code() {
        let err = ProviderError::Api {
            code: Some(400),
            message: "bad symbol".to_string(),
        };
        assert_eq!(err.to_string(), "provider api error code=400: bad symbol");
    }

    #[test]
    fn provider_error_display_api_no_code() {
        let err = ProviderError::Api {
            code: None,
            message: "rate limited".to_string(),
        };
        assert_eq!(err.to_string(), "provider api error: rate limited");
    }

    #[test]
    fn provider_error_display_transport() {
        let err = ProviderError::Transport("connection refused".to_string());
        assert_eq!(err.to_string(), "transport error: connection refused");
    }

    #[test]
    fn raw_bar_clone_eq() {
        let a = sample_bar("SPY");
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn provider_is_object_safe_via_box() {
        // Compile-time proof: trait object can be constructed.
        let _p: Box<dyn Provider> = Box::new(MockProvider { bars: vec![] });
    }
}
