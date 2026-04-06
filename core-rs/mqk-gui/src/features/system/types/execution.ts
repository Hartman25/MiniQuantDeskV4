// core-rs/mqk-gui/src/features/system/types/execution.ts
//
// Execution, OMS, order lifecycle, timeline, trace, replay, and chart types.

import type { EnvironmentMode, HealthState, OmsState, Severity } from "./core";

export interface ExecutionSummary {
  active_orders: number;
  pending_orders: number;
  dispatching_orders: number;
  reject_count_today: number;
  cancel_replace_count_today: number;
  avg_ack_latency_ms: number | null;
  stuck_orders: number;
}

export interface ExecutionOrderRow {
  internal_order_id: string;
  broker_order_id: string | null;
  symbol: string;
  /** null — OMS runtime has no per-order strategy attribution. */
  strategy_id?: string;
  /** null — per-order side is not tracked in the OMS snapshot. */
  side?: "buy" | "sell";
  /** null — order type is not captured at OMS snapshot level. */
  order_type?: "market" | "limit" | "stop" | "stop_limit";
  requested_qty: number;
  filled_qty: number;
  current_status: string;
  current_stage: string;
  /** null — per-order creation time is not in the OMS snapshot. */
  age_ms?: number;
  has_warning: boolean;
  has_critical: boolean;
  updated_at: string;
}

export type TimelineStageStatus = "not_reached" | "active" | "completed" | "failed" | "late" | "missing" | "skipped";

export interface TimelineStage {
  stage_key: string;
  stage_label: string;
  sequence: number;
  status: TimelineStageStatus;
  started_at: string | null;
  completed_at: string | null;
  duration_ms: number | null;
  age_ms: number | null;
  expected: boolean;
  present: boolean;
  source_system: string;
  source_ref: string | null;
  details: string;
  severity: Severity;
  is_current: boolean;
  is_terminal_stage: boolean;
}

export interface TimelineIncident {
  incident_id: string;
  incident_type:
    | "delayed_ack"
    | "stuck_in_state"
    | "duplicate_broker_event"
    | "late_fill_after_restart"
    | "replace_attempt"
    | "cancel_attempt"
    | "cancel_reject"
    | "reconcile_mismatch"
    | "missing_expected_stage"
    | "runtime_restart_detected"
    | "broker_disconnect_during_lifecycle"
    | "risk_denial"
    | "integrity_halt"
    | "unknown_transition";
  severity: Severity;
  message: string;
  at: string;
}

export interface TimelineEventRow {
  event_id: string;
  timestamp: string;
  event_type: string;
  source_system: string;
  severity: Severity;
  order_id: string;
  broker_order_id: string | null;
  stage_key: string | null;
  message: string;
  payload_json: string;
  is_duplicate: boolean;
  is_replayed: boolean;
  is_operator_visible: boolean;
}

export interface ReconcileSummary {
  status: HealthState;
  last_run_at: string | null;
  mismatched_positions: number;
  mismatched_orders: number;
  mismatched_fills: number;
  unmatched_broker_events: number;
}

export interface ExecutionTimeline {
  timeline_id: string;
  environment: EnvironmentMode;
  account_id: string | null;
  symbol: string;
  strategy_id: string;
  internal_order_id: string;
  broker_order_id: string | null;
  parent_intent_id: string | null;
  side: "buy" | "sell";
  order_type: string;
  requested_qty: number;
  filled_qty: number;
  current_status: string;
  current_stage: string;
  opened_at: string;
  last_updated_at: string;
  is_terminal: boolean;
  has_warning: boolean;
  has_critical: boolean;
  stages: TimelineStage[];
  incident_events: TimelineIncident[];
  event_rows: TimelineEventRow[];
  reconcile_summary: ReconcileSummary;
}

export interface OmsStateNode {
  state: OmsState;
  active_count: number;
  warning_count: number;
  over_sla_count: number;
  avg_dwell_ms: number | null;
  p95_dwell_ms: number | null;
}

