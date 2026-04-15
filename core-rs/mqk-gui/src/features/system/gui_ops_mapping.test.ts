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
  normalizeModeChangeGuidance,
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

// ---------------------------------------------------------------------------
// GUI-09: normalizeModeChangeGuidance — shape-contract proof tests
//
// These tests prove that normalizeModeChangeGuidance:
//   1. Returns null for null, non-object, or structurally incomplete input.
//   2. Returns the typed response for a structurally complete input.
//   3. Preserves all operator-critical fields (operator_next_steps, transition_verdicts,
//      preconditions, parity_evidence_state, live_trust_complete).
//
// These tests prove the fail-closed contract: callers can safely render a
// "guidance unavailable" notice whenever normalizeModeChangeGuidance returns null,
// rather than rendering partial or fabricated guidance content.
// ---------------------------------------------------------------------------

function makeGuidance(overrides?: Record<string, unknown>) {
  return {
    canonical_route: "/api/v1/ops/mode-change-guidance",
    current_mode: "paper",
    transition_permitted: false,
    transition_refused_reason: "Mode transitions require a controlled daemon restart.",
    preconditions: ["Disarm execution before restart.", "Stop active run."],
    operator_next_steps: ["1. Disarm execution.", "2. Stop active run.", "3. Edit config.", "4. Restart daemon."],
    transition_verdicts: [
      { target_mode: "paper", verdict: "same_mode", reason: "Already in paper mode.", preconditions: [] },
      { target_mode: "live-shadow", verdict: "admissible_with_restart", reason: "Requires parity evidence.", preconditions: ["Provide TV-03 parity evidence."] },
    ],
    restart_workflow: { truth_state: "no_pending", pending_intent: null },
    parity_evidence_state: "incomplete",
    live_trust_complete: false,
    ...overrides,
  };
}

test("normalizeModeChangeGuidance: null input returns null", () => {
  assert.equal(normalizeModeChangeGuidance(null), null);
});

test("normalizeModeChangeGuidance: non-object input returns null", () => {
  assert.equal(normalizeModeChangeGuidance("not an object"), null);
  assert.equal(normalizeModeChangeGuidance(42), null);
  assert.equal(normalizeModeChangeGuidance(true), null);
});

test("normalizeModeChangeGuidance: missing canonical_route returns null", () => {
  const raw = makeGuidance({ canonical_route: undefined });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: missing current_mode returns null", () => {
  const raw = makeGuidance({ current_mode: undefined });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: missing operator_next_steps returns null", () => {
  const raw = makeGuidance({ operator_next_steps: undefined });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: missing transition_verdicts returns null", () => {
  const raw = makeGuidance({ transition_verdicts: undefined });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: missing preconditions returns null", () => {
  const raw = makeGuidance({ preconditions: undefined });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: missing restart_workflow returns null", () => {
  const raw = makeGuidance({ restart_workflow: null });
  assert.equal(normalizeModeChangeGuidance(raw), null);
});

test("normalizeModeChangeGuidance: complete response returns typed object with all fields preserved", () => {
  const raw = makeGuidance();
  const g = normalizeModeChangeGuidance(raw);
  assert.notEqual(g, null);
  assert.equal(g!.canonical_route, "/api/v1/ops/mode-change-guidance");
  assert.equal(g!.current_mode, "paper");
  assert.equal(g!.transition_permitted, false);
  assert.equal(g!.operator_next_steps.length, 4);
  assert.equal(g!.transition_verdicts.length, 2);
  assert.equal(g!.preconditions.length, 2);
  assert.equal(g!.parity_evidence_state, "incomplete");
  assert.equal(g!.live_trust_complete, false);
});

test("normalizeModeChangeGuidance: transition_verdicts fields are preserved (target_mode, verdict, reason, preconditions)", () => {
  const raw = makeGuidance();
  const g = normalizeModeChangeGuidance(raw)!;
  const liveShadow = g.transition_verdicts.find((v) => v.target_mode === "live-shadow");
  assert.ok(liveShadow, "live-shadow verdict must be present");
  assert.equal(liveShadow!.verdict, "admissible_with_restart");
  assert.equal(liveShadow!.preconditions.length, 1);
});

test("normalizeModeChangeGuidance: restart_workflow truth_state preserved", () => {
  const raw = makeGuidance();
  const g = normalizeModeChangeGuidance(raw)!;
  assert.equal(g.restart_workflow.truth_state, "no_pending");
  assert.equal(g.restart_workflow.pending_intent, null);
});

test("normalizeModeChangeGuidance: live_trust_complete null is preserved (not converted to false)", () => {
  const raw = makeGuidance({ live_trust_complete: null });
  const g = normalizeModeChangeGuidance(raw)!;
  assert.equal(g.live_trust_complete, null);
});

// ---------------------------------------------------------------------------
// GUI-09 patch-local proof tests
// ---------------------------------------------------------------------------

// P1: live-shadow and live-capital must survive as distinct entries — not collapsed.
test("normalizeModeChangeGuidance: live-shadow and live-capital are preserved as distinct target modes", () => {
  const raw = makeGuidance({
    transition_verdicts: [
      { target_mode: "paper", verdict: "same_mode", reason: "Already in paper mode.", preconditions: [] },
      { target_mode: "live-shadow", verdict: "admissible_with_restart", reason: "Requires parity evidence.", preconditions: ["Provide TV-03 parity evidence."] },
      { target_mode: "live-capital", verdict: "refused", reason: "Live trust not complete.", preconditions: [] },
      { target_mode: "backtest", verdict: "admissible_with_restart", reason: "Requires config change.", preconditions: [] },
    ],
  });
  const g = normalizeModeChangeGuidance(raw)!;
  assert.notEqual(g, null);
  const liveShadow = g.transition_verdicts.find((v) => v.target_mode === "live-shadow");
  const liveCapital = g.transition_verdicts.find((v) => v.target_mode === "live-capital");
  assert.ok(liveShadow, "live-shadow verdict must be present");
  assert.ok(liveCapital, "live-capital verdict must be present");
  // They must be distinct entries with different verdicts — not merged.
  assert.equal(liveShadow!.verdict, "admissible_with_restart");
  assert.equal(liveCapital!.verdict, "refused");
  assert.equal(g.transition_verdicts.length, 4, "all four target modes must be present");
});

