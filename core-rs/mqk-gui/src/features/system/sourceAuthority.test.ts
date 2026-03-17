import test from "node:test";
import assert from "node:assert/strict";
import { CORE_PANEL_KEYS, type DataSourceDetail } from "./types.ts";
import {
  FIELD_EVIDENCE_HINTS,
  classifyAuthority,
  classifyFieldSource,
  classifyPanelSources,
} from "./sourceAuthority.ts";

function baseDataSource(overrides: Partial<DataSourceDetail> = {}): DataSourceDetail {
  return {
    state: "real",
    reachable: true,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: [],
    ...overrides,
  };
}

test("disconnected operator model returns unknown for all panels", () => {
  const sources = classifyPanelSources(baseDataSource({ state: "disconnected", reachable: false }), false);
  for (const panel of CORE_PANEL_KEYS) {
    assert.equal(sources[panel], "unknown");
  }
});

test("mock fallback model returns placeholder for all panels", () => {
  const sources = classifyPanelSources(baseDataSource({ state: "mock", mockSections: ["all"] }), true);
  for (const panel of CORE_PANEL_KEYS) {
    assert.equal(sources[panel], "placeholder");
  }
});

test("pure DB evidence classifies as db_truth", () => {
  assert.equal(classifyAuthority({ hasDb: true, hasRuntime: false, hasBroker: false, hasPlaceholder: false }, true), "db_truth");
});

test("pure runtime evidence classifies as runtime_memory", () => {
  assert.equal(classifyAuthority({ hasDb: false, hasRuntime: true, hasBroker: false, hasPlaceholder: false }, true), "runtime_memory");
});

test("pure broker evidence classifies as broker_snapshot", () => {
  assert.equal(classifyAuthority({ hasDb: false, hasRuntime: false, hasBroker: true, hasPlaceholder: false }, true), "broker_snapshot");
});

test("multiple real evidence types classify as mixed", () => {
  assert.equal(classifyAuthority({ hasDb: true, hasRuntime: true, hasBroker: false, hasPlaceholder: false }, true), "mixed");
});

test("placeholder plus real evidence is classified conservatively as mixed", () => {
  assert.equal(classifyAuthority({ hasDb: true, hasRuntime: false, hasBroker: false, hasPlaceholder: true }, true), "mixed");
});

test("panel source map is exhaustive for every core panel", () => {
  const sources = classifyPanelSources(
    baseDataSource({
      realEndpoints: ["/api/v1/system/status", "/api/v1/system/preflight", "/api/v1/system/runtime-leadership"],
      mockSections: [],
    }),
    true,
  );

  assert.deepEqual(Object.keys(sources).sort(), [...CORE_PANEL_KEYS].sort());
  for (const panel of CORE_PANEL_KEYS) {
    assert.ok(sources[panel]);
  }
});


test("field source classification returns placeholder for mock datasource", () => {
  const authority = classifyFieldSource(
    baseDataSource({ state: "mock", mockSections: ["status"] }),
    true,
    { db: ["/system/status"], runtime: ["/system/status"], broker: [], placeholder: ["status"] },
  );

  assert.equal(authority, "placeholder");
});

test("field source classification surfaces mixed when db and runtime both back a field", () => {
  const authority = classifyFieldSource(
    baseDataSource({ realEndpoints: ["/api/v1/system/status", "/api/v1/system/config-fingerprint"] }),
    true,
    { db: ["/system/config-fingerprint"], runtime: ["/system/status"], broker: [], placeholder: [] },
  );

  assert.equal(authority, "mixed");
});

test("portfolio panel remains broker_snapshot when only broker-backed portfolio endpoints are present", () => {
  const sources = classifyPanelSources(
    baseDataSource({
      realEndpoints: ["/api/v1/portfolio/summary", "/api/v1/portfolio/positions", "/api/v1/portfolio/orders/open", "/api/v1/portfolio/fills"],
    }),
    true,
  );

  assert.equal(sources.portfolio, "broker_snapshot");
});

test("risk denials field remains runtime_memory", () => {
  const authority = classifyFieldSource(
    baseDataSource({ realEndpoints: ["/api/v1/risk/denials"] }),
    true,
    FIELD_EVIDENCE_HINTS.riskDenials,
  );

  assert.equal(authority, "runtime_memory");
});

test("reconcile mismatches field is mixed because detail rows are derived from runtime plus broker snapshots", () => {
  const authority = classifyFieldSource(
    baseDataSource({ realEndpoints: ["/api/v1/reconcile/mismatches"] }),
    true,
    FIELD_EVIDENCE_HINTS.reconcileMismatches,
  );

  assert.equal(authority, "mixed");
});

test("runtime leadership field is mixed because the route blends runtime and durable evidence", () => {
  const authority = classifyFieldSource(
    baseDataSource({ realEndpoints: ["/api/v1/system/runtime-leadership"] }),
    true,
    FIELD_EVIDENCE_HINTS.runtimeLeadership,
  );

  assert.equal(authority, "mixed");
});