export interface OmsTransitionEdge {
  from_state: OmsState;
  to_state: OmsState;
  transition_count: number;
  median_latency_ms: number | null;
  anomaly_count: number;
}

export interface OmsOrderStateRow {
  internal_order_id: string;
  broker_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  requested_qty: number;
  filled_qty: number;
  oms_state: OmsState;
  execution_stage: string;
  entered_state_at: string;
  dwell_ms: number;
  sla_ms: number;
  is_stuck: boolean;
  severity: Severity;
}

export interface OmsOverview {
  total_active_orders: number;
  stuck_orders: number;
  missing_transition_orders: number;
  state_nodes: OmsStateNode[];
  transition_edges: OmsTransitionEdge[];
  orders: OmsOrderStateRow[];
}

export interface TraceCorrelationIds {
  internal_order_id: string;
  broker_order_id: string | null;
  parent_order_id: string | null;
  outbox_id: string | null;
  claim_token: string | null;
  dispatch_attempt_id: string | null;
  inbox_ids: string[];
  fill_ids: string[];
  reconcile_case_id: string | null;
  audit_chain_id: string | null;
}

export interface TraceEventRow {
  trace_event_id: string;
  timestamp: string;
  subsystem: "strategy" | "runtime" | "risk" | "db" | "execution" | "broker" | "reconcile" | "audit";
  event_type: string;
  before_state: string;
  after_state: string;
  latency_since_prev_ms: number | null;
  summary: string;
  payload_digest: string;
  anomaly_tags: string[];
}

export interface TraceBrokerMessageRow {
  message_id: string;
  timestamp: string;
  direction: "outbound" | "inbound";
  message_type: string;
  normalized_summary: string;
  raw_payload: string;
}

export interface TraceFillRow {
  fill_id: string;
  timestamp: string;
  qty: number;
  price: number;
  liquidity_flag: string | null;
  fee_estimate: number | null;
  fee_actual: number | null;
  cumulative_filled_qty: number;
  average_fill_price: number;
  slippage_bps: number | null;
}

export interface StateLadderRow {
  key: string;
  oms_state: string;
  execution_state: string;
  broker_state: string;
  reconcile_state: string;
  at: string;
}

export interface CausalityNode {
  node_key: string;
  node_type: "signal" | "intent" | "risk_gate" | "outbox" | "broker_event" | "oms_transition" | "portfolio_effect" | "reconcile_outcome";
  title: string;
  status: "ok" | "warning" | "critical" | "derived";
  subsystem: "strategy" | "runtime" | "risk" | "execution" | "broker" | "portfolio" | "reconcile" | "audit";
  linked_id: string | null;
  timestamp: string | null;
  elapsed_from_prev_ms: number | null;
  anomaly_tags: string[];
  summary: string;
}

export interface CausalityTimelineRow {
  row_id: string;
  timestamp: string;
  subsystem: "strategy" | "runtime" | "risk" | "execution" | "broker" | "oms" | "portfolio" | "reconcile" | "audit";
  event_type: string;
  correlation_id: string;
  before_state: string;
  after_state: string;
  latency_since_prev_ms: number | null;
  summary: string;
  anomaly_tags: string[];
}

export interface PortfolioEffectSummary {
  pre_position_qty: number;
  post_position_qty: number;
  net_position_delta: number;
  average_price_effect: number | null;
  realized_pnl_delta: number | null;
  unrealized_pnl_delta: number | null;
  cash_delta: number | null;
  buying_power_delta: number | null;
  exposure_delta: number | null;
  strategy_allocation_effect: string;
}

export interface ReconcileOutcomeSummary {
  reconcile_status: "matched_cleanly" | "corrected" | "unresolved" | "missing_fill_detected" | "unknown_broker_order_detected";
  drift_detected: boolean;
  drift_type: string | null;
  correction_applied: boolean;
  correction_timestamp: string | null;
  corrected_fields: string[];
  remaining_discrepancy: string | null;
  operator_escalation_status: string;
}

