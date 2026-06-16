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
use std::sync::{Arc, Mutex};

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
// Capability-aware provider contract
// ---------------------------------------------------------------------------

/// Canonical provider output used by the capability-aware market-data contract.
///
/// This aliases the existing crate-level `ProviderBar` so this contract does not
/// introduce a second OHLCV model.
pub type CanonicalBar = crate::ProviderBar;

/// Asset classes a provider declares support for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAssetClass {
    Equity,
    Etf,
    Crypto,
    Futures,
    Options,
    Forex,
    Other(String),
}

/// Capability declarations for a market-data provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDataProviderCapabilities {
    pub historical_bars: bool,
    pub latest_closed_bar: bool,
    pub completed_bar_stream: bool,
    pub supported_asset_classes: Vec<ProviderAssetClass>,
    pub supported_timeframes: Vec<crate::Timeframe>,
}

impl MarketDataProviderCapabilities {
    pub fn historical_only(supported_timeframes: Vec<crate::Timeframe>) -> Self {
        Self {
            historical_bars: true,
            latest_closed_bar: false,
            completed_bar_stream: false,
            supported_asset_classes: vec![ProviderAssetClass::Equity],
            supported_timeframes,
        }
    }
}

/// Provider availability status. It is metadata only and must not trigger routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketDataProviderHealthStatus {
    Available,
    Degraded,
    Unavailable,
    Unknown,
}

/// Provider health metadata. `last_error` must not contain credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDataProviderHealth {
    pub status: MarketDataProviderHealthStatus,
    pub last_error: Option<String>,
}

impl MarketDataProviderHealth {
    pub fn unknown() -> Self {
        Self {
            status: MarketDataProviderHealthStatus::Unknown,
            last_error: None,
        }
    }
}

/// Optional provider rate-limit metadata. Unknown values remain `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDataProviderRateLimits {
    pub calls_per_minute: Option<u32>,
    pub calls_per_day: Option<u32>,
    pub remaining_calls: Option<u32>,
    pub notes: Option<String>,
}

/// Historical bars request for the capability-aware provider contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalBarsRequest {
    pub symbols: Vec<String>,
    pub timeframe: crate::Timeframe,
    pub start: chrono::NaiveDate,
    pub end: chrono::NaiveDate,
}

impl From<HistoricalBarsRequest> for crate::FetchBarsRequest {
    fn from(request: HistoricalBarsRequest) -> Self {
        Self {
            symbols: request.symbols,
            timeframe: request.timeframe,
            start: request.start,
            end: request.end,
        }
    }
}

/// Latest closed bar request for one symbol/timeframe at a deterministic reference time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestClosedBarRequest {
    pub symbol: String,
    pub timeframe: crate::Timeframe,
    pub reference_ts: i64,
}

/// Typed failures returned by the capability-aware provider contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketDataProviderError {
    UnsupportedCapability {
        provider_id: String,
        capability: String,
    },
    InvalidSymbol {
        symbol: String,
    },
    InvalidTimeframe {
        timeframe: String,
    },
    MissingCredential {
        provider_id: String,
        credential: String,
    },
    RateLimited {
        provider_id: String,
        retry_after_secs: Option<u64>,
    },
    ProviderUnavailable {
        provider_id: String,
        message: String,
    },
    MalformedResponse {
        provider_id: String,
        message: String,
    },
    EmptyResponse {
        provider_id: String,
        message: String,
    },
    Other {
        provider_id: String,
        message: String,
    },
}

