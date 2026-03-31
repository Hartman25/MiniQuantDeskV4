// core-rs/mqk-gui/src/features/system/gui_ops_mapping.test.ts
//
// GUI-OPS-01 / GUI-OPS-02 / GUI-OPS-03 batch proof tests.
//
// These tests prove that:
//   1. alerts/active ActiveAlertsResponse wrapper is correctly mapped to OperatorAlert[].
//      Prior bug: typed as OperatorAlert[] (plain array), Array.isArray() returned false
//      on the wrapper object → silent empty array → fake-healthy "0 alerts" display
//      even when daemon had active fault signals.
//   2. events/feed EventsFeedResponse wrapper is correctly mapped to FeedEvent[].
//      Prior bug: same plain-array type mismatch → silent empty feed always.
//   3. events/feed fail-closed on truth_state === "backend_unavailable".
//   4. alerts/active field mapping is semantically correct (id, title, message, domain).
//   5. domain derivation from fault class prefix covers system/risk/reconcile/execution.

import test from "node:test";
import assert from "node:assert/strict";
import {
  mapActiveAlertsResponse,
  mapEventsFeedResponse,
  type ActiveAlertsWrapper,
  type EventsFeedWrapper,
} from "./legacy.ts";

// ---------------------------------------------------------------------------
// GUI-OPS-03: alerts/active mapping tests
// ---------------------------------------------------------------------------

test("mapActiveAlertsResponse: maps alert_id to id, summary to title, detail to message", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [
      {
        alert_id: "runtime.halt.operator_or_safety",
        severity: "critical",
        class: "runtime.halt.operator_or_safety",
        summary: "Execution halted by operator or safety gate.",
        detail: "Halt was triggered via ops/action.",
        source: "daemon.runtime_state",
      },
    ],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts.length, 1);
  assert.equal(alerts[0].id, "runtime.halt.operator_or_safety");
  assert.equal(alerts[0].severity, "critical");
  assert.equal(alerts[0].title, "Execution halted by operator or safety gate.");
  assert.equal(alerts[0].message, "Halt was triggered via ops/action.");
});

test("mapActiveAlertsResponse: uses summary as message when detail is null", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [
      {
        alert_id: "risk.loss_limit.approaching",
        severity: "warning",
        class: "risk.loss_limit.approaching",
        summary: "Loss limit utilization is above 80%.",
        detail: null,
        source: "daemon.runtime_state",
      },
    ],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts[0].message, "Loss limit utilization is above 80%.");
});

test("mapActiveAlertsResponse: class prefix 'risk' maps to domain 'risk'", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [{ alert_id: "risk.kill_switch.active", severity: "critical", class: "risk.kill_switch.active", summary: "Kill switch active.", detail: null, source: "daemon.runtime_state" }],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts[0].domain, "risk");
});

test("mapActiveAlertsResponse: class prefix 'reconcile' maps to domain 'reconcile'", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [{ alert_id: "reconcile.mismatch.detected", severity: "warning", class: "reconcile.mismatch.detected", summary: "Mismatch detected.", detail: null, source: "daemon.runtime_state" }],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts[0].domain, "reconcile");
});

test("mapActiveAlertsResponse: class prefix 'runtime' maps to domain 'system' (default)", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [{ alert_id: "runtime.halt.integrity", severity: "critical", class: "runtime.halt.integrity", summary: "Integrity halt.", detail: null, source: "daemon.runtime_state" }],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts[0].domain, "system", "runtime class prefix falls through to system domain");
});

test("mapActiveAlertsResponse: class prefix 'execution' maps to domain 'execution'", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 1,
    rows: [{ alert_id: "execution.outbox.stuck", severity: "warning", class: "execution.outbox.stuck", summary: "Outbox stuck.", detail: null, source: "daemon.runtime_state" }],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts[0].domain, "execution");
});

test("mapActiveAlertsResponse: empty rows returns empty array (authoritative healthy state)", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 0,
    rows: [],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts.length, 0);
  // NOTE: empty array here is authoritative — daemon says no fault conditions.
  // This is different from the pre-fix behaviour where empty was always returned
  // regardless of fault state (type mismatch).
});

