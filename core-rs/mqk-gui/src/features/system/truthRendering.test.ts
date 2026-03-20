import test from "node:test";
import assert from "node:assert/strict";
import type { SourceAuthority, SystemModel } from "./types.ts";
import { panelTruthRenderState } from "./truthRendering.ts";

type MinimalModel = Pick<SystemModel, "connected" | "dataSource" | "panelSources" | "status" | "runtimeLeadership">;

function buildModel(overrides: Partial<MinimalModel> = {}): SystemModel {
  const base: MinimalModel & Pick<SystemModel, "strategySummaryTruth"> = {
    connected: true,
    dataSource: {
      state: "real",
      reachable: true,
      realEndpoints: [
        "/api/v1/execution/summary",
        "/api/v1/execution/orders",
        "/api/v1/risk/summary",
        "/api/v1/reconcile/status",
        "/api/v1/reconcile/mismatches",
      ],
      missingEndpoints: [],
      mockSections: [],
      message: "",
    },
    panelSources: {
      execution: "db_truth",
      risk: "db_truth",
      reconcile: "db_truth",
    } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    status: {
      runtime_status: "running",
      last_heartbeat: new Date().toISOString(),
    } as SystemModel["status"],
    runtimeLeadership: {
      post_restart_recovery_state: "complete",
    } as SystemModel["runtimeLeadership"],
    strategySummaryTruth: {
      truth_state: "unknown",
      backend: null,
    },
  };

  return { ...base, ...overrides } as SystemModel;
}

test("renders unavailable when disconnected", () => {
  const state = panelTruthRenderState(buildModel({ connected: false }), "execution");
  assert.equal(state, "unavailable");
});

test("renders unimplemented for placeholder authority", () => {
  const state = panelTruthRenderState(buildModel({ panelSources: { execution: "placeholder" } as Record<string, SourceAuthority> as SystemModel["panelSources"] }), "execution");
  assert.equal(state, "unimplemented");
});

test("renders degraded for degraded runtime state", () => {
  const state = panelTruthRenderState(buildModel({ status: { runtime_status: "degraded" } as SystemModel["status"] }), "risk");
  assert.equal(state, "degraded");
});

test("renders stale when heartbeat freshness exceeds threshold", () => {
  const state = panelTruthRenderState(buildModel({ status: { runtime_status: "running", last_heartbeat: new Date(Date.now() - 121_000).toISOString() } as SystemModel["status"] }), "reconcile");
  assert.equal(state, "stale");
});

test("renders no_snapshot when required panel endpoints are absent in partial mode", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: [],
        missingEndpoints: ["/api/v1/reconcile/status", "/api/v1/reconcile/mismatches"],
        mockSections: [],
      },
    }),
    "reconcile",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot fires for reconcile when status resolves but mismatches are missing in partial mode", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/reconcile/status"],
        missingEndpoints: ["/api/v1/reconcile/mismatches"],
        mockSections: ["mismatches"],
      },
    }),
    "reconcile",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot fires for reconcile when mismatches resolve but status is missing in partial mode", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/reconcile/mismatches"],
        missingEndpoints: ["/api/v1/reconcile/status"],
        mockSections: ["reconcileSummary"],
      },
    }),
    "reconcile",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot fires for portfolio panel when portfolio/positions is missing in partial mode", () => {
  // Simulates: broker_snapshot absent → positions IIFE returns ok:false →
  // /api/v1/portfolio/positions in missingEndpoints → isMissingPanelTruth fires.
  // portfolio/summary stays in realEndpoints (it returns HTTP 200 even when has_snapshot=false),
  // but that is intentional — only the row-level gate matters for the panel block.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/system/status", "/api/v1/portfolio/summary"],
        missingEndpoints: ["/api/v1/portfolio/positions"],
        mockSections: ["positions", "openOrders", "fills"],
      },
      panelSources: { portfolio: "mixed" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "portfolio",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot does NOT fire for portfolio when positions resolves active (authoritative empty is safe)", () => {
  // Simulates: broker_snapshot present, account holds zero positions.
  // positions IIFE returns ok:true + snapshot_state="active" + rows=[].
  // /api/v1/portfolio/positions is in realEndpoints — not missingEndpoints.
  // isMissingPanelTruth must NOT fire. Authoritative empty ≠ missing truth.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: [
          "/api/v1/portfolio/summary",
          "/api/v1/portfolio/positions",
          "/api/v1/portfolio/orders/open",
          "/api/v1/portfolio/fills",
        ],
        missingEndpoints: [],
        mockSections: [],
      },
      panelSources: { portfolio: "broker_snapshot" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "portfolio",
  );
  assert.equal(state, null, "active snapshot with zero rows must render as healthy (null)");
});

test("no_snapshot fires for strategy panel when strategy/summary missing in partial mode", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/system/status"],
        missingEndpoints: ["/api/v1/strategy/summary"],
        mockSections: [],
      },
      panelSources: { strategy: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "strategy",
  );
  assert.equal(state, "no_snapshot");
});

