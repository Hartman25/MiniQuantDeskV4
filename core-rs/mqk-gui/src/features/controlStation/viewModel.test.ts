// core-rs/mqk-gui/src/features/controlStation/viewModel.test.ts
//
// GUI-CS-01A proof: focused shape tests plus the three mission-required
// negative controls for the Control Station truth/view model.

import test from "node:test";
import assert from "node:assert/strict";
import { MOCK_MODEL } from "../system/mockData";
import type { SystemModel } from "../system/types";
import { buildControlStationViewModel } from "./viewModel";

function baseModel(): SystemModel {
  return structuredClone(MOCK_MODEL);
}

// ---------------------------------------------------------------------------
// Negative control 1: daemon online + runtime idle remain distinct.
// ---------------------------------------------------------------------------

test("negative control: daemon online and runtime idle are reported independently, never collapsed", () => {
  const model = baseModel();
  model.connected = true;
  model.status.daemon_reachable = true;
  model.status.runtime_status = "idle";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.daemonOnline, true);
  assert.equal(vm.tradingDomain.runtimeStatus, "idle");
});

test("negative control: daemon offline still reports the actual runtime_status value, not a fabricated one", () => {
  const model = baseModel();
  model.connected = true;
  model.status.daemon_reachable = false;
  model.status.runtime_status = "running";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.daemonOnline, false);
  assert.equal(vm.tradingDomain.runtimeStatus, "running");
});

// ---------------------------------------------------------------------------
// Negative control 2: broker position present + no active session cannot
// collapse to position_count=0.
// ---------------------------------------------------------------------------

test("negative control: a broker position with zero active-session orders does not collapse to position_count=0", () => {
  const model = baseModel();
  model.positions = [
    { symbol: "AAPL", strategy_id: undefined, qty: 10, avg_price: 150, broker_qty: 10 },
  ];
  model.executionSummary = {
    active_orders: 0,
    pending_orders: 0,
    dispatching_orders: 0,
    reject_count_today: 0,
    cancel_replace_count_today: 0,
    avg_ack_latency_ms: null,
    stuck_orders: 0,
  };
  model.status.runtime_status = "idle";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.portfolio.brokerPositionCount, 1);
  assert.equal(vm.portfolio.activeSessionOrderCount, 0);
});

test("negative control: broker portfolio and active-session execution truth are independently sourced", () => {
  const model = baseModel();
  model.positions = [{ symbol: "MSFT", qty: 5, avg_price: 300, broker_qty: 5 }];
  model.openOrders = [];
  model.executionSummary.active_orders = 3;

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.portfolio.brokerPositionCount, 1);
  assert.equal(vm.portfolio.activeSessionOrderCount, 3);
});

// ---------------------------------------------------------------------------
// Negative control 3: unknown/degraded reconcile/system truth cannot render
// healthy.
// ---------------------------------------------------------------------------

test("negative control: reconcile_status unknown never yields a good system tone", () => {
  const model = baseModel();
  model.status.reconcile_status = "unknown";
  model.status.kill_switch_active = false;
  model.status.integrity_halt_active = false;
  model.status.risk_halt_active = false;

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
});

test("negative control: disconnected reconcile_status never yields a good system tone", () => {
  const model = baseModel();
  model.status.reconcile_status = "disconnected";

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
});

test("negative control: degraded runtime_status never yields a good trading-domain tone", () => {
  const model = baseModel();
  model.status.runtime_status = "degraded";
  model.status.live_routing_enabled = false;

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.tradingDomain.tone, "good");
});

test("negative control: a fully disconnected model never yields a good system tone", () => {
  const model = baseModel();
  model.connected = false;
  model.status.daemon_reachable = false;

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.daemonOnline, false);
});

// ---------------------------------------------------------------------------
// System tone escalation
// ---------------------------------------------------------------------------

test("kill switch active forces a bad system tone regardless of other health fields", () => {
  const model = baseModel();
  model.status.db_status = "ok";
  model.status.broker_status = "ok";
  model.status.market_data_health = "ok";
  model.status.reconcile_status = "ok";
  model.status.integrity_status = "ok";
  model.status.kill_switch_active = true;

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "bad");
});

