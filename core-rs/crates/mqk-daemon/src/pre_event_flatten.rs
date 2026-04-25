//! Pre-event position flattening — pure lookahead trigger detection and
//! close-order construction.
//!
//! # What is closed
//!
//! ## EVENT-RISK-FLATTEN-01: detection sub-seam
//!
//! Pure lookahead evaluator for both event-risk sources:
//! - `blackout-v1` (`MQK_EVENT_RISK_BLACKOUT_PATH`)
//! - `earnings-calendar-v1` (`MQK_EARNINGS_CALENDAR_PATH`)
//!
//! Returns `FlattenRequired` when a blackout period is active *or* is starting
//! within `lead_secs` of the current timestamp.
//!
//! ## EVENT-RISK-FLATTEN-WIRE-01: orchestrator wiring sub-seam
//!
//! The execution loop in `state::loop_runner` calls this evaluator per tick for
//! every non-flat position.  When `FlattenRequired` or `Unavailable` is returned,
//! a market close order is enqueued into the outbox via `outbox_enqueue`.
//! The close order is dispatched by the orchestrator's Phase 1 on the next tick.
//!
//! # Status surface
//!
//! `pre_event_flattening` on `/api/v1/execution/event-risk-status`:
//! - `"active"`          — wiring is in the loop AND ≥1 source is configured.
//! - `"wired_no_source"` — wiring is in the loop but no source is configured;
//!   evaluator returns `NotConfigured` every tick (no-op).
//!
//! # Fail-closed contract
//!
//! - `Unavailable` from either configured source → enqueue close (fail-closed;
//!   cannot verify it is safe to hold).
//! - `FlattenRequired` from any source → enqueue close.
//! - `NotConfigured` only when **neither** source is configured → no-op.

use serde::Deserialize;

/// Default flatten lead: 30 minutes before blackout start.
///
/// The runtime should flatten positions at least this far before any blackout
/// period begins, to allow time for order submission and settlement.
pub const DEFAULT_FLATTEN_LEAD_SECS: i64 = 1800;

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// Outcome of a pre-event flatten trigger evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum FlattenTriggerOutcome {
    /// Neither event-risk source is configured — no flatten check enforced.
    NotConfigured,
    /// No upcoming or active blackout within the lead window — no flatten needed.
    NoFlattenRequired,
    /// A blackout period is active or starting within `lead_secs` — flatten positions.
    FlattenRequired { note: String },
    /// A source is configured but cannot be read or parsed — fail-closed.
    Unavailable { reason: String },
}

impl FlattenTriggerOutcome {
    /// `true` when positions in this symbol should be flattened.
    pub fn is_flatten_required(&self) -> bool {
        matches!(self, Self::FlattenRequired { .. })
    }

    /// `true` when a configured source is unreadable — runtime should treat
    /// this as a flatten blocker (cannot confirm it is safe to hold).
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}

// ---------------------------------------------------------------------------
// Combined env-backed entry point
// ---------------------------------------------------------------------------

/// Evaluate pre-event flatten trigger for a symbol using both configured sources.
///
/// Reads `MQK_EVENT_RISK_BLACKOUT_PATH` and `MQK_EARNINGS_CALENDAR_PATH` from
/// the environment.  `lead_secs` is the lookahead window: positions are flagged
/// for flattening when a blackout period will start within `lead_secs` from `ts`.
///
/// Decision table:
/// - `FlattenRequired` from any source → `FlattenRequired` (flatten wins).
/// - `Unavailable` from either configured source (unless FlattenRequired already
///   found) → `Unavailable` (fail-closed; cannot verify it is safe to hold).
/// - Both sources `NotConfigured` → `NotConfigured`.
/// - At least one source configured, no trigger, no error → `NoFlattenRequired`.
pub fn evaluate_flatten_trigger_from_env(
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    let blackout_result = evaluate_flatten_trigger_from_blackout_env(symbol, ts, lead_secs);
    let earnings_result = evaluate_flatten_trigger_from_earnings_env(symbol, ts, lead_secs);

    // FlattenRequired from any source wins immediately.
    if matches!(
        blackout_result,
        FlattenTriggerOutcome::FlattenRequired { .. }
    ) {
        return blackout_result;
    }
    if matches!(
        earnings_result,
        FlattenTriggerOutcome::FlattenRequired { .. }
    ) {
        return earnings_result;
    }

    // Unavailable from either configured source → fail-closed.
    if matches!(blackout_result, FlattenTriggerOutcome::Unavailable { .. }) {
        return blackout_result;
    }
    if matches!(earnings_result, FlattenTriggerOutcome::Unavailable { .. }) {
        return earnings_result;
    }

    // Both not configured → NotConfigured.
    if matches!(blackout_result, FlattenTriggerOutcome::NotConfigured)
        && matches!(earnings_result, FlattenTriggerOutcome::NotConfigured)
    {
        return FlattenTriggerOutcome::NotConfigured;
    }

    // At least one source configured, no trigger found.
    FlattenTriggerOutcome::NoFlattenRequired
}

