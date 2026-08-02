// core-rs/mqk-gui/src/features/system/dynamicSelectionEvidence.ts
//
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 5: fail-closed parsing
// for the read-only durable dynamic-selection evidence routes
// (GET /api/v1/dynamic-selection/*).
//
// Mirrors strategyConflict.ts's contract-hardening pattern: the daemon
// already carries explicit truth/validation-state vocabularies per
// response, but this module is the single seam that decides whether an
// HTTP-200 body is actually trusted as that state, or downgraded to an
// unavailable sentinel. An unrecognized value, a non-finite number, or an
// internally-inconsistent response fails closed here rather than being
// rendered as proven truth.

import type {
  DynamicSelectionCandidateRow,
  DynamicSelectionPlanDetail,
  DynamicSelectionPlanSummaryRow,
  DynamicSelectionPlansList,
  DynamicSelectionStatus,
  DynamicSelectionSymbolRow,
} from "./types/dynamicSelectionEvidence";

// ---------------------------------------------------------------------------
// Closed vocabularies
// ---------------------------------------------------------------------------

export const DYNAMIC_SELECTION_LIFECYCLE_STATES = [
  "idle",
  "starting",
  "running",
  "degraded",
] as const;

export const DYNAMIC_SELECTION_MODES = ["off", "shadow", "paper_enforced"] as const;

export const DYNAMIC_SELECTION_DISPOSITIONS = [
  "off",
  "shadow_allowed",
  "shadow_invalid",
  "paper_enforced_allowed",
  "paper_enforced_refused",
] as const;

export const DYNAMIC_SELECTION_SYMBOL_DISPOSITIONS = [
  "selected",
  "not_selected",
  "refused",
] as const;

export const DYNAMIC_SELECTION_VALIDATION_STATES = [
  "valid",
  "missing",
  "incomplete",
  "identity_mismatch",
  "run_mismatch",
  "candidate_mismatch",
  "runtime_mismatch",
  "live_approval_violation",
] as const;
export type DynamicSelectionValidationState = (typeof DYNAMIC_SELECTION_VALIDATION_STATES)[number];

// ---------------------------------------------------------------------------
// Shape helpers
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
function isNullableBoolean(v: unknown): v is boolean | null {
  return v === null || typeof v === "boolean";
}
function isSafeInteger(v: unknown): v is number {
  return typeof v === "number" && Number.isSafeInteger(v);
}
function isNullableSafeInteger(v: unknown): v is number | null {
  return v === null || (typeof v === "number" && Number.isSafeInteger(v));
}
function isEnumValue<T extends string>(v: unknown, allowed: readonly T[]): v is T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v);
}
function isNullableEnumValue<T extends string>(v: unknown, allowed: readonly T[]): v is T | null {
  return v === null || isEnumValue(v, allowed);
}
function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every(isString);
}

// ---------------------------------------------------------------------------
// Fail-closed sentinels
// ---------------------------------------------------------------------------

export function unavailableDynamicSelectionStatus(reason: string): DynamicSelectionStatus {
  return {
    lifecycle: "idle",
    active_run_id: null,
    committed_configured_mode: null,
    committed_effective_mode: null,
    committed_live_lock_applied: false,
    approved_for_live: false,
    disposition: null,
    committed_plan_id: null,
    committed_source_kind: null,
    committed_source_identity: null,
    committed_plan_truth_state: null,
    evidence_persisted: false,
    evidence_validation_state: null,
    evidence_validation_detail: reason,
    selected: [],
    refused: [],
    host_pool_present: false,
    host_pool_count: 0,
    validation_blockers: [reason],
    preview_configured_mode: "off",
    preview_effective_mode: "off",
    preview_live_lock_applied: false,
    checked_at_utc: new Date().toISOString(),
  };
}

export function unavailableDynamicSelectionPlansList(): DynamicSelectionPlansList {
  return { run_id: null, plans: [], checked_at_utc: new Date().toISOString() };
}

