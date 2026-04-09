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
