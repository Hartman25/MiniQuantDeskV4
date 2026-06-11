// core-rs/mqk-gui/src/features/system/haltSummary.test.ts
//
// GUI-HALT-CAUSE-TIMELINE-SURFACE-01: proof tests for deriveLatestHaltSummary.

import test from "node:test";
import assert from "node:assert/strict";
import type { FeedEvent, SystemModel } from "./types.ts";
import { deriveLatestHaltSummary, selectHaltEvents } from "./haltSummary.ts";

type MinimalModel = Pick<SystemModel, "connected" | "status" | "feed" | "autonomousPaperStatus">;

function buildModel(overrides: Partial<MinimalModel> = {}): SystemModel {
  const base: MinimalModel = {
    connected: true,
    status: {
      runtime_status: "running",
      kill_switch_active: false,
      integrity_halt_active: false,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [],
    autonomousPaperStatus: {
      reconcile_status: "ok",
      next_operator_action: null,
    } as SystemModel["autonomousPaperStatus"],
  };

  return { ...base, ...overrides } as SystemModel;
}

function haltEvent(overrides: Partial<FeedEvent> = {}): FeedEvent {
  return {
    id: "audit_events:evt-1",
    at: "2026-06-09T20:00:00Z",
    severity: "info",
    source: "orchestrator_halt",
    text: "IntegrityViolation",
    run_id: "run-1",
    audit_event_id: "evt-1",
    ...overrides,
  };
}

test("deriveLatestHaltSummary: active integrity halt with matching orchestrator_halt event", () => {
  const model = buildModel({
    status: {
      runtime_status: "halted",
      kill_switch_active: false,
      integrity_halt_active: true,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [haltEvent({ text: "IntegrityViolation", run_id: "run-1", audit_event_id: "evt-1" })],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "active_halt");
  assert.equal(summary.reason, "IntegrityViolation");
  assert.equal(summary.run_id, "run-1");
  assert.equal(summary.audit_event_id, "evt-1");
  assert.equal(summary.integrity_halt_active, true);
  assert.equal(summary.severity, "critical");
});

test("deriveLatestHaltSummary: kill switch active but no orchestrator_halt event row", () => {
  const model = buildModel({
    status: {
      runtime_status: "halted",
      kill_switch_active: true,
      integrity_halt_active: false,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "active_halt");
  assert.equal(summary.reason, "unknown");
  assert.equal(summary.run_id, null);
  assert.equal(summary.audit_event_id, null);
  assert.equal(summary.kill_switch_active, true);
  assert.equal(summary.severity, "critical");
});

test("deriveLatestHaltSummary: recent halt event but current system not halted", () => {
  const model = buildModel({
    status: {
      runtime_status: "running",
      kill_switch_active: false,
      integrity_halt_active: false,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [haltEvent({ text: "ReconcileDrift", run_id: "run-1", audit_event_id: "evt-2" })],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "recent_halt");
  assert.equal(summary.reason, "ReconcileDrift");
  assert.equal(summary.run_id, "run-1");
  assert.equal(summary.audit_event_id, "evt-2");
  assert.equal(summary.severity, "warning");
});

test("deriveLatestHaltSummary: no halt — quiet system reports no_halt with info severity", () => {
  const model = buildModel();

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "no_halt");
  assert.equal(summary.reason, null);
  assert.equal(summary.run_id, null);
  assert.equal(summary.audit_event_id, null);
  assert.equal(summary.kill_switch_active, false);
  assert.equal(summary.integrity_halt_active, false);
  assert.equal(summary.risk_halt_active, false);
  assert.equal(summary.severity, "info");
});

test("deriveLatestHaltSummary: live_routing_enabled=true forces critical/anomaly tone even with no_halt", () => {
  const model = buildModel({
    status: {
      runtime_status: "running",
      kill_switch_active: false,
      integrity_halt_active: false,
      risk_halt_active: false,
      live_routing_enabled: true,
      deadman_status: "ok",
    } as SystemModel["status"],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "no_halt");
  assert.equal(summary.live_routing_enabled, true);
  assert.equal(summary.severity, "critical");
});

test("deriveLatestHaltSummary: dirty reconcile is shown as unsafe (warning) even without a halt", () => {
  const model = buildModel({
    autonomousPaperStatus: {
      reconcile_status: "dirty",
      next_operator_action: "Reconcile is 'dirty'. Review /api/v1/reconcile/mismatches and resolve before start",
    } as SystemModel["autonomousPaperStatus"],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "no_halt");
  assert.equal(summary.reconcile_status, "dirty");
  assert.equal(summary.severity, "warning");
  assert.equal(
    summary.next_operator_action,
    "Reconcile is 'dirty'. Review /api/v1/reconcile/mismatches and resolve before start",
  );
});

test("deriveLatestHaltSummary: missing run_id/audit_event_id on the halt event degrade safely to null", () => {
  const model = buildModel({
    status: {
      runtime_status: "halted",
      kill_switch_active: false,
      integrity_halt_active: true,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [haltEvent({ text: "RecoveryQuarantine", run_id: null, audit_event_id: null })],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "active_halt");
  assert.equal(summary.reason, "RecoveryQuarantine");
  assert.equal(summary.run_id, null);
  assert.equal(summary.audit_event_id, null);
  assert.equal(summary.severity, "critical");
});

test("deriveLatestHaltSummary: disconnected model reports unknown, not a fake no_halt", () => {
  const model = buildModel({ connected: false });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "unknown");
  assert.equal(summary.reason, null);
  assert.equal(summary.kill_switch_active, null);
  assert.equal(summary.live_routing_enabled, null);
});

test("deriveLatestHaltSummary: multiple orchestrator_halt rows pick the newest by timestamp", () => {
  const model = buildModel({
    status: {
      runtime_status: "running",
      kill_switch_active: false,
      integrity_halt_active: false,
      risk_halt_active: false,
      live_routing_enabled: false,
      deadman_status: "ok",
    } as SystemModel["status"],
    feed: [
      haltEvent({ id: "a", at: "2026-06-09T18:00:00Z", text: "LeaderLeaseLost", run_id: "run-old", audit_event_id: "evt-old" }),
      haltEvent({ id: "b", at: "2026-06-09T20:00:00Z", text: "ReconcileDrift", run_id: "run-new", audit_event_id: "evt-new" }),
    ],
  });

  const summary = deriveLatestHaltSummary(model);
  assert.equal(summary.status, "recent_halt");
  assert.equal(summary.reason, "ReconcileDrift");
  assert.equal(summary.run_id, "run-new");
  assert.equal(summary.audit_event_id, "evt-new");
});

// ---------------------------------------------------------------------------
// GUI-ALERTS-HALT-HISTORY-FILTER-SURFACE-01: selectHaltEvents
// ---------------------------------------------------------------------------

test("selectHaltEvents: filters only orchestrator_halt events", () => {
  const feed: FeedEvent[] = [
    haltEvent({ id: "a", at: "2026-06-09T18:00:00Z", source: "reconcile", text: "Order drift" }),
    haltEvent({ id: "b", at: "2026-06-09T19:00:00Z", text: "IntegrityViolation" }),
    haltEvent({ id: "c", at: "2026-06-09T20:00:00Z", source: "operator_action", text: "Operator armed run" }),
  ];

  const halts = selectHaltEvents(feed);
  assert.equal(halts.length, 1);
  assert.equal(halts[0].id, "b");
  assert.equal(halts[0].source, "orchestrator_halt");
});

test("selectHaltEvents: preserves run_id and audit_event_id", () => {
  const feed: FeedEvent[] = [
    haltEvent({ id: "a", at: "2026-06-09T18:00:00Z", text: "ReconcileDrift", run_id: "run-1", audit_event_id: "evt-1" }),
  ];

  const halts = selectHaltEvents(feed);
  assert.equal(halts.length, 1);
  assert.equal(halts[0].run_id, "run-1");
  assert.equal(halts[0].audit_event_id, "evt-1");
});

test("selectHaltEvents: newest-first ordering", () => {
  const feed: FeedEvent[] = [
    haltEvent({ id: "old", at: "2026-06-09T18:00:00Z", text: "LeaderLeaseLost" }),
    haltEvent({ id: "new", at: "2026-06-09T20:00:00Z", text: "ReconcileDrift" }),
    haltEvent({ id: "mid", at: "2026-06-09T19:00:00Z", text: "IntegrityViolation" }),
  ];

  const halts = selectHaltEvents(feed);
  assert.deepEqual(halts.map((e) => e.id), ["new", "mid", "old"]);
});

test("selectHaltEvents: empty feed returns empty array", () => {
  assert.deepEqual(selectHaltEvents([]), []);
  assert.deepEqual(selectHaltEvents(null), []);
  assert.deepEqual(selectHaltEvents(undefined), []);
});

test("selectHaltEvents: missing fields degrade safely", () => {
  const feed: FeedEvent[] = [
    haltEvent({ id: "no-ts", at: "", text: "RecoveryQuarantine", run_id: null, audit_event_id: null }),
    haltEvent({ id: "valid", at: "2026-06-09T20:00:00Z", text: "ReconcileDrift", run_id: "run-1", audit_event_id: "evt-1" }),
  ];

  const halts = selectHaltEvents(feed);
  assert.equal(halts.length, 2);
  assert.ok(halts.some((e) => e.id === "no-ts" && e.run_id === null && e.audit_event_id === null));
  // Unparseable timestamp sorts as oldest (0), so the valid event comes first.
  assert.equal(halts[0].id, "valid");
});

test("selectHaltEvents: respects the limit parameter", () => {
  const feed: FeedEvent[] = Array.from({ length: 12 }, (_, i) =>
    haltEvent({ id: `evt-${i}`, at: `2026-06-09T${String(i).padStart(2, "0")}:00:00Z`, text: "ReconcileDrift" }),
  );

  const halts = selectHaltEvents(feed);
  assert.equal(halts.length, 10);
  assert.equal(halts[0].id, "evt-11");
});
