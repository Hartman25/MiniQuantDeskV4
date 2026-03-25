//! CC-02A: Durable strategy suppression write seam.
//!
//! Provides the narrowest fail-closed write path for strategy suppressions,
//! validated against canonical strategy registry identity.
//!
//! # Entry points
//!
//! - [`suppress_strategy`] — create an active suppression for a registered strategy
//! - [`clear_suppression`]  — deactivate an existing active suppression by ID
//!
//! # Design
//!
//! Suppressions must key off canonical strategy identity from
//! `sys_strategy_registry`.  Unknown/unregistered strategies fail closed.
//! Disabled strategies may be suppressed — suppression is about halting
//! activity, not about registration state.
//!
//! All writes route through the existing `mqk-db` durable functions
//! (`insert_strategy_suppression`, `clear_strategy_suppression`).
//! No side channels or synthetic writes.
//!
//! Timestamps are caller-injected; no `now()` derivation inside this module.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Public input type
// ---------------------------------------------------------------------------

/// Arguments for creating a new active strategy suppression.
#[derive(Debug, Clone)]
pub struct SuppressStrategyArgs {
    /// Caller-provided UUID; used as the stable idempotency key.
    ///
    /// Re-submitting the same `suppression_id` is safe: the underlying
    /// insert is `ON CONFLICT (suppression_id) DO NOTHING`.
    pub suppression_id: Uuid,
    /// Authoritative strategy identity.  Must exist in `sys_strategy_registry`.
    pub strategy_id: String,
    /// Category of the trigger (e.g. `"operator"`, `"risk"`, `"integrity"`).
    /// Must not be blank.
    pub trigger_domain: String,
    /// Human-readable one-line reason.  Must not be blank.
    pub trigger_reason: String,
    /// Caller-injected timestamp.  Never derived from `now()` inside this module.
    pub started_at_utc: DateTime<Utc>,
    /// Optional operator note; empty string is acceptable.
    pub note: String,
}

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

/// Outcome of a single call to [`suppress_strategy`].
#[derive(Debug, Clone)]
pub struct SuppressOutcome {
    /// `true` only when a row was inserted (new or idempotent re-submit).
    pub suppressed: bool,
    /// Machine-readable disposition:
    ///
    /// | value          | meaning                                                   |
    /// |----------------|-----------------------------------------------------------|
    /// | `"suppressed"` | row inserted (new or idempotent duplicate — both succeed) |
    /// | `"rejected"`   | field validation failure or strategy not in registry      |
    /// | `"unavailable"`| no DB or transient I/O failure                            |
    pub disposition: String,
    /// Echoed from [`SuppressStrategyArgs::suppression_id`].
    pub suppression_id: Uuid,
    /// Echoed from [`SuppressStrategyArgs::strategy_id`] (trimmed).
    pub strategy_id: String,
    /// Human-readable explanations for non-suppressed outcomes.  Empty on success.
    pub blockers: Vec<String>,
}

