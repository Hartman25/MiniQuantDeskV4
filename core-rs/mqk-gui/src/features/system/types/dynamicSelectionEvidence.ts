// core-rs/mqk-gui/src/features/system/types/dynamicSelectionEvidence.ts
//
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 5: read-only durable
// dynamic-selection plan evidence types (GET /api/v1/dynamic-selection/*).

export interface DynamicSelectionSymbolRow {
  symbol: string;
  selected_strategy_id: string | null;
  disposition: string;
  reason_code: string;
  exact_reason_code: string;
}

export interface DynamicSelectionStatus {
  /** "idle" | "starting" | "running" | "degraded". */
  lifecycle: string;
  active_run_id: string | null;

  committed_configured_mode: string | null;
  committed_effective_mode: string | null;
  committed_live_lock_applied: boolean;
  approved_for_live: boolean;
  disposition: string | null;
  committed_plan_id: string | null;
  committed_source_kind: string | null;
  committed_source_identity: string | null;
  committed_plan_truth_state: string | null;
  evidence_persisted: boolean;
  /** "valid" | "missing" | "incomplete" | "identity_mismatch" |
   * "run_mismatch" | "candidate_mismatch" | "runtime_mismatch" |
   * "live_approval_violation", or null when no committed plan_id exists. */
  evidence_validation_state: string | null;
  evidence_validation_detail: string | null;
  selected: DynamicSelectionSymbolRow[];
  refused: DynamicSelectionSymbolRow[];
  host_pool_present: boolean;
  host_pool_count: number;
  validation_blockers: string[];

  preview_configured_mode: string;
  preview_effective_mode: string;
  preview_live_lock_applied: boolean;

  checked_at_utc: string;
}

export interface DynamicSelectionPlanSummaryRow {
  plan_id: string;
  run_id: string;
  disposition: string;
  effective_mode: string;
  truth_state: string;
  selected_count: number;
  candidate_count: number;
  created_at_utc: string;
  validation_state: string;
}

export interface DynamicSelectionPlansList {
  run_id: string | null;
  plans: DynamicSelectionPlanSummaryRow[];
  checked_at_utc: string;
}

export interface DynamicSelectionCandidateRow {
  symbol: string;
  strategy_id: string;
  timeframe_secs: number;
  selected: boolean;
  disposition: string;
  reason_code: string;
  exact_reason_code: string;
  canonical_score_decimal: string | null;
  scanner_rank: number | null;
}

export interface DynamicSelectionPlanDetail {
  found: boolean;
  plan_id: string;
  run_id: string | null;
  disposition: string | null;
  effective_mode: string | null;
  configured_mode: string | null;
  live_lock_applied: boolean | null;
  approved_for_live: boolean | null;
  truth_state: string | null;
  blockers: string[];
  created_at_utc: string | null;
  symbols: DynamicSelectionSymbolRow[];
  candidates: DynamicSelectionCandidateRow[];
  validation_state: string;
  validation_detail: string | null;
  checked_at_utc: string;
}
