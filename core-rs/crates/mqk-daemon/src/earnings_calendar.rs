//! Earnings-calendar gate — operator-configured earnings event proximity source.
//!
//! EVENT-RISK-CALENDAR-01: Adds a file-backed earnings calendar source that
//! derives signal-admission blackout windows from declared earnings events.
//!
//! # Distinction from `blackout-v1`
//!
//! `blackout-v1` (`MQK_EVENT_RISK_BLACKOUT_PATH`) stores raw timestamp ranges
//! declared by the operator for any reason (earnings, FOMC, scheduled halts, etc.).
//! This schema stores the earnings event timestamp plus operator-specified pre/post
//! windows, and the evaluator derives the effective blackout interval at check time.
//! The two sources are independent and can both be configured simultaneously.
//!
//! # Configuration
//!
//! Set `MQK_EARNINGS_CALENDAR_PATH` to the path of a JSON earnings calendar file.
//! When the variable is absent or empty, the gate is `NotConfigured` —
//! signals pass through without earnings-proximity enforcement.
//!
//! # Earnings calendar file schema (`earnings-calendar-v1`)
//!
//! ```json
//! {
//!   "schema_version": "earnings-calendar-v1",
//!   "entries": [
//!     {
//!       "symbol": "AAPL",
//!       "earnings_ts": 1700043600,
//!       "pre_window_secs": 86400,
//!       "post_window_secs": 14400
//!     }
//!   ]
//! }
//! ```
//!
//! Timestamps are Unix epoch seconds.  The effective blackout interval is
//! `[earnings_ts − pre_window_secs, earnings_ts + post_window_secs]`, inclusive.
//! A signal arriving within that interval is refused fail-closed.
//!
//! # Gate outcomes
//!
//! - `NotConfigured`    — no calendar file configured; gate not enforced.
//! - `Authorized`       — symbol has no earnings proximity at the given timestamp.
//! - `EarningsBlackout` — signal falls within a declared earnings window; refused.
//! - `Unavailable`      — file configured but unreadable or unparseable (fail-closed).
//!
//! # Honest limits
//!
//! This is an operator-declared file source only.  No live earnings calendar feed
//! (data-provider API) is connected.  The `earnings_calendar_feed` surface label
//! transitions to `"operator_file"` when this gate is configured; it remains
//! `"not_connected"` when `MQK_EARNINGS_CALENDAR_PATH` is absent.

use serde::Deserialize;

pub const ENV_EARNINGS_CALENDAR_PATH: &str = "MQK_EARNINGS_CALENDAR_PATH";
const EXPECTED_SCHEMA_VERSION: &str = "earnings-calendar-v1";

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// Outcome of an earnings-calendar proximity evaluation at signal admission.
#[derive(Debug, Clone, PartialEq)]
pub enum EarningsCalendarOutcome {
    /// No calendar file is configured — gate not enforced; signal may proceed.
    NotConfigured,
    /// Symbol has no matching earnings proximity at the given timestamp — authorized.
    Authorized,
    /// Signal falls within a declared earnings proximity window — refused (fail-closed).
    EarningsBlackout { note: String },
    /// Calendar file is configured but cannot be read or parsed — fail-closed.
    Unavailable { reason: String },
}

impl EarningsCalendarOutcome {
    /// `true` when the signal may safely proceed past this gate.
    pub fn is_signal_safe(&self) -> bool {
        matches!(self, Self::NotConfigured | Self::Authorized)
    }
}

// ---------------------------------------------------------------------------
// Surface label
// ---------------------------------------------------------------------------

/// Returns the `earnings_calendar_feed` value for the event-risk-status surface.
///
/// - `"operator_file"` — `MQK_EARNINGS_CALENDAR_PATH` is set (calendar source active).
/// - `"not_connected"` — env var absent or empty (no calendar source configured).
pub fn earnings_calendar_surface_label() -> &'static str {
    match std::env::var(ENV_EARNINGS_CALENDAR_PATH) {
        Ok(p) if !p.trim().is_empty() => "operator_file",
        _ => "not_connected",
    }
}

// ---------------------------------------------------------------------------
// Evaluators
// ---------------------------------------------------------------------------

/// Evaluate earnings-calendar proximity for a symbol at a Unix epoch timestamp.
///
/// Reads `MQK_EARNINGS_CALENDAR_PATH` from the environment.
pub fn evaluate_earnings_calendar_from_env(symbol: &str, ts: i64) -> EarningsCalendarOutcome {
    let path = match std::env::var(ENV_EARNINGS_CALENDAR_PATH) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return EarningsCalendarOutcome::NotConfigured,
    };
    evaluate_earnings_calendar_from_file(&path, symbol, ts)
}

/// Read the earnings calendar file at `path` and evaluate.
pub fn evaluate_earnings_calendar_from_file(
    path: &str,
    symbol: &str,
    ts: i64,
) -> EarningsCalendarOutcome {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return EarningsCalendarOutcome::Unavailable {
                reason: format!("cannot read earnings calendar at '{path}': {e}"),
            };
        }
    };
    evaluate_earnings_calendar(content.trim(), symbol, ts)
}

/// Core pure evaluator — accepts the JSON content of an earnings calendar directly.
///
/// Exported for tests that supply JSON without a filesystem dependency.
pub fn evaluate_earnings_calendar(json: &str, symbol: &str, ts: i64) -> EarningsCalendarOutcome {
    let file: EarningsCalendarFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            return EarningsCalendarOutcome::Unavailable {
                reason: format!("earnings calendar parse error: {e}"),
            };
        }
    };

    if file.schema_version != EXPECTED_SCHEMA_VERSION {
        return EarningsCalendarOutcome::Unavailable {
            reason: format!(
                "earnings calendar schema_version '{}' is not supported \
                 (expected '{EXPECTED_SCHEMA_VERSION}')",
                file.schema_version
            ),
        };
    }

    for entry in &file.entries {
        if entry.symbol != symbol {
            continue;
        }
        let blackout_start = entry.earnings_ts.saturating_sub(entry.pre_window_secs);
        let blackout_end = entry.earnings_ts.saturating_add(entry.post_window_secs);
        if ts >= blackout_start && ts <= blackout_end {
            return EarningsCalendarOutcome::EarningsBlackout {
                note: format!(
                    "symbol '{}' is within an earnings proximity window \
                     [earnings_ts={}, pre={}s, post={}s; effective blackout={}-{}] \
                     — signal refused",
                    symbol,
                    entry.earnings_ts,
                    entry.pre_window_secs,
                    entry.post_window_secs,
                    blackout_start,
                    blackout_end,
                ),
            };
        }
    }

    EarningsCalendarOutcome::Authorized
}

// ---------------------------------------------------------------------------
// Internal schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EarningsCalendarFile {
    schema_version: String,
    entries: Vec<EarningsEntry>,
}

#[derive(Debug, Deserialize)]
struct EarningsEntry {
    symbol: String,
    earnings_ts: i64,
    pre_window_secs: i64,
    post_window_secs: i64,
}
