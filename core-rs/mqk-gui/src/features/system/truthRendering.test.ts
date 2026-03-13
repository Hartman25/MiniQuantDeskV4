import test from "node:test";
import assert from "node:assert/strict";
import type { SourceAuthority, SystemModel } from "./types.ts";
import { panelTruthRenderState } from "./truthRendering.ts";

type MinimalModel = Pick<SystemModel, "connected" | "dataSource" | "panelSources" | "status" | "runtimeLeadership">;

function buildModel(overrides: Partial<MinimalModel> = {}): SystemModel {
  const base: MinimalModel = {
    connected: true,
    dataSource: {
      state: "real",
      reachable: true,
      realEndpoints: [
        "/api/v1/execution/summary",
        "/api/v1/execution/orders",
        "/api/v1/execution/timeline",
        "/api/v1/risk/summary",
        "/api/v1/reconcile/summary",
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
        missingEndpoints: ["/api/v1/reconcile/summary", "/api/v1/reconcile/mismatches"],
        mockSections: [],
      },
    }),
    "reconcile",
  );
  assert.equal(state, "no_snapshot");
});
