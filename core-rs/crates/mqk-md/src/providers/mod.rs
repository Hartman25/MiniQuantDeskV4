//! Concrete provider implementations grouped by provider, not by shape.
//!
//! `coinlore` produces [`crate::latest_mark::LatestMark`] values only, never
//! a completed OHLCV bar. `kraken` produces completed OHLCV bars
//! (`crate::ProviderBar`) via a parser plus a `crate::HistoricalProvider`
//! adapter, default-disabled in `config/providers/providers.json`.

pub mod coinlore;
pub mod kraken;
