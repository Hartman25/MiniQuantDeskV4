// core-rs/mqk-gui/src/features/system/strategyConflict.ts
//
// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: fail-closed parsing for the
// read-only conflict-policy truth routes (GET /api/v1/strategy/conflict/*).
//
// Mirrors runtimeOpportunityAllocation.ts's contract-hardening pattern: the
// daemon already carries an explicit truth_state per response, but this
// module is the single seam that decides whether an HTTP 200 body is
// actually trusted as that truth_state, or downgraded to an unavailable
// sentinel. An unrecognized truth_state, a non-finite number, a
// null-where-required-present field, or an internally-inconsistent "active"
// response all fail closed here rather than being rendered as proven truth.

import type {
  ConflictPlanCandidateRow,
  ConflictPlanDetail,
  ConflictPlanRow,
  ConflictPlansList,
  ConflictStatus,
} from "./types/strategyConflict";

// ---------------------------------------------------------------------------
// Closed vocabularies
// ---------------------------------------------------------------------------

export const CONFLICT_TRUTH_STATES = [
  "active",
  "invalid_configuration",
  "invalid_evidence",
  "db_unavailable",
  "query_failed",
  "not_found",
] as const;
export type ConflictTruthState = (typeof CONFLICT_TRUTH_STATES)[number];

export const CONFLICT_MODES = ["off", "shadow", "paper_enforced"] as const;

export const CONFLICT_SIDES = ["buy", "sell"] as const;

export const CONFLICT_DISPOSITIONS = [
  "passthrough",
  "selected",
  "not_selected",
  "refused_invalid",
  "refused_conflict",
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
function isNullableSafeInteger(v: unknown): v is number | null {
  return v === null || (typeof v === "number" && Number.isSafeInteger(v));
}
function isSafeInteger(v: unknown): v is number {
  return typeof v === "number" && Number.isSafeInteger(v);
}
function isEnumValue<T extends string>(v: unknown, allowed: readonly T[]): v is T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v);
}
function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every(isString);
}

// ---------------------------------------------------------------------------
// Fail-closed sentinels
// ---------------------------------------------------------------------------

export function unavailableConflictStatus(reason: string): ConflictStatus {
  return {
    truth_state: "db_unavailable",
    mode_configured: "off",
    mode_effective: "off",
    invalid_configuration: reason,
    live_lock_applied: false,
    approved_for_live: false,
    current_runtime_limitation: "",
    run_id: null,
    latest_plan_id: null,
    latest_plan_created_at_utc: null,
    latest_plan_symbol_group_count: null,
    latest_plan_candidate_count: null,
    latest_plan_selected_count: null,
    evidence_blockers: [],
    checked_at_utc: new Date().toISOString(),
  };
}

export function unavailableConflictPlanDetail(): ConflictPlanDetail {
  return {
    truth_state: "db_unavailable",
    plan: null,
    candidates: [],
    evidence_blockers: [],
    checked_at_utc: new Date().toISOString(),
  };
}

