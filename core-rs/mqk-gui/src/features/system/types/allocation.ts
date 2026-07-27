// core-rs/mqk-gui/src/features/system/types/allocation.ts
//
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase H: read-only allocation truth types.

export interface AllocationStatus {
  truth_state: string;
  mode_configured: string;
  mode_effective: string;
  invalid_configuration: string | null;
  live_lock_applied: boolean;
  approved_for_live: boolean;
  runtime_influence: string;
  run_id: string | null;
  latest_plan_id: string | null;
  latest_plan_created_at_utc: string | null;
  latest_plan_candidate_count: number | null;
  latest_plan_allowed_count: number | null;
  checked_at_utc: string;
}

export interface AllocationPlanCandidateRow {
  symbol: string;
  strategy_id: string;
  input_score: number;
  target_weight: number;
  current_qty: number;
  strategy_target_qty: number;
  allocation_target_qty: number;
  final_target_qty: number;
  disposition: string;
  reason_code: string;
  evaluation_price_micros: number;
}

export interface AllocationPlanRow {
  plan_id: string;
  cycle_id: string;
  run_id: string;
  mode: string;
  opportunity_artifact_id: string;
  source_snapshot_id: string | null;
  equity_micros: number;
  candidate_count: number;
  allowed_count: number;
  gross_weight: number;
  net_weight: number;
  truth_state: string;
  blockers: string[];
  created_at_utc: string;
  approved_for_live: boolean;
}

export interface AllocationPlanDetail {
  truth_state: string;
  plan: AllocationPlanRow | null;
  candidates: AllocationPlanCandidateRow[];
  checked_at_utc: string;
}

export interface AllocationPlansList {
  truth_state: string;
  run_id: string | null;
  plans: AllocationPlanRow[];
  checked_at_utc: string;
}
