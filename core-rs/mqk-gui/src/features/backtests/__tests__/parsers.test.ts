import test from "node:test";
import assert from "node:assert/strict";
import {
  deriveStrategyFitGateFlags,
  formatMicrosAsDollars,
  formatNullableNumber,
  formatNullablePercent,
  microsToUsd,
  parseCsvRows,
  parseEquityCurve,
  parseFills,
  parseManifest,
  parseMetrics,
  parseOrders,
  parsePaperReadiness,
  parsePremarketRevalidation,
  parseStrategyFit,
  parseWatchlistPromotion,
} from "../parsers.ts";

// --- microsToUsd ---

test("microsToUsd converts 1_000_000 to 1.0", () => {
  assert.equal(microsToUsd(1_000_000), 1.0);
});

test("microsToUsd converts 100_000_000_000 to 100000", () => {
  assert.equal(microsToUsd(100_000_000_000), 100_000);
});

test("microsToUsd handles zero", () => {
  assert.equal(microsToUsd(0), 0);
});

test("microsToUsd handles negative", () => {
  assert.equal(microsToUsd(-500_000), -0.5);
});

// --- formatMicrosAsDollars ---

test("formatMicrosAsDollars formats positive micros as USD", () => {
  const result = formatMicrosAsDollars(1_000_000);
  assert.ok(result.includes("1"), `expected dollar amount in '${result}'`);
});

test("formatMicrosAsDollars returns em-dash for null", () => {
  assert.equal(formatMicrosAsDollars(null), "—");
});

test("formatMicrosAsDollars returns em-dash for undefined", () => {
  assert.equal(formatMicrosAsDollars(undefined), "—");
});

// --- formatNullableNumber ---

test("formatNullableNumber returns em-dash for null", () => {
  assert.equal(formatNullableNumber(null), "—");
});

test("formatNullableNumber formats a number with default 4 digits", () => {
  const result = formatNullableNumber(1.5);
  assert.equal(result, "1.5000");
});

test("formatNullableNumber respects digits param", () => {
  assert.equal(formatNullableNumber(2.5, 2), "2.50");
});

// --- formatNullablePercent ---

test("formatNullablePercent returns em-dash for null", () => {
  assert.equal(formatNullablePercent(null), "—");
});

test("formatNullablePercent formats percent with 2 decimals", () => {
  assert.equal(formatNullablePercent(12.3), "12.30%");
});

test("formatNullablePercent formats zero", () => {
  assert.equal(formatNullablePercent(0), "0.00%");
});

// --- parseCsvRows ---

test("parseCsvRows parses headers and rows", () => {
  const csv = "a,b,c\n1,2,3\n4,5,6";
  const { headers, rows } = parseCsvRows(csv);
  assert.deepEqual(headers, ["a", "b", "c"]);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].a, "1");
  assert.equal(rows[1].c, "6");
});

test("parseCsvRows ignores blank trailing lines", () => {
  const csv = "a,b\n1,2\n\n\n";
  const { rows } = parseCsvRows(csv);
  assert.equal(rows.length, 1);
});

test("parseCsvRows handles Windows CRLF line endings", () => {
  const csv = "a,b\r\n1,2\r\n3,4\r\n";
  const { rows } = parseCsvRows(csv);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].a, "1");
});

test("parseCsvRows returns empty for empty string", () => {
  const { headers, rows } = parseCsvRows("");
  assert.deepEqual(headers, []);
  assert.equal(rows.length, 0);
});

test("parseCsvRows returns empty for whitespace-only string", () => {
  const { rows } = parseCsvRows("   \n  \n");
  assert.equal(rows.length, 0);
});

test("parseCsvRows fills missing cells with empty string", () => {
  const csv = "a,b,c\n1,2";
  const { rows } = parseCsvRows(csv);
  assert.equal(rows[0].c, "");
});

// --- parseEquityCurve ---

test("parseEquityCurve parses valid equity curve", () => {
  const csv = "ts_utc,equity\n60,100000000000\n120,100500000000\n180,101000000000";
  const { rows, malformed } = parseEquityCurve(csv);
  assert.equal(malformed, 0);
  assert.equal(rows.length, 3);
  assert.equal(rows[0].ts_utc, "60");
  assert.equal(rows[0].equity, 100_000_000_000);
  assert.equal(rows[2].equity, 101_000_000_000);
});

test("parseEquityCurve counts malformed rows where equity is not a number", () => {
  const csv = "ts_utc,equity\n60,bad_value\n120,100500000000";
  const { rows, malformed } = parseEquityCurve(csv);
  assert.equal(malformed, 1);
  assert.equal(rows.length, 1);
});

test("parseEquityCurve returns empty for header-only CSV", () => {
  const csv = "ts_utc,equity";
  const { rows, malformed } = parseEquityCurve(csv);
  assert.equal(rows.length, 0);
  assert.equal(malformed, 0);
});

