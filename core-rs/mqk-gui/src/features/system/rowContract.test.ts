import test from "node:test";
import assert from "node:assert/strict";
import { fetchOperatorModel } from "./api";
import { DEFAULT_STATUS } from "./types";

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
