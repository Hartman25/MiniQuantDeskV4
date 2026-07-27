// core-rs/mqk-gui/src/features/system/types/strategyConflict.ts
//
// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: read-only conflict-policy truth types.

export interface ConflictStatus {
  truth_state: string;
  mode_configured: string;
  mode_effective: string;
  invalid_configuration: string | null;
  live_lock_applied: boolean;
  approved_for_live: boolean;
  current_runtime_limitation: string;
  run_id: string | null;
  latest_plan_id: string | null;
  latest_plan_created_at_utc: string | null;
  latest_plan_symbol_group_count: number | null;
  latest_plan_candidate_count: number | null;
  latest_plan_selected_count: number | null;
  checked_at_utc: string;
}

export interface ConflictPlanCandidateRow {
  symbol: string;
  strategy_id: string;
  timeframe_secs: number;
  side: string;
  qty: number;
  current_qty: number;
  proposed_target_qty: number | null;
  bar_end_ts: number | null;
  selected: boolean;
  disposition: string;
  reason_code: string;
}

export interface ConflictPlanRow {
  plan_id: string;
  cycle_id: string;
  run_id: string;
  mode: string;
  market_date: string;
  policy_schema_version: string;
  symbol_group_count: number;
  candidate_count: number;
  selected_count: number;
  refused_count: number;
  truth_state: string;
  blockers: string[];
  created_at_utc: string;
  approved_for_live: boolean;
}

export interface ConflictPlanDetail {
  truth_state: string;
  plan: ConflictPlanRow | null;
  candidates: ConflictPlanCandidateRow[];
  checked_at_utc: string;
}

export interface ConflictPlansList {
  truth_state: string;
  run_id: string | null;
  plans: ConflictPlanRow[];
  checked_at_utc: string;
}