test("parseEquityCurve returns empty for completely empty CSV", () => {
  const { rows } = parseEquityCurve("");
  assert.equal(rows.length, 0);
});

// --- parseOrders ---

test("parseOrders parses valid orders", () => {
  const csv =
    "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n" +
    "2026-01-01T09:30:00Z,ord-001,AAPL,buy,100,market,,, filled\n" +
    "2026-01-01T09:31:00Z,ord-002,AAPL,sell,50,limit,150.00,,canceled";
  const { rows, malformed } = parseOrders(csv);
  assert.equal(malformed, 0);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].order_id, "ord-001");
  assert.equal(rows[0].symbol, "AAPL");
  assert.equal(rows[1].side, "sell");
});

test("parseOrders skips rows with missing order_id", () => {
  const csv =
    "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n" +
    ",,,buy,100,market,,,filled\n" +
    "2026-01-01T09:30:00Z,ord-001,AAPL,buy,100,market,,,filled";
  const { rows, malformed } = parseOrders(csv);
  assert.equal(malformed, 1);
  assert.equal(rows.length, 1);
});

test("parseOrders returns empty for no data rows", () => {
  const csv = "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status";
  const { rows, malformed } = parseOrders(csv);
  assert.equal(rows.length, 0);
  assert.equal(malformed, 0);
});

// --- parseFills ---

test("parseFills parses valid fills", () => {
  const csv =
    "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n" +
    "2026-01-01T09:30:01Z,fill-001,ord-001,AAPL,buy,100,150000000,100000";
  const { rows, malformed } = parseFills(csv);
  assert.equal(malformed, 0);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].fill_id, "fill-001");
  assert.equal(rows[0].order_id, "ord-001");
  assert.equal(rows[0].price, "150000000");
});

test("parseFills skips rows with missing fill_id", () => {
  const csv =
    "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n" +
    "2026-01-01T09:30:01Z,,ord-001,AAPL,buy,100,150000000,100000";
  const { rows, malformed } = parseFills(csv);
  assert.equal(malformed, 1);
  assert.equal(rows.length, 0);
});

test("parseFills returns empty for no data rows", () => {
  const csv = "ts_utc,fill_id,order_id,symbol,side,qty,price,fee";
  const { rows } = parseFills(csv);
  assert.equal(rows.length, 0);
});

// --- parseManifest ---

test("parseManifest parses a valid manifest", () => {
  const json = JSON.stringify({
    schema_version: 1,
    run_id: "abc-123",
    strategy_name: "swing_momentum",
    engine_id: "mqk-backtest",
    mode: "backtest",
    created_at_utc: "2026-05-01T12:00:00Z",
  });
  const manifest = parseManifest(json);
  assert.equal(manifest.run_id, "abc-123");
  assert.equal(manifest.strategy_name, "swing_momentum");
});

test("parseManifest throws on missing run_id", () => {
  const json = JSON.stringify({ schema_version: 1, strategy_name: "x", created_at_utc: "t" });
  assert.throws(() => parseManifest(json), /missing run_id/);
});

test("parseManifest throws on missing created_at_utc", () => {
  const json = JSON.stringify({ schema_version: 1, run_id: "x", strategy_name: "x" });
  assert.throws(() => parseManifest(json), /missing created_at_utc/);
});

test("parseManifest throws on non-object JSON", () => {
  assert.throws(() => parseManifest("[1,2,3]"), /expected a JSON object/);
});

test("parseManifest throws on invalid JSON", () => {
  assert.throws(() => parseManifest("{bad json}"));
});

// --- parseMetrics ---

test("parseMetrics parses a valid metrics object", () => {
  const json = JSON.stringify({
    schema_version: 1,
    run_id: "abc-123",
    strategy_name: "swing_momentum",
    halted: false,
    halt_reason: null,
    execution_blocked: false,
    bars: 100,
    orders: 10,
    orders_filled: 8,
    orders_rejected: 2,
    fills: 8,
    final_equity_micros: 100_500_000_000,
    symbols: ["AAPL"],
    starting_equity_micros: 100_000_000_000,
    ending_equity_micros: 100_500_000_000,
    total_return_micros: 500_000_000,
    total_return_pct: 0.5,
    equity_high_water_mark_micros: 101_000_000_000,
    max_drawdown_micros: 200_000_000,
    max_drawdown_pct: 0.2,
    total_commission_micros: 8_000_000,
    trade_count: 4,
    winning_trade_count: 3,
    losing_trade_count: 1,
    flat_trade_count: 0,
    win_rate_pct: 75.0,
    gross_profit_micros: 700_000_000,
    gross_loss_micros: 200_000_000,
    profit_factor: 3.5,
    average_win_micros: 233_333_333,
    average_loss_micros: 200_000_000,
    expectancy_micros: 125_000_000,
    best_trade_micros: 400_000_000,
    worst_trade_micros: -200_000_000,
    sharpe_ratio: 1.8,
    sortino_ratio: 2.1,
    exposure_bars: 50,
    exposure_time_pct: 50.0,
  });
  const m = parseMetrics(json);
  assert.equal(m.run_id, "abc-123");
  assert.equal(m.bars, 100);
  assert.equal(m.win_rate_pct, 75.0);
  assert.equal(m.sharpe_ratio, 1.8);
});

