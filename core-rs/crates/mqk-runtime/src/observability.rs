//! B4: Observability & Operator Diagnostics
//!
//! Pure read-only snapshot types and builder functions for the execution
//! pipeline.  No execution semantics are touched here.
//!
//! All builder functions are pure (no DB access, no side-effects, deterministic).
//! Only `collect_db_snapshot` is async and touches the DB.
//!
//! # [T]-guard compliance
//!
//! This module never calls `Utc::now()` directly.  All functions that need
//! a timestamp accept `now: DateTime<Utc>` from the caller.  The orchestrator
//! method `snapshot()` sources `now` from `self.time_source.now_utc()`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use mqk_db::{InboxRow, OutboxRow};
use mqk_execution::{
    oms::state_machine::{OmsOrder, OrderState},
    BrokerOrderMap,
};
use mqk_portfolio::PortfolioState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// Read-only snapshot of a single live OMS order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderSnapshot {
    pub order_id: String,
    /// Broker-assigned order ID, if the submit has been confirmed.
    pub broker_order_id: Option<String>,
    pub symbol: String,
    pub total_qty: i64,
    pub filled_qty: i64,
    /// One of: "Open" | "PartiallyFilled" | "Filled" | "CancelPending" |
    /// "Cancelled" | "ReplacePending" | "Rejected"
    pub status: String,
}

/// Read-only snapshot of a single outbox row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxSnapshot {
    pub outbox_id: i64,
    pub idempotency_key: String,
    /// One of: PENDING | CLAIMED | DISPATCHING | SENT | ACKED | FAILED | AMBIGUOUS
    pub status: String,
    pub created_at_utc: DateTime<Utc>,
    pub sent_at_utc: Option<DateTime<Utc>>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub dispatching_at_utc: Option<DateTime<Utc>>,
}

/// Read-only snapshot of a single inbox event row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxEventSnapshot {
    pub broker_message_id: String,
    /// Extracted from `message_json["type"]` (e.g. `"fill"` | `"partial_fill"`).
    /// Falls back to `"unknown"` if the field is absent or non-string.
    pub event_type: String,
    pub received_at_utc: DateTime<Utc>,
    pub applied: bool,
    pub applied_at_utc: Option<DateTime<Utc>>,
}

/// Read-only snapshot of a single position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub symbol: String,
    /// Signed quantity: positive = long, negative = short, 0 = flat.
    pub net_qty: i64,
}

/// Read-only snapshot of portfolio state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSnapshot {
    pub cash_micros: i64,
    pub realized_pnl_micros: i64,
    pub positions: Vec<PositionSnapshot>,
}

/// One structured record of a risk gate denial captured during a tick.
///
/// Populated by the orchestrator when `RiskGate::evaluate_gate()` returns
/// `RiskDecision::Deny`.  Fields map directly from `RiskDenial.reason` and
/// `RiskDenial.evidence`; no values are inferred or fabricated.
///
/// `strategy_id` is not available from the risk gate path — the gate operates
/// on the order itself, not on the strategy that generated it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialRecord {
    /// Stable display ID: `"{denied_at_utc_micros}:{rule_code}"`.
    /// Unique for all practical purposes within a session.
    pub id: String,
    /// UTC timestamp when the denial was captured.
    pub denied_at_utc: DateTime<Utc>,
    /// Machine-readable rule code, e.g. `"POSITION_LIMIT_EXCEEDED"`.
    pub rule: String,
    /// Human-readable one-line message from `RiskReason::as_summary()`.
    pub message: String,
    /// Symbol from the order being submitted when the denial fired.
    pub symbol: Option<String>,
    /// Requested order quantity, if populated by the risk rule.
    pub requested_qty: Option<i64>,
    /// Configured limit that was breached, if populated by the risk rule.
    pub limit: Option<i64>,
    /// Always `"critical"` for risk gate denials (all variants block execution).
    pub severity: String,
}