impl fmt::Display for MarketDataProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarketDataProviderError::UnsupportedCapability {
                provider_id,
                capability,
            } => write!(
                f,
                "market-data provider {provider_id} does not support capability {capability}"
            ),
            MarketDataProviderError::InvalidSymbol { symbol } => {
                write!(f, "invalid market-data symbol: {symbol}")
            }
            MarketDataProviderError::InvalidTimeframe { timeframe } => {
                write!(f, "invalid market-data timeframe: {timeframe}")
            }
            MarketDataProviderError::MissingCredential {
                provider_id,
                credential,
            } => write!(
                f,
                "market-data provider {provider_id} missing credential {credential}"
            ),
            MarketDataProviderError::RateLimited {
                provider_id,
                retry_after_secs: Some(retry_after_secs),
            } => write!(
                f,
                "market-data provider {provider_id} rate limited; retry_after_secs={retry_after_secs}"
            ),
            MarketDataProviderError::RateLimited {
                provider_id,
                retry_after_secs: None,
            } => write!(f, "market-data provider {provider_id} rate limited"),
            MarketDataProviderError::ProviderUnavailable {
                provider_id,
                message,
            } => write!(
                f,
                "market-data provider {provider_id} unavailable: {message}"
            ),
            MarketDataProviderError::MalformedResponse {
                provider_id,
                message,
            } => write!(
                f,
                "market-data provider {provider_id} malformed response: {message}"
            ),
            MarketDataProviderError::EmptyResponse {
                provider_id,
                message,
            } => write!(
                f,
                "market-data provider {provider_id} empty response: {message}"
            ),
            MarketDataProviderError::Other {
                provider_id,
                message,
            } => write!(f, "market-data provider {provider_id} error: {message}"),
        }
    }
}

impl std::error::Error for MarketDataProviderError {}

/// Capability-aware market-data provider boundary.
#[async_trait::async_trait]
pub trait MarketDataProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn capabilities(&self) -> MarketDataProviderCapabilities;
    fn health(&self) -> MarketDataProviderHealth;
    fn rate_limits(&self) -> Option<MarketDataProviderRateLimits>;

    async fn fetch_historical_bars(
        &self,
        request: HistoricalBarsRequest,
    ) -> Result<Vec<CanonicalBar>, MarketDataProviderError>;

    async fn fetch_latest_closed_bar(
        &self,
        request: LatestClosedBarRequest,
    ) -> Result<Option<CanonicalBar>, MarketDataProviderError>;

    fn completed_bar_stream_supported(&self) -> bool {
        self.capabilities().completed_bar_stream
    }
}

/// Adapter for the existing async historical-provider trait.
///
/// This preserves current historical callers while exposing the new contract to
/// future ingestion code.
pub struct HistoricalProviderMarketDataAdapter<P> {
    inner: P,
    provider_id: String,
    display_name: String,
    capabilities: MarketDataProviderCapabilities,
    health: MarketDataProviderHealth,
    rate_limits: Option<MarketDataProviderRateLimits>,
}

impl<P> HistoricalProviderMarketDataAdapter<P>
where
    P: crate::HistoricalProvider,
{
    pub fn new(
        inner: P,
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
        supported_timeframes: Vec<crate::Timeframe>,
    ) -> Self {
        Self {
            inner,
            provider_id: provider_id.into(),
            display_name: display_name.into(),
            capabilities: MarketDataProviderCapabilities::historical_only(supported_timeframes),
            health: MarketDataProviderHealth::unknown(),
            rate_limits: None,
        }
    }

    pub fn with_rate_limits(mut self, rate_limits: MarketDataProviderRateLimits) -> Self {
        self.rate_limits = Some(rate_limits);
        self
    }

    pub fn with_health(mut self, health: MarketDataProviderHealth) -> Self {
        self.health = health;
        self
    }
}