test("parseMetrics accepts null nullable fields", () => {
  const json = JSON.stringify({
    schema_version: 1,
    run_id: "r1",
    strategy_name: "s",
    halted: false,
    halt_reason: null,
    execution_blocked: false,
    bars: 3,
    orders: 0,
    orders_filled: 0,
    orders_rejected: 0,
    fills: 0,
    final_equity_micros: 100_000_000_000,
    symbols: [],
    starting_equity_micros: 100_000_000_000,
    ending_equity_micros: 100_000_000_000,
    total_return_micros: 0,
    total_return_pct: 0.0,
    equity_high_water_mark_micros: 100_000_000_000,
    max_drawdown_micros: 0,
    max_drawdown_pct: 0.0,
    total_commission_micros: 0,
    trade_count: 0,
    winning_trade_count: 0,
    losing_trade_count: 0,
    flat_trade_count: 0,
    win_rate_pct: null,
    gross_profit_micros: 0,
    gross_loss_micros: 0,
    profit_factor: null,
    average_win_micros: null,
    average_loss_micros: null,
    expectancy_micros: null,
    best_trade_micros: null,
    worst_trade_micros: null,
    sharpe_ratio: null,
    sortino_ratio: null,
    exposure_bars: 0,
    exposure_time_pct: 0.0,
  });
  const m = parseMetrics(json);
  assert.equal(m.win_rate_pct, null);
  assert.equal(m.sharpe_ratio, null);
  assert.equal(m.profit_factor, null);
});

test("parseMetrics throws on missing run_id", () => {
  const json = JSON.stringify({ schema_version: 1, bars: 0 });
  assert.throws(() => parseMetrics(json), /missing run_id/);
});

test("parseMetrics throws on missing bars", () => {
  const json = JSON.stringify({ schema_version: 1, run_id: "x" });
  assert.throws(() => parseMetrics(json), /missing bars/);
});

// --- deriveStrategyFitGateFlags ---

test("deriveStrategyFitGateFlags: known failure reasons map to true", () => {
  const flags = deriveStrategyFitGateFlags([
    "validation_metrics_missing",
    "profit_factor_failed",
    "expectancy_failed",
    "cost_adjusted_edge_failed",
    "out_of_sample_failed",
    "sample_quality_failed",
    "parameter_stability_failed",
  ]);
  assert.equal(flags.profit_factor_failed, true);
  assert.equal(flags.expectancy_failed, true);
  assert.equal(flags.cost_adjusted_edge_failed, true);
  assert.equal(flags.out_of_sample_failed, true);
  assert.equal(flags.sample_quality_failed, true);
  assert.equal(flags.parameter_stability_failed, true);
  assert.equal(flags.validation_metrics_missing, true);
});

test("deriveStrategyFitGateFlags: empty failure reasons map to all-false", () => {
  const flags = deriveStrategyFitGateFlags([]);
  assert.equal(flags.profit_factor_failed, false);
  assert.equal(flags.expectancy_failed, false);
  assert.equal(flags.cost_adjusted_edge_failed, false);
  assert.equal(flags.out_of_sample_failed, false);
  assert.equal(flags.sample_quality_failed, false);
  assert.equal(flags.parameter_stability_failed, false);
  assert.equal(flags.validation_metrics_missing, false);
});

test("deriveStrategyFitGateFlags: unrelated failure reasons do not set known flags", () => {
  const flags = deriveStrategyFitGateFlags(["min_bars_failed", "min_trades_failed"]);
  assert.equal(flags.profit_factor_failed, false);
  assert.equal(flags.expectancy_failed, false);
  assert.equal(flags.validation_metrics_missing, false);
});

// --- parseStrategyFit ---