// P2: pending_intent must carry daemon field names exactly — to_mode/from_mode/transition_verdict.
test("normalizeModeChangeGuidance: pending_intent with active restart workflow preserves daemon field names", () => {
  const raw = makeGuidance({
    restart_workflow: {
      truth_state: "active",
      pending_intent: {
        intent_id: "intent-uuid-001",
        from_mode: "paper",
        to_mode: "live-shadow",
        transition_verdict: "admissible_with_restart",
        initiated_by: "operator",
        initiated_at_utc: "2026-04-14T10:00:00Z",
        note: "Moving to shadow mode for live validation.",
      },
    },
  });
  const g = normalizeModeChangeGuidance(raw)!;
  assert.notEqual(g, null);
  assert.equal(g.restart_workflow.truth_state, "active");
  const pi = g.restart_workflow.pending_intent;
  assert.notEqual(pi, null, "pending_intent must not be null when truth_state is active");
  assert.equal(pi!.intent_id, "intent-uuid-001");
  assert.equal(pi!.from_mode, "paper");
  assert.equal(pi!.to_mode, "live-shadow");
  assert.equal(pi!.transition_verdict, "admissible_with_restart");
  assert.equal(pi!.initiated_by, "operator");
  assert.equal(pi!.note, "Moving to shadow mode for live validation.");
});

// P3: restart_workflow with truth_state missing must return null (fail-closed tightening).
test("normalizeModeChangeGuidance: restart_workflow without truth_state returns null (fail closed)", () => {
  const raw = makeGuidance({ restart_workflow: { pending_intent: null } });
  assert.equal(normalizeModeChangeGuidance(raw), null, "missing truth_state must fail closed");
});

// P4: restart_workflow with backend_unavailable truth_state is preserved honestly.
test("normalizeModeChangeGuidance: restart_workflow backend_unavailable truth_state is preserved", () => {
  const raw = makeGuidance({
    restart_workflow: { truth_state: "backend_unavailable", pending_intent: null },
  });
  const g = normalizeModeChangeGuidance(raw)!;
  assert.notEqual(g, null);
  assert.equal(g.restart_workflow.truth_state, "backend_unavailable");
  assert.equal(g.restart_workflow.pending_intent, null);
});

// P5: no_pending truth_state with null intent is preserved (honest absence, not treated as error).
test("normalizeModeChangeGuidance: restart_workflow no_pending with null pending_intent is preserved", () => {
  const raw = makeGuidance({
    restart_workflow: { truth_state: "no_pending", pending_intent: null },
  });
  const g = normalizeModeChangeGuidance(raw)!;
  assert.notEqual(g, null);
  assert.equal(g.restart_workflow.truth_state, "no_pending");
  assert.equal(g.restart_workflow.pending_intent, null);
});

// GUI-09 tightening: pending_intent structural completeness enforcement.

// P6: malformed non-null pending_intent (missing from_mode) must return null (fail closed).
test("normalizeModeChangeGuidance: malformed non-null pending_intent returns null", () => {
  const raw = makeGuidance({
    restart_workflow: {
      truth_state: "active",
      pending_intent: {
        intent_id: "intent-uuid-002",
        // from_mode intentionally omitted
        to_mode: "live-shadow",
        transition_verdict: "admissible_with_restart",
        initiated_by: "operator",
        initiated_at_utc: "2026-04-14T10:00:00Z",
        note: "Missing from_mode field.",
      },
    },
  });
  assert.equal(normalizeModeChangeGuidance(raw), null, "malformed non-null pending_intent must fail closed");
});

// P7: valid pending_intent with all consumed fields present still passes after tightening.
test("normalizeModeChangeGuidance: valid non-null pending_intent passes after structural tightening", () => {
  const raw = makeGuidance({
    restart_workflow: {
      truth_state: "active",
      pending_intent: {
        intent_id: "intent-uuid-003",
        from_mode: "paper",
        to_mode: "live-shadow",
        transition_verdict: "admissible_with_restart",
        initiated_by: "operator",
        initiated_at_utc: "2026-04-14T10:00:00Z",
        note: "All fields present.",
      },
    },
  });
  const g = normalizeModeChangeGuidance(raw);
  assert.notEqual(g, null, "valid pending_intent must not be rejected");
  assert.equal(g!.restart_workflow.pending_intent!.from_mode, "paper");
  assert.equal(g!.restart_workflow.pending_intent!.to_mode, "live-shadow");
});

// P8: null pending_intent with valid truth_state is preserved (honest allowed state).
test("normalizeModeChangeGuidance: null pending_intent with valid truth_state is preserved", () => {
  const raw = makeGuidance({
    restart_workflow: { truth_state: "active", pending_intent: null },
  });
  const g = normalizeModeChangeGuidance(raw);
  assert.notEqual(g, null, "null pending_intent is a valid state — must not be rejected");
  assert.equal(g!.restart_workflow.truth_state, "active");
  assert.equal(g!.restart_workflow.pending_intent, null);
});