test("all-ok health fields with no halts yields a good system tone", () => {
  const model = baseModel();
  model.connected = true;
  model.status.daemon_reachable = true;
  model.status.db_status = "ok";
  model.status.broker_status = "ok";
  model.status.market_data_health = "ok";
  model.status.reconcile_status = "ok";
  model.status.integrity_status = "ok";
  model.status.kill_switch_active = false;
  model.status.integrity_halt_active = false;
  model.status.risk_halt_active = false;
  // GUI-CS-01D: MOCK_MODEL's defaults (has_warning=true, deadman_status="ok",
  // a non-production string) are not an honest "all healthy" state under the
  // fuller tone derivation — pin them explicitly rather than relying on
  // incidental mock defaults.
  model.status.has_warning = false;
  model.status.has_critical = false;
  model.status.broker_snapshot_source = "synthetic";
  model.status.alpaca_ws_continuity = "not_applicable";
  model.status.runtime_status = "idle";
  model.status.deadman_status = "inactive";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
});

// ---------------------------------------------------------------------------
// GUI-CS-01D: fail-closed system-health headline.
//
// buildSystemSection() previously derived tone from only db/broker/
// market_data/reconcile/integrity health plus kill-switch/halt flags — it
// ignored alpaca_ws_continuity, deadman_status, and status.has_warning/
// has_critical entirely, and market_data_health's real daemon values
// ("not_configured" | "signal_ingestion_ready") fell through the generic
// HealthState switch as neither good nor bad. Each "otherwise healthy"
// fixture below pins every other field to its cleanest state so only the
// field under test can be responsible for the resulting tone.
// ---------------------------------------------------------------------------

function otherwiseHealthyModel(): SystemModel {
  const model = baseModel();
  model.connected = true;
  model.status.daemon_reachable = true;
  model.status.runtime_status = "idle";
  model.status.db_status = "ok";
  model.status.broker_status = "ok";
  model.status.market_data_health = "ok";
  model.status.reconcile_status = "ok";
  model.status.integrity_status = "ok";
  model.status.kill_switch_active = false;
  model.status.integrity_halt_active = false;
  model.status.risk_halt_active = false;
  model.status.has_warning = false;
  model.status.has_critical = false;
  model.status.broker_snapshot_source = "synthetic";
  model.status.alpaca_ws_continuity = "not_applicable";
  model.status.deadman_status = "inactive";
  return model;
}

test("all-ok otherwise-healthy fixture yields a good system tone (baseline sanity)", () => {
  const vm = buildControlStationViewModel(otherwiseHealthyModel());
  assert.equal(vm.system.tone, "good");
});

// --- A/B/C: external WS continuity gate --------------------------------

test("negative control: external broker with WS gap_detected never yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.broker_snapshot_source = "external";
  model.status.alpaca_ws_continuity = "gap_detected";

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.tone, "bad");
  assert.equal(vm.system.wsTone, "bad");
});

test("negative control: external broker with WS cold_start_unproven never yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.broker_snapshot_source = "external";
  model.status.alpaca_ws_continuity = "cold_start_unproven";

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.wsTone, "warn");
});

test("positive control: external broker with WS live and otherwise healthy yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.broker_snapshot_source = "external";
  model.status.alpaca_ws_continuity = "live";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.wsTone, "good");
});

test("positive control: synthetic broker with WS not_applicable and otherwise healthy yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.broker_snapshot_source = "synthetic";
  model.status.alpaca_ws_continuity = "not_applicable";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.wsTone, "good");
});

// --- E/F/G: deadman watchdog gate ---------------------------------------

test("negative control: deadman expired never yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.runtime_status = "running";
  model.status.deadman_status = "expired";

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.deadmanTone, "bad");
});

test("negative control: running runtime with non-healthy deadman never silently yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.runtime_status = "running";
  model.status.deadman_status = "inactive";

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.deadmanTone, "unknown");
});

test("positive control: idle runtime with deadman inactive is legitimate and yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.runtime_status = "idle";
  model.status.deadman_status = "inactive";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.deadmanTone, "good");
});