/// Outcome of a single call to [`clear_suppression`].
#[derive(Debug, Clone)]
pub struct ClearOutcome {
    /// `true` only when an active row was transitioned to `'cleared'`.
    pub cleared: bool,
    /// Machine-readable disposition:
    ///
    /// | value          | meaning                                                |
    /// |----------------|--------------------------------------------------------|
    /// | `"cleared"`    | active row successfully transitioned to cleared        |
    /// | `"not_active"` | no active row found (already cleared or never existed) |
    /// | `"unavailable"`| no DB or transient I/O failure                         |
    pub disposition: String,
    /// Echoed from the `suppression_id` argument.
    pub suppression_id: Uuid,
    /// Human-readable explanations for non-cleared outcomes.  Empty on success.
    pub blockers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Internal outcome constructors
// ---------------------------------------------------------------------------

fn suppress_ok(suppression_id: Uuid, strategy_id: String) -> SuppressOutcome {
    SuppressOutcome {
        suppressed: true,
        disposition: "suppressed".to_string(),
        suppression_id,
        strategy_id,
        blockers: vec![],
    }
}

fn suppress_err(
    disposition: &str,
    suppression_id: Uuid,
    strategy_id: String,
    blockers: Vec<String>,
) -> SuppressOutcome {
    SuppressOutcome {
        suppressed: false,
        disposition: disposition.to_string(),
        suppression_id,
        strategy_id,
        blockers,
    }
}

fn clear_ok(suppression_id: Uuid) -> ClearOutcome {
    ClearOutcome {
        cleared: true,
        disposition: "cleared".to_string(),
        suppression_id,
        blockers: vec![],
    }
}

fn clear_err(disposition: &str, suppression_id: Uuid, blockers: Vec<String>) -> ClearOutcome {
    ClearOutcome {
        cleared: false,
        disposition: disposition.to_string(),
        suppression_id,
        blockers,
    }
}

// ---------------------------------------------------------------------------
// Gate 0 helper: field validation
// ---------------------------------------------------------------------------

fn validate_suppress_fields(args: &SuppressStrategyArgs) -> Result<(), Vec<String>> {
    let mut blockers = Vec::new();

    if args.strategy_id.trim().is_empty() {
        blockers.push("strategy_id must not be blank".to_string());
    }
    if args.trigger_domain.trim().is_empty() {
        blockers.push("trigger_domain must not be blank".to_string());
    }
    if args.trigger_reason.trim().is_empty() {
        blockers.push("trigger_reason must not be blank".to_string());
    }

    if blockers.is_empty() {
        Ok(())
    } else {
        Err(blockers)
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Create a durable active suppression for a registered strategy.
///
/// # Gate sequence (fail-closed)
///
/// ```text
/// 0. field_validation  — strategy_id / trigger_domain / trigger_reason non-blank
/// 1. db_present        — no DB → unavailable
/// 2. registry_check    — strategy must exist in sys_strategy_registry
///                        (disabled strategies are suppressible)
/// 3. insert            — insert_strategy_suppression (ON CONFLICT DO NOTHING)
/// ```
pub async fn suppress_strategy(
    state: &Arc<AppState>,
    args: SuppressStrategyArgs,
) -> SuppressOutcome {
    let sid = args.strategy_id.trim().to_string();
    let id = args.suppression_id;

    // Gate 0: field validation.
    if let Err(blockers) = validate_suppress_fields(&args) {
        return suppress_err("rejected", id, sid, blockers);
    }

    // Gate 1: DB must be present.
    let Some(db) = state.db.as_ref() else {
        return suppress_err(
            "unavailable",
            id,
            sid,
            vec!["durable suppression DB truth is unavailable on this daemon".to_string()],
        );
    };

    // Gate 2: strategy must be registered (any enabled state).
    match mqk_db::fetch_strategy_registry_entry(db, &sid).await {
        Ok(Some(_)) => {
            // Registered — pass.  Enabled/disabled does not affect suppressibility.
        }
        Ok(None) => {
            return suppress_err(
                "rejected",
                id,
                sid.clone(),
                vec![format!(
                    "suppression refused: strategy '{sid}' is not registered \
                     in the strategy registry"
                )],
            );
        }
        Err(err) => {
            return suppress_err(
                "unavailable",
                id,
                sid,
                vec![format!(
                    "suppression unavailable: registry lookup failed: {err}"
                )],
            );
        }
    }

    // Gate 3: insert suppression (idempotent).
    let db_args = mqk_db::InsertStrategySuppressionArgs {
        suppression_id: id,
        strategy_id: sid.clone(),
        trigger_domain: args.trigger_domain.trim().to_string(),
        trigger_reason: args.trigger_reason.trim().to_string(),
        started_at_utc: args.started_at_utc,
        note: args.note,
    };

    match mqk_db::insert_strategy_suppression(db, &db_args).await {
        Ok(()) => suppress_ok(id, sid),
        Err(err) => suppress_err(
            "unavailable",
            id,
            sid,
            vec![format!("suppression write failed: {err}")],
        ),
    }
}

/// Deactivate an existing active suppression.
///
/// Sets `state = 'cleared'` and `cleared_at_utc` on the row.
/// Returns `"not_active"` if no active row exists for the given ID
/// (already cleared or never recorded) — this is not an error, it is
/// explicit fail-closed honest accounting.
///
/// # Gate sequence (fail-closed)
///
/// ```text
/// 1. db_present  — no DB → unavailable
/// 2. clear       — clear_strategy_suppression (state transition)
/// ```
pub async fn clear_suppression(
    state: &Arc<AppState>,
    suppression_id: Uuid,
    cleared_at_utc: DateTime<Utc>,
) -> ClearOutcome {
    // Gate 1: DB must be present.
    let Some(db) = state.db.as_ref() else {
        return clear_err(
            "unavailable",
            suppression_id,
            vec!["durable suppression DB truth is unavailable on this daemon".to_string()],
        );
    };

    // Gate 2: clear the suppression (state transition).
    match mqk_db::clear_strategy_suppression(db, suppression_id, cleared_at_utc).await {
        Ok(true) => clear_ok(suppression_id),
        Ok(false) => clear_err(
            "not_active",
            suppression_id,
            vec![format!(
                "suppression '{suppression_id}' has no active row \
                 (already cleared or not found)"
            )],
        ),
        Err(err) => clear_err(
            "unavailable",
            suppression_id,
            vec![format!("suppression clear failed: {err}")],
        ),
    }
}
