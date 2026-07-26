//! mqk-daemon library target.
//!
//! Exposes the router and state for integration tests.
//! The binary `main.rs` depends on this library target.

pub mod api_types;
pub mod artifact_intake;
pub mod backtest_jobs;
pub mod bind;
pub mod capital_policy;
pub mod cors;
pub mod daily_data_readiness;
pub mod decision;
pub mod dev_gate;
pub mod earnings_calendar;
pub mod event_risk_blackout;
pub mod ingest_jobs;
pub mod market_data_freshness;
pub mod mode_transition;
pub mod notify;
pub mod parity_evidence;
pub mod pre_event_flatten;
pub mod promotion_gate;
pub mod routes;
pub mod runtime_opportunity_allocation;
pub mod runtime_opportunity_artifact;
pub mod runtime_opportunity_mode;
pub mod state;
pub mod strategy_scan_jobs;
pub mod suppression;
pub mod watchlist_intake;