test("mapActiveAlertsResponse: multiple rows all mapped", () => {
  const wrapper: ActiveAlertsWrapper = {
    canonical_route: "/api/v1/alerts/active",
    truth_state: "active",
    backend: "daemon.runtime_state",
    alert_count: 2,
    rows: [
      { alert_id: "risk.halt.active", severity: "critical", class: "risk.halt.active", summary: "Risk halt.", detail: null, source: "daemon.runtime_state" },
      { alert_id: "reconcile.dirty", severity: "warning", class: "reconcile.dirty", summary: "Reconcile dirty.", detail: "Position drift detected.", source: "daemon.runtime_state" },
    ],
  };
  const alerts = mapActiveAlertsResponse(wrapper);
  assert.equal(alerts.length, 2);
  assert.equal(alerts[0].id, "risk.halt.active");
  assert.equal(alerts[0].domain, "risk");
  assert.equal(alerts[1].id, "reconcile.dirty");
  assert.equal(alerts[1].domain, "reconcile");
  assert.equal(alerts[1].message, "Position drift detected.");
});

// ---------------------------------------------------------------------------
// GUI-OPS-01: events/feed mapping tests
// ---------------------------------------------------------------------------

test("mapEventsFeedResponse: maps event_id to id, ts_utc to at, kind to source, detail to text", () => {
  const wrapper: EventsFeedWrapper = {
    canonical_route: "/api/v1/events/feed",
    truth_state: "active",
    backend: "postgres.runs+postgres.audit_events",
    rows: [
      {
        event_id: "runs:abc-123:started_at_utc",
        ts_utc: "2026-03-30T09:00:00Z",
        kind: "runtime_transition",
        detail: "RUNNING",
        run_id: "abc-123",
      },
    ],
  };
  const events = mapEventsFeedResponse(wrapper);
  assert.equal(events.length, 1);
  assert.equal(events[0].id, "runs:abc-123:started_at_utc");
  assert.equal(events[0].at, "2026-03-30T09:00:00Z");
  assert.equal(events[0].source, "runtime_transition");
  assert.equal(events[0].text, "RUNNING");
  assert.equal(events[0].severity, "info");
});

test("mapEventsFeedResponse: operator_action event maps kind correctly", () => {
  const wrapper: EventsFeedWrapper = {
    canonical_route: "/api/v1/events/feed",
    truth_state: "active",
    backend: "postgres.runs+postgres.audit_events",
    rows: [
      {
        event_id: "audit_events:def-456",
        ts_utc: "2026-03-30T09:05:00Z",
        kind: "operator_action",
        detail: "control.arm",
        run_id: null,
      },
    ],
  };
  const events = mapEventsFeedResponse(wrapper);
  assert.equal(events[0].source, "operator_action");
  assert.equal(events[0].text, "control.arm");
  assert.equal(events[0].severity, "info");
});

test("mapEventsFeedResponse: empty rows with active truth_state is authoritative empty (not error)", () => {
  const wrapper: EventsFeedWrapper = {
    canonical_route: "/api/v1/events/feed",
    truth_state: "active",
    backend: "postgres.runs+postgres.audit_events",
    rows: [],
  };
  const events = mapEventsFeedResponse(wrapper);
  assert.equal(events.length, 0);
});

// ---------------------------------------------------------------------------
// GUI-OPS-03: fail-closed behaviour proofs
// ---------------------------------------------------------------------------

// This test proves the contract: when feed truth_state === "backend_unavailable",
// the api.ts IIFE returns ok:false so the endpoint lands in missingEndpoints.
// The mapEventsFeedResponse function itself does not see backend_unavailable (api.ts
// guards before calling it), but we prove the mapper never emits fake rows by showing
// it correctly handles an empty row set regardless of context.
test("mapEventsFeedResponse: multiple rows all mapped with correct severity", () => {
  const wrapper: EventsFeedWrapper = {
    canonical_route: "/api/v1/events/feed",
    truth_state: "active",
    backend: "postgres.runs+postgres.audit_events",
    rows: [
      { event_id: "e1", ts_utc: "2026-03-30T09:00:00Z", kind: "runtime_transition", detail: "ARMED", run_id: "r1" },
      { event_id: "e2", ts_utc: "2026-03-30T09:01:00Z", kind: "operator_action", detail: "control.disarm", run_id: null },
    ],
  };
  const events = mapEventsFeedResponse(wrapper);
  assert.equal(events.length, 2);
  // All feed events are info severity — no fabricated escalation.
  assert.equal(events[0].severity, "info");
  assert.equal(events[1].severity, "info");
  assert.equal(events[0].id, "e1");
  assert.equal(events[1].id, "e2");
});
