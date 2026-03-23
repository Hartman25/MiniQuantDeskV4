// core-rs/mqk-gui/src/features/system/types/strategy.ts
//
// Strategy fleet, suppression, and config diff types.

import type { HealthState } from "./core";

export interface StrategyRow {
  strategy_id: string;
  enabled: boolean;
  armed: boolean;
  health: HealthState;
  universe: string;
  pending_intents: number;
  open_positions: number;
  today_pnl: number | null;
  drawdown_pct: number | null;
  regime: string | null;
  throttle_state: string;
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
