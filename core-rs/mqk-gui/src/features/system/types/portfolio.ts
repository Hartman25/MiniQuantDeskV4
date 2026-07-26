// core-rs/mqk-gui/src/features/system/types/portfolio.ts
//
// Portfolio, position, fill, risk, and reconcile row types.

import type { Severity } from "./core";

export interface PositionRow {
  symbol: string;
  /** null — broker-snapshot positions have no strategy attribution. */
  strategy_id?: string;
  qty: number;
  avg_price: number;
  /** null — mark prices are not present in the broker snapshot. */
  mark_price?: number;
  /** null — broker snapshot has no unrealized PnL. */
  unrealized_pnl?: number;
  /** null — broker snapshot has no today-only realized PnL. */
  realized_pnl_today?: number;
  broker_qty: number;
  /** null — reconcile-level drift is not assessed at broker snapshot layer. */
  drift?: boolean;
}

export interface OpenOrderRow {
  internal_order_id: string;
  symbol: string;
  /** null — broker snapshot has no strategy attribution. */
  strategy_id?: string;
  side: string;
  status: string;
  broker_order_id: string | null;
  requested_qty: number;
  /** null — partial fill quantity is not tracked in the broker snapshot. */
  filled_qty?: number;
  entered_at: string;
}

export interface FillRow {
  fill_id: string;
  internal_order_id: string;
  symbol: string;
  /** null — broker snapshot has no strategy attribution. */
  strategy_id?: string;
  side: string;
  qty: number;
  price: number;
  broker_exec_id: string;
  applied: boolean;
  at: string;
}

export interface PortfolioSummary {
  account_equity: number;
  cash: number;
  long_market_value: number;
  short_market_value: number;
  daily_pnl: number;
  buying_power: number;
}

export interface RiskSummary {
  gross_exposure: number;
  net_exposure: number;
  concentration_pct: number;
  daily_pnl: number;
  drawdown_pct: number;
  loss_limit_utilization_pct: number;
  kill_switch_active: boolean;
  active_breaches: number;
}

export interface RiskDenialRow {
  id: string;
  at: string;
  /** Always null — strategy attribution is not available on the risk gate path. */
  strategy_id: string | null;
  symbol: string;
  rule: string;
  message: string;
  severity: Severity;
}

export interface ReconcileMismatchRow {
  id: string;
  domain: "position" | "order" | "fill" | "cash" | "event";
  symbol: string;
  internal_value: string;
  broker_value: string;
  status: Severity;
  note: string;
}

// ---------------------------------------------------------------------------
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01F: restart-surviving durable portfolio
// truth (GET /api/v1/portfolio/durable-summary et al). Distinct from
// PortfolioSummary above, which is broker-snapshot-derived and reset on
// every daemon restart. `null` here always means unavailable/unproven —
// never zero; a true zero is the literal number `0`.
// ---------------------------------------------------------------------------

export interface DurablePortfolioSummary {
  truth_state: string;
  snapshot_truth_state: string;
  snapshot_id: string | null;
  captured_at_utc: string | null;
  source: string | null;
  deployment_mode: string | null;
  account_equity: number | null;
  cash: number | null;
  currency: string | null;
  run_id: string | null;
  operation_id: string | null;
  accounting_truth_state: string;
  accounting_epoch: string | null;
  accounting_epoch_reason: string | null;
  accounting_source_snapshot_id: string | null;
  last_applied_inbox_id: number | null;
  realized_pnl: number | null;
  realized_pnl_truth_state: string;
  realized_pnl_unavailable_reason: string | null;
  fees: number | null;
  cumulative_cash_movement: number | null;
  unrealized_pnl: number | null;
  unrealized_pnl_truth_state: string;
  unrealized_pnl_unavailable_reason: string | null;
  daily_pnl: number | null;
  daily_pnl_truth_state: string;
  daily_pnl_unavailable_reason: string | null;
  blockers: string[];
}

export interface DurablePortfolioPositionRow {
  symbol: string;
  qty_signed: number;
  avg_entry_price: number;
  provenance: string;
}

export interface DurablePortfolioPositionsResponse {
  truth_state: string;
  snapshot_id: string | null;
  captured_at_utc: string | null;
  run_id: string | null;
  positions: DurablePortfolioPositionRow[];
}

export interface DurablePortfolioSnapshotRow {
  snapshot_id: string;
  captured_at_utc: string;
  deployment_mode: string;
  source: string;
  equity: number;
  cash: number;
  currency: string;
  truth_state: string;
  run_id: string | null;
  operation_id: string | null;
}

export interface DurablePortfolioSnapshotsResponse {
  truth_state: string;
  snapshots: DurablePortfolioSnapshotRow[];
}
