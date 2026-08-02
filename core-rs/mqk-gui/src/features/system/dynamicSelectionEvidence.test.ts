// core-rs/mqk-gui/src/features/system/dynamicSelectionEvidence.test.ts
//
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 5: proof tests for the
// dynamic-selection evidence runtime validators/fail-closed sentinels.

import test from "node:test";
import assert from "node:assert/strict";
import {
  parseDynamicSelectionPlanDetail,
  parseDynamicSelectionPlansList,
  parseDynamicSelectionStatus,
} from "./dynamicSelectionEvidence.ts";

function validStatusRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    lifecycle: "running",
    active_run_id: "run-1",
    committed_configured_mode: "paper_enforced",
    committed_effective_mode: "paper_enforced",
    committed_live_lock_applied: false,
    approved_for_live: false,
    disposition: "paper_enforced_allowed",
    committed_plan_id: "plan-1",
    committed_source_kind: "watchlist_v2",
    committed_source_identity: "watchlist",
    committed_plan_truth_state: "computed",
    evidence_persisted: true,
    evidence_validation_state: "valid",
    evidence_validation_detail: null,
    selected: [],
    refused: [],
    host_pool_present: true,
    host_pool_count: 1,
    validation_blockers: [],
    preview_configured_mode: "paper_enforced",
    preview_effective_mode: "paper_enforced",
    preview_live_lock_applied: false,
    checked_at_utc: "2099-05-01T13:05:00Z",
    ...overrides,
  };
}

function validSymbolRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    symbol: "AAPL",
    selected_strategy_id: "swing_momentum",
    disposition: "selected",
    reason_code: "selected_highest_score",
    exact_reason_code: "selected_highest_score",
    ...overrides,
  };
}

function validCandidateRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    symbol: "AAPL",
    strategy_id: "swing_momentum",
    timeframe_secs: 300,
    selected: true,
    disposition: "selected",
    reason_code: "selected_highest_score",
    exact_reason_code: "selected_highest_score",
    canonical_score_decimal: "1.250000",
    scanner_rank: 1,
    ...overrides,
  };
}

function validPlanSummaryRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plan_id: "plan-1",
    run_id: "run-1",
    disposition: "paper_enforced_allowed",
    effective_mode: "paper_enforced",
    truth_state: "computed",
    selected_count: 1,
    candidate_count: 1,
    created_at_utc: "2099-05-01T13:00:00Z",
    validation_state: "valid",
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// parseDynamicSelectionStatus
// ---------------------------------------------------------------------------

test("parseDynamicSelectionStatus accepts a valid committed response", () => {
  const parsed = parseDynamicSelectionStatus(validStatusRaw());
  assert.equal(parsed.lifecycle, "running");
  assert.equal(parsed.evidence_validation_state, "valid");
  assert.equal(parsed.approved_for_live, false);
});

test("parseDynamicSelectionStatus rejects a non-object body", () => {
  const parsed = parseDynamicSelectionStatus(null);
  assert.equal(parsed.lifecycle, "idle");
  assert.equal(parsed.evidence_validation_state, null);
});

test("parseDynamicSelectionStatus rejects approved_for_live=true (hard invariant)", () => {
  const parsed = parseDynamicSelectionStatus(validStatusRaw({ approved_for_live: true }));
  assert.equal(parsed.approved_for_live, false);
  assert.equal(parsed.lifecycle, "idle");
});

test("parseDynamicSelectionStatus rejects an unrecognized lifecycle value", () => {
  const parsed = parseDynamicSelectionStatus(validStatusRaw({ lifecycle: "bogus" }));
  assert.equal(parsed.lifecycle, "idle");
});

test("parseDynamicSelectionStatus rejects an unrecognized evidence_validation_state", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({ evidence_validation_state: "totally_bogus" }),
  );
  assert.equal(parsed.lifecycle, "idle");
});

test("parseDynamicSelectionStatus accepts a fully neutral no-commitment response", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({
      committed_configured_mode: null,
      committed_effective_mode: null,
      disposition: null,
      committed_plan_id: null,
      committed_source_kind: null,
      committed_source_identity: null,
      committed_plan_truth_state: null,
      evidence_persisted: false,
      evidence_validation_state: null,
      host_pool_present: false,
      host_pool_count: 0,
    }),
  );
  assert.equal(parsed.committed_plan_id, null);
  assert.equal(parsed.disposition, null);
});

test("parseDynamicSelectionStatus rejects a partial commitment (plan_id null but disposition set)", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({ committed_plan_id: null, disposition: "paper_enforced_allowed" }),
  );
  assert.equal(parsed.lifecycle, "idle", "must fail closed on an internally-inconsistent response");
});

test("parseDynamicSelectionStatus surfaces validation_blockers truthfully", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({
      evidence_validation_state: "identity_mismatch",
      validation_blockers: ["stored plan_id=x recomputed plan_id=y"],
    }),
  );
  assert.equal(parsed.evidence_validation_state, "identity_mismatch");
  assert.equal(parsed.validation_blockers.length, 1);
});

test("parseDynamicSelectionStatus accepts a valid selected symbol row", () => {
  const parsed = parseDynamicSelectionStatus(validStatusRaw({ selected: [validSymbolRaw()] }));
  assert.equal(parsed.selected.length, 1);
  assert.equal(parsed.selected[0].symbol, "AAPL");
});