/// Structured description of why the system is currently blocked, if it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlockState {
    /// Machine-readable reason code.
    ///
    /// One of: `"HALTED_IN_DB"` | `"INTEGRITY_DISARMED"`.
    pub reason_code: String,
    /// Human-readable one-line summary.
    pub reason_summary: String,
    /// Supporting key-value evidence pairs for operator inspection.
    pub evidence: Vec<(String, String)>,
}

/// Full point-in-time execution pipeline snapshot.
///
/// Constructed by [`ExecutionOrchestrator::snapshot`] — never written by
/// `tick()`.  This type is entirely read-only and has no effect on execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSnapshot {
    pub run_id: Option<Uuid>,
    /// Live (non-terminal) OMS orders from in-memory state.
    pub active_orders: Vec<OrderSnapshot>,
    /// Outbox rows that are not yet ACKED (pending / in-flight / failed).
    pub pending_outbox: Vec<OutboxSnapshot>,
    /// Unapplied inbox rows for the current run.
    pub recent_inbox_events: Vec<InboxEventSnapshot>,
    pub portfolio: PortfolioSnapshot,
    /// Present only when the system is in a blocked / halted state.
    pub system_block_state: Option<SystemBlockState>,
    /// Risk gate denials captured during this session.
    ///
    /// Populated from the orchestrator's bounded ring buffer; empty only when
    /// the risk gate has not denied any order since the execution loop started.
    /// This is authoritative: `[]` means genuinely zero denials this session,
    /// not "source not wired."  Overlaid by the orchestrator after DB snapshot.
    pub recent_risk_denials: Vec<RiskDenialRecord>,
    /// UTC timestamp at which this snapshot was taken.
    pub snapshot_at_utc: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Pure builder functions
// ---------------------------------------------------------------------------

/// Convert an [`OrderState`] to its canonical string name.
pub fn order_state_name(state: &OrderState) -> &'static str {
    match state {
        OrderState::Open => "Open",
        OrderState::PartiallyFilled => "PartiallyFilled",
        OrderState::Filled => "Filled",
        OrderState::CancelPending => "CancelPending",
        OrderState::Cancelled => "Cancelled",
        OrderState::ReplacePending => "ReplacePending",
        OrderState::Rejected => "Rejected",
    }
}

/// Build order snapshots from in-memory OMS state.
///
/// Joins each order with the broker-assigned ID from [`BrokerOrderMap`].
/// Orders for which the submit has not yet been confirmed will have
/// `broker_order_id = None`.
pub fn build_order_snapshots(
    oms_orders: &BTreeMap<String, OmsOrder>,
    broker_order_map: &BrokerOrderMap,
) -> Vec<OrderSnapshot> {
    oms_orders
        .values()
        .map(|o| OrderSnapshot {
            order_id: o.order_id.clone(),
            broker_order_id: broker_order_map.broker_id(&o.order_id).map(str::to_string),
            symbol: o.symbol.clone(),
            total_qty: o.total_qty,
            filled_qty: o.filled_qty,
            status: order_state_name(&o.state).to_string(),
        })
        .collect()
}

/// Build a portfolio snapshot from in-memory portfolio state.
pub fn build_portfolio_snapshot(portfolio: &PortfolioState) -> PortfolioSnapshot {
    let positions = portfolio
        .positions
        .values()
        .map(|p| PositionSnapshot {
            symbol: p.symbol.clone(),
            net_qty: p.qty_signed(),
        })
        .collect();

    PortfolioSnapshot {
        cash_micros: portfolio.cash_micros,
        realized_pnl_micros: portfolio.realized_pnl_micros,
        positions,
    }
}

/// Build outbox snapshots from DB rows.
pub fn build_outbox_snapshots(rows: &[OutboxRow]) -> Vec<OutboxSnapshot> {
    rows.iter()
        .map(|r| OutboxSnapshot {
            outbox_id: r.outbox_id,
            idempotency_key: r.idempotency_key.clone(),
            status: r.status.clone(),
            created_at_utc: r.created_at_utc,
            sent_at_utc: r.sent_at_utc,
            claimed_at_utc: r.claimed_at_utc,
            dispatching_at_utc: r.dispatching_at_utc,
        })
        .collect()
}