export function unavailableDynamicSelectionPlanDetail(planId: string): DynamicSelectionPlanDetail {
  return {
    found: false,
    plan_id: planId,
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
    validation_detail: "fetch failed",
    checked_at_utc: new Date().toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Row validators
// ---------------------------------------------------------------------------

function isValidSymbolRow(v: unknown): v is DynamicSelectionSymbolRow {
  if (!v || typeof v !== "object") return false;
  const s = v as Record<string, unknown>;
  return (
    isString(s.symbol) &&
    isNullableString(s.selected_strategy_id) &&
    isNullableSafeInteger(s.timeframe_secs) &&
    isEnumValue(s.disposition, DYNAMIC_SELECTION_SYMBOL_DISPOSITIONS) &&
    isString(s.reason_code) &&
    isString(s.exact_reason_code) &&
    // selected <-> selected_strategy_id/timeframe_secs present coherence
    (s.disposition === "selected") === (s.selected_strategy_id !== null) &&
    (s.disposition === "selected") === (s.timeframe_secs !== null)
  );
}

function isValidCandidateRow(v: unknown): v is DynamicSelectionCandidateRow {
  if (!v || typeof v !== "object") return false;
  const c = v as Record<string, unknown>;
  return (
    isString(c.symbol) &&
    isString(c.strategy_id) &&
    isSafeInteger(c.timeframe_secs) &&
    isBoolean(c.selected) &&
    isEnumValue(c.disposition, DYNAMIC_SELECTION_SYMBOL_DISPOSITIONS) &&
    isString(c.reason_code) &&
    isString(c.exact_reason_code) &&
    isNullableString(c.canonical_score_decimal) &&
    isNullableSafeInteger(c.scanner_rank)
  );
}

function isValidPlanSummaryRow(v: unknown): v is DynamicSelectionPlanSummaryRow {
  if (!v || typeof v !== "object") return false;
  const p = v as Record<string, unknown>;
  return (
    isString(p.plan_id) &&
    isString(p.run_id) &&
    isEnumValue(p.disposition, DYNAMIC_SELECTION_DISPOSITIONS) &&
    isEnumValue(p.effective_mode, DYNAMIC_SELECTION_MODES) &&
    isString(p.truth_state) &&
    isSafeInteger(p.selected_count) &&
    isSafeInteger(p.candidate_count) &&
    isString(p.created_at_utc) &&
    isEnumValue(p.validation_state, DYNAMIC_SELECTION_VALIDATION_STATES)
  );
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/** Parses a raw HTTP-200 body for GET /api/v1/dynamic-selection/status. Never throws. */
export function parseDynamicSelectionStatus(raw: unknown): DynamicSelectionStatus {
  const reject = () => unavailableDynamicSelectionStatus("malformed dynamic-selection status response");
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.lifecycle, DYNAMIC_SELECTION_LIFECYCLE_STATES)) return reject();
  if (!isNullableString(r.active_run_id)) return reject();
  if (!isNullableEnumValue(r.committed_configured_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isNullableEnumValue(r.committed_effective_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isBoolean(r.committed_live_lock_applied)) return reject();
  if (r.approved_for_live !== false) return reject(); // hard invariant — never true
  if (!isNullableEnumValue(r.disposition, DYNAMIC_SELECTION_DISPOSITIONS)) return reject();
  if (!isNullableString(r.committed_plan_id)) return reject();
  if (!isNullableString(r.committed_source_kind)) return reject();
  if (!isNullableString(r.committed_source_identity)) return reject();
  if (!isNullableString(r.committed_plan_truth_state)) return reject();
  if (!isBoolean(r.evidence_persisted)) return reject();
  if (!isNullableEnumValue(r.evidence_validation_state, DYNAMIC_SELECTION_VALIDATION_STATES)) return reject();
  if (!isNullableString(r.evidence_validation_detail)) return reject();
  if (!Array.isArray(r.selected) || !r.selected.every(isValidSymbolRow)) return reject();
  if (!Array.isArray(r.refused) || !r.refused.every(isValidSymbolRow)) return reject();
  if (!isBoolean(r.host_pool_present)) return reject();
  if (!isSafeInteger(r.host_pool_count) || (r.host_pool_count as number) < 0) return reject();
  if (!isStringArray(r.validation_blockers)) return reject();
  if (!isEnumValue(r.preview_configured_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isEnumValue(r.preview_effective_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isBoolean(r.preview_live_lock_applied)) return reject();
  if (!isString(r.checked_at_utc)) return reject();

  // No committed plan_id means no committed truth at all -- every
  // committed_* field alongside it must be neutral/absent, never a partial
  // commitment.
  if (r.committed_plan_id === null) {
    if (
      r.committed_configured_mode !== null ||
      r.committed_effective_mode !== null ||
      r.disposition !== null ||
      r.evidence_validation_state !== null ||
      r.host_pool_present !== false
    ) {
      return reject();
    }
  }

  return {
    lifecycle: r.lifecycle as string,
    active_run_id: r.active_run_id as string | null,
    committed_configured_mode: r.committed_configured_mode as string | null,
    committed_effective_mode: r.committed_effective_mode as string | null,
    committed_live_lock_applied: r.committed_live_lock_applied as boolean,
    approved_for_live: false,
    disposition: r.disposition as string | null,
    committed_plan_id: r.committed_plan_id as string | null,
    committed_source_kind: r.committed_source_kind as string | null,
    committed_source_identity: r.committed_source_identity as string | null,
    committed_plan_truth_state: r.committed_plan_truth_state as string | null,
    evidence_persisted: r.evidence_persisted as boolean,
    evidence_validation_state: r.evidence_validation_state as string | null,
    evidence_validation_detail: r.evidence_validation_detail as string | null,
    selected: r.selected as DynamicSelectionSymbolRow[],
    refused: r.refused as DynamicSelectionSymbolRow[],
    host_pool_present: r.host_pool_present as boolean,
    host_pool_count: r.host_pool_count as number,
    validation_blockers: r.validation_blockers as string[],
    preview_configured_mode: r.preview_configured_mode as string,
    preview_effective_mode: r.preview_effective_mode as string,
    preview_live_lock_applied: r.preview_live_lock_applied as boolean,
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET /api/v1/dynamic-selection/plans. Never throws. */
export function parseDynamicSelectionPlansList(raw: unknown): DynamicSelectionPlansList {
  const reject = unavailableDynamicSelectionPlansList;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isNullableString(r.run_id)) return reject();
  if (!Array.isArray(r.plans) || !r.plans.every(isValidPlanSummaryRow)) return reject();
  if (!isString(r.checked_at_utc)) return reject();

  return {
    run_id: r.run_id as string | null,
    plans: r.plans as DynamicSelectionPlanSummaryRow[],
    checked_at_utc: r.checked_at_utc as string,
  };
}

/** Parses a raw HTTP-200 body for GET /api/v1/dynamic-selection/plans/:plan_id. Never throws. */
export function parseDynamicSelectionPlanDetail(raw: unknown, planId: string): DynamicSelectionPlanDetail {
  const reject = () => unavailableDynamicSelectionPlanDetail(planId);
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isBoolean(r.found)) return reject();
  if (!isString(r.plan_id)) return reject();
  if (!isNullableString(r.run_id)) return reject();
  if (!isNullableEnumValue(r.disposition, DYNAMIC_SELECTION_DISPOSITIONS)) return reject();
  if (!isNullableEnumValue(r.effective_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isNullableEnumValue(r.configured_mode, DYNAMIC_SELECTION_MODES)) return reject();
  if (!isNullableBoolean(r.live_lock_applied)) return reject();
  if (r.approved_for_live !== null && r.approved_for_live !== false) return reject(); // never true
  if (!isNullableString(r.truth_state)) return reject();
  if (!isStringArray(r.blockers)) return reject();
  if (!isNullableString(r.created_at_utc)) return reject();
  if (!Array.isArray(r.symbols) || !r.symbols.every(isValidSymbolRow)) return reject();
  if (!Array.isArray(r.candidates) || !r.candidates.every(isValidCandidateRow)) return reject();
  if (!isEnumValue(r.validation_state, DYNAMIC_SELECTION_VALIDATION_STATES)) return reject();
  if (!isNullableString(r.validation_detail)) return reject();
  if (!isString(r.checked_at_utc)) return reject();

  // found: false must never carry a real run/disposition/symbols/candidates
  // alongside it -- absence and presence are never mixed.
  if (
    r.found === false &&
    (r.run_id !== null ||
      r.disposition !== null ||
      (r.symbols as unknown[]).length > 0 ||
      (r.candidates as unknown[]).length > 0)
  ) {
    return reject();
  }

  return {
    found: r.found as boolean,
    plan_id: r.plan_id as string,
    run_id: r.run_id as string | null,
    disposition: r.disposition as string | null,
    effective_mode: r.effective_mode as string | null,
    configured_mode: r.configured_mode as string | null,
    live_lock_applied: r.live_lock_applied as boolean | null,
    approved_for_live: r.approved_for_live as boolean | null,
    truth_state: r.truth_state as string | null,
    blockers: r.blockers as string[],
    created_at_utc: r.created_at_utc as string | null,
    symbols: r.symbols as DynamicSelectionSymbolRow[],
    candidates: r.candidates as DynamicSelectionCandidateRow[],
    validation_state: r.validation_state as string,
    validation_detail: r.validation_detail as string | null,
    checked_at_utc: r.checked_at_utc as string,
  };
}