#[async_trait::async_trait]
impl<P> MarketDataProvider for HistoricalProviderMarketDataAdapter<P>
where
    P: crate::HistoricalProvider,
{
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> MarketDataProviderCapabilities {
        self.capabilities.clone()
    }

    fn health(&self) -> MarketDataProviderHealth {
        self.health.clone()
    }

    fn rate_limits(&self) -> Option<MarketDataProviderRateLimits> {
        self.rate_limits.clone()
    }

    async fn fetch_historical_bars(
        &self,
        request: HistoricalBarsRequest,
    ) -> Result<Vec<CanonicalBar>, MarketDataProviderError> {
        self.inner
            .fetch_bars(request.into())
            .await
            .map_err(|err| MarketDataProviderError::Other {
                provider_id: self.provider_id.clone(),
                message: err.to_string(),
            })
    }

    async fn fetch_latest_closed_bar(
        &self,
        _request: LatestClosedBarRequest,
    ) -> Result<Option<CanonicalBar>, MarketDataProviderError> {
        Err(MarketDataProviderError::UnsupportedCapability {
            provider_id: self.provider_id.clone(),
            capability: "latest_closed_bar".to_string(),
        })
    }
}

/// Call counters for deterministic fake-provider tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeMarketDataProviderCallCounts {
    pub historical_bars: u64,
    pub latest_closed_bar: u64,
}

/// Deterministic in-memory provider for contract tests.
#[derive(Debug, Clone)]
pub struct FakeMarketDataProvider {
    provider_id: String,
    display_name: String,
    capabilities: MarketDataProviderCapabilities,
    health: MarketDataProviderHealth,
    rate_limits: Option<MarketDataProviderRateLimits>,
    historical_bars: Vec<CanonicalBar>,
    latest_closed_bar: Option<CanonicalBar>,
    historical_error: Option<MarketDataProviderError>,
    latest_error: Option<MarketDataProviderError>,
    call_counts: Arc<Mutex<FakeMarketDataProviderCallCounts>>,
}

impl FakeMarketDataProvider {
    pub fn new(
        historical_bars: Vec<CanonicalBar>,
        latest_closed_bar: Option<CanonicalBar>,
    ) -> Self {
        Self {
            provider_id: "fake".to_string(),
            display_name: "Fake Market Data".to_string(),
            capabilities: MarketDataProviderCapabilities {
                historical_bars: true,
                latest_closed_bar: true,
                completed_bar_stream: false,
                supported_asset_classes: vec![ProviderAssetClass::Equity],
                supported_timeframes: vec![
                    crate::Timeframe::D1,
                    crate::Timeframe::H1,
                    crate::Timeframe::M1,
                    crate::Timeframe::M5,
                ],
            },
            health: MarketDataProviderHealth {
                status: MarketDataProviderHealthStatus::Available,
                last_error: None,
            },
            rate_limits: Some(MarketDataProviderRateLimits {
                calls_per_minute: Some(60),
                calls_per_day: None,
                remaining_calls: None,
                notes: Some("fake provider has deterministic in-memory limits".to_string()),
            }),
            historical_bars,
            latest_closed_bar,
            historical_error: None,
            latest_error: None,
            call_counts: Arc::new(Mutex::new(FakeMarketDataProviderCallCounts::default())),
        }
    }

    pub fn with_capabilities(mut self, capabilities: MarketDataProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_health(mut self, health: MarketDataProviderHealth) -> Self {
        self.health = health;
        self
    }

    pub fn with_rate_limits(mut self, rate_limits: Option<MarketDataProviderRateLimits>) -> Self {
        self.rate_limits = rate_limits;
        self
    }

    pub fn with_historical_error(mut self, error: MarketDataProviderError) -> Self {
        self.historical_error = Some(error);
        self
    }

    pub fn with_latest_error(mut self, error: MarketDataProviderError) -> Self {
        self.latest_error = Some(error);
        self
    }

    pub fn call_counts(&self) -> FakeMarketDataProviderCallCounts {
        self.call_counts
            .lock()
            .expect("fake market data provider call count mutex poisoned")
            .clone()
    }

    fn validate_symbol(symbol: &str) -> Result<(), MarketDataProviderError> {
        if symbol.trim().is_empty() {
            return Err(MarketDataProviderError::InvalidSymbol {
                symbol: symbol.to_string(),
            });
        }
        Ok(())
    }

    fn validate_timeframe(
        &self,
        timeframe: crate::Timeframe,
    ) -> Result<(), MarketDataProviderError> {
        if !self.capabilities.supported_timeframes.is_empty()
            && !self.capabilities.supported_timeframes.contains(&timeframe)
        {
            return Err(MarketDataProviderError::InvalidTimeframe {
                timeframe: timeframe.as_str().to_string(),
            });
        }
        Ok(())
    }

    fn increment_historical_calls(&self) {
        self.call_counts
            .lock()
            .expect("fake market data provider call count mutex poisoned")
            .historical_bars += 1;
    }

    fn increment_latest_calls(&self) {
        self.call_counts
            .lock()
            .expect("fake market data provider call count mutex poisoned")
            .latest_closed_bar += 1;
    }
}

#[async_trait::async_trait]
impl MarketDataProvider for FakeMarketDataProvider {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> MarketDataProviderCapabilities {
        self.capabilities.clone()
    }

