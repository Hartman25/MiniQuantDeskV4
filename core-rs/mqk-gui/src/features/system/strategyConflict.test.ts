// core-rs/mqk-gui/src/features/system/strategyConflict.test.ts
//
// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: proof tests for the
// conflict-policy runtime validators/fail-closed sentinels.

import test from "node:test";
import assert from "node:assert/strict";
import {
  parseConflictPlanDetail,
  parseConflictPlansList,
  parseConflictStatus,
} from "./strategyConflict.ts";

function validStatusRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    truth_state: "active",
    mode_configured: "shadow",
    mode_effective: "shadow",
    invalid_configuration: null,
    live_lock_applied: false,
    approved_for_live: false,
    current_runtime_limitation: "one strategy host across every configured symbol",
    run_id: "run-1",
    latest_plan_id: "plan-1",
    latest_plan_created_at_utc: "2099-05-01T13:00:00Z",
    latest_plan_symbol_group_count: 1,
    latest_plan_candidate_count: 2,
    latest_plan_selected_count: 1,
    checked_at_utc: "2099-05-01T13:05:00Z",
    ...overrides,
  };
}

function validCandidateRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    symbol: "AAPL",
    strategy_id: "strategy_a",
    timeframe_secs: 300,
    side: "sell",
    qty: 5,
    current_qty: 20,
    proposed_target_qty: 15,
    bar_end_ts: null,
    selected: true,
    disposition: "selected",
    reason_code: "risk_reducing_candidate_selected",
    ...overrides,
  };
}

function validPlanRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plan_id: "plan-1",
    cycle_id: "plan-1",
    run_id: "run-1",
    mode: "shadow",
    market_date: "2099-05-01",
    policy_schema_version: "multi-strategy-conflict-policy-v1",
    symbol_group_count: 1,
    candidate_count: 2,
    selected_count: 1,
    refused_count: 1,
    truth_state: "computed",
    blockers: [],
    created_at_utc: "2099-05-01T13:00:00Z",
    approved_for_live: false,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// parseConflictStatus
// ---------------------------------------------------------------------------

test("parseConflictStatus accepts a valid active response", () => {
  const parsed = parseConflictStatus(validStatusRaw());
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.mode_effective, "shadow");
  assert.equal(parsed.approved_for_live, false);
});

test("parseConflictStatus rejects a non-object body", () => {
  const parsed = parseConflictStatus(null);
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictStatus rejects approved_for_live=true (hard invariant)", () => {
  const parsed = parseConflictStatus(validStatusRaw({ approved_for_live: true }));
  assert.equal(parsed.truth_state, "db_unavailable");
  assert.equal(parsed.approved_for_live, false);
});

test("parseConflictStatus rejects an unrecognized truth_state", () => {
  const parsed = parseConflictStatus(validStatusRaw({ truth_state: "totally_bogus" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictStatus rejects an unrecognized mode", () => {
  const parsed = parseConflictStatus(validStatusRaw({ mode_effective: "enforced_hard" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictStatus surfaces invalid_configuration truthfully", () => {
  const parsed = parseConflictStatus(
    validStatusRaw({ truth_state: "invalid_configuration", invalid_configuration: "bogus_value" }),
  );
  assert.equal(parsed.truth_state, "invalid_configuration");
  assert.equal(parsed.invalid_configuration, "bogus_value");
});

test("parseConflictStatus preserves the current runtime limitation string", () => {
  const parsed = parseConflictStatus(validStatusRaw());
  assert.ok(parsed.current_runtime_limitation.includes("one strategy host"));
});

// ---------------------------------------------------------------------------
// parseConflictPlanDetail
// ---------------------------------------------------------------------------

test("parseConflictPlanDetail accepts a valid active plan with candidates", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "active",
    plan: validPlanRaw(),
    candidates: [validCandidateRaw(), validCandidateRaw({ selected: false, disposition: "not_selected", reason_code: "not_selected" })],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.plan?.plan_id, "plan-1");
  assert.equal(parsed.candidates.length, 2);
});

test("parseConflictPlanDetail rejects active truth_state with a null plan", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "active",
    plan: null,
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictPlanDetail rejects a non-active truth_state smuggling a plan", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "not_found",
    plan: validPlanRaw(),
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictPlanDetail rejects a plan with approved_for_live=true", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "active",
    plan: validPlanRaw({ approved_for_live: true }),
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictPlanDetail rejects a candidate with an unrecognized disposition", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "active",
    plan: validPlanRaw(),
    candidates: [validCandidateRaw({ disposition: "made_up" })],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictPlanDetail reports not_found truthfully", () => {
  const parsed = parseConflictPlanDetail({
    truth_state: "not_found",
    plan: null,
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "not_found");
});

// ---------------------------------------------------------------------------
// parseConflictPlansList
// ---------------------------------------------------------------------------

test("parseConflictPlansList accepts a valid active list", () => {
  const parsed = parseConflictPlansList({
    truth_state: "active",
    run_id: "run-1",
    plans: [validPlanRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.plans.length, 1);
});

test("parseConflictPlansList rejects a non-active truth_state with plans present", () => {
  const parsed = parseConflictPlansList({
    truth_state: "query_failed",
    run_id: "run-1",
    plans: [validPlanRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseConflictPlansList reports db_unavailable truthfully with empty plans", () => {
  const parsed = parseConflictPlansList({
    truth_state: "db_unavailable",
    run_id: null,
    plans: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
  assert.equal(parsed.plans.length, 0);
});
