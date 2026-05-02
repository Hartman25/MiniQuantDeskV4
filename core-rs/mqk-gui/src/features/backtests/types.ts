export interface BacktestManifest {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  engine_id: string;
  mode: string;
  git_hash?: string | null;
  config_hash?: string | null;
  host_fingerprint?: string | null;
  created_at_utc: string;
  artifacts?: Record<string, string>;
}

export interface BacktestMetrics {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  halted: boolean;
  halt_reason: string | null;
  execution_blocked: boolean;
  bars: number;
  orders: number;
  orders_filled: number;
  orders_rejected: number;
  fills: number;
  final_equity_micros: number;
  symbols: string[];
  starting_equity_micros: number;
  ending_equity_micros: number;
  total_return_micros: number;
  total_return_pct: number;
  equity_high_water_mark_micros: number;
  max_drawdown_micros: number;
  max_drawdown_pct: number;
  total_commission_micros: number;
  trade_count: number;
  winning_trade_count: number;
  losing_trade_count: number;
  flat_trade_count: number;
  win_rate_pct: number | null;
  gross_profit_micros: number;
  gross_loss_micros: number;
  profit_factor: number | null;
  average_win_micros: number | null;
  average_loss_micros: number | null;
  expectancy_micros: number | null;
  best_trade_micros: number | null;
  worst_trade_micros: number | null;
  sharpe_ratio: number | null;
  sortino_ratio: number | null;
  exposure_bars: number;
  exposure_time_pct: number;
}

export interface EquityCurveRow {
  ts_utc: string;
  equity: number;
}

export interface OrderRow {
  ts_utc: string;
  order_id: string;
  symbol: string;
  side: string;
  qty: string;
  order_type: string;
  limit_price: string;
  stop_price: string;
  status: string;
}

export interface FillRow {
  ts_utc: string;
  fill_id: string;
  order_id: string;
  symbol: string;
  side: string;
  qty: string;
  price: string;
  fee: string;
}

export type FileResult<T> =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ok"; data: T }
  | { kind: "missing" }
  | { kind: "parse_error"; message: string }
  | { kind: "read_error"; message: string };

export interface ParsedCsvResult<T> {
  rows: T[];
  malformed: number;
}

export interface ArtifactBundle {
  manifest: FileResult<BacktestManifest>;
  metrics: FileResult<BacktestMetrics>;
  equityCurve: FileResult<ParsedCsvResult<EquityCurveRow>>;
  orders: FileResult<ParsedCsvResult<OrderRow>>;
  fills: FileResult<ParsedCsvResult<FillRow>>;
}
