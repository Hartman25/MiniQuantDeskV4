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