/// Build inbox event snapshots from DB rows.
///
/// Extracts the event type from `message_json["type"]`, falling back to
/// `"unknown"` if the field is absent or not a string.
pub fn build_inbox_snapshots(rows: &[InboxRow]) -> Vec<InboxEventSnapshot> {
    rows.iter()
        .map(|r| {
            let event_type = r
                .message_json
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            InboxEventSnapshot {
                broker_message_id: r.broker_message_id.clone(),
                event_type,
                received_at_utc: r.received_at_utc,
                applied: r.applied_at_utc.is_some(),
                applied_at_utc: r.applied_at_utc,
            }
        })
        .collect()
}

/// Build a system block state from pre-fetched gate signals.
///
/// Returns `None` if the system is not blocked.
///
/// Priority order:
/// 1. `HALTED_IN_DB` — run record shows HALTED status.
/// 2. `INTEGRITY_DISARMED` — arm state is DISARMED.
pub fn build_system_block_state(
    run_halted: bool,
    integrity_disarmed: bool,
    arm_reason: Option<&str>,
    extra_evidence: Vec<(String, String)>,
) -> Option<SystemBlockState> {
    if run_halted {
        let mut evidence = vec![("run_status".to_string(), "HALTED".to_string())];
        evidence.extend(extra_evidence);
        return Some(SystemBlockState {
            reason_code: "HALTED_IN_DB".to_string(),
            reason_summary: "Run is HALTED — no further ticks will be accepted".to_string(),
            evidence,
        });
    }

    if integrity_disarmed {
        let mut evidence = vec![];
        if let Some(reason) = arm_reason {
            evidence.push(("disarm_reason".to_string(), reason.to_string()));
        }
        evidence.extend(extra_evidence);
        return Some(SystemBlockState {
            reason_code: "INTEGRITY_DISARMED".to_string(),
            reason_summary: "Integrity gate is disarmed — broker submissions blocked".to_string(),
            evidence,
        });
    }

    None
}

// ---------------------------------------------------------------------------
// DB-backed async collection
// ---------------------------------------------------------------------------

/// Collect a full execution snapshot from the DB for the given run.
///
/// This is the DB-backed half of the snapshot.  Callers must overlay
/// in-memory OMS and portfolio state afterwards — see
/// [`ExecutionOrchestrator::snapshot`][crate::orchestrator::ExecutionOrchestrator::snapshot].
///
/// `now` must be sourced from the caller's injected `TimeSource`; this
/// function never calls `Utc::now()` directly.
pub async fn collect_db_snapshot(
    pool: &PgPool,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<ExecutionSnapshot> {
    let outbox_rows = mqk_db::outbox_list_unacked_for_run(pool, run_id).await?;
    let inbox_rows = mqk_db::inbox_load_unapplied_for_run(pool, run_id).await?;

    let run = mqk_db::fetch_run(pool, run_id).await?;
    let arm = mqk_db::load_arm_state(pool).await?;

    let run_halted = matches!(run.status, mqk_db::RunStatus::Halted);
    let (integrity_disarmed, arm_reason) = match &arm {
        Some((state, reason)) if state == "DISARMED" => (true, reason.as_deref()),
        _ => (false, None),
    };

    let system_block_state =
        build_system_block_state(run_halted, integrity_disarmed, arm_reason, vec![]);

    Ok(ExecutionSnapshot {
        run_id: Some(run_id),
        // Overlaid by the caller with in-memory OMS state.
        active_orders: vec![],
        pending_outbox: build_outbox_snapshots(&outbox_rows),
        recent_inbox_events: build_inbox_snapshots(&inbox_rows),
        // Overlaid by the caller with in-memory portfolio state.
        portfolio: PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: vec![],
        },
        system_block_state,
        // Overlaid by the caller with the orchestrator's denial ring buffer.
        recent_risk_denials: vec![],
        snapshot_at_utc: now,
    })
}