// ---------------------------------------------------------------------------
// Blackout-v1 evaluators
// ---------------------------------------------------------------------------

fn evaluate_flatten_trigger_from_blackout_env(
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    use crate::event_risk_blackout::ENV_BLACKOUT_PATH;
    let path = match std::env::var(ENV_BLACKOUT_PATH) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return FlattenTriggerOutcome::NotConfigured,
    };
    evaluate_flatten_trigger_from_blackout_file(&path, symbol, ts, lead_secs)
}

/// Read the blackout-v1 file at `path` and evaluate the flatten trigger.
pub fn evaluate_flatten_trigger_from_blackout_file(
    path: &str,
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return FlattenTriggerOutcome::Unavailable {
                reason: format!("cannot read blackout file at '{path}': {e}"),
            };
        }
    };
    evaluate_flatten_trigger_from_blackout(content.trim(), symbol, ts, lead_secs)
}

/// Core pure evaluator for `blackout-v1` JSON.
///
/// `FlattenRequired` when any period for `symbol` satisfies either:
/// - Active: `ts` is within `[start_ts, end_ts]` — existing positions must close.
/// - Upcoming: `start_ts` is within `(ts, ts + lead_secs]` — blackout imminent.
///
/// Exported for tests that supply JSON directly without a filesystem dependency.
pub fn evaluate_flatten_trigger_from_blackout(
    json: &str,
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    let file: BlackoutFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            return FlattenTriggerOutcome::Unavailable {
                reason: format!("blackout file parse error: {e}"),
            };
        }
    };

    if file.schema_version != "blackout-v1" {
        return FlattenTriggerOutcome::Unavailable {
            reason: format!(
                "blackout file schema_version '{}' is not supported (expected 'blackout-v1')",
                file.schema_version
            ),
        };
    }

    for period in &file.periods {
        if period.symbol != symbol {
            continue;
        }
        // Active: currently inside blackout window — existing positions must close.
        if ts >= period.start_ts && ts <= period.end_ts {
            return FlattenTriggerOutcome::FlattenRequired {
                note: format!(
                    "symbol '{}' is in an active blackout period [start_ts={}, end_ts={}] — \
                     flatten existing positions",
                    symbol, period.start_ts, period.end_ts
                ),
            };
        }
        // Upcoming: blackout starts within the lead window.
        if period.start_ts > ts && period.start_ts <= ts.saturating_add(lead_secs) {
            return FlattenTriggerOutcome::FlattenRequired {
                note: format!(
                    "symbol '{}' has a blackout period starting in {}s [start_ts={}, end_ts={}] — \
                     flatten positions before blackout opens",
                    symbol,
                    period.start_ts.saturating_sub(ts),
                    period.start_ts,
                    period.end_ts
                ),
            };
        }
    }

    FlattenTriggerOutcome::NoFlattenRequired
}

// ---------------------------------------------------------------------------
// Earnings-calendar-v1 evaluators
// ---------------------------------------------------------------------------

fn evaluate_flatten_trigger_from_earnings_env(
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    use crate::earnings_calendar::ENV_EARNINGS_CALENDAR_PATH;
    let path = match std::env::var(ENV_EARNINGS_CALENDAR_PATH) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return FlattenTriggerOutcome::NotConfigured,
    };
    evaluate_flatten_trigger_from_earnings_file(&path, symbol, ts, lead_secs)
}

/// Read the earnings-calendar-v1 file at `path` and evaluate the flatten trigger.
pub fn evaluate_flatten_trigger_from_earnings_file(
    path: &str,
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return FlattenTriggerOutcome::Unavailable {
                reason: format!("cannot read earnings calendar at '{path}': {e}"),
            };
        }
    };
    evaluate_flatten_trigger_from_earnings(content.trim(), symbol, ts, lead_secs)
}