test("returns null (green) when all panel truth endpoints resolve", () => {
  const panels = ["execution", "risk", "reconcile"] as const;
  for (const panel of panels) {
    const state = panelTruthRenderState(buildModel(), panel);
    assert.equal(state, null, `${panel} should return null when all endpoints resolve`);
  }
});

// --- stale/degraded hard-block coverage (PATCH-1 closure) ---

test("stale fires for ops panel when heartbeat exceeds threshold", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date(Date.now() - 125_000).toISOString(),
      } as SystemModel["status"],
    }),
    "ops",
  );
  assert.equal(state, "stale");
});

test("stale fires for dashboard panel when heartbeat exceeds threshold", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date(Date.now() - 125_000).toISOString(),
      } as SystemModel["status"],
    }),
    "dashboard",
  );
  assert.equal(state, "stale");
});

test("degraded fires for execution panel when runtime_status is degraded", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: { runtime_status: "degraded" } as SystemModel["status"],
    }),
    "execution",
  );
  assert.equal(state, "degraded");
});

test("degraded fires for portfolio panel when runtime_status is degraded", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: { runtime_status: "degraded" } as SystemModel["status"],
    }),
    "portfolio",
  );
  assert.equal(state, "degraded");
});

test("degraded fires for reconcile panel when runtime_status is degraded", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: { runtime_status: "degraded" } as SystemModel["status"],
    }),
    "reconcile",
  );
  assert.equal(state, "degraded");
});

test("degraded fires for session panel when runtime_status is degraded", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: { runtime_status: "degraded" } as SystemModel["status"],
    }),
    "session",
  );
  assert.equal(state, "degraded");
});

test("null returned for ops panel when truth is fully healthy", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/system/status"],
        missingEndpoints: [],
        mockSections: [],
      },
    }),
    "ops",
  );
  assert.equal(state, null);
});

test("no_snapshot fires for alerts panel when alerts/active missing in partial mode", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/system/status"],
        missingEndpoints: ["/api/v1/alerts/active"],
        mockSections: [],
      },
      panelSources: { alerts: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "alerts",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot does not fire for alerts when alerts/active resolves", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/alerts/active"],
        missingEndpoints: [],
        mockSections: [],
      },
      panelSources: { alerts: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "alerts",
  );
  assert.equal(state, null);
});

// --- status canonical tracking (PC-1 closure) ---

test("ops panel returns unimplemented when status resolved via legacy only (status in mockSections)", () => {
  // Simulates: statusCanonical=false → usedMockSections.push("status")
  // ops evidence hints have placeholder: ["status", ...] so authority degrades to "placeholder".
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/v1/status"],
        missingEndpoints: [],
        mockSections: ["status"],
      },
      panelSources: { ops: "placeholder" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "ops",
  );
  assert.equal(state, "unimplemented");
});

test("ops panel returns null when canonical status resolves (status not in mockSections)", () => {
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/system/status"],
        missingEndpoints: [],
        mockSections: [],
      },
      panelSources: { ops: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "ops",
  );
  assert.equal(state, null);
});

// --- execution orders no-snapshot semantic gate (Cluster 1 retry) ---
// Proves that HTTP 503 from /api/v1/execution/orders (→ missingEndpoints) blocks
// the execution panel even when execution_summary still resolves at HTTP 200.

// --- risk denial truth gate (Cluster 3) ---
// Proves that the risk panel gate is driven by /risk/denials (not /risk/summary).
// /risk/summary always returns HTTP 200 so it never lands in missingEndpoints and
// cannot drive no_snapshot.  /risk/denials IIFE returns ok: false when the execution
// loop is not running (truth_state === "no_snapshot") → endpoint goes to missingEndpoints
// → isMissingPanelTruth fires → risk panel blocks.

test("no_snapshot fires for risk panel when risk/denials is missing in partial mode", () => {
  // Simulates: execution loop not running → risk_denials IIFE returns ok:false →
  // /api/v1/risk/denials in missingEndpoints.
  // /api/v1/risk/summary is still in realEndpoints (HTTP 200, has_snapshot=false).
  // With hint ["/risk/denials"] and every(), the check reduces to:
  //   "is /risk/denials in missingEndpoints?" → yes → no_snapshot fires.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/risk/summary"],
        missingEndpoints: ["/api/v1/risk/denials"],
        mockSections: ["riskDenials"],
      },
      panelSources: { risk: "mixed" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "risk",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot fires for risk panel when truth is not_wired (loop running, denial source absent)", () => {
  // Simulates: execution loop IS running but denial accumulator is not yet wired.
  // risk_denials IIFE reads truth_state === "not_wired" → returns ok:false →
  // /api/v1/risk/denials lands in missingEndpoints → isMissingPanelTruth fires.
  // execution_snapshot present does NOT mean denial detail truth is available.
  // The risk panel must stay fail-closed; an empty denial table must not render.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/risk/summary"],
        missingEndpoints: ["/api/v1/risk/denials"],
        mockSections: ["riskDenials"],
      },
      panelSources: { risk: "mixed" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "risk",
  );
  assert.equal(state, "no_snapshot", "not_wired denial truth must keep risk panel blocked with no_snapshot");
});

