// core-rs/mqk-gui/src/features/controlStation/controlStationTruthGate.test.ts
//
// GUI-CS-01C proof: ControlStationScreen must consume its OWN
// panelTruthRenderState(model, "controlStation") disposition and hard-block,
// surface, or render normally per the existing isTruthHardBlock contract.
// This exercises the SAME exported helper the screen actually consumes —
// not panelTruthRenderState in isolation — so a regression that bypasses the
// screen's own gate (e.g. reverting to checking only the "dashboard" panel,
// as the pre-patch screen did) fails here.

import test from "node:test";
import assert from "node:assert/strict";
import { MOCK_MODEL } from "../system/mockData";
import { classifyPanelSources } from "../system/sourceAuthority";
import type { SystemModel } from "../system/types";
import { controlStationDisposition } from "./controlStationTruthGate";

function mockModel(): SystemModel {
  return structuredClone(MOCK_MODEL);
}

// A "real" (non-mock), freshly-heartbeating, non-degraded base model — the
// positive control every other case mutates away from a healthy baseline.
function healthyRealModel(): SystemModel {
  const model = structuredClone(MOCK_MODEL);
  model.connected = true;
  model.status.daemon_reachable = true;
  model.status.runtime_status = "running";
  model.status.last_heartbeat = new Date().toISOString();
  model.runtimeLeadership.post_restart_recovery_state = "complete";
  model.dataSource = {
    state: "real",
    reachable: true,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: [],
  };
  model.panelSources = classifyPanelSources(model.dataSource, model.connected);
  return model;
}

test("A. MOCK/PLACEHOLDER: the established MOCK_MODEL hard-blocks as unimplemented, never a healthy disposition", () => {
  const disposition = controlStationDisposition(mockModel());
  assert.equal(disposition.kind, "hard_block");
  assert.equal(disposition.kind === "hard_block" && disposition.state, "unimplemented");
});

test("B. UNAVAILABLE: disconnected/unreachable truth hard-blocks as unavailable", () => {
  const model = healthyRealModel();
  model.connected = false;
  model.status.daemon_reachable = false;

  const disposition = controlStationDisposition(model);
  assert.equal(disposition.kind, "hard_block");
  assert.equal(disposition.kind === "hard_block" && disposition.state, "unavailable");
});

test("C. STALE: stale heartbeat is surfaced as compromised, never a healthy disposition", () => {
  const model = healthyRealModel();
  model.status.last_heartbeat = new Date(Date.now() - 10 * 60_000).toISOString();

  const disposition = controlStationDisposition(model);
  assert.equal(disposition.kind, "compromised");
  assert.equal(disposition.kind === "compromised" && disposition.state, "stale");
});

test("D. DEGRADED: degraded runtime truth is surfaced as compromised, never a healthy disposition", () => {
  const model = healthyRealModel();
  model.status.runtime_status = "degraded";

  const disposition = controlStationDisposition(model);
  assert.equal(disposition.kind, "compromised");
  assert.equal(disposition.kind === "compromised" && disposition.state, "degraded");
});

test("E. HEALTHY POSITIVE CONTROL: a genuinely authoritative healthy model renders normally", () => {
  const disposition = controlStationDisposition(healthyRealModel());
  assert.deepEqual(disposition, { kind: "healthy" });
});
