import test from "node:test";
import assert from "node:assert/strict";
import {
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
