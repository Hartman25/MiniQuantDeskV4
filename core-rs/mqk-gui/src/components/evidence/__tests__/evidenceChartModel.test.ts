import test from "node:test";
import assert from "node:assert/strict";
import { buildEvidenceChartModel, timeFraction } from "../evidenceChartModel.ts";
import type {
  ArtifactBundle,
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  FillRow,
  OrderRow,
} from "../../../features/backtests/types.ts";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function baseManifest(overrides: Partial<BacktestManifest> = {}): BacktestManifest {
  return {
    schema_version: 1,
    run_id: "run-1",
    strategy_name: "swing_momentum",
    engine_id: "mqk-backtest",
    mode: "backtest",
    created_at_utc: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function baseMetrics(overrides: Partial<BacktestMetrics> = {}): BacktestMetrics {
  return {
    schema_version: 1,
    run_id: "run-1",
    strategy_name: "swing_momentum",
    halted: false,
    halt_reason: null,
    execution_blocked: false,
    bars: 3,
    orders: 0,
    orders_filled: 0,
    orders_rejected: 0,
    fills: 0,
    final_equity_micros: 100_000_000_000,
    symbols: ["AAPL"],
    starting_equity_micros: 100_000_000_000,
    ending_equity_micros: 100_000_000_000,
    total_return_micros: 0,
    total_return_pct: 0,
    equity_high_water_mark_micros: 100_000_000_000,
    max_drawdown_micros: 0,
    max_drawdown_pct: 0,
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
    exposure_time_pct: 0,
    ...overrides,
  };
}

function equityRow(ts_utc: string, equity: number): EquityCurveRow {
  return { ts_utc, equity };
}

function orderRow(overrides: Partial<OrderRow> = {}): OrderRow {
  return {
    ts_utc: "2026-01-01T00:00:00Z",
    order_id: "o1",
    symbol: "AAPL",
    side: "buy",
    qty: "10",
    order_type: "market",
    limit_price: "",
    stop_price: "",
    status: "filled",
    ...overrides,
  };
}

function fillRow(overrides: Partial<FillRow> = {}): FillRow {
  return {
    ts_utc: "2026-01-01T00:05:00Z",
    fill_id: "f1",
    order_id: "o1",
    symbol: "AAPL",
    side: "buy",
    qty: "10",
    price: "150000000",
    fee: "1000000",
    ...overrides,
  };
}

function baseBundle(overrides: Partial<ArtifactBundle> = {}): ArtifactBundle {
  return {
    manifest: { kind: "ok", data: baseManifest() },
    metrics: { kind: "ok", data: baseMetrics() },
    equityCurve: {
      kind: "ok",
      data: {
        rows: [
          equityRow("2026-01-01T00:00:00Z", 100_000_000),
          equityRow("2026-01-01T01:00:00Z", 101_000_000),
          equityRow("2026-01-01T02:00:00Z", 99_000_000),
        ],
        malformed: 0,
      },
    },
    orders: { kind: "missing" },
    fills: { kind: "missing" },
    strategyFit: { kind: "missing" },
    paperReadiness: { kind: "missing" },
    watchlistPromotion: { kind: "missing" },
    premarketRevalidation: { kind: "missing" },
    evidenceReview: { kind: "missing" },
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// A — missing price source: chart does NOT manufacture bars.
// ---------------------------------------------------------------------------

test("A: priceSeries is always not_wired regardless of input — never manufactures bars", () => {
  const model = buildEvidenceChartModel(baseBundle());
  assert.equal(model.priceSeries.status, "not_wired");
  assert.deepEqual(model.priceSeries.bars, []);
});

// ---------------------------------------------------------------------------
// B — a research recommendation artifact (strategyFit) existing does not
// manufacture a fill marker. (No fwd_ret/classification artifact exists in
// ArtifactBundle at all; strategyFit is the closest research-recommendation
// analog available on this bundle.)
// ---------------------------------------------------------------------------

test("B: strategyFit present with no fills.csv produces zero fill markers (positive control: literal fill produces one)", () => {
  const noFills = buildEvidenceChartModel(
    baseBundle({
      strategyFit: {
        kind: "ok",
        data: {
          schema_version: "strategy-fit-v1",
          artifact_id: null,
          symbol: "AAPL",
          strategy_id: "swing_momentum",
          timeframe: "1h",
          trades: 5,
          profit_factor: 1.4,
          expectancy_bps: 3,
          net_expectancy_after_cost_bps: 2,
          recommended_for_paper: true,
          recommended_for_live: false,
          recommended_for_live_present: true,
          failure_reasons: [],
          gateFlags: {
            profit_factor_failed: false,
            expectancy_failed: false,
            cost_adjusted_edge_failed: false,
            out_of_sample_failed: false,
            sample_quality_failed: false,
            parameter_stability_failed: false,
            validation_metrics_missing: false,
          },
        },
      },
      fills: { kind: "missing" },
    }),
  );
  assert.equal(noFills.fillMarkers.markers.length, 0);
  assert.equal(noFills.fillMarkers.status, "unavailable");

  // Positive control: the same bundle, but with one literal fills.csv row.
  const withFill = buildEvidenceChartModel(
    baseBundle({ fills: { kind: "ok", data: { rows: [fillRow()], malformed: 0 } } }),
  );
  assert.equal(withFill.fillMarkers.markers.length, 1);
  assert.equal(withFill.fillMarkers.status, "authoritative");
});

// ---------------------------------------------------------------------------
// C — signal exists without order intent => no order marker. Since no signal
// artifact ever exists in ArtifactBundle, signalMarkers is always not_wired
// and orderIntentMarkers is only ever built from literal orders.csv rows.
// ---------------------------------------------------------------------------

test("C: signalMarkers is always not_wired; orderIntentMarkers only reflects literal orders.csv rows", () => {
  const model = buildEvidenceChartModel(
    baseBundle({ orders: { kind: "ok", data: { rows: [orderRow()], malformed: 0 } } }),
  );
  assert.equal(model.signalMarkers.status, "not_wired");
  assert.equal(model.signalMarkers.markers.length, 0);
  assert.equal(model.orderIntentMarkers.markers.length, 1);
  assert.equal(model.orderIntentMarkers.markers[0].id, "order:o1");
});

// ---------------------------------------------------------------------------
// D — order intent exists without fill => no fill marker for it.
// ---------------------------------------------------------------------------

test("D: an order with no matching fills.csv row produces zero fill markers (positive control: matching fill produces one)", () => {
  const noFill = buildEvidenceChartModel(
    baseBundle({
      orders: { kind: "ok", data: { rows: [orderRow({ order_id: "o1" })], malformed: 0 } },
      fills: { kind: "missing" },
    }),
  );
  assert.equal(noFill.orderIntentMarkers.markers.length, 1);
  assert.equal(noFill.fillMarkers.markers.length, 0);

  const withFill = buildEvidenceChartModel(
    baseBundle({
      orders: { kind: "ok", data: { rows: [orderRow({ order_id: "o1" })], malformed: 0 } },
      fills: { kind: "ok", data: { rows: [fillRow({ order_id: "o1", fill_id: "f1" })], malformed: 0 } },
    }),
  );
  assert.equal(withFill.fillMarkers.markers.length, 1);
  assert.equal(withFill.fillMarkers.markers[0].id, "fill:f1");
});

// ---------------------------------------------------------------------------
// E — missing/unknown timestamp => event is not silently placed at a
// guessed bar.
// ---------------------------------------------------------------------------

test("E: a fill with an empty ts_utc is marked unknown and cannot resolve a time fraction (positive control: known ts resolves)", () => {
  const model = buildEvidenceChartModel(
    baseBundle({
      fills: {
        kind: "ok",
        data: {
          rows: [
            fillRow({ fill_id: "unknown-ts", ts_utc: "" }),
            fillRow({ fill_id: "known-ts", ts_utc: "2026-01-01T01:00:00Z" }),
          ],
          malformed: 0,
        },
      },
    }),
  );
  const unknownMarker = model.fillMarkers.markers.find((m) => m.id === "fill:unknown-ts");
  const knownMarker = model.fillMarkers.markers.find((m) => m.id === "fill:known-ts");
  assert.ok(unknownMarker);
  assert.ok(knownMarker);

  assert.equal(unknownMarker!.timeStatus, "unknown");
  assert.equal(unknownMarker!.tsMs, null);
  assert.equal(timeFraction(model.timeRange, unknownMarker!.tsMs), null);

  assert.equal(knownMarker!.timeStatus, "known");
  assert.notEqual(knownMarker!.tsMs, null);
  assert.notEqual(timeFraction(model.timeRange, knownMarker!.tsMs), null);
});

// ---------------------------------------------------------------------------
// F — fold/OOS region identity: this wave has no fold/OOS boundary source at
// all, so the strongest available proof is that these lanes can never be
// tricked into manufacturing a region regardless of arbitrary bundle content.
// ---------------------------------------------------------------------------

test("F: foldRegions/oosRegions never manufacture a region regardless of bundle content", () => {
  const modelA = buildEvidenceChartModel(baseBundle());
  const modelB = buildEvidenceChartModel(
    baseBundle({
      manifest: { kind: "ok", data: baseManifest({ run_id: "different-run-id" }) },
      orders: { kind: "ok", data: { rows: [orderRow()], malformed: 0 } },
    }),
  );
  for (const model of [modelA, modelB]) {
    assert.equal(model.foldRegions.status, "not_wired");
    assert.deepEqual(model.foldRegions.regions, []);
    assert.equal(model.oosRegions.status, "not_wired");
    assert.deepEqual(model.oosRegions.regions, []);
  }
});

// ---------------------------------------------------------------------------
// G — a transport/layout-only artifact change does not manufacture a new
// semantic candidate/event identity (marker ids are content-derived, not
// array-position-derived).
// ---------------------------------------------------------------------------

test("G: marker identity is content-derived (order_id/fill_id), stable under row reordering/insertion", () => {
  const before = buildEvidenceChartModel(
    baseBundle({
      orders: {
        kind: "ok",
        data: { rows: [orderRow({ order_id: "o-a" }), orderRow({ order_id: "o-b" })], malformed: 0 },
      },
    }),
  );
  const after = buildEvidenceChartModel(
    baseBundle({
      orders: {
        kind: "ok",
        data: {
          rows: [orderRow({ order_id: "o-new" }), orderRow({ order_id: "o-a" }), orderRow({ order_id: "o-b" })],
          malformed: 0,
        },
      },
    }),
  );
  const beforeIds = before.orderIntentMarkers.markers.map((m) => m.id);
  const afterIds = after.orderIntentMarkers.markers.map((m) => m.id);
  assert.ok(beforeIds.includes("order:o-a"));
  assert.ok(beforeIds.includes("order:o-b"));
  // Inserting a row at the front must not change the identity of o-a/o-b —
  // an index-keyed implementation would shift them to order:0/order:1 vs
  // order:1/order:2 here.
  assert.ok(afterIds.includes("order:o-a"));
  assert.ok(afterIds.includes("order:o-b"));
  assert.ok(afterIds.includes("order:o-new"));
});

// ---------------------------------------------------------------------------
// H — empty authoritative list vs unavailable/not-wired must not collapse.
// ---------------------------------------------------------------------------

test("H: authoritative empty orders.csv is distinct from unavailable orders.csv", () => {
  const empty = buildEvidenceChartModel(baseBundle({ orders: { kind: "ok", data: { rows: [], malformed: 0 } } }));
  assert.equal(empty.orderIntentMarkers.status, "authoritative_empty");
  assert.equal(empty.orderIntentMarkers.markers.length, 0);

  const missing = buildEvidenceChartModel(baseBundle({ orders: { kind: "missing" } }));
  assert.equal(missing.orderIntentMarkers.status, "unavailable");
  assert.equal(missing.orderIntentMarkers.markers.length, 0);

  const readError = buildEvidenceChartModel(baseBundle({ orders: { kind: "read_error", message: "disk error" } }));
  assert.equal(readError.orderIntentMarkers.status, "unavailable");
});

// ---------------------------------------------------------------------------
// Additional identity / cost-event coverage
// ---------------------------------------------------------------------------

test("identity is derived from manifest, falling back to metrics when manifest is unavailable", () => {
  const withManifest = buildEvidenceChartModel(baseBundle());
  assert.equal(withManifest.identity.runId, "run-1");
  assert.equal(withManifest.identity.strategyName, "swing_momentum");

  const noManifest = buildEvidenceChartModel(baseBundle({ manifest: { kind: "missing" } }));
  assert.equal(noManifest.identity.runId, "run-1"); // falls back to metrics.run_id
  assert.equal(noManifest.identity.engineId, null); // engine_id only exists on manifest
});

test("order limit_price is converted from micros to dollars, matching fills.csv's price/fee unit convention", () => {
  const model = buildEvidenceChartModel(
    baseBundle({
      orders: {
        kind: "ok",
        data: { rows: [orderRow({ order_id: "o-limit", order_type: "limit", limit_price: "300000000" })], malformed: 0 },
      },
    }),
  );
  const marker = model.orderIntentMarkers.markers.find((m) => m.id === "order:o-limit");
  assert.ok(marker);
  assert.equal(marker!.price, 300);
});

test("cost events only appear for fills with a parseable fee, never fabricated for blank fees", () => {
  const model = buildEvidenceChartModel(
    baseBundle({
      fills: {
        kind: "ok",
        data: {
          rows: [
            fillRow({ fill_id: "has-fee", fee: "500000" }),
            fillRow({ fill_id: "blank-fee", fee: "" }),
          ],
          malformed: 0,
        },
      },
    }),
  );
  assert.equal(model.costEvents.markers.length, 1);
  assert.equal(model.costEvents.markers[0].id, "cost:has-fee");
});

test("timeFraction never guesses: unresolved range or timestamp yields null, not a default position", () => {
  const unavailableRange = { status: "unavailable" as const, startTsUtc: null, endTsUtc: null, startMs: null, endMs: null };
  assert.equal(timeFraction(unavailableRange, 12345), null);

  const resolvedRange = { status: "authoritative" as const, startTsUtc: "a", endTsUtc: "b", startMs: 0, endMs: 1000 };
  assert.equal(timeFraction(resolvedRange, null), null);
  assert.equal(timeFraction(resolvedRange, 500), 0.5);
});

// ---------------------------------------------------------------------------
// OT-MQD-01R: timeFraction fail-closed chronology negative/positive controls.
// An event outside the authoritative range must never be relocated onto the
// chart boundary — it must not plot at all.
// ---------------------------------------------------------------------------

const RANGE_0_1000 = { status: "authoritative" as const, startTsUtc: "a", endTsUtc: "b", startMs: 0, endMs: 1000 };
const REVERSED_RANGE = { status: "authoritative" as const, startTsUtc: "b", endTsUtc: "a", startMs: 1000, endMs: 0 };
const DEGENERATE_RANGE = { status: "authoritative" as const, startTsUtc: "a", endTsUtc: "a", startMs: 500, endMs: 500 };

test("A1: a timestamp before the range does not plot (never relocated to start)", () => {
  assert.equal(timeFraction(RANGE_0_1000, -500), null);
});

test("A2: a timestamp after the range does not plot (never relocated to end)", () => {
  assert.equal(timeFraction(RANGE_0_1000, 1500), null);
});

test("A3: a missing timestamp does not plot", () => {
  assert.equal(timeFraction(RANGE_0_1000, null), null);
});

test("A4: a reversed/invalid range fails closed — no timestamp plots, including one inside the numeric bounds", () => {
  assert.equal(timeFraction(REVERSED_RANGE, 500), null);
  assert.equal(timeFraction(REVERSED_RANGE, 0), null);
  assert.equal(timeFraction(REVERSED_RANGE, 1000), null);
});

test("A5: a degenerate range (start == end) does not plot a different timestamp", () => {
  assert.equal(timeFraction(DEGENERATE_RANGE, 501), null);
  assert.equal(timeFraction(DEGENERATE_RANGE, 499), null);
});

test("A6: exact range start resolves to fraction 0", () => {
  assert.equal(timeFraction(RANGE_0_1000, 0), 0);
});

test("A7: exact range end resolves to fraction 1", () => {
  assert.equal(timeFraction(RANGE_0_1000, 1000), 1);
});

test("A8: a midpoint timestamp resolves to the correct fractional position", () => {
  assert.equal(timeFraction(RANGE_0_1000, 250), 0.25);
  assert.equal(timeFraction(RANGE_0_1000, 750), 0.75);
});

test("A9: a degenerate range resolves the exact matching timestamp to the midpoint", () => {
  assert.equal(timeFraction(DEGENERATE_RANGE, 500), 0.5);
});

// ---------------------------------------------------------------------------
// OT-MQD-01R: identity provenance conflict — manifest vs metrics disagreement
// must never silently pick a side.
// ---------------------------------------------------------------------------

test("B1: matching manifest/metrics run_id is accepted as authoritative", () => {
  const model = buildEvidenceChartModel(baseBundle());
  assert.equal(model.identity.runId, "run-1");
  assert.equal(model.identity.runIdConflict, false);
});

test("B2: manifest-only run_id is honest but partial", () => {
  const model = buildEvidenceChartModel(baseBundle({ metrics: { kind: "missing" } }));
  assert.equal(model.identity.runId, "run-1");
  assert.equal(model.identity.runIdConflict, false);
});

test("B3: metrics-only run_id is honest but partial", () => {
  const model = buildEvidenceChartModel(baseBundle({ manifest: { kind: "missing" } }));
  assert.equal(model.identity.runId, "run-1");
  assert.equal(model.identity.runIdConflict, false);
});

test("B4: conflicting run_id yields no selected run ID and a visible conflict flag", () => {
  const model = buildEvidenceChartModel(
    baseBundle({ manifest: { kind: "ok", data: baseManifest({ run_id: "run-manifest" }) } }),
  );
  assert.equal(model.identity.runId, null);
  assert.equal(model.identity.runIdConflict, true);
  // Marker provenance must not carry the contested run id either.
  const withOrders = buildEvidenceChartModel(
    baseBundle({
      manifest: { kind: "ok", data: baseManifest({ run_id: "run-manifest" }) },
      orders: { kind: "ok", data: { rows: [orderRow()], malformed: 0 } },
    }),
  );
  const marker = withOrders.orderIntentMarkers.markers[0];
  assert.equal(marker.provenance.runId, null);
  assert.equal(marker.provenance.runIdConflict, true);
});

test("B5: matching strategy_name produces an honest label, no conflict", () => {
  const model = buildEvidenceChartModel(baseBundle());
  assert.equal(model.identity.strategyName, "swing_momentum");
  assert.equal(model.identity.strategyNameConflict, false);
});

test("B6: conflicting strategy_name is a conflict, not a silent preference", () => {
  const model = buildEvidenceChartModel(
    baseBundle({ manifest: { kind: "ok", data: baseManifest({ strategy_name: "mean_reversion" }) } }),
  );
  assert.equal(model.identity.strategyName, null);
  assert.equal(model.identity.strategyNameConflict, true);
});

test("B7: strategy_name is never exposed as a field named strategyId", () => {
  const model = buildEvidenceChartModel(baseBundle());
  assert.ok(!("strategyId" in model.identity));
  assert.ok("strategyName" in model.identity);
});
