// core-rs/mqk-gui/src/features/system/types/strategy.ts
//
// Strategy fleet, suppression, and config diff types.

import type { HealthState } from "./core";

export interface StrategyRow {
  strategy_id: string;
  enabled: boolean;
  armed: boolean;
  /** B2B: "runnable" | "blocked_disabled" | "blocked_not_registered" | "not_configured" | "no_fleet_configured" */
  admission_state: string;
  health: HealthState;
  universe: string;
  pending_intents: number;
  open_positions: number;
  today_pnl: number | null;
  drawdown_pct: number | null;
  regime: string | null;
  /** B3: "open" | "day_limit_reached" | null (null = not wired for this strategy) */
  throttle_state: string | null;
  last_decision_time: string | null;
}

export interface StrategySuppressionRow {
  suppression_id: string;
  strategy_id: string;
  state: "active" | "cleared";
  trigger_domain: "risk" | "market_data" | "runtime" | "reconcile" | "operator";
  trigger_reason: string;
  started_at: string;
  cleared_at: string | null;
  note: string;
}

export interface ConfigDiffRow {
  diff_id: string;
  changed_at: string;
  changed_domain: "config" | "risk" | "strategy_bundle" | "runtime";
  before_version: string;
  after_version: string;
  summary: string;
}

/**
 * STRATEGY-DECISION-OBSERVABILITY-01: Read-only diagnostic snapshot from the
 * most recent native strategy bar dispatch.
 *
 * Populated only for paper+alpaca when at least one bar has been dispatched.
 * Source: GET /api/v1/autonomous/readiness → strategy_decision_diagnostics.
 *
 * Read-only. Never used to place an order or mutate state.
 */
export interface StrategyDecisionDiagnostics {
  strategy_id: string;
  symbol: string;
  timeframe: string;
  lookback_bars: number;
  threshold_bps: number;
  latest_bar_ts: number | null;
  latest_close_micros: number | null;
  lookback_bar_ts: number | null;
  lookback_close_micros: number | null;
  /** Signed displacement: (latest_close - lookback_close) * 10_000 / lookback_close */
  move_bps: number | null;
  abs_move_bps: number | null;
  /** threshold_bps - abs_move_bps. Positive = still below threshold. */
  gap_to_threshold_bps: number | null;
  /** +1 bullish, 0 neutral, -1 bearish */
  raw_direction: number;
  /** "signal_long" | "flat_due_to_negative_direction" | "flat_below_threshold" | "insufficient_bars" */
  decision: string;
  reason: string;
}

/**
 * MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01: Read-only per-symbol dispatch row
 * surfaced by GET /api/v1/strategy/multi-symbol-dispatch-summary.
 *
 * Mirrors PerSymbolDispatchRow in core-rs/crates/mqk-daemon/src/api_types.rs.
 * Read-only. Never used to place, cancel, or replace an order.
 */
export interface PerSymbolDispatchRow {
  symbol: string;
  strategy_id: string;
  current_qty: number;
  target_qty: number;
  delta: number;
  no_order_reason: string;
  last_decision_id: string | null;
  last_decision_disposition: string | null;
  day_order_count: number;
  day_order_limit: number | null;
  bar_staleness_secs: number | null;
}

/**
 * "active" and "no_snapshot" are returned by the daemon route itself.
 * "unavailable" is a GUI-only sentinel used when the route cannot be
 * reached at all (fail-soft).
 */
export type MultiSymbolDispatchSummaryTruthState =
  | "active"
  | "no_snapshot"
  | "unavailable";

/**
 * MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01: Read-only multi-symbol dispatch
 * summary surface for the Strategy screen.
 *
 * Source: GET /api/v1/strategy/multi-symbol-dispatch-summary
 * (MultiSymbolDispatchSummaryResponse, Patch 9, already CLOSED).
 */
export interface MultiSymbolDispatchSummarySurface {
  truth_state: MultiSymbolDispatchSummaryTruthState;
  backend: string;
  runtime_execution_mode: string;
  configured_symbol_count: number;
  per_symbol: PerSymbolDispatchRow[];
}
