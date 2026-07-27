// core-rs/mqk-gui/src/features/system/runtimeOpportunityAllocation.ts
//
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase H: fail-closed parsing for the
// read-only allocation truth routes (GET /api/v1/portfolio/allocation/*).
//
// Mirrors durablePortfolio.ts's contract-hardening pattern: the daemon
// already carries an explicit truth_state per response, but this module is
// the single seam that decides whether an HTTP 200 body is actually trusted
// as that truth_state, or downgraded to an unavailable sentinel. An
// unrecognized truth_state, a non-finite number, a null-where-required-
// present field, or an internally-inconsistent "active" response all fail
// closed here rather than being rendered as proven truth.

import type {
  AllocationPlanCandidateRow,
  AllocationPlanDetail,
  AllocationPlanRow,
  AllocationPlansList,
  AllocationStatus,
} from "./types/allocation";

// ---------------------------------------------------------------------------
// Closed vocabularies
// ---------------------------------------------------------------------------

export const ALLOCATION_TRUTH_STATES = [
  "active",
  "invalid_configuration",
  "db_unavailable",
  "query_failed",
  "not_found",
] as const;
export type AllocationTruthState = (typeof ALLOCATION_TRUTH_STATES)[number];

export const ALLOCATION_MODES = ["off", "shadow", "paper_enforced"] as const;

export const ALLOCATION_RUNTIME_INFLUENCE = ["none", "shadow", "paper_enforced"] as const;

export const ALLOCATION_DISPOSITIONS = [
  "allowed",
  "clamped_down",
  "refused_no_capital",
  "refused_fail_closed",
] as const;

// ---------------------------------------------------------------------------
// Numeric / shape helpers
// ---------------------------------------------------------------------------

function isString(v: unknown): v is string {
  return typeof v === "string";
}
function isNullableString(v: unknown): v is string | null {
  return v === null || typeof v === "string";
}
function isBoolean(v: unknown): v is boolean {
  return typeof v === "boolean";
}
function isFiniteNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}
function isNullableSafeInteger(v: unknown): v is number | null {
  return v === null || (typeof v === "number" && Number.isSafeInteger(v));
}
function isEnumValue<T extends string>(v: unknown, allowed: readonly T[]): v is T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v);
}

// ---------------------------------------------------------------------------
// Fail-closed sentinels
// ---------------------------------------------------------------------------

export function unavailableAllocationStatus(reason: string): AllocationStatus {
  return {
    truth_state: "db_unavailable",
    mode_configured: "off",
    mode_effective: "off",
    invalid_configuration: reason,
    live_lock_applied: false,
    approved_for_live: false,
    runtime_influence: "none",
    run_id: null,
    latest_plan_id: null,
    latest_plan_created_at_utc: null,
    latest_plan_candidate_count: null,
    latest_plan_allowed_count: null,
    checked_at_utc: new Date().toISOString(),
  };
}

export function unavailableAllocationPlanDetail(): AllocationPlanDetail {
  return { truth_state: "db_unavailable", plan: null, candidates: [], checked_at_utc: new Date().toISOString() };
}

export function unavailableAllocationPlansList(): AllocationPlansList {
  return { truth_state: "db_unavailable", run_id: null, plans: [], checked_at_utc: new Date().toISOString() };
}

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

function isValidCandidateRow(v: unknown): v is AllocationPlanCandidateRow {
  if (!v || typeof v !== "object") return false;
  const c = v as Record<string, unknown>;
  return (
    isString(c.symbol) &&
    isString(c.strategy_id) &&
    isFiniteNumber(c.input_score) &&
    isFiniteNumber(c.target_weight) &&
    isNullableSafeInteger(c.current_qty) &&
    isNullableSafeInteger(c.strategy_target_qty) &&
    isNullableSafeInteger(c.allocation_target_qty) &&
    isNullableSafeInteger(c.final_target_qty) &&
    isEnumValue(c.disposition, ALLOCATION_DISPOSITIONS) &&
    isString(c.reason_code) &&
    isNullableSafeInteger(c.evaluation_price_micros)
  );
}

function isValidPlanRow(v: unknown): v is AllocationPlanRow {
  if (!v || typeof v !== "object") return false;
  const p = v as Record<string, unknown>;
  return (
    isString(p.plan_id) &&
    isString(p.cycle_id) &&
    isString(p.run_id) &&
    isEnumValue(p.mode, ALLOCATION_MODES.filter((m) => m !== "off")) &&
    isString(p.opportunity_artifact_id) &&
    isNullableString(p.source_snapshot_id) &&
    isNullableSafeInteger(p.equity_micros) &&
    isNullableSafeInteger(p.candidate_count) &&
    isNullableSafeInteger(p.allowed_count) &&
    isFiniteNumber(p.gross_weight) &&
    isFiniteNumber(p.net_weight) &&
    isString(p.truth_state) &&
    Array.isArray(p.blockers) &&
    p.blockers.every(isString) &&
    isString(p.created_at_utc) &&
    p.approved_for_live === false
  );
}