export interface CausalityTrace {
  incident_id: string;
  internal_order_id: string;
  broker_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  target_qty: number;
  filled_qty: number;
  current_oms_state: string;
  current_execution_state: string;
  current_reconcile_status: string;
  terminal_outcome: string;
  replay_available: boolean;
  audit_chain_id: string | null;
  severity: Severity;
  correlation: {
    signal_id: string | null;
    decision_id: string | null;
    intent_id: string | null;
    internal_order_id: string;
    broker_order_id: string | null;
    outbox_id: string | null;
    claim_token: string | null;
    dispatch_attempt_id: string | null;
    inbox_ids: string[];
    fill_ids: string[];
    portfolio_event_ids: string[];
    reconcile_case_id: string | null;
    audit_chain_id: string | null;
    run_id: string | null;
  };
  nodes: CausalityNode[];
  timeline: CausalityTimelineRow[];
  portfolio_effects: PortfolioEffectSummary;
  reconcile_outcome: ReconcileOutcomeSummary;
  anomalies: string[];
}

export interface ExecutionTrace {
  internal_order_id: string;
  broker_order_id: string | null;
  parent_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  qty: number;
  current_execution_state: string;
  current_oms_state: string;
  submit_time: string;
  terminal_time: string | null;
  replay_available: boolean;
  correlation: TraceCorrelationIds;
  timeline: TraceEventRow[];
  broker_messages: TraceBrokerMessageRow[];
  state_ladder: StateLadderRow[];
  fills: TraceFillRow[];
}

export interface ReplayFrame {
  frame_id: string;
  timestamp: string;
  subsystem: string;
  event_type: string;
  state_delta: string;
  message_digest: string;
  order_execution_state: string;
  oms_state: string;
  filled_qty: number;
  open_qty: number;
  risk_state: string;
  reconcile_state: string;
  queue_status: string;
  anomaly_tags: string[];
  boundary_tags: string[];
}

export interface ExecutionReplay {
  replay_id: string;
  replay_scope: "single_order" | "incident" | "time_window";
  source: "audit_log" | "execution_events" | "reconcile_case" | "full_correlated";
  title: string;
  current_frame_index: number;
  frames: ReplayFrame[];
}

