import test from "node:test";
import assert from "node:assert/strict";
import { fetchOperatorModel } from "./api";
import { DEFAULT_STATUS } from "./types";
import { mapExecutionOutboxWrapper, mapFillQualityWrapper, mapPaperJournalWrapper,
  mapLegacyPositionsResponse, mapLegacyPortfolioSummary, mapLegacyTradingOrdersToExecutionOrders, mapLegacyTradingFillsToRows } from "./legacy";

test("malformed row wrappers fail their own probes without erasing independent alerts", async () => {
  const original = globalThis.fetch;
  const paths = ["/api/v1/audit/operator-actions", "/api/v1/audit/artifacts", "/api/v1/strategy/summary",
    "/api/v1/strategy/suppressions", "/api/v1/system/config-diffs", "/api/v1/risk/denials", "/api/v1/reconcile/mismatches",
    "/api/v1/portfolio/positions", "/api/v1/portfolio/orders/open", "/api/v1/portfolio/fills",
    "/api/v1/execution/summary", "/api/v1/execution/orders", "/api/v1/system/session", "/api/v1/system/config-fingerprint",
    "/api/v1/system/metadata", "/api/v1/system/runtime-leadership", "/api/v1/system/topology"];
  try {
    for (const malformed of [null, {}, { truth_state: "active", rows: {} }, { truth_state: "future", rows: [], denials: [] },
      { truth_state: "active", snapshot_state: "active", rows: [{ symbol: {} }], denials: [{}] }]) {
      globalThis.fetch = (async (input) => {
        const path = new URL(String(input)).pathname;
        if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
        if (path === "/api/v1/alerts/active") return Response.json({ truth_state: "active", alert_count: 0, rows: [] });
        if (paths.includes(path) || path === "/api/v1/paper/journal") return Response.json(malformed);
        return Response.json({}, { status: 404 });
      }) as typeof fetch;
      const model = await fetchOperatorModel();
      for (const path of paths) assert.ok(model.dataSource.missingEndpoints.includes(path), path);
      assert.ok(model.dataSource.realEndpoints.includes("/api/v1/alerts/active"));
      assert.equal(model.paperJournal.fills_truth_state, "unavailable");
    }
  } finally { globalThis.fetch = original; }
});

test("durable lane row validation fails independently and legacy conversions never repair missing economics", () => {
  for (const rows of [{}, [{}], [{ symbol: {} }]]) {
    assert.equal(mapExecutionOutboxWrapper({ truth_state: "active", rows } as never).truth_state, "unavailable");
    assert.equal(mapFillQualityWrapper({ truth_state: "active", rows } as never).truth_state, "unavailable");
    const journal = mapPaperJournalWrapper({ run_id: "run-1", fills_lane: { truth_state: "active", rows },
      admissions_lane: { truth_state: "active", rows: [] } } as never);
    assert.equal(journal.fills_truth_state, "unavailable");
    assert.equal(journal.admissions_truth_state, "active");
  }
  assert.equal(mapLegacyPortfolioSummary({ account: { equity: "", cash: "garbage" } } as never), null);
  assert.equal(mapLegacyPositionsResponse({ positions: [{ symbol: "SIM", qty: "garbage", avg_price: "10" }] } as never), null);
  const order = { broker_order_id: "broker-1", client_order_id: "order-1", symbol: "SIM", side: "buy", type: "limit", status: "filled", qty: "2", created_at_utc: "2026-10-10T10:00:00Z" };
  assert.equal(mapLegacyTradingOrdersToExecutionOrders({ orders: [order] } as never)![0].filled_qty, null);
  for (const invalid of [{ side: "future" }, { qty: "NaN" }, { created_at_utc: "not a date" }, { type: "future" }]) {
    assert.equal(mapLegacyTradingOrdersToExecutionOrders({ orders: [{ ...order, ...invalid }] } as never), null);
  }
  assert.equal(mapLegacyTradingFillsToRows({ fills: [{ broker_fill_id: "fill-1", broker_order_id: "broker-1", client_order_id: "order-1",
    symbol: "SIM", side: "buy", qty: "1", price: "10", ts_utc: "not a date" }] } as never), null);
});