test("parseDynamicSelectionStatus rejects a selected symbol row with no selected_strategy_id", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({ selected: [validSymbolRaw({ selected_strategy_id: null })] }),
  );
  assert.equal(parsed.lifecycle, "idle");
});

test("parseDynamicSelectionStatus rejects a refused symbol row that carries a selected_strategy_id", () => {
  const parsed = parseDynamicSelectionStatus(
    validStatusRaw({
      refused: [validSymbolRaw({ disposition: "refused", selected_strategy_id: "swing_momentum" })],
    }),
  );
  assert.equal(parsed.lifecycle, "idle");
});

// ---------------------------------------------------------------------------
// parseDynamicSelectionPlansList
// ---------------------------------------------------------------------------

test("parseDynamicSelectionPlansList accepts a valid list", () => {
  const parsed = parseDynamicSelectionPlansList({
    run_id: "run-1",
    plans: [validPlanSummaryRaw()],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.plans.length, 1);
  assert.equal(parsed.plans[0].validation_state, "valid");
});

test("parseDynamicSelectionPlansList rejects a non-object body", () => {
  const parsed = parseDynamicSelectionPlansList(undefined);
  assert.equal(parsed.plans.length, 0);
  assert.equal(parsed.run_id, null);
});

test("parseDynamicSelectionPlansList rejects a plan row with an unrecognized disposition", () => {
  const parsed = parseDynamicSelectionPlansList({
    run_id: "run-1",
    plans: [validPlanSummaryRaw({ disposition: "bogus" })],
    checked_at_utc: "2099-05-01T13:05:00Z",
  });
  assert.equal(parsed.plans.length, 0);
});

// ---------------------------------------------------------------------------
// parseDynamicSelectionPlanDetail
// ---------------------------------------------------------------------------

function validPlanDetailRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    found: true,
    plan_id: "plan-1",
    run_id: "run-1",
    disposition: "paper_enforced_allowed",
    effective_mode: "paper_enforced",
    configured_mode: "paper_enforced",
    live_lock_applied: false,
    approved_for_live: false,
    truth_state: "computed",
    blockers: [],
    created_at_utc: "2099-05-01T13:00:00Z",
    symbols: [validSymbolRaw()],
    candidates: [validCandidateRaw()],
    validation_state: "valid",
    validation_detail: null,
    checked_at_utc: "2099-05-01T13:05:00Z",
    ...overrides,
  };
}

test("parseDynamicSelectionPlanDetail accepts a valid found response", () => {
  const parsed = parseDynamicSelectionPlanDetail(validPlanDetailRaw(), "plan-1");
  assert.equal(parsed.found, true);
  assert.equal(parsed.candidates.length, 1);
  assert.equal(parsed.validation_state, "valid");
});

test("parseDynamicSelectionPlanDetail rejects a non-object body and preserves the requested plan_id", () => {
  const parsed = parseDynamicSelectionPlanDetail(null, "plan-unknown");
  assert.equal(parsed.found, false);
  assert.equal(parsed.plan_id, "plan-unknown");
  assert.equal(parsed.validation_state, "missing");
});

test("parseDynamicSelectionPlanDetail accepts a genuine not-found response", () => {
  const parsed = parseDynamicSelectionPlanDetail(
    {
      found: false,
      plan_id: "plan-unknown",
      run_id: null,
      disposition: null,
      effective_mode: null,
      configured_mode: null,
      live_lock_applied: null,
      approved_for_live: null,
      truth_state: null,
      blockers: [],
      created_at_utc: null,
      symbols: [],
      candidates: [],
      validation_state: "missing",
      validation_detail: null,
      checked_at_utc: "2099-05-01T13:05:00Z",
    },
    "plan-unknown",
  );
  assert.equal(parsed.found, false);
  assert.equal(parsed.validation_state, "missing");
});

test("parseDynamicSelectionPlanDetail rejects found=false carrying real candidates", () => {
  const parsed = parseDynamicSelectionPlanDetail(
    validPlanDetailRaw({ found: false }),
    "plan-1",
  );
  assert.equal(parsed.found, false, "must fail closed to the unavailable sentinel");
  assert.equal(parsed.candidates.length, 0);
});

test("parseDynamicSelectionPlanDetail rejects approved_for_live=true (hard invariant)", () => {
  const parsed = parseDynamicSelectionPlanDetail(
    validPlanDetailRaw({ approved_for_live: true }),
    "plan-1",
  );
  assert.equal(parsed.found, false);
});

test("parseDynamicSelectionPlanDetail rejects an unrecognized validation_state", () => {
  const parsed = parseDynamicSelectionPlanDetail(
    validPlanDetailRaw({ validation_state: "bogus" }),
    "plan-1",
  );
  assert.equal(parsed.found, false);
});

test("parseDynamicSelectionPlanDetail rejects a candidate row with a non-finite timeframe_secs", () => {
  const parsed = parseDynamicSelectionPlanDetail(
    validPlanDetailRaw({ candidates: [validCandidateRaw({ timeframe_secs: Number.NaN })] }),
    "plan-1",
  );
  assert.equal(parsed.found, false);
});