export interface ExecutionChartBar {
  ts: string;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

export type ExecutionOverlayKind =
  | "signal"
  | "intent"
  | "risk_pass"
  | "order_sent"
  | "broker_ack"
  | "partial_fill"
  | "fill"
  | "replace"
  | "cancel"
  | "reconcile"
  | "portfolio"
  | "expected_price";

export interface ExecutionOverlayEvent {
  overlay_id: string;
  kind: ExecutionOverlayKind;
  ts: string;
  price: number;
  label: string;
  detail: string;
  severity: Severity;
  linked_frame_id: string | null;
}

export interface ExecutionChartModel {
  order_id: string;
  symbol: string;
  timeframe: "1m" | "5m" | "15m" | "1h";
  bars: ExecutionChartBar[];
  overlays: ExecutionOverlayEvent[];
  reference_price: number | null;
}

// ---------------------------------------------------------------------------
// GUI-OPS-02: Execution outbox surface (durable intent timeline)
// ---------------------------------------------------------------------------

/** One row from the durable execution outbox for a run. */
export interface ExecutionOutboxRow {
  idempotency_key: string;
  run_id: string;
  /** Durable status: "PENDING" | "CLAIMED" | "DISPATCHING" | "SENT" | "ACKED" | "FAILED" | "AMBIGUOUS" */
  status: string;
  /** Display-friendly lifecycle stage derived from status. */
  lifecycle_stage: string;
  symbol: string | null;
  side: string | null;
  qty: number | null;
  order_type: string | null;
  strategy_id: string | null;
  /** "external_signal_ingestion" for strategy-driven; null for manual. */
  signal_source: string | null;
  created_at_utc: string;
  claimed_at_utc: string | null;
  dispatching_at_utc: string | null;
  sent_at_utc: string | null;
}

export type ExecutionOutboxTruthState = "active" | "no_active_run" | "no_db" | "unavailable";

/** Wrapper carrying the outbox truth state alongside rows. */
export interface ExecutionOutboxSurface {
  truth_state: ExecutionOutboxTruthState;
  run_id: string | null;
  rows: ExecutionOutboxRow[];
}

// ---------------------------------------------------------------------------
// GUI-OPS-02: Fill quality telemetry surface (TV-EXEC-01)
// ---------------------------------------------------------------------------

/** One fill quality telemetry row. Prices are in integer micros (divide by 1_000_000 for dollars). */
export interface FillQualityRow {
  telemetry_id: string;
  run_id: string;
  internal_order_id: string;
  broker_order_id: string | null;
  symbol: string;
  side: string;
  ordered_qty: number;
  fill_qty: number;
  fill_price_micros: number;
  reference_price_micros: number | null;
  /** Slippage in basis points. Null when reference price is absent (market orders). */
  slippage_bps: number | null;
  fill_kind: string;
  fill_received_at_utc: string;
  submit_to_fill_ms: number | null;
}

export type FillQualityTruthState = "active" | "no_active_run" | "no_db" | "unavailable";

/** Wrapper carrying fill quality truth state alongside rows. */
export interface FillQualitySurface {
  truth_state: FillQualityTruthState;
  rows: FillQualityRow[];
}

// ---------------------------------------------------------------------------
// A5A: per-order execution timeline (GET /api/v1/execution/orders/:id/timeline)
// ---------------------------------------------------------------------------

/** One fill event in the per-order execution timeline. */
export interface OrderTimelineRow {
  event_id: string;
  ts_utc: string;
  /** "partial_fill" | "final_fill" */
  stage: string;
  /** "fill_quality_telemetry" */
  source: string;
  detail: string | null;
  fill_qty: number | null;
  fill_price_micros: number | null;
  slippage_bps: number | null;
  provenance_ref: string | null;
}

export type OrderTimelineTruthState = "active" | "no_fills_yet" | "no_order" | "no_db";

/**
 * Per-order execution timeline surface backed by `postgres.fill_quality_telemetry`.
 *
 * truth_state semantics:
 * - "active"       — at least one fill row found; `rows` is authoritative.
 * - "no_fills_yet" — order visible in OMS snapshot but no fills yet; `rows` empty.
 * - "no_order"     — order_id unknown to any current source; `rows` empty.
 * - "no_db"        — no DB pool; `rows` empty and not authoritative.
 *
 * Fields sourced from the in-memory execution snapshot (`symbol`, `requested_qty`,
 * `filled_qty`, `current_status`, `current_stage`) are nullable because the snapshot
 * is ephemeral and absent across daemon restarts.
 *
 * Honest limit: only fill events are represented. Pre-fill outbox lifecycle events
 * (queued/claimed/dispatching/sent) are not yet linked to internal_order_id.
 */
export interface OrderTimelineSurface {
  canonical_route: string;
  truth_state: OrderTimelineTruthState;
  backend: string;
  internal_order_id: string;
  broker_order_id: string | null;
  /** null when execution snapshot is absent. */
  symbol: string | null;
  /** Always null — OMS has no per-order strategy attribution. */
  strategy_id: string | null;
  /** null when execution snapshot is absent. */
  requested_qty: number | null;
  /** null when execution snapshot is absent. */
  filled_qty: number | null;
  /** null when execution snapshot is absent. */
  current_status: string | null;
  /** Derived from current_status by the daemon. null when current_status is absent. */
  current_stage: string | null;
  /** RFC 3339 timestamp of the most recent fill event. null when no fills yet. */
  last_updated_at: string | null;
  rows: OrderTimelineRow[];
}
