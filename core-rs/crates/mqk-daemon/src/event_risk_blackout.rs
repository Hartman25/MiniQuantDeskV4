//! Event-risk blackout gate — operator-configured static blackout periods.
//!
//! EVENT-RISK-GATE-01: Adds a fail-closed signal-admission blackout gate
//! analogous to `CorporateActionPolicy::ForbidPeriods` in the backtest engine.
//!
//! # Configuration
//!
//! Set `MQK_EVENT_RISK_BLACKOUT_PATH` to the path of a JSON blackout file.
//! When the variable is absent or empty, the gate is `NotConfigured` —
//! signals pass through without blackout enforcement.
//!
//! # Blackout file schema (`blackout-v1`)
//!
//! ```json
//! {
//!   "schema_version": "blackout-v1",
//!   "periods": [
//!     { "symbol": "AAPL", "start_ts": 1700000000, "end_ts": 1700086400 },
//!     { "symbol": "MSFT", "start_ts": 1700100000, "end_ts": 1700186400 }
//!   ]
//! }
//! ```
//!
//! Timestamps are Unix epoch seconds.  Bounds are inclusive: a signal at
//! exactly `start_ts` or `end_ts` is refused.
//!
//! # Gate outcomes
//!
//! - `NotConfigured`  — no blackout file configured; gate is not enforced.
//! - `Authorized`     — symbol has no matching period at the given timestamp.
//! - `Blackout`       — symbol is in a declared period; signal refused (fail-closed).
//! - `Unavailable`    — file configured but unreadable or unparseable (fail-closed).
//!
//! # Honest limits
//!
//! This gate enforces operator-declared static blackout periods only.  No live
//! earnings calendar feed is connected (`earnings_calendar_feed = "not_connected"`
//! on the event-risk-status surface).  Pre-event position flattening is also not
//! wired.  This gate closes only the `signal_admission_gate` sub-seam.

use serde::Deserialize;

pub const ENV_BLACKOUT_PATH: &str = "MQK_EVENT_RISK_BLACKOUT_PATH";
const EXPECTED_SCHEMA_VERSION: &str = "blackout-v1";

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// Outcome of an event-risk blackout evaluation at signal admission.
#[derive(Debug, Clone, PartialEq)]
pub enum EventRiskBlackoutOutcome {
    /// No blackout file is configured — gate not enforced; signal may proceed.
    NotConfigured,
    /// Symbol has no matching blackout period at the given timestamp — authorized.
    Authorized,
    /// Symbol is in a declared blackout period — signal must be refused.
    Blackout { note: String },
    /// Blackout file is configured but cannot be read or parsed — fail-closed.
    Unavailable { reason: String },
}

impl EventRiskBlackoutOutcome {
    /// `true` when the signal may safely proceed past this gate.
    pub fn is_signal_safe(&self) -> bool {
        matches!(self, Self::NotConfigured | Self::Authorized)
    }
}

// ---------------------------------------------------------------------------
// Surface label
// ---------------------------------------------------------------------------

/// Returns the `signal_admission_gate` value for the event-risk-status surface.
///
/// - `"configured"`     — `MQK_EVENT_RISK_BLACKOUT_PATH` is set (gate active).
/// - `"not_configured"` — env var absent or empty (gate present but not enforced).
pub fn blackout_surface_label() -> &'static str {
    match std::env::var(ENV_BLACKOUT_PATH) {
        Ok(p) if !p.trim().is_empty() => "configured",
        _ => "not_configured",
    }
}

// ---------------------------------------------------------------------------
// Evaluators
// ---------------------------------------------------------------------------

/// Evaluate event-risk blackout for a symbol at a Unix epoch timestamp.
///
/// Reads `MQK_EVENT_RISK_BLACKOUT_PATH` from the environment.
pub fn evaluate_event_risk_blackout_from_env(symbol: &str, ts: i64) -> EventRiskBlackoutOutcome {
    let path = match std::env::var(ENV_BLACKOUT_PATH) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return EventRiskBlackoutOutcome::NotConfigured,
    };
    evaluate_event_risk_blackout_from_file(&path, symbol, ts)
}

/// Read the blackout file at `path` and evaluate.
pub fn evaluate_event_risk_blackout_from_file(
    path: &str,
    symbol: &str,
    ts: i64,
) -> EventRiskBlackoutOutcome {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return EventRiskBlackoutOutcome::Unavailable {
                reason: format!("cannot read blackout file at '{path}': {e}"),
            };
        }
    };
    evaluate_event_risk_blackout(content.trim(), symbol, ts)
}

/// Core pure evaluator — accepts the JSON content of a blackout file directly.
///
/// Exported for tests that supply JSON without a filesystem dependency.
pub fn evaluate_event_risk_blackout(json: &str, symbol: &str, ts: i64) -> EventRiskBlackoutOutcome {
    let file: BlackoutFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            return EventRiskBlackoutOutcome::Unavailable {
                reason: format!("blackout file parse error: {e}"),
            };
        }
    };

    if file.schema_version != EXPECTED_SCHEMA_VERSION {
        return EventRiskBlackoutOutcome::Unavailable {
            reason: format!(
                "blackout file schema_version '{}' is not supported \
                 (expected '{EXPECTED_SCHEMA_VERSION}')",
                file.schema_version
            ),
        };
    }

    for period in &file.periods {
        if period.symbol == symbol && ts >= period.start_ts && ts <= period.end_ts {
            return EventRiskBlackoutOutcome::Blackout {
                note: format!(
                    "symbol '{}' is in a declared event-risk blackout period \
                     [start_ts={}, end_ts={}] — signal refused",
                    symbol, period.start_ts, period.end_ts
                ),
            };
        }
    }

    EventRiskBlackoutOutcome::Authorized
}

// ---------------------------------------------------------------------------
// Internal schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BlackoutFile {
    schema_version: String,
    periods: Vec<BlackoutPeriod>,
}

#[derive(Debug, Deserialize)]
struct BlackoutPeriod {
    symbol: String,
    start_ts: i64,
    end_ts: i64,
}
