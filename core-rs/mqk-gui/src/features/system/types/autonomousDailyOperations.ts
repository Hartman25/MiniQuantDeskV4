// core-rs/mqk-gui/src/features/system/types/autonomousDailyOperations.ts
//
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION:
// GUI-side mirror of the accepted E4 read-only daily-operation API response
// types (core-rs/crates/mqk-daemon/src/api_types.rs). No field is reinterpreted
// or recomputed here — this module only names the shape the daemon already
// returns, plus one GUI-only transport-failure sentinel for when the HTTP
// endpoint itself cannot be reached (distinct from an authoritative daemon
// truth_state).

/** Daemon-authoritative truth_state values for GET /api/v1/autonomous/daily-operation. */
export type AutonomousDailyOperationTruthState =
  | "active"
  | "not_found"
  | "backend_unavailable"
  | "query_failed";

/** Daemon-authoritative truth_state values for GET /api/v1/autonomous/daily-operations. */
export type AutonomousDailyOperationsTruthState =
  | "active"
  | "backend_unavailable"
  | "query_failed";

/**
 * One projected `sys_autonomous_daily_operations` row, verbatim from the
 * daemon's `AutonomousDailyOperationApiRow`. `strategy_evaluation_count` /
 * `order_activity_count` / `fill_count` are `null` only when the full run
 * lineage could not be established or read — never a fabricated `0`.
 */
export interface AutonomousDailyOperationApiRow {
  operation_id: string;
  market_date: string;
  deployment_mode: string;
  adapter_id: string;

  state: string;
  state_reason_code: string | null;
  /** "finalized" | "awaiting_finalization" | "blocked_insufficient_evidence" | "not_yet_eligible" */
  finalization_status: string;

  /** "no_trade" | "with_activity" | "completed"; null while nonterminal. */
  outcome_class: string | null;
  outcome_reason_code: string | null;
  finalized_at_utc: string | null;

  run_id: string | null;
  bars_observed: number;
  bars_dispatched: number;
  last_completed_bar_ts: number | null;
  last_dispatched_bar_ts: number | null;

  strategy_evaluation_count: number | null;
  order_activity_count: number | null;
  fill_count: number | null;

  /** "complete" | "pending" | "degraded" | "unavailable" */
  evidence_state: string;
  evidence_blockers: string[];

  created_at_utc: string;
  updated_at_utc: string;
}

/**
 * GUI-side read model for GET /api/v1/autonomous/daily-operation.
 *
 * `transport_state === "endpoint_unavailable"` is a GUI-only sentinel for a
 * network/HTTP failure reaching the daemon at all — it must never be
 * conflated with the daemon's own authoritative `truth_state`. When
 * `transport_state === "available"`, `truth_state` carries the daemon's
 * verbatim value and `operation`/`message` mirror the daemon response.
 */
export interface AutonomousDailyOperationSurface {
  transport_state: "available" | "endpoint_unavailable";
  canonical_route: string | null;
  truth_state: AutonomousDailyOperationTruthState | null;
  operation: AutonomousDailyOperationApiRow | null;
  message: string | null;
}

/** GUI-side read model for GET /api/v1/autonomous/daily-operations?limit=N. */
export interface AutonomousDailyOperationsSurface {
  transport_state: "available" | "endpoint_unavailable";
  canonical_route: string | null;
  truth_state: AutonomousDailyOperationsTruthState | null;
  requested_limit: number | null;
  effective_limit: number | null;
  rows: AutonomousDailyOperationApiRow[];
  message: string | null;
}