    fn health(&self) -> MarketDataProviderHealth {
        self.health.clone()
    }

    fn rate_limits(&self) -> Option<MarketDataProviderRateLimits> {
        self.rate_limits.clone()
    }

    async fn fetch_historical_bars(
        &self,
        request: HistoricalBarsRequest,
    ) -> Result<Vec<CanonicalBar>, MarketDataProviderError> {
        self.increment_historical_calls();

        if !self.capabilities.historical_bars {
            return Err(MarketDataProviderError::UnsupportedCapability {
                provider_id: self.provider_id.clone(),
                capability: "historical_bars".to_string(),
            });
        }
        for symbol in &request.symbols {
            Self::validate_symbol(symbol)?;
        }
        self.validate_timeframe(request.timeframe)?;
        if let Some(error) = &self.historical_error {
            return Err(error.clone());
        }

        let start_ts = request
            .start
            .and_hms_opt(0, 0, 0)
            .expect("valid start date")
            .and_utc()
            .timestamp();
        let end_ts = request
            .end
            .and_hms_opt(23, 59, 59)
            .expect("valid end date")
            .and_utc()
            .timestamp();
        let timeframe = request.timeframe.as_str();

        Ok(self
            .historical_bars
            .iter()
            .filter(|bar| {
                request.symbols.iter().any(|symbol| symbol == &bar.symbol)
                    && bar.timeframe == timeframe
                    && bar.end_ts >= start_ts
                    && bar.end_ts <= end_ts
            })
            .cloned()
            .collect())
    }

