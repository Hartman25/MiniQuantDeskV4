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
 * AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01:
 * closed GUI-side mirrors of the daemon's bounded `finalization_status` /
 * `outcome_class` / `evidence_state` vocabularies
 * (`core-rs/crates/mqk-daemon/src/api_types.rs`). An unrecognized string in
 * any of these three fields is a malformed row, never a value silently
 * widened into GUI truth — see `isAutonomousDailyOperationApiRow` below.
 */
export type AutonomousDailyFinalizationStatus =
  | "finalized"
  | "awaiting_finalization"
  | "blocked_insufficient_evidence"
  | "not_yet_eligible";

export type AutonomousDailyOutcomeClass = "no_trade" | "with_activity" | "completed";

export type AutonomousDailyEvidenceState = "complete" | "pending" | "degraded" | "unavailable";

const VALID_FINALIZATION_STATUSES: ReadonlySet<string> = new Set<AutonomousDailyFinalizationStatus>([
  "finalized",
  "awaiting_finalization",
  "blocked_insufficient_evidence",
  "not_yet_eligible",
]);

const VALID_OUTCOME_CLASSES: ReadonlySet<string> = new Set<AutonomousDailyOutcomeClass>([
  "no_trade",
  "with_activity",
  "completed",
]);

const VALID_EVIDENCE_STATES: ReadonlySet<string> = new Set<AutonomousDailyEvidenceState>([
  "complete",
  "pending",
  "degraded",
  "unavailable",
]);

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
  finalization_status: AutonomousDailyFinalizationStatus;

  /** null while nonterminal. */
  outcome_class: AutonomousDailyOutcomeClass | null;
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

  evidence_state: AutonomousDailyEvidenceState;
  evidence_blockers: string[];

  created_at_utc: string;
  updated_at_utc: string;
}

function isFiniteInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && Number.isInteger(value);
}

function isFiniteIntegerOrNull(value: unknown): value is number | null {
  return value === null || isFiniteInteger(value);
}

function isStringOrNull(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

/**
 * Complete runtime validator for one `AutonomousDailyOperationApiRow`. Every
 * required field is checked by type, nullability, and (for the three closed
 * vocabularies) exact membership; `evidence_blockers` must be an actual
 * array of strings. Nothing here repairs, defaults, coerces, trims, sorts,
 * or reinterprets a malformed field — an undefined/missing field, a NaN or
 * infinite number, or an unrecognized vocabulary string is rejected outright
 * rather than silently converted to `null` or `0`.
 */
export function isAutonomousDailyOperationApiRow(value: unknown): value is AutonomousDailyOperationApiRow {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const row = value as Record<string, unknown>;

  if (typeof row.operation_id !== "string") return false;
  if (typeof row.market_date !== "string") return false;
  if (typeof row.deployment_mode !== "string") return false;
  if (typeof row.adapter_id !== "string") return false;

  if (typeof row.state !== "string") return false;
  if (!isStringOrNull(row.state_reason_code)) return false;
  if (typeof row.finalization_status !== "string" || !VALID_FINALIZATION_STATUSES.has(row.finalization_status)) {
    return false;
  }

  if (
    !(row.outcome_class === null || (typeof row.outcome_class === "string" && VALID_OUTCOME_CLASSES.has(row.outcome_class)))
  ) {
    return false;
  }
  if (!isStringOrNull(row.outcome_reason_code)) return false;
  if (!isStringOrNull(row.finalized_at_utc)) return false;

  if (!isStringOrNull(row.run_id)) return false;
  if (!isFiniteInteger(row.bars_observed)) return false;
  if (!isFiniteInteger(row.bars_dispatched)) return false;
  if (!isFiniteIntegerOrNull(row.last_completed_bar_ts)) return false;
  if (!isFiniteIntegerOrNull(row.last_dispatched_bar_ts)) return false;

  if (!isFiniteIntegerOrNull(row.strategy_evaluation_count)) return false;
  if (!isFiniteIntegerOrNull(row.order_activity_count)) return false;
  if (!isFiniteIntegerOrNull(row.fill_count)) return false;

  if (typeof row.evidence_state !== "string" || !VALID_EVIDENCE_STATES.has(row.evidence_state)) return false;
  if (!Array.isArray(row.evidence_blockers) || !row.evidence_blockers.every((b) => typeof b === "string")) {
    return false;
  }

  if (typeof row.created_at_utc !== "string") return false;
  if (typeof row.updated_at_utc !== "string") return false;

  return true;
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
