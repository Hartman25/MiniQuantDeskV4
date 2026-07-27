// core-rs/mqk-gui/src/features/system/runtimeOpportunityAllocation.test.ts
//
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase H: proof tests for the allocation
// runtime validators/fail-closed sentinels.

import test from "node:test";
import assert from "node:assert/strict";
import {
  parseAllocationPlanDetail,
  parseAllocationPlansList,
  parseAllocationStatus,
} from "./runtimeOpportunityAllocation.ts";

function validStatusRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    truth_state: "active",
    mode_configured: "shadow",
    mode_effective: "shadow",
    invalid_configuration: null,
    live_lock_applied: false,
    approved_for_live: false,
    runtime_influence: "shadow",
    run_id: "run-1",
    latest_plan_id: "plan-1",
    latest_plan_created_at_utc: "2099-05-01T13:00:00Z",
    latest_plan_candidate_count: 2,
    latest_plan_allowed_count: 1,
    checked_at_utc: "2099-05-01T13:05:00Z",
    ...overrides,
  };
}

function validCandidateRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    symbol: "AAPL",
    strategy_id: "intraday_scalper",
    input_score: 0.9,
    target_weight: 0.2,
    current_qty: 0,
    strategy_target_qty: 10,
    allocation_target_qty: 10,
    final_target_qty: 10,
    disposition: "allowed",
    reason_code: "allocator_full_target_granted",
    evaluation_price_micros: 100_000_000,
    ...overrides,
  };
}

function validPlanRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plan_id: "plan-1",
    cycle_id: "plan-1",
    run_id: "run-1",
    mode: "shadow",
    opportunity_artifact_id: "artifact-1",
    source_snapshot_id: null,
    equity_micros: 100_000_000_000,
    candidate_count: 1,
    allowed_count: 1,
    gross_weight: 0.2,
    net_weight: 0.2,
    truth_state: "computed",
    blockers: [],
    created_at_utc: "2099-05-01T13:00:00Z",
    approved_for_live: false,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// parseAllocationStatus
// ---------------------------------------------------------------------------

test("parseAllocationStatus accepts a valid active response", () => {
  const parsed = parseAllocationStatus(validStatusRaw());
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.runtime_influence, "shadow");
  assert.equal(parsed.approved_for_live, false);
});

test("parseAllocationStatus rejects unrecognized truth_state", () => {
  const parsed = parseAllocationStatus(validStatusRaw({ truth_state: "made_up" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationStatus rejects approved_for_live=true (hard invariant)", () => {
  const parsed = parseAllocationStatus(validStatusRaw({ approved_for_live: true }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationStatus rejects mode/influence mismatch", () => {
  const parsed = parseAllocationStatus(
    validStatusRaw({ mode_effective: "off", runtime_influence: "paper_enforced" }),
  );
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationStatus rejects non-object body", () => {
  const parsed = parseAllocationStatus(null);
  assert.equal(parsed.truth_state, "db_unavailable");
  assert.equal(parsed.approved_for_live, false);
});

test("parseAllocationStatus rejects NaN/Infinity candidate counts", () => {
  const parsed = parseAllocationStatus(validStatusRaw({ latest_plan_candidate_count: Number.NaN }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationStatus accepts off/none with no plan", () => {
  const parsed = parseAllocationStatus(
    validStatusRaw({
      mode_configured: "off",
      mode_effective: "off",
      runtime_influence: "none",
      latest_plan_id: null,
      latest_plan_created_at_utc: null,
      latest_plan_candidate_count: null,
      latest_plan_allowed_count: null,
    }),
  );
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.latest_plan_id, null);
});

// ---------------------------------------------------------------------------
// parseAllocationPlanDetail
// ---------------------------------------------------------------------------

test("parseAllocationPlanDetail accepts a valid active plan with candidates", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "active",
    plan: validPlanRaw(),
    candidates: [validCandidateRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.plan?.plan_id, "plan-1");
  assert.equal(parsed.candidates.length, 1);
});

test("parseAllocationPlanDetail rejects active truth_state with null plan", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "active",
    plan: null,
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationPlanDetail rejects non-active truth_state smuggling a plan", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "not_found",
    plan: validPlanRaw(),
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationPlanDetail rejects unrecognized disposition", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "active",
    plan: validPlanRaw(),
    candidates: [validCandidateRaw({ disposition: "made_up" })],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationPlanDetail rejects plan with approved_for_live=true", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "active",
    plan: validPlanRaw({ approved_for_live: true }),
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationPlanDetail returns not_found sentinel unchanged", () => {
  const parsed = parseAllocationPlanDetail({
    truth_state: "not_found",
    plan: null,
    candidates: [],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "not_found");
});

// ---------------------------------------------------------------------------
// parseAllocationPlansList
// ---------------------------------------------------------------------------

test("parseAllocationPlansList accepts a valid list", () => {
  const parsed = parseAllocationPlansList({
    truth_state: "active",
    run_id: "run-1",
    plans: [validPlanRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.plans.length, 1);
});

test("parseAllocationPlansList rejects non-active truth_state smuggling plans", () => {
  const parsed = parseAllocationPlansList({
    truth_state: "query_failed",
    run_id: "run-1",
    plans: [validPlanRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseAllocationPlansList rejects malformed plan row", () => {
  const parsed = parseAllocationPlansList({
    truth_state: "active",
    run_id: "run-1",
    plans: [{ plan_id: "only-a-partial-row" }],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.truth_state, "db_unavailable");
});