test("parseStrategyFit: valid rejected artifact (real AAPL strategy-fit-v1 shape)", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    generated_at_utc: "2026-06-10T02:59:52+00:00",
    source_queue_id: "bq-11eb5b628bb08a4e5825d4fb6be1f98c",
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
    timeframe: "5m",
    regime_label: "trending_up",
    bars_used: 5342,
    trades: 380,
    win_rate: 0.06315789473684211,
    profit_factor: 0.04966283909815413,
    expectancy_bps: -41.6610626075103,
    max_drawdown_bps: 48.78588379157287,
    net_expectancy_after_cost_bps: -41.98562201811041,
    recommended_for_paper: false,
    recommended_for_live: false,
    failure_reasons: [
      "validation_metrics_missing",
      "profit_factor_failed",
      "expectancy_failed",
      "cost_adjusted_edge_failed",
      "out_of_sample_failed",
      "sample_quality_failed",
      "parameter_stability_failed",
    ],
    status: "complete",
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.symbol, "AAPL");
  assert.equal(result.data.strategy_id, "intraday_scalper");
  assert.equal(result.data.timeframe, "5m");
  assert.equal(result.data.trades, 380);
  assert.equal(result.data.profit_factor, 0.04966283909815413);
  assert.equal(result.data.recommended_for_paper, false);
  assert.equal(result.data.recommended_for_live, false);
  assert.equal(result.data.recommended_for_live_present, true);
  assert.equal(result.data.failure_reasons.length, 7);
  assert.equal(result.data.gateFlags.profit_factor_failed, true);
  assert.equal(result.data.gateFlags.expectancy_failed, true);
  assert.equal(result.data.gateFlags.cost_adjusted_edge_failed, true);
  assert.equal(result.data.gateFlags.out_of_sample_failed, true);
  assert.equal(result.data.gateFlags.sample_quality_failed, true);
  assert.equal(result.data.gateFlags.parameter_stability_failed, true);
  assert.equal(result.data.gateFlags.validation_metrics_missing, true);
});

test("parseStrategyFit: valid passing/recommended artifact", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "MSFT",
    strategy_id: "swing_momentum",
    timeframe: "1d",
    trades: 45,
    profit_factor: 1.8,
    expectancy_bps: 12.5,
    net_expectancy_after_cost_bps: 8.2,
    recommended_for_paper: true,
    recommended_for_live: false,
    failure_reasons: [],
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.recommended_for_paper, true);
  assert.equal(result.data.recommended_for_live, false);
  assert.equal(result.data.recommended_for_live_present, true);
  assert.equal(result.data.failure_reasons.length, 0);
  assert.equal(result.data.gateFlags.profit_factor_failed, false);
});

test("parseStrategyFit: missing recommended_for_live stays safe (not invented as true)", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "TSLA",
    strategy_id: "mean_reversion",
    recommended_for_paper: false,
    failure_reasons: ["profit_factor_failed"],
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.recommended_for_live, false);
  assert.equal(result.data.recommended_for_live_present, false);
});

test("parseStrategyFit: explicit recommended_for_live true is passed through honestly (never suppressed)", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
    recommended_for_paper: true,
    recommended_for_live: true,
    failure_reasons: [],
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.recommended_for_live, true);
  assert.equal(result.data.recommended_for_live_present, true);
});

test("parseStrategyFit: missing failure_reasons handled safely (defaults to empty, all flags false)", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "NVDA",
    strategy_id: "trend_follow",
    recommended_for_paper: false,
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.failure_reasons, []);
  assert.equal(result.data.gateFlags.profit_factor_failed, false);
  assert.equal(result.data.gateFlags.validation_metrics_missing, false);
  assert.equal(result.data.recommended_for_live, false);
  assert.equal(result.data.recommended_for_live_present, false);
});

test("parseStrategyFit: unsupported schema_version is explicit", () => {
  const json = JSON.stringify({
    schema_version: "gate-v1",
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
    recommended_for_paper: false,
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, "gate-v1");
});

test("parseStrategyFit: missing schema_version is unsupported (not assumed)", () => {
  const json = JSON.stringify({
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
    recommended_for_paper: false,
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, null);
});

test("parseStrategyFit: malformed JSON is explicit", () => {
  const result = parseStrategyFit("{bad json}");
  assert.equal(result.kind, "malformed");
});

test("parseStrategyFit: non-object JSON is malformed", () => {
  const result = parseStrategyFit("[1,2,3]");
  assert.equal(result.kind, "malformed");
});

test("parseStrategyFit: missing symbol is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    strategy_id: "intraday_scalper",
    recommended_for_paper: false,
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "missing_fields");
});

test("parseStrategyFit: missing strategy_id is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "AAPL",
    recommended_for_paper: false,
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "missing_fields");
});

test("parseStrategyFit: missing recommended_for_paper is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "strategy-fit-v1",
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
  });
  const result = parseStrategyFit(json);
  assert.equal(result.kind, "missing_fields");
});

// --- parsePaperReadiness ---

