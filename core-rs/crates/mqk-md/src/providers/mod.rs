//! Latest-mark provider implementations.
//!
//! Distinct from `provider_registry`'s bar-oriented `MarketDataProvider`
//! factory: these providers produce [`crate::latest_mark::LatestMark`]
//! values only, never a completed OHLCV bar.

pub mod coinlore;