export function unavailableConflictPlansList(): ConflictPlansList {
  return {
    truth_state: "db_unavailable",
    run_id: null,
    plans: [],
    excluded_malformed_count: 0,
    checked_at_utc: new Date().toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

function isValidCandidateRow(v: unknown): v is ConflictPlanCandidateRow {
  if (!v || typeof v !== "object") return false;
  const c = v as Record<string, unknown>;
  return (
    isString(c.symbol) &&
    isString(c.strategy_id) &&
    isSafeInteger(c.timeframe_secs) &&
    isEnumValue(c.side, CONFLICT_SIDES) &&
    isSafeInteger(c.qty) &&
    isSafeInteger(c.current_qty) &&
    isNullableSafeInteger(c.proposed_target_qty) &&
    isNullableSafeInteger(c.bar_end_ts) &&
    isBoolean(c.selected) &&
    isEnumValue(c.disposition, CONFLICT_DISPOSITIONS) &&
    isString(c.reason_code)
  );
}

function isValidPlanRow(v: unknown): v is ConflictPlanRow {
  if (!v || typeof v !== "object") return false;
  const p = v as Record<string, unknown>;
  return (
    isString(p.plan_id) &&
    isString(p.cycle_id) &&
    isString(p.run_id) &&
    isEnumValue(p.mode, CONFLICT_MODES.filter((m) => m !== "off")) &&
    (p.configured_mode === null || isEnumValue(p.configured_mode, CONFLICT_MODES)) &&
    isString(p.market_date) &&
    isString(p.policy_schema_version) &&
    isSafeInteger(p.symbol_group_count) &&
    isSafeInteger(p.candidate_count) &&
    isSafeInteger(p.selected_count) &&
    isSafeInteger(p.refused_count) &&
    isString(p.truth_state) &&
    Array.isArray(p.blockers) &&
    p.blockers.every(isString) &&
    isString(p.created_at_utc) &&
    p.approved_for_live === false
  );
}

/** Parses a raw HTTP-200 body for GET conflict/status. Never throws. */
export function parseConflictStatus(raw: unknown): ConflictStatus {
  const reject = () => unavailableConflictStatus("malformed conflict status response");
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, CONFLICT_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.mode_configured, CONFLICT_MODES)) return reject();
  if (!isEnumValue(r.mode_effective, CONFLICT_MODES)) return reject();
  if (!isNullableString(r.invalid_configuration)) return reject();
  if (!isBoolean(r.live_lock_applied)) return reject();
  if (r.approved_for_live !== false) return reject(); // hard invariant — never true
  if (!isString(r.current_runtime_limitation)) return reject();
  if (!isNullableString(r.run_id)) return reject();
  if (!isNullableString(r.latest_plan_id)) return reject();
  if (!isNullableString(r.latest_plan_created_at_utc)) return reject();
  if (!isNullableSafeInteger(r.latest_plan_symbol_group_count)) return reject();
  if (!isNullableSafeInteger(r.latest_plan_candidate_count)) return reject();
  if (!isNullableSafeInteger(r.latest_plan_selected_count)) return reject();
  if (!isStringArray(r.evidence_blockers)) return reject();
  if (!isString(r.checked_at_utc)) return reject();

  // invalid_evidence must never carry a latest-plan projection alongside it
  // (the whole point is refusing to project the malformed plan), and every
  // other truth_state must carry zero evidence_blockers.
  const evidenceBlockers = r.evidence_blockers as string[];
  if (r.truth_state === "invalid_evidence") {
    if (evidenceBlockers.length === 0) return reject();
    if (r.latest_plan_id !== null) return reject();
  } else if (evidenceBlockers.length > 0) {
    return reject();
  }

  return {
    truth_state: r.truth_state as string,
    mode_configured: r.mode_configured as string,
    mode_effective: r.mode_effective as string,
    invalid_configuration: r.invalid_configuration as string | null,
    live_lock_applied: r.live_lock_applied as boolean,
    approved_for_live: false,
    current_runtime_limitation: r.current_runtime_limitation as string,
    run_id: r.run_id as string | null,
    latest_plan_id: r.latest_plan_id as string | null,
    latest_plan_created_at_utc: r.latest_plan_created_at_utc as string | null,
    latest_plan_symbol_group_count: r.latest_plan_symbol_group_count as number | null,
    latest_plan_candidate_count: r.latest_plan_candidate_count as number | null,
    latest_plan_selected_count: r.latest_plan_selected_count as number | null,
    evidence_blockers: evidenceBlockers,
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET conflict/plans/:plan_id. Never throws. */
export function parseConflictPlanDetail(raw: unknown): ConflictPlanDetail {
  const reject = unavailableConflictPlanDetail;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, CONFLICT_TRUTH_STATES)) return reject();
  if (!isString(r.checked_at_utc)) return reject();
  if (r.plan !== null && !isValidPlanRow(r.plan)) return reject();
  if (!Array.isArray(r.candidates) || !r.candidates.every(isValidCandidateRow)) return reject();
  if (!isStringArray(r.evidence_blockers)) return reject();

  // "active" must carry a real plan; a truth_state claiming success with a
  // null plan is internally inconsistent and must fail closed.
  if (r.truth_state === "active" && r.plan === null) return reject();
  // Any non-active truth_state must never smuggle a plan/candidates through
  // alongside it — the whole response fails closed together, not partially.
  if (r.truth_state !== "active" && (r.plan !== null || (r.candidates as unknown[]).length > 0)) {
    return reject();
  }
  const evidenceBlockers = r.evidence_blockers as string[];
  if (r.truth_state === "invalid_evidence" && evidenceBlockers.length === 0) return reject();
  if (r.truth_state !== "invalid_evidence" && evidenceBlockers.length > 0) return reject();

  return {
    truth_state: r.truth_state as string,
    plan: r.plan as ConflictPlanRow | null,
    candidates: r.candidates as ConflictPlanCandidateRow[],
    evidence_blockers: evidenceBlockers,
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET conflict/plans. Never throws. */
export function parseConflictPlansList(raw: unknown): ConflictPlansList {
  const reject = unavailableConflictPlansList;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, CONFLICT_TRUTH_STATES)) return reject();
  if (!isNullableString(r.run_id)) return reject();
  if (!isString(r.checked_at_utc)) return reject();
  if (!Array.isArray(r.plans) || !r.plans.every(isValidPlanRow)) return reject();
  if (!isSafeInteger(r.excluded_malformed_count) || (r.excluded_malformed_count as number) < 0) return reject();

  if (r.truth_state !== "active" && (r.plans as unknown[]).length > 0) return reject();

  return {
    truth_state: r.truth_state as string,
    run_id: r.run_id as string | null,
    plans: r.plans as ConflictPlanRow[],
    excluded_malformed_count: r.excluded_malformed_count as number,
    checked_at_utc: r.checked_at_utc as string,
  };
}