test("parsePaperReadiness: valid blocked artifact (real AAPL paper-readiness-v1 shape)", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: [
      "paper_readiness_risk_simulation_failed",
      "paper_readiness_premarket_revalidation_failed",
      "strategy_fit_not_recommended_for_paper",
      "risk_simulation_required",
      "operator_review_required",
      "premarket_revalidation_required",
    ],
    top_symbol: "AAPL",
    symbol_inputs_status: "complete",
    risk_simulation_passed: false,
    premarket_revalidation_passed: false,
    promotion_passed: false,
    approved_for_autonomous_paper: false,
    approved_for_live: false,
    live_locked: true,
    daemon_enforcement_executed: false,
    paper_handoff_requested: false,
    paper_handoff_executed: false,
    upstream_pipeline_toggles: { scanner_pipeline_enabled: false, backtest_pipeline_enabled: false },
    artifacts_read: {
      watchlist_path: "C:\\repo\\watchlist.json",
      bars_root: "C:\\repo\\bars",
      strategy_fit_dir: "C:\\repo\\strategy_fit",
      liquidity_root: null,
    },
    artifacts_written: {
      symbol_inputs_output_path: "C:\\repo\\symbol_inputs.json",
      readiness_report_output_path: "C:\\repo\\readiness_report.json",
    },
    notes: "blocked: blocked: strategy_fit_not_recommended_for_paper; risk_simulation_required",
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.status, "blocked");
  assert.equal(result.data.reasons.length, 6);
  assert.equal(result.data.top_symbol, "AAPL");
  assert.equal(result.data.symbol_inputs_status, "complete");
  assert.equal(result.data.risk_simulation_passed, false);
  assert.equal(result.data.premarket_revalidation_passed, false);
  assert.equal(result.data.promotion_passed, false);
  assert.equal(result.data.approved_for_autonomous_paper, false);
  assert.equal(result.data.approved_for_live, false);
  assert.equal(result.data.live_locked, true);
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
  assert.equal(result.data.upstream_pipeline_toggles.scanner_pipeline_enabled, false);
  assert.equal(result.data.artifacts_read.liquidity_root, null);
  assert.equal(result.data.artifacts_written.readiness_report_output_path, "C:\\repo\\readiness_report.json");
});

test("parsePaperReadiness: valid ready_for_paper_handoff artifact (all gates pass)", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_paper_handoff",
    reasons: [],
    top_symbol: "MSFT",
    symbol_inputs_status: "complete",
    risk_simulation_passed: true,
    premarket_revalidation_passed: true,
    promotion_passed: true,
    approved_for_autonomous_paper: true,
    approved_for_live: false,
    live_locked: true,
    daemon_enforcement_executed: false,
    paper_handoff_requested: true,
    paper_handoff_executed: false,
    upstream_pipeline_toggles: { scanner_pipeline_enabled: true, backtest_pipeline_enabled: true },
    artifacts_read: {},
    artifacts_written: {},
    notes: "",
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.status, "ready_for_paper_handoff");
  assert.equal(result.data.approved_for_autonomous_paper, true);
  assert.equal(result.data.risk_simulation_passed, true);
  assert.equal(result.data.premarket_revalidation_passed, true);
  assert.equal(result.data.promotion_passed, true);
  assert.equal(result.data.paper_handoff_requested, true);
  assert.equal(result.data.paper_handoff_executed, false);
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parsePaperReadiness: missing approved_for_live stays safe (defaults false, no anomaly)", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: ["paper_readiness_watchlist_path_missing"],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.approved_for_live, false);
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parsePaperReadiness: explicit approved_for_live=true is passed through and flagged as anomaly", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_paper_handoff",
    reasons: [],
    approved_for_live: true,
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.approved_for_live, true);
  assert.ok(result.data.hardInvariantAnomalies.includes("approved_for_live_true"));
});

test("parsePaperReadiness: missing live_locked defaults to true (safe), no anomaly", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: [],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.live_locked, true);
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parsePaperReadiness: explicit live_locked=false is passed through and flagged as anomaly", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_paper_handoff",
    reasons: [],
    live_locked: false,
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.live_locked, false);
  assert.ok(result.data.hardInvariantAnomalies.includes("live_locked_false"));
});

test("parsePaperReadiness: explicit daemon_enforcement_executed=true is passed through and flagged as anomaly", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_paper_handoff",
    reasons: [],
    daemon_enforcement_executed: true,
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.daemon_enforcement_executed, true);
  assert.ok(result.data.hardInvariantAnomalies.includes("daemon_enforcement_executed_true"));
});

test("parsePaperReadiness: explicit paper_handoff_executed=true is passed through and flagged as anomaly", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_paper_handoff",
    reasons: [],
    paper_handoff_executed: true,
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.paper_handoff_executed, true);
  assert.ok(result.data.hardInvariantAnomalies.includes("paper_handoff_executed_true"));
});

test("parsePaperReadiness: unsupported schema_version is explicit", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v0",
    status: "blocked",
    reasons: [],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, "paper-readiness-v0");
});

test("parsePaperReadiness: missing schema_version is unsupported (not assumed)", () => {
  const json = JSON.stringify({
    status: "blocked",
    reasons: [],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, null);
});

test("parsePaperReadiness: malformed JSON is explicit", () => {
  const result = parsePaperReadiness("{bad json}");
  assert.equal(result.kind, "malformed");
});

test("parsePaperReadiness: non-object JSON is malformed", () => {
  const result = parsePaperReadiness("[1,2,3]");
  assert.equal(result.kind, "malformed");
});

test("parsePaperReadiness: missing status is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    reasons: [],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "missing_fields");
});