/** Parses a raw HTTP-200 body for GET allocation/status. Never throws. */
export function parseAllocationStatus(raw: unknown): AllocationStatus {
  const reject = () => unavailableAllocationStatus("malformed allocation status response");
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, ALLOCATION_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.mode_configured, ALLOCATION_MODES)) return reject();
  if (!isEnumValue(r.mode_effective, ALLOCATION_MODES)) return reject();
  if (!isEnumValue(r.runtime_influence, ALLOCATION_RUNTIME_INFLUENCE)) return reject();
  if (!isNullableString(r.invalid_configuration)) return reject();
  if (!isBoolean(r.live_lock_applied)) return reject();
  if (r.approved_for_live !== false) return reject(); // hard invariant — never true
  if (!isNullableString(r.run_id)) return reject();
  if (!isNullableString(r.latest_plan_id)) return reject();
  if (!isNullableString(r.latest_plan_created_at_utc)) return reject();
  if (!isNullableSafeInteger(r.latest_plan_candidate_count)) return reject();
  if (!isNullableSafeInteger(r.latest_plan_allowed_count)) return reject();
  if (!isString(r.checked_at_utc)) return reject();

  // Never let a Shadow/PaperEnforced effective mode masquerade as live-ready:
  // approved_for_live is already hard-checked above, but runtime_influence
  // must also agree with mode_effective (both come from the same daemon
  // computation, but the GUI never trusts that without re-checking).
  const modeToInfluence: Record<string, string> = {
    off: "none",
    shadow: "shadow",
    paper_enforced: "paper_enforced",
  };
  if (modeToInfluence[r.mode_effective as string] !== r.runtime_influence) return reject();

  return {
    truth_state: r.truth_state as string,
    mode_configured: r.mode_configured as string,
    mode_effective: r.mode_effective as string,
    invalid_configuration: r.invalid_configuration as string | null,
    live_lock_applied: r.live_lock_applied as boolean,
    approved_for_live: false,
    runtime_influence: r.runtime_influence as string,
    run_id: r.run_id as string | null,
    latest_plan_id: r.latest_plan_id as string | null,
    latest_plan_created_at_utc: r.latest_plan_created_at_utc as string | null,
    latest_plan_candidate_count: r.latest_plan_candidate_count as number | null,
    latest_plan_allowed_count: r.latest_plan_allowed_count as number | null,
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET allocation/plans/:plan_id. Never throws. */
export function parseAllocationPlanDetail(raw: unknown): AllocationPlanDetail {
  const reject = unavailableAllocationPlanDetail;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, ALLOCATION_TRUTH_STATES)) return reject();
  if (!isString(r.checked_at_utc)) return reject();
  if (r.plan !== null && !isValidPlanRow(r.plan)) return reject();
  if (!Array.isArray(r.candidates) || !r.candidates.every(isValidCandidateRow)) return reject();

  // "active" must carry a real plan; a truth_state claiming success with a
  // null plan is internally inconsistent and must fail closed.
  if (r.truth_state === "active" && r.plan === null) return reject();
  // Any non-active truth_state must never smuggle a plan/candidates through
  // alongside it — the whole response fails closed together, not partially.
  if (r.truth_state !== "active" && (r.plan !== null || (r.candidates as unknown[]).length > 0)) {
    return reject();
  }

  return {
    truth_state: r.truth_state as string,
    plan: r.plan as AllocationPlanRow | null,
    candidates: r.candidates as AllocationPlanCandidateRow[],
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET allocation/plans. Never throws. */
export function parseAllocationPlansList(raw: unknown): AllocationPlansList {
  const reject = unavailableAllocationPlansList;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, ALLOCATION_TRUTH_STATES)) return reject();
  if (!isNullableString(r.run_id)) return reject();
  if (!isString(r.checked_at_utc)) return reject();
  if (!Array.isArray(r.plans) || !r.plans.every(isValidPlanRow)) return reject();

  if (r.truth_state !== "active" && (r.plans as unknown[]).length > 0) return reject();

  return {
    truth_state: r.truth_state as string,
    run_id: r.run_id as string | null,
    plans: r.plans as AllocationPlanRow[],
    checked_at_utc: r.checked_at_utc as string,
  };
}
