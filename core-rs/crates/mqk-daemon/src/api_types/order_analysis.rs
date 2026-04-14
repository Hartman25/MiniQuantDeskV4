//! Per-order execution-analysis DTOs.
//!
//! Extracted from `api_types.rs` (MT-07F).
//! Routes: `/api/v1/execution/orders/:order_id/timeline`,
//!         `/api/v1/execution/orders/:order_id/trace`,
//!         `/api/v1/execution/orders/:order_id/replay`,
//!         `/api/v1/execution/orders/:order_id/chart`,
//!         `/api/v1/execution/orders/:order_id/causality`

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/timeline (Batch A5A)
// ---------------------------------------------------------------------------

/// One fill event row in the per-order execution timeline.
///
/// Source: `postgres.fill_quality_telemetry` for the active run.
/// Only fill events are represented; pre-fill outbox lifecycle events are not
/// joined to `internal_order_id` in the current schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTimelineRow {
    /// Stable event identifier derived from `telemetry_id`.
    pub event_id: String,
    /// RFC 3339 timestamp when this fill event was received.
    pub ts_utc: String,
    /// Event kind: `"partial_fill"` | `"final_fill"`.
    pub stage: String,
    /// Data provenance: always `"fill_quality_telemetry"`.
    pub source: String,
    /// Human-readable summary (e.g. `"qty=50 @ $150.250000 (partial_fill)"`).
    pub detail: Option<String>,
    pub fill_qty: Option<i64>,
    pub fill_price_micros: Option<i64>,
    pub slippage_bps: Option<i64>,
    /// Always `"oms_inbox:{broker_message_id}"` from the fill row.
    pub provenance_ref: Option<String>,
}