test("parsePaperReadiness: missing reasons is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "missing_fields");
});

test("parsePaperReadiness: empty reasons array on ready_for_operator_review parses ok", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "ready_for_operator_review",
    reasons: [],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.status, "ready_for_operator_review");
  assert.deepEqual(result.data.reasons, []);
});

test("parsePaperReadiness: mixed gate pass/fail/null parsed independently", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "partial",
    reasons: ["paper_readiness_symbol_inputs_partial"],
    risk_simulation_passed: true,
    premarket_revalidation_passed: false,
    // promotion_passed intentionally omitted
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.risk_simulation_passed, true);
  assert.equal(result.data.premarket_revalidation_passed, false);
  assert.equal(result.data.promotion_passed, null);
});

test("parsePaperReadiness: non-boolean toggle values and non-string/null artifact paths are filtered out", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: [],
    upstream_pipeline_toggles: { scanner_pipeline_enabled: true, weird: "not-a-bool" },
    artifacts_read: { watchlist_path: "C:\\repo\\watchlist.json", weird: 123 },
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.upstream_pipeline_toggles, { scanner_pipeline_enabled: true });
  assert.deepEqual(result.data.artifacts_read, { watchlist_path: "C:\\repo\\watchlist.json" });
});

test("parsePaperReadiness: non-string entries in reasons are filtered out", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: ["paper_readiness_watchlist_path_missing", 42, null, "operator_review_required"],
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.reasons, [
    "paper_readiness_watchlist_path_missing",
    "operator_review_required",
  ]);
});

test("parsePaperReadiness: null top_symbol and symbol_inputs_status handled", () => {
  const json = JSON.stringify({
    schema_version: "paper-readiness-v1",
    status: "blocked",
    reasons: ["paper_readiness_watchlist_path_missing"],
    top_symbol: null,
    symbol_inputs_status: null,
  });
  const result = parsePaperReadiness(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.top_symbol, null);
  assert.equal(result.data.symbol_inputs_status, null);
});

// --- parseWatchlistPromotion ---

test("parseWatchlistPromotion: valid approved artifact (real AAPL watchlist-v1 shape)", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    generated_at_utc: "2026-06-05T00:00:00+00:00",
    trade_date: "2026-06-08",
    mode: "paper",
    approved_for_autonomous_paper: true,
    approved_for_live: false,
    max_symbols_to_trade: 1,
    max_concurrent_positions: 1,
    symbols: ["AAPL"],
    strategy_assignments: { AAPL: "intraday_scalper" },
    paper_qty_limits: { AAPL: 1 },
    notional_limits: { AAPL: 1000.0 },
    ranked_candidates: [
      {
        rank: 1,
        symbol: "AAPL",
        strategy_id: "intraday_scalper",
        regime_label: "trending_up",
        regime_score: 0.82,
        net_expectancy_after_cost_bps: 7.0,
      },
    ],
    selection_reason: "ranked_candidate_artifact_chain_proof",
    promotion_decision: {
      passed: true,
      failure_reasons: [],
      approved_symbols: ["AAPL"],
      strategy_assignments: { AAPL: "intraday_scalper" },
      notes: "promotion_passed",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.mode, "paper");
  assert.equal(result.data.approved_for_autonomous_paper, true);
  assert.equal(result.data.approved_for_live, false);
  assert.deepEqual(result.data.symbols, ["AAPL"]);
  assert.deepEqual(result.data.strategy_assignments, { AAPL: "intraday_scalper" });
  assert.deepEqual(result.data.paper_qty_limits, { AAPL: 1 });
  assert.deepEqual(result.data.notional_limits, { AAPL: 1000.0 });
  assert.equal(result.data.ranked_candidates.length, 1);
  assert.equal(result.data.ranked_candidates[0].symbol, "AAPL");
  assert.equal(result.data.ranked_candidates[0].rank, 1);
  assert.equal(result.data.promotion_decision?.passed, true);
  assert.deepEqual(result.data.promotion_decision?.approved_symbols, ["AAPL"]);
  assert.equal(result.data.top_symbol, "AAPL");
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parseWatchlistPromotion: valid blocked artifact (no symbols approved, ranked_candidates still present)", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    mode: "paper",
    approved_for_autonomous_paper: false,
    approved_for_live: false,
    max_symbols_to_trade: 1,
    max_concurrent_positions: 1,
    symbols: [],
    strategy_assignments: {},
    ranked_candidates: [
      { rank: 1, symbol: "MSFT", strategy_id: "intraday_scalper", regime_label: "choppy", regime_score: 0.41 },
    ],
    selection_reason: "blocked_by_promotion_gate",
    promotion_decision: {
      passed: false,
      failure_reasons: ["strategy_fit_not_recommended_for_paper", "operator_review_required"],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "blocked: strategy_fit_not_recommended_for_paper; operator_review_required",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.approved_for_autonomous_paper, false);
  assert.deepEqual(result.data.symbols, []);
  assert.equal(result.data.promotion_decision?.passed, false);
  assert.deepEqual(result.data.promotion_decision?.failure_reasons, [
    "strategy_fit_not_recommended_for_paper",
    "operator_review_required",
  ]);
  assert.equal(result.data.top_symbol, "MSFT");
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parseWatchlistPromotion: unsupported schema_version is explicit", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v0",
    symbols: [],
    approved_for_autonomous_paper: false,
    promotion_decision: {
      passed: false,
      failure_reasons: [],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, "watchlist-v0");
});