    async fn fetch_latest_closed_bar(
        &self,
        request: LatestClosedBarRequest,
    ) -> Result<Option<CanonicalBar>, MarketDataProviderError> {
        self.increment_latest_calls();

        if !self.capabilities.latest_closed_bar {
            return Err(MarketDataProviderError::UnsupportedCapability {
                provider_id: self.provider_id.clone(),
                capability: "latest_closed_bar".to_string(),
            });
        }
        Self::validate_symbol(&request.symbol)?;
        self.validate_timeframe(request.timeframe)?;
        if let Some(error) = &self.latest_error {
            return Err(error.clone());
        }

        let timeframe = request.timeframe.as_str();
        Ok(self.latest_closed_bar.as_ref().and_then(|bar| {
            (bar.symbol == request.symbol
                && bar.timeframe == timeframe
                && bar.is_complete
                && bar.end_ts <= request.reference_ts)
                .then(|| bar.clone())
        }))
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

    fn sample_canonical_bar(symbol: &str, end_ts: i64) -> CanonicalBar {
        CanonicalBar {
            symbol: symbol.to_string(),
            timeframe: crate::Timeframe::D1.as_str().to_string(),
            end_ts,
            open: "100".to_string(),
            high: "105".to_string(),
            low: "99".to_string(),
            close: "103".to_string(),
            volume: 1_000,
            is_complete: true,
        }
    }

    fn historical_provider_request() -> HistoricalBarsRequest {
        HistoricalBarsRequest {
            symbols: vec!["AAPL".to_string()],
            timeframe: crate::Timeframe::D1,
            start: chrono::NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(2023, 11, 15).unwrap(),
        }
    }

    fn latest_provider_request() -> LatestClosedBarRequest {
        LatestClosedBarRequest {
            symbol: "AAPL".to_string(),
            timeframe: crate::Timeframe::D1,
            reference_ts: 1_700_000_100,
        }
    }

    #[tokio::test]
    async fn fake_market_data_provider_reports_capabilities_correctly() {
        let provider = FakeMarketDataProvider::new(vec![], None);

        assert_eq!(provider.provider_id(), "fake");
        assert_eq!(provider.display_name(), "Fake Market Data");

        let capabilities = provider.capabilities();
        assert!(capabilities.historical_bars);
        assert!(capabilities.latest_closed_bar);
        assert!(!capabilities.completed_bar_stream);
        assert!(provider.completed_bar_stream_supported() == capabilities.completed_bar_stream);
        assert!(capabilities
            .supported_asset_classes
            .contains(&ProviderAssetClass::Equity));
        assert!(capabilities
            .supported_timeframes
            .contains(&crate::Timeframe::D1));
    }

    #[tokio::test]
    async fn fake_market_data_provider_returns_historical_bars() {
        let bar = sample_canonical_bar("AAPL", 1_700_000_000);
        let provider = FakeMarketDataProvider::new(vec![bar.clone()], None);

        let result = provider
            .fetch_historical_bars(historical_provider_request())
            .await
            .unwrap();

        assert_eq!(result, vec![bar]);
        assert_eq!(provider.call_counts().historical_bars, 1);
    }

    #[tokio::test]
    async fn fake_market_data_provider_returns_latest_closed_bar() {
        let bar = sample_canonical_bar("AAPL", 1_700_000_000);
        let provider = FakeMarketDataProvider::new(vec![], Some(bar.clone()));

        let result = provider
            .fetch_latest_closed_bar(latest_provider_request())
            .await
            .unwrap();

        assert_eq!(result, Some(bar));
        assert_eq!(provider.call_counts().latest_closed_bar, 1);
    }

    #[tokio::test]
    async fn fake_market_data_provider_unsupported_latest_returns_typed_error() {
        let mut capabilities = FakeMarketDataProvider::new(vec![], None).capabilities();
        capabilities.latest_closed_bar = false;
        let provider = FakeMarketDataProvider::new(vec![], None).with_capabilities(capabilities);

        let err = provider
            .fetch_latest_closed_bar(latest_provider_request())
            .await
            .unwrap_err();

        assert_eq!(
            err,
            MarketDataProviderError::UnsupportedCapability {
                provider_id: "fake".to_string(),
                capability: "latest_closed_bar".to_string()
            }
        );
    }

    #[tokio::test]
    async fn fake_market_data_provider_unsupported_historical_returns_typed_error() {
        let mut capabilities = FakeMarketDataProvider::new(vec![], None).capabilities();
        capabilities.historical_bars = false;
        let provider = FakeMarketDataProvider::new(vec![], None).with_capabilities(capabilities);

        let err = provider
            .fetch_historical_bars(historical_provider_request())
            .await
            .unwrap_err();

        assert_eq!(
            err,
            MarketDataProviderError::UnsupportedCapability {
                provider_id: "fake".to_string(),
                capability: "historical_bars".to_string()
            }
        );
    }

    #[tokio::test]
    async fn fake_market_data_provider_error_surfaces_truthfully() {
        let expected = MarketDataProviderError::RateLimited {
            provider_id: "fake".to_string(),
            retry_after_secs: Some(30),
        };
        let provider =
            FakeMarketDataProvider::new(vec![], None).with_historical_error(expected.clone());

        let err = provider
            .fetch_historical_bars(historical_provider_request())
            .await
            .unwrap_err();

        assert_eq!(err, expected);
    }

    #[tokio::test]
    async fn fake_market_data_provider_empty_latest_returns_none() {
        let provider = FakeMarketDataProvider::new(vec![], None);

        let result = provider
            .fetch_latest_closed_bar(latest_provider_request())
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn fake_market_data_provider_health_and_rate_limits_have_no_secret_material() {
        let provider = FakeMarketDataProvider::new(vec![], None)
            .with_health(MarketDataProviderHealth {
                status: MarketDataProviderHealthStatus::Degraded,
                last_error: Some("provider returned retryable status".to_string()),
            })
            .with_rate_limits(Some(MarketDataProviderRateLimits {
                calls_per_minute: Some(8),
                calls_per_day: None,
                remaining_calls: None,
                notes: Some("free-tier test metadata".to_string()),
            }));

        let metadata = format!("{:?}{:?}", provider.health(), provider.rate_limits());

        assert!(provider.health().last_error.is_some());
        assert_eq!(provider.rate_limits().unwrap().calls_per_minute, Some(8));
        assert!(!metadata.contains("SECRET"));
        assert!(!metadata.contains("API_KEY"));
    }

    #[tokio::test]
    async fn fake_market_data_provider_supported_timeframe_is_accepted() {
        let bar = sample_canonical_bar("AAPL", 1_700_000_000);
        let provider = FakeMarketDataProvider::new(vec![bar], None).with_capabilities(
            MarketDataProviderCapabilities {
                historical_bars: true,
                latest_closed_bar: true,
                completed_bar_stream: false,
                supported_asset_classes: vec![ProviderAssetClass::Equity],
                supported_timeframes: vec![crate::Timeframe::D1],
            },
        );

        let result = provider
            .fetch_historical_bars(historical_provider_request())
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn fake_market_data_provider_unsupported_timeframe_returns_typed_error() {
        let provider = FakeMarketDataProvider::new(vec![], None).with_capabilities(
            MarketDataProviderCapabilities {
                historical_bars: true,
                latest_closed_bar: true,
                completed_bar_stream: false,
                supported_asset_classes: vec![ProviderAssetClass::Equity],
                supported_timeframes: vec![crate::Timeframe::D1],
            },
        );
        let mut request = historical_provider_request();
        request.timeframe = crate::Timeframe::M5;

        let err = provider.fetch_historical_bars(request).await.unwrap_err();

        assert_eq!(
            err,
            MarketDataProviderError::InvalidTimeframe {
                timeframe: "5m".to_string()
            }
        );
    }

    struct AdapterHistoricalProvider {
        bars: Vec<CanonicalBar>,
    }

    #[async_trait::async_trait]
    impl crate::HistoricalProvider for AdapterHistoricalProvider {
        fn source_name(&self) -> &'static str {
            "adapter-test"
        }

        async fn fetch_bars(
            &self,
            _req: crate::FetchBarsRequest,
        ) -> anyhow::Result<Vec<CanonicalBar>> {
            Ok(self.bars.clone())
        }
    }

    #[tokio::test]
    async fn historical_provider_market_data_adapter_preserves_existing_fetch() {
        let bar = sample_canonical_bar("AAPL", 1_700_000_000);
        let adapter = HistoricalProviderMarketDataAdapter::new(
            AdapterHistoricalProvider {
                bars: vec![bar.clone()],
            },
            "adapter-test",
            "Adapter Test",
            vec![crate::Timeframe::D1],
        );

        let result = adapter
            .fetch_historical_bars(historical_provider_request())
            .await
            .unwrap();

        assert_eq!(adapter.provider_id(), "adapter-test");
        assert_eq!(adapter.display_name(), "Adapter Test");
        assert_eq!(result, vec![bar]);
        assert!(matches!(
            adapter
                .fetch_latest_closed_bar(latest_provider_request())
                .await
                .unwrap_err(),
            MarketDataProviderError::UnsupportedCapability { .. }
        ));
    }
}