test("no_snapshot fires for execution panel when execution_orders is missing in partial mode", () => {
  // Simulates: daemon returns 503 from /api/v1/execution/orders (no execution loop running).
  // execution_summary is still in realEndpoints (HTTP 200, has_snapshot=false, zero counts).
  // With hint ["/execution/orders"] and every(), the check reduces to:
  //   "is /execution/orders in missingEndpoints?" → yes → no_snapshot fires.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "partial",
        reachable: true,
        realEndpoints: ["/api/v1/execution/summary"],
        missingEndpoints: ["/api/v1/execution/orders"],
        mockSections: ["executionOrders"],
      },
      panelSources: { execution: "mixed" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "execution",
  );
  assert.equal(state, "no_snapshot");
});

test("no_snapshot does not fire for execution when execution_orders resolves (snapshot active)", () => {
  // Simulates: daemon has a live execution snapshot → 200 + array.
  // Both execution_summary and execution_orders are in realEndpoints.
  // isMissingPanelTruth: state is "real" → returns false immediately.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/execution/summary", "/api/v1/execution/orders"],
        missingEndpoints: [],
        mockSections: [],
      },
      panelSources: { execution: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "execution",
  );
  assert.equal(state, null);
});

// --- AP-09: external broker WS continuity gate ---
// Proves that execution and reconcile panels block when broker_snapshot_source is
// "external" but alpaca_ws_continuity is not "live" (cold_start_unproven or
// gap_detected).  Portfolio is NOT gated — it uses the REST snapshot, not WS events.

test("AP-09: no_snapshot for execution when external broker and cold_start_unproven", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "external",
        alpaca_ws_continuity: "cold_start_unproven",
      } as SystemModel["status"],
    }),
    "execution",
  );
  assert.equal(state, "no_snapshot", "external broker with cold_start_unproven must block execution panel");
});

test("AP-09: no_snapshot for reconcile when external broker and cold_start_unproven", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "external",
        alpaca_ws_continuity: "cold_start_unproven",
      } as SystemModel["status"],
    }),
    "reconcile",
  );
  assert.equal(state, "no_snapshot", "external broker with cold_start_unproven must block reconcile panel");
});

test("AP-09: no_snapshot for execution when external broker and gap_detected", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "external",
        alpaca_ws_continuity: "gap_detected",
      } as SystemModel["status"],
    }),
    "execution",
  );
  assert.equal(state, "no_snapshot", "external broker with gap_detected must block execution panel");
});

test("AP-09: null for execution when external broker and continuity is live (proven)", () => {
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "external",
        alpaca_ws_continuity: "live",
      } as SystemModel["status"],
    }),
    "execution",
  );
  assert.equal(state, null, "external broker with live continuity must not block execution panel");
});

test("AP-09: portfolio NOT gated on external broker WS continuity (REST-independent truth)", () => {
  // Portfolio positions come from Alpaca REST snapshot, not from WS trade events.
  // cold_start_unproven must not block portfolio — positions are still authoritative.
  const state = panelTruthRenderState(
    buildModel({
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/portfolio/positions"],
        missingEndpoints: [],
        mockSections: [],
      },
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "external",
        alpaca_ws_continuity: "cold_start_unproven",
      } as SystemModel["status"],
    }),
    "portfolio",
  );
  assert.equal(state, null, "portfolio must not be blocked by WS continuity gap (REST-independent)");
});

test("AP-09: synthetic broker not affected by continuity gate (paper mode unaffected)", () => {
  // Paper mode: broker_snapshot_source = "synthetic", alpaca_ws_continuity = "not_applicable".
  // The gate must never fire for synthetic broker regardless of continuity value.
  const state = panelTruthRenderState(
    buildModel({
      status: {
        runtime_status: "running",
        last_heartbeat: new Date().toISOString(),
        broker_snapshot_source: "synthetic",
        alpaca_ws_continuity: "not_applicable",
      } as SystemModel["status"],
    }),
    "execution",
  );
  assert.equal(state, null, "synthetic broker must never trigger external continuity gate");
});


test("strategy panel renders not_wired when daemon explicitly reports mounted-but-unwired summary truth", () => {
  const state = panelTruthRenderState(
    buildModel({
      strategySummaryTruth: { truth_state: "not_wired", backend: "not_wired" },
      dataSource: {
        state: "real",
        reachable: true,
        realEndpoints: ["/api/v1/system/status", "/api/v1/strategy/summary"],
        missingEndpoints: [],
        mockSections: [],
      },
      panelSources: { strategy: "runtime_memory" } as Record<string, SourceAuthority> as SystemModel["panelSources"],
    }),
    "strategy",
  );
  assert.equal(state, "not_wired");
});