test("parseWatchlistPromotion: missing schema_version is unsupported (not assumed)", () => {
  const json = JSON.stringify({
    symbols: [],
    approved_for_autonomous_paper: false,
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, null);
});

test("parseWatchlistPromotion: malformed JSON is explicit", () => {
  const result = parseWatchlistPromotion("{bad json}");
  assert.equal(result.kind, "malformed");
});

test("parseWatchlistPromotion: non-object JSON is malformed", () => {
  const result = parseWatchlistPromotion("[1,2,3]");
  assert.equal(result.kind, "malformed");
});

test("parseWatchlistPromotion: missing symbols is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    approved_for_autonomous_paper: false,
    promotion_decision: {
      passed: false,
      failure_reasons: [],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "missing_fields");
});

test("parseWatchlistPromotion: missing approved_for_autonomous_paper is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: [],
    promotion_decision: {
      passed: false,
      failure_reasons: [],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "missing_fields");
});

test("parseWatchlistPromotion: missing promotion_decision is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["AAPL"],
    approved_for_autonomous_paper: true,
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "missing_fields");
});

test("parseWatchlistPromotion: explicit approved_for_live=true is passed through and flagged as anomaly", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["AAPL"],
    approved_for_autonomous_paper: true,
    approved_for_live: true,
    promotion_decision: {
      passed: true,
      failure_reasons: ["live_approval_forbidden"],
      approved_symbols: ["AAPL"],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.approved_for_live, true);
  assert.ok(result.data.hardInvariantAnomalies.includes("watchlist_approved_for_live_true"));
});

test("parseWatchlistPromotion: missing approved_for_live defaults false, no anomaly", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: [],
    approved_for_autonomous_paper: false,
    promotion_decision: {
      passed: false,
      failure_reasons: [],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.approved_for_live, false);
  assert.deepEqual(result.data.hardInvariantAnomalies, []);
});

test("parseWatchlistPromotion: top_symbol falls back to symbols[0] when no rank-1 candidate present", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["TSLA"],
    approved_for_autonomous_paper: true,
    ranked_candidates: [{ rank: 2, symbol: "NVDA" }],
    promotion_decision: {
      passed: true,
      failure_reasons: [],
      approved_symbols: ["TSLA"],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.top_symbol, "TSLA");
});

test("parseWatchlistPromotion: top_symbol is null when symbols and ranked_candidates are both empty", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: [],
    approved_for_autonomous_paper: false,
    promotion_decision: {
      passed: false,
      failure_reasons: [],
      approved_symbols: [],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.top_symbol, null);
});

test("parseWatchlistPromotion: ranked_candidates with partial fields parsed leniently (missing fields default to null)", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["AAPL"],
    approved_for_autonomous_paper: true,
    ranked_candidates: [{ rank: 1, symbol: "AAPL", strategy_id: "intraday_scalper" }],
    promotion_decision: {
      passed: true,
      failure_reasons: [],
      approved_symbols: ["AAPL"],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  const candidate = result.data.ranked_candidates[0];
  assert.equal(candidate.rank, 1);
  assert.equal(candidate.symbol, "AAPL");
  assert.equal(candidate.strategy_id, "intraday_scalper");
  assert.equal(candidate.timeframe, null);
  assert.equal(candidate.total_score, null);
  assert.equal(candidate.liquidity_score, null);
  assert.equal(candidate.regime_label, null);
  assert.equal(candidate.net_expectancy_after_cost_bps, null);
});

test("parseWatchlistPromotion: non-string entries in symbols and non-string values in strategy_assignments are filtered out", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["AAPL", 42, null, "MSFT"],
    approved_for_autonomous_paper: true,
    strategy_assignments: { AAPL: "intraday_scalper", MSFT: 7 },
    promotion_decision: {
      passed: true,
      failure_reasons: [],
      approved_symbols: ["AAPL", "MSFT"],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.symbols, ["AAPL", "MSFT"]);
  assert.deepEqual(result.data.strategy_assignments, { AAPL: "intraday_scalper" });
});