test("positive control: running runtime with deadman healthy yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.runtime_status = "running";
  model.status.deadman_status = "healthy";

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.deadmanTone, "good");
});

// --- H/I: aggregate backend warning/critical -----------------------------

test("negative control: has_warning=true never yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.has_warning = true;

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
});

test("negative control: has_critical=true yields a bad system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.has_critical = true;

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "bad");
});

// --- J/K: unrecognized health strings must fail closed -------------------

test("negative control: an unrecognized market_data_health string never yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  // Simulates an arbitrary/unexpected daemon string arriving over JSON —
  // TypeScript's HealthState typing cannot prevent this at runtime.
  model.status.market_data_health = "totally_unrecognized_value" as SystemModel["status"]["market_data_health"];

  const vm = buildControlStationViewModel(model);

  assert.notEqual(vm.system.tone, "good");
  assert.equal(vm.system.marketDataTone, "unknown");
});

test("positive control: market_data_health=signal_ingestion_ready on an otherwise-healthy Paper+Alpaca state yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.market_data_health = "signal_ingestion_ready" as SystemModel["status"]["market_data_health"];

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.marketDataTone, "good");
});

test("positive control: market_data_health=not_configured on an otherwise-healthy state yields a good system tone", () => {
  const model = otherwiseHealthyModel();
  model.status.market_data_health = "not_configured" as SystemModel["status"]["market_data_health"];

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.system.tone, "good");
  assert.equal(vm.system.marketDataTone, "good");
});

// ---------------------------------------------------------------------------
// M1 validation: always explicit not_wired, never a hardcoded count.
// ---------------------------------------------------------------------------

test("M1 validation section is always not_wired with a non-empty reason", () => {
  const vm = buildControlStationViewModel(baseModel());

  assert.equal(vm.m1Validation.state, "not_wired");
  assert.ok(vm.m1Validation.reason.length > 0);
});

// ---------------------------------------------------------------------------
// Incidents
// ---------------------------------------------------------------------------

test("incidents section counts only open/investigating incidents as open", () => {
  const model = baseModel();
  model.incidents = [
    { incident_id: "I-1", severity: "critical", title: "a", status: "open", opened_at: "t", updated_at: "t", impacted_orders: [], impacted_strategies: [], impacted_subsystems: [], alerts: [], reconcile_case_ids: [], operator_actions_taken: [], final_disposition: "" },
    { incident_id: "I-2", severity: "warning", title: "b", status: "investigating", opened_at: "t", updated_at: "t", impacted_orders: [], impacted_strategies: [], impacted_subsystems: [], alerts: [], reconcile_case_ids: [], operator_actions_taken: [], final_disposition: "" },
    { incident_id: "I-3", severity: "info", title: "c", status: "resolved", opened_at: "t", updated_at: "t", impacted_orders: [], impacted_strategies: [], impacted_subsystems: [], alerts: [], reconcile_case_ids: [], operator_actions_taken: [], final_disposition: "" },
  ];

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.incidents.totalIncidentCount, 3);
  assert.equal(vm.incidents.openIncidentCount, 2);
});

// ---------------------------------------------------------------------------
// Autonomy
// ---------------------------------------------------------------------------

test("autonomy section reports not-applicable when preflight says autonomous readiness is not applicable", () => {
  const model = baseModel();
  model.preflight.autonomous_readiness_applicable = false;

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.autonomy.applicable, false);
});

test("autonomy section mirrors daily-operation transport/truth state verbatim without fabricating a value", () => {
  const model = baseModel();
  model.autonomousDailyOperation = {
    transport_state: "endpoint_unavailable",
    canonical_route: null,
    truth_state: null,
    operation: null,
    message: null,
  };

  const vm = buildControlStationViewModel(model);

  assert.equal(vm.autonomy.dailyOperationTransportState, "endpoint_unavailable");
  assert.equal(vm.autonomy.dailyOperationTruthState, null);
  assert.equal(vm.autonomy.dailyOperationFinalizationStatus, null);
});