/// Response wrapper for `GET /api/v1/execution/orders/:order_id/timeline`.
///
/// # Truth states
///
/// - `"active"` — DB + active run + at least one fill row found; `rows` is
///   authoritative and `backend` names the exact source table.
/// - `"no_fills_yet"` — DB + active run available, order is visible in the OMS
///   execution snapshot, but no fill rows exist yet; `rows` is empty.
/// - `"no_order"` — `order_id` was not found in any current authoritative source
///   (no active run, no snapshot, or no fill history).  `rows` is empty.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
///
/// # Sources
///
/// - `symbol`, `requested_qty`, `filled_qty`, `current_status`, `current_stage`
///   — from the in-memory execution snapshot (ephemeral; not durable across restart).
/// - `rows` — from `postgres.fill_quality_telemetry` (durable, per active run).
///
/// # Honest limits
///
/// Timeline rows represent fill events only.  Pre-fill outbox lifecycle events
/// (queued/claimed/dispatching/sent) are not yet linked to `internal_order_id`
/// and are therefore absent from this surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTimelineResponse {
    /// Self-identifying canonical route including the resolved `order_id`.
    pub canonical_route: String,
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` | `"unavailable"`.
    pub backend: String,
    pub order_id: String,
    /// `null` until the broker submit is confirmed.
    pub broker_order_id: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub symbol: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub requested_qty: Option<i64>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub filled_qty: Option<i64>,
    /// Canonical OMS status from execution snapshot. `null` when snapshot is absent.
    pub current_status: Option<String>,
    /// Display-friendly stage derived from `current_status`. `null` when `current_status` is absent.
    pub current_stage: Option<String>,
    /// RFC 3339 timestamp of the most recent fill event. `null` when no fills have been received.
    pub last_event_at: Option<String>,
    /// Fill events for this order, oldest-first. At most 50 rows.
    /// Authoritative only when `truth_state == "active"`.
    pub rows: Vec<OrderTimelineRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/trace (Batch A5B)
// ---------------------------------------------------------------------------

/// One correlated event row in the per-order execution trace.
///
/// Source: `postgres.fill_quality_telemetry` for the active run.
/// Only fill events are represented; pre-fill outbox lifecycle events are not
/// joined to `internal_order_id` in the current schema.
///
/// This type extends [`OrderTimelineRow`] with additional telemetry fields
/// that are available in `fill_quality_telemetry` but absent from the timeline
/// surface: `submit_ts_utc`, `submit_to_fill_ms`, and `side`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTraceRow {
    /// Stable event identifier derived from `telemetry_id`.
    pub event_id: String,
    /// RFC 3339 timestamp when this fill event was received.
    pub ts_utc: String,
    /// Event kind: `"partial_fill"` | `"final_fill"`.
    pub stage: String,
    /// Data provenance: always `"fill_quality_telemetry"`.
    pub source: String,
    /// Human-readable summary.
    pub detail: Option<String>,
    pub fill_qty: Option<i64>,
    pub fill_price_micros: Option<i64>,
    pub slippage_bps: Option<i64>,
    /// Broker-recorded timestamp when the order was submitted, if available.
    /// Sourced from `fill_quality_telemetry.submit_ts_utc`.
    pub submit_ts_utc: Option<String>,
    /// Elapsed milliseconds from submit to fill receipt, if available.
    /// Sourced from `fill_quality_telemetry.submit_to_fill_ms`.
    pub submit_to_fill_ms: Option<i64>,
    /// Order direction: `"buy"` | `"sell"` (or broker-native string).
    /// Sourced from `fill_quality_telemetry.side`.
    pub side: Option<String>,
    /// Always `"oms_inbox:{broker_message_id}"` from the fill row.
    pub provenance_ref: Option<String>,
}

/// Response wrapper for `GET /api/v1/execution/orders/:order_id/trace`.
///
/// # Truth states
///
/// - `"active"` — DB + active run + at least one fill row found; `rows` is
///   authoritative and `backend` names the exact source table.
/// - `"no_fills_yet"` — DB + active run available, order is visible in the OMS
///   execution snapshot, but no fill rows exist yet; `rows` is empty.
/// - `"no_order"` — `order_id` was not found in any current authoritative source.
///   `rows` is empty.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
///
/// # Sources
///
/// - `symbol`, `requested_qty`, `filled_qty`, `current_status`, `current_stage`
///   — from the in-memory execution snapshot (ephemeral; not durable across restart).
/// - `outbox_status`, `outbox_lifecycle_stage`
///   — from `execution_snapshot.pending_outbox`, matched by `idempotency_key == order_id`
///   (ephemeral; not durable across restart; absent if order left the outbox window).
/// - `rows` — from `postgres.fill_quality_telemetry` (durable, per active run).
///
/// # Honest limits
///
/// - Trace rows represent fill events only.  Pre-fill outbox lifecycle events
///   (queued/claimed/dispatching/sent timestamps) are not linked to
///   `internal_order_id` in the DB schema and are therefore absent from rows.
/// - `outbox_status` is only present while the order is in the in-memory pending
///   outbox window.  Completed (ACKED) orders rotate out of this window.
/// - Broker ACK events, cancel-ack events, and replace-ack events are not surfaced
///   here; they are not currently joinable by `internal_order_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTraceResponse {
    /// Self-identifying canonical route including the resolved `order_id`.
    pub canonical_route: String,
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` | `"unavailable"`.
    pub backend: String,
    pub order_id: String,
    /// `null` until the broker submit is confirmed.
    pub broker_order_id: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub symbol: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub requested_qty: Option<i64>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub filled_qty: Option<i64>,
    /// Canonical OMS status from execution snapshot. `null` when snapshot is absent.
    pub current_status: Option<String>,
    /// Display-friendly stage derived from `current_status`. `null` when absent.
    pub current_stage: Option<String>,
    /// Current outbox transport status for this order, from the in-memory pending
    /// outbox window (e.g. `"PENDING"`, `"SENT"`, `"ACKED"`).
    /// `null` when the order is not in the current outbox window or no snapshot.
    pub outbox_status: Option<String>,
    /// Display-friendly outbox lifecycle stage derived from `outbox_status`.
    /// `null` when `outbox_status` is absent.
    pub outbox_lifecycle_stage: Option<String>,
    /// RFC 3339 timestamp of the most recent fill event. `null` when no fills received.
    pub last_event_at: Option<String>,
    /// Fill events for this order, oldest-first. At most 50 rows.
    /// Authoritative only when `truth_state == "active"`.
    pub rows: Vec<OrderTraceRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/replay (Batch A5C)
// ---------------------------------------------------------------------------

/// One frame in the per-order execution replay.
///
/// Each frame is derived from a single durable fill event in
/// `fill_quality_telemetry`.  Fields that have no joinable durable source
/// (`risk_state`, `reconcile_state`) are honestly reported as `"unknown"`.
/// `oms_state` and `queue_status` reflect the **request-time** ephemeral
/// snapshot, not the historical per-frame state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderReplayFrame {
    /// Stable frame identifier derived from `telemetry_id`.
    pub frame_id: String,
    /// RFC 3339 timestamp when this fill event was received.
    pub timestamp: String,
    /// Subsystem that produced this frame: always `"execution"`.
    pub subsystem: String,
    /// Event kind: `"partial_fill"` | `"final_fill"`.
    pub event_type: String,
    /// Human-readable state delta, e.g. `"fill_qty=N fill_price=X.XXXXXX (partial_fill)"`.
    pub state_delta: String,
    /// Provenance reference for this fill: `"oms_inbox:{broker_message_id}"`.
    pub message_digest: String,
    /// OMS status at request time (ephemeral snapshot). `"unknown"` when snapshot absent.
    pub order_execution_state: String,
    /// OMS status at request time (ephemeral snapshot). `"unknown"` when snapshot absent.
    pub oms_state: String,
    /// Running cumulative fill quantity up to and including this frame.
    pub filled_qty: i64,
    /// Open quantity remaining (`requested_qty - filled_qty`). `null` when snapshot absent.
    pub open_qty: Option<i64>,
    /// Risk state is not joinable from current sources: always `"unknown"`.
    pub risk_state: String,
    /// Reconcile state is not joinable from current sources: always `"unknown"`.
    pub reconcile_state: String,
    /// Outbox status from in-memory pending outbox window. `"unknown"` when absent.
    pub queue_status: String,
    /// Empty — anomaly detection is not available from current fill sources.
    pub anomaly_tags: Vec<String>,
    /// `["final_fill"]` when this is the terminal fill event; otherwise empty.
    pub boundary_tags: Vec<String>,
}

/// Response wrapper for `GET /api/v1/execution/orders/:order_id/replay`.
///
/// # Truth states
///
/// - `"active"` — DB + active run + at least one fill row; `frames` is authoritative.
/// - `"no_fills_yet"` — DB + active run, order is visible in the OMS snapshot, but no
///   fill rows exist yet; `frames` is empty.
/// - `"no_order"` — `order_id` not found in any current authoritative source; `frames` empty.
/// - `"no_db"` — no DB pool configured; `frames` is empty and not authoritative.
///
/// # Sources
///
/// - `frames` — from `postgres.fill_quality_telemetry` for the active run (fill events only).
/// - `oms_state`, `order_execution_state` per frame — from the in-memory execution snapshot
///   (ephemeral; reflects current state, not per-frame historical state).
///
/// # Honest limits
///
/// - Replay frames represent fill events only.  Pre-fill outbox lifecycle events, broker
///   ACK events, cancel/replace lifecycle events are not joinable by `internal_order_id`
///   in the current schema and are absent from frames.
/// - `risk_state` and `reconcile_state` per frame are always `"unknown"`.
/// - `oms_state` and `queue_status` in frames reflect the current request-time snapshot,
///   not reconstructed per-frame history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderReplayResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Authority state: `"active"` | `"no_fills_yet"` | `"no_order"` | `"no_db"`.
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` | `"unavailable"`.
    pub backend: String,
    /// Requested order identifier.
    pub order_id: String,
    /// Replay session identifier: equals `order_id` for single-order scope.
    pub replay_id: String,
    /// Always `"single_order"` for this route.
    pub replay_scope: String,
    /// Data source label: always `"fill_quality_telemetry"`.
    pub source: String,
    /// Human-readable replay title derived from symbol and order_id.
    pub title: String,
    /// Index of the most recent (last) frame. 0 when no frames.
    pub current_frame_index: usize,
    /// Fill event frames for this order, oldest-first. At most 50 rows.
    pub frames: Vec<OrderReplayFrame>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/chart  (Batch A5D)
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/execution/orders/:order_id/chart`.
///
/// # Truth-state contract
///
/// | Condition                                  | truth_state |
/// |--------------------------------------------|-------------|
/// | No per-order chart source available        | no_bars     |
/// | Order not found in any current source      | no_order    |
/// | No DB pool (identity probe impossible)     | no_db       |
///
/// # What this surface does NOT claim
///
/// - Real OHLCV bar data: no per-order chart/candle source is wired.
/// - Signal, fill, or execution overlays: not available without bar timestamps.
/// - Reference price series: not available without bar data.
///
/// `bars` and `overlays` are always empty — consumers must gate on `truth_state`
/// and treat empty arrays as non-authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderChartResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Authority state: `"no_bars"` | `"no_order"` | `"no_db"`.
    pub truth_state: String,
    /// `"unavailable"` — no chart backend is wired.
    pub backend: String,
    /// Requested order identifier.
    pub order_id: String,
    /// Symbol from in-memory OMS snapshot. `None` when snapshot absent.
    pub symbol: Option<String>,
    /// Human-readable explanation of the truth state.
    pub comment: String,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/causality  (Batch A5E)
// ---------------------------------------------------------------------------

/// One causality node derived from a durable fill event.
///
/// Represents a single execution-fill causality node from `fill_quality_telemetry`.
/// Upstream lanes (signal, intent, risk, broker_ack, reconcile, portfolio) are
/// not representable here because `internal_order_id` is not joinable to those
/// subsystems in the current schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCausalityCausalNode {
    /// Deterministic key: `"execution_fill_{telemetry_id}"`.
    pub node_key: String,
    /// Always `"execution_fill"`.
    pub node_type: String,
    /// Human-readable event title, e.g. `"partial_fill NVDA"`.
    pub title: String,
    /// Always `"ok"` — fills sourced from telemetry are confirmed events.
    pub status: String,
    /// Always `"execution"`.
    pub subsystem: String,
    /// `broker_fill_id` from telemetry, if present.
    pub linked_id: Option<String>,
    /// `fill_received_at_utc` as RFC 3339.
    pub timestamp: Option<String>,
    /// Milliseconds elapsed since the previous node. `None` for the first node.
    pub elapsed_from_prev_ms: Option<i64>,
    /// Always empty — anomaly detection is not available for fill telemetry nodes.
    pub anomaly_tags: Vec<String>,
    /// Fill summary, e.g. `"fill_qty=20 fill_price=944.200000 (partial_fill)"`.
    pub summary: String,
    /// UTC timestamp of the submit event that preceded this fill, RFC 3339.
    /// `None` when submit timing was not recorded (market orders, legacy rows).
    pub submit_ts_utc: Option<String>,
    /// Milliseconds from order submit to this fill confirmation.
    /// `None` when `submit_ts_utc` is absent.
    pub submit_to_fill_ms: Option<i64>,
}

/// Response for `GET /api/v1/execution/orders/:order_id/causality`.
///
/// # Truth-state contract
///
/// | Condition                                              | truth_state   |
/// |--------------------------------------------------------|---------------|
/// | DB + active run + outbox row or fill row found         | partial       |
/// | DB + active run + order in memory, no outbox/fills     | no_fills_yet  |
/// | DB + active run + order not found anywhere             | no_order      |
/// | No DB pool                                             | no_db         |
///
/// `"partial"` (not `"active"`) is intentional: full causality
/// (signal→intent→risk→outbox→broker→portfolio→reconcile) is not provable here.
///
/// # What this surface proves
///
/// - Intent lane: `outbox_enqueued` and `outbox_sent` nodes from `oms_outbox`
///   when an outbox row exists for the `order_id` (idempotency_key convention).
/// - Execution-fill lane: fill events from `fill_quality_telemetry`.
///
/// # What this surface does NOT claim
///
/// - Signal provenance: not joinable to `internal_order_id`.
/// - Broker ACK events: not joinable to `internal_order_id` in current schema.
/// - Portfolio effects: not captured per-order in durable form.
/// - Reconcile outcomes: not joinable to `internal_order_id` at this tier.
///
/// Consumers must treat `unproven_lanes` as explicitly unavailable, not as
/// empty/passed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCausalityResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Authority state: `"partial"` | `"no_fills_yet"` | `"no_order"` | `"no_db"`.
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` | `"unavailable"`.
    pub backend: String,
    /// Requested order identifier.
    pub order_id: String,
    /// Symbol from in-memory OMS snapshot. `None` when snapshot absent.
    pub symbol: Option<String>,
    /// Causality lanes for which proof exists: `["execution_fill"]` when partial,
    /// `[]` otherwise.
    pub proven_lanes: Vec<String>,
    /// Causality lanes that are explicitly not available in the current schema:
    /// `["signal", "intent", "broker_ack", "risk", "reconcile", "portfolio"]`.
    pub unproven_lanes: Vec<String>,
    /// Fill-derived causality nodes (execution lane only), oldest-first. Empty when
    /// `truth_state != "partial"`.
    pub nodes: Vec<OrderCausalityCausalNode>,
    /// Human-readable explanation of what is and is not proven.
    pub comment: String,
}
