//! Trade lifecycle alert events emitted by the execution orchestrator.
//!
//! These events are injected via `ExecutionOrchestrator::set_alert_sink` and
//! delivered best-effort to an operator notification channel (Discord).
//!
//! All events carry `run_id` and enough context for an operator to identify
//! the affected order without reading logs.  No secrets or internal state
//! are included.
//!
//! Delivery is best-effort: failure must never block or halt the trading path.

use uuid::Uuid;

/// A single trade lifecycle event emitted after a durable state transition.
///
/// Every variant fires *after* the relevant DB write has committed so the
/// operator notification never races the ground-truth DB state.
#[derive(Debug, Clone)]
pub enum TradeLifecycleEvent {
    /// An outbox row was successfully dispatched to the broker (SENT).
    ///
    /// Fired in Phase 1 after `outbox_mark_sent_with_broker_map` succeeds.
    OrderSubmitted {
        run_id: Uuid,
        order_id: String,
        symbol: String,
        qty: i64,
    },
    /// A broker ACK was received and durably applied.
    ///
    /// Fired in Phase 3 after `inbox_mark_applied` succeeds for an Ack event.
    OrderAcked {
        run_id: Uuid,
        order_id: String,
        broker_order_id: Option<String>,
        symbol: Option<String>,
    },
    /// A broker fill (partial or terminal) was durably applied.
    ///
    /// Fired in Phase 3 after `inbox_mark_applied` succeeds for a Fill or
    /// PartialFill event.  `terminal` is `true` when the fill exhausted the
    /// order's remaining quantity.
    FillApplied {
        run_id: Uuid,
        order_id: String,
        symbol: String,
        side: String,
        qty: i64,
        price_micros: i64,
        terminal: bool,
    },
    /// Phase 0c detected irrecoverable reconcile drift and halted the run.
    ///
    /// Fired after `persist_halt_and_disarm` completes for any
    /// `RECONCILE_DRIFT` or `ReconcileDrift` path.
    ReconcileDriftHalt { run_id: Uuid, reason: String },
    /// Phase 0b detected ambiguous outbox rows on restart and quarantined.
    ///
    /// Fired after `persist_halt_and_disarm` completes for `RecoveryQuarantine`.
    RecoveryQuarantine { run_id: Uuid },
}