test("parseWatchlistPromotion: non-object entries in ranked_candidates are skipped", () => {
  const json = JSON.stringify({
    schema_version: "watchlist-v1",
    symbols: ["AAPL"],
    approved_for_autonomous_paper: true,
    ranked_candidates: ["not-an-object", { rank: 1, symbol: "AAPL" }, 42],
    promotion_decision: {
      passed: true,
      failure_reasons: [],
      approved_symbols: ["AAPL"],
      strategy_assignments: {},
      notes: "",
    },
  });
  const result = parseWatchlistPromotion(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.ranked_candidates.length, 1);
  assert.equal(result.data.ranked_candidates[0].symbol, "AAPL");
});

// --- parsePremarketRevalidation ---

test("parsePremarketRevalidation: valid passed artifact (real AAPL premarket-revalidation-v1 shape)", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: true,
    failure_reasons: [],
    top_symbol: "AAPL",
    symbol_results: {
      AAPL: { passed: true, failure_reasons: [] },
    },
    notes: "premarket_revalidation_passed",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.passed, true);
  assert.deepEqual(result.data.failure_reasons, []);
  assert.equal(result.data.top_symbol, "AAPL");
  assert.deepEqual(result.data.symbol_results.AAPL, { passed: true, failure_reasons: [] });
  assert.equal(result.data.notes, "premarket_revalidation_passed");
});

test("parsePremarketRevalidation: valid blocked artifact with per-symbol failure reasons", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: false,
    failure_reasons: ["premarket_data_quality_failed"],
    top_symbol: "MSFT",
    symbol_results: {
      MSFT: { passed: false, failure_reasons: ["premarket_bar_stale", "premarket_spread_too_wide"] },
    },
    notes: "blocked: premarket_data_quality_failed",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.passed, false);
  assert.deepEqual(result.data.failure_reasons, ["premarket_data_quality_failed"]);
  assert.deepEqual(result.data.symbol_results.MSFT, {
    passed: false,
    failure_reasons: ["premarket_bar_stale", "premarket_spread_too_wide"],
  });
});

test("parsePremarketRevalidation: unsupported schema_version is explicit", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v0",
    passed: false,
    failure_reasons: [],
    symbol_results: {},
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, "premarket-revalidation-v0");
});

test("parsePremarketRevalidation: missing schema_version is unsupported (not assumed)", () => {
  const json = JSON.stringify({
    passed: false,
    failure_reasons: [],
    symbol_results: {},
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "unsupported_schema");
  if (result.kind !== "unsupported_schema") return;
  assert.equal(result.schemaVersion, null);
});

test("parsePremarketRevalidation: malformed JSON is explicit", () => {
  const result = parsePremarketRevalidation("{bad json}");
  assert.equal(result.kind, "malformed");
});

test("parsePremarketRevalidation: non-object JSON is malformed", () => {
  const result = parsePremarketRevalidation("[1,2,3]");
  assert.equal(result.kind, "malformed");
});

test("parsePremarketRevalidation: missing passed is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    failure_reasons: [],
    symbol_results: {},
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "missing_fields");
});

test("parsePremarketRevalidation: missing failure_reasons is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: true,
    symbol_results: {},
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "missing_fields");
});

test("parsePremarketRevalidation: missing symbol_results is missing_fields", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: true,
    failure_reasons: [],
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "missing_fields");
});

test("parsePremarketRevalidation: null top_symbol handled", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: false,
    failure_reasons: ["premarket_watchlist_schema_invalid"],
    top_symbol: null,
    symbol_results: {},
    notes: "",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.top_symbol, null);
});

test("parsePremarketRevalidation: empty symbol_results object parses ok", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: false,
    failure_reasons: ["premarket_mode_not_paper"],
    top_symbol: null,
    symbol_results: {},
    notes: "blocked: premarket_mode_not_paper",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.symbol_results, {});
});

test("parsePremarketRevalidation: non-object and invalid entries in symbol_results are skipped", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: false,
    failure_reasons: [],
    top_symbol: "AAPL",
    symbol_results: {
      AAPL: { passed: true, failure_reasons: [] },
      MSFT: "not-an-object",
      NVDA: { failure_reasons: ["premarket_liquidity_failed"] },
    },
    notes: "",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(Object.keys(result.data.symbol_results), ["AAPL"]);
});

test("parsePremarketRevalidation: non-string entries in failure_reasons are filtered out", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: false,
    failure_reasons: ["premarket_rvol_too_low", 42, null, "premarket_price_too_low"],
    top_symbol: "AAPL",
    symbol_results: {},
    notes: "",
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.deepEqual(result.data.failure_reasons, ["premarket_rvol_too_low", "premarket_price_too_low"]);
});

test("parsePremarketRevalidation: missing notes defaults to empty string", () => {
  const json = JSON.stringify({
    schema_version: "premarket-revalidation-v1",
    passed: true,
    failure_reasons: [],
    top_symbol: "AAPL",
    symbol_results: { AAPL: { passed: true, failure_reasons: [] } },
  });
  const result = parsePremarketRevalidation(json);
  assert.equal(result.kind, "ok");
  if (result.kind !== "ok") return;
  assert.equal(result.data.notes, "");
});