/// Core pure evaluator for `earnings-calendar-v1` JSON.
///
/// The effective blackout interval for each entry is
/// `[earnings_ts − pre_window_secs, earnings_ts + post_window_secs]`.
///
/// `FlattenRequired` when any entry for `symbol` satisfies either:
/// - Active: `ts` is within the derived blackout interval.
/// - Upcoming: the blackout interval start is within `(ts, ts + lead_secs]`.
///
/// Exported for tests that supply JSON directly without a filesystem dependency.
pub fn evaluate_flatten_trigger_from_earnings(
    json: &str,
    symbol: &str,
    ts: i64,
    lead_secs: i64,
) -> FlattenTriggerOutcome {
    let file: EarningsCalendarFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            return FlattenTriggerOutcome::Unavailable {
                reason: format!("earnings calendar parse error: {e}"),
            };
        }
    };

    if file.schema_version != "earnings-calendar-v1" {
        return FlattenTriggerOutcome::Unavailable {
            reason: format!(
                "earnings calendar schema_version '{}' is not supported \
                 (expected 'earnings-calendar-v1')",
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

        // Active: currently inside the earnings blackout window.
        if ts >= blackout_start && ts <= blackout_end {
            return FlattenTriggerOutcome::FlattenRequired {
                note: format!(
                    "symbol '{}' is in an active earnings blackout window \
                     [earnings_ts={}, pre={}s, post={}s; effective={}-{}] — \
                     flatten existing positions",
                    symbol,
                    entry.earnings_ts,
                    entry.pre_window_secs,
                    entry.post_window_secs,
                    blackout_start,
                    blackout_end
                ),
            };
        }
        // Upcoming: blackout interval start is within the lead window.
        if blackout_start > ts && blackout_start <= ts.saturating_add(lead_secs) {
            return FlattenTriggerOutcome::FlattenRequired {
                note: format!(
                    "symbol '{}' has an earnings blackout starting in {}s \
                     [earnings_ts={}, pre={}s, post={}s; effective={}-{}] — \
                     flatten positions before blackout opens",
                    symbol,
                    blackout_start.saturating_sub(ts),
                    entry.earnings_ts,
                    entry.pre_window_secs,
                    entry.post_window_secs,
                    blackout_start,
                    blackout_end
                ),
            };
        }
    }

    FlattenTriggerOutcome::NoFlattenRequired
}

// ---------------------------------------------------------------------------
// Internal schema types (private — not shared with existing modules to avoid
// coupling; schemas are small and stable)
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

// ---------------------------------------------------------------------------
// Close-order construction
// ---------------------------------------------------------------------------

/// Provenance value written into every outbox row sourced from the pre-event
/// flatten path.  Carried in `order_json["signal_source"]`.
pub const FLATTEN_SIGNAL_SOURCE: &str = "pre_event_flatten";

/// Build the outbox `order_json` and idempotency key for a flatten close order.
///
/// Parameters:
/// - `symbol`: the position symbol to close.
/// - `net_qty_signed`: signed position quantity (+long, -short). Must be non-zero.
/// - `ts_secs`: current UNIX epoch seconds; used to derive the minute bucket.
/// - `run_id`: the active run UUID; scopes idempotency to this run.
///
/// The key is minute-bucketed via UUIDv5:
///   `"mqk-flatten.v1.{run_id}.{symbol}.{ts_secs / 60}"`
///
/// Within any 60-second window, re-enqueuing the same trigger for the same
/// symbol is idempotent (`outbox_enqueue` returns `Ok(false)` on conflict).
/// After 60 seconds a new key allows a fresh enqueue if the position was
/// re-entered.
///
/// The returned `order_json` is a market order payload accepted by
/// `build_claimed_outbox_request` (field set matches submit-path validation):
/// `{ symbol, qty, side, order_type: "market", signal_source, request_type: "submit" }`.
pub fn build_flatten_close_order_json(
    symbol: &str,
    net_qty_signed: i64,
    ts_secs: i64,
    run_id: uuid::Uuid,
) -> (String, serde_json::Value) {
    let ts_minute = ts_secs / 60;
    let side = if net_qty_signed > 0 { "sell" } else { "buy" };
    let qty = net_qty_signed.abs();

    let key_input = format!("mqk-flatten.v1.{run_id}.{symbol}.{ts_minute}");
    let idempotency_key =
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, key_input.as_bytes()).to_string();

    let order_json = serde_json::json!({
        "symbol": symbol,
        "qty": qty,
        "side": side,
        "order_type": "market",
        "signal_source": FLATTEN_SIGNAL_SOURCE,
        "request_type": "submit",
    });

    (idempotency_key, order_json)
}
