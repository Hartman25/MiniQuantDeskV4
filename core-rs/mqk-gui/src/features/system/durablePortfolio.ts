// core-rs/mqk-gui/src/features/system/durablePortfolio.ts
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL closure repair (Repair H): runtime
// contract hardening for the three durable-portfolio routes
// (durable-summary, durable-positions, durable-snapshots).
//
// The daemon responses already carry an explicit truth_state field per
// route (B4-A/B4-E contract), but api.ts previously trusted an HTTP 200
// body's shape unconditionally (`result.data as DurablePortfolioSummary`).
// A malformed 200 body, an unrecognized truth_state, or a summary/positions
// pair naming two different runs must all fail closed instead of rendering
// as if they were proven active truth — this module is the single seam
// that decides that.

import type {
  DurablePortfolioPositionRow,
  DurablePortfolioPositionsResponse,
  DurablePortfolioSnapshotRow,
  DurablePortfolioSnapshotsResponse,
  DurablePortfolioSummary,
} from "./types/portfolio";

// ---------------------------------------------------------------------------
// Closed truth-state vocabularies
// ---------------------------------------------------------------------------

export const DURABLE_SUMMARY_TRUTH_STATES = [
  "active",
  "snapshot_stale",
  "snapshot_unavailable",
  "db_unavailable",
  "query_failed",
  "not_found",
  "unsupported_source",
] as const;
export type DurableSummaryTruthState = (typeof DURABLE_SUMMARY_TRUTH_STATES)[number];

export const DURABLE_ACCOUNTING_TRUTH_STATES = [
  "active",
  "fill_history_incomplete",
  "accounting_epoch_unavailable",
  "not_found",
  "db_unavailable",
  "query_failed",
  "unsupported_source",
] as const;
export type DurableAccountingTruthState = (typeof DURABLE_ACCOUNTING_TRUTH_STATES)[number];

export const DURABLE_POSITIONS_TRUTH_STATES = [
  "active",
  "snapshot_stale",
  "snapshot_unavailable",
  "db_unavailable",
  "query_failed",
  "not_found",
  "unsupported_source",
] as const;
export type DurablePositionsTruthState = (typeof DURABLE_POSITIONS_TRUTH_STATES)[number];

export const DURABLE_SNAPSHOTS_TRUTH_STATES = ["active", "db_unavailable", "query_failed"] as const;
export type DurableSnapshotsTruthState = (typeof DURABLE_SNAPSHOTS_TRUTH_STATES)[number];

function isString(v: unknown): v is string {
  return typeof v === "string";
}
function isNullableString(v: unknown): v is string | null {
  return v === null || typeof v === "string";
}
function isNullableNumber(v: unknown): v is number | null {
  return v === null || typeof v === "number";
}

// ---------------------------------------------------------------------------
// Fail-closed sentinels — used whenever validation rejects a response.
// ---------------------------------------------------------------------------

export function unavailableDurablePortfolioSummary(reason: string): DurablePortfolioSummary {
  return {
    truth_state: "db_unavailable",
    snapshot_truth_state: "db_unavailable",
    snapshot_id: null,
    captured_at_utc: null,
    source: null,
    deployment_mode: null,
    account_equity: null,
    cash: null,
    currency: null,
    run_id: null,
    operation_id: null,
    accounting_truth_state: "db_unavailable",
    accounting_epoch: null,
    accounting_epoch_reason: null,
    accounting_source_snapshot_id: null,
    last_applied_inbox_id: null,
    realized_pnl: null,
    realized_pnl_truth_state: "db_unavailable",
    realized_pnl_unavailable_reason: reason,
    fees: null,
    cumulative_cash_movement: null,
    unrealized_pnl: null,
    unrealized_pnl_truth_state: "db_unavailable",
    unrealized_pnl_unavailable_reason: reason,
    daily_pnl: null,
    daily_pnl_truth_state: "db_unavailable",
    daily_pnl_unavailable_reason: reason,
    blockers: [reason],
  };
}

export function unavailableDurablePortfolioPositions(): DurablePortfolioPositionsResponse {
  return { truth_state: "db_unavailable", snapshot_id: null, captured_at_utc: null, run_id: null, positions: [] };
}

export function unavailableDurablePortfolioSnapshots(): DurablePortfolioSnapshotsResponse {
  return { truth_state: "db_unavailable", snapshots: [] };
}

// ---------------------------------------------------------------------------
// Validators — every required field is checked; an unrecognized truth_state
// or a wrong-typed field fails closed rather than being coerced.
// ---------------------------------------------------------------------------

/** Parses a raw HTTP-200 body for GET durable-summary. Never throws. */
export function parseDurablePortfolioSummary(raw: unknown): DurablePortfolioSummary {
  const reject = () => unavailableDurablePortfolioSummary("malformed durable-summary response");
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isString(r.truth_state) || !isString(r.snapshot_truth_state)) return reject();
  if (!DURABLE_SUMMARY_TRUTH_STATES.includes(r.truth_state as DurableSummaryTruthState)) return reject();
  if (!isString(r.accounting_truth_state)) return reject();
  if (!DURABLE_ACCOUNTING_TRUTH_STATES.includes(r.accounting_truth_state as DurableAccountingTruthState)) {
    return reject();
  }
  if (!isString(r.realized_pnl_truth_state)) return reject();
  if (!isString(r.unrealized_pnl_truth_state)) return reject();
  if (!isString(r.daily_pnl_truth_state)) return reject();
  if (!Array.isArray(r.blockers) || !r.blockers.every(isString)) return reject();

  const nullableStringFields = [
    "snapshot_id",
    "captured_at_utc",
    "source",
    "deployment_mode",
    "currency",
    "run_id",
    "operation_id",
    "accounting_epoch",
    "accounting_epoch_reason",
    "accounting_source_snapshot_id",
    "realized_pnl_unavailable_reason",
    "unrealized_pnl_unavailable_reason",
    "daily_pnl_unavailable_reason",
  ] as const;
  for (const field of nullableStringFields) {
    if (!isNullableString(r[field])) return reject();
  }
  const nullableNumberFields = [
    "account_equity",
    "cash",
    "last_applied_inbox_id",
    "realized_pnl",
    "fees",
    "cumulative_cash_movement",
    "unrealized_pnl",
    "daily_pnl",
  ] as const;
  for (const field of nullableNumberFields) {
    if (!isNullableNumber(r[field])) return reject();
  }

  return {
    truth_state: r.truth_state,
    snapshot_truth_state: r.snapshot_truth_state,
    snapshot_id: r.snapshot_id as string | null,
    captured_at_utc: r.captured_at_utc as string | null,
    source: r.source as string | null,
    deployment_mode: r.deployment_mode as string | null,
    account_equity: r.account_equity as number | null,
    cash: r.cash as number | null,
    currency: r.currency as string | null,
    run_id: r.run_id as string | null,
    operation_id: r.operation_id as string | null,
    accounting_truth_state: r.accounting_truth_state,
    accounting_epoch: r.accounting_epoch as string | null,
    accounting_epoch_reason: r.accounting_epoch_reason as string | null,
    accounting_source_snapshot_id: r.accounting_source_snapshot_id as string | null,
    last_applied_inbox_id: r.last_applied_inbox_id as number | null,
    realized_pnl: r.realized_pnl as number | null,
    realized_pnl_truth_state: r.realized_pnl_truth_state,
    realized_pnl_unavailable_reason: r.realized_pnl_unavailable_reason as string | null,
    fees: r.fees as number | null,
    cumulative_cash_movement: r.cumulative_cash_movement as number | null,
    unrealized_pnl: r.unrealized_pnl as number | null,
    unrealized_pnl_truth_state: r.unrealized_pnl_truth_state,
    unrealized_pnl_unavailable_reason: r.unrealized_pnl_unavailable_reason as string | null,
    daily_pnl: r.daily_pnl as number | null,
    daily_pnl_truth_state: r.daily_pnl_truth_state,
    daily_pnl_unavailable_reason: r.daily_pnl_unavailable_reason as string | null,
    blockers: r.blockers as string[],
  };
}

function isValidPositionRow(v: unknown): v is DurablePortfolioPositionRow {
  if (!v || typeof v !== "object") return false;
  const p = v as Record<string, unknown>;
  return (
    isString(p.symbol) &&
    typeof p.qty_signed === "number" &&
    typeof p.avg_entry_price === "number" &&
    isString(p.provenance)
  );
}

/** Parses a raw HTTP-200 body for GET durable-positions. Never throws. */
export function parseDurablePortfolioPositions(raw: unknown): DurablePortfolioPositionsResponse {
  const reject = unavailableDurablePortfolioPositions;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isString(r.truth_state)) return reject();
  if (!DURABLE_POSITIONS_TRUTH_STATES.includes(r.truth_state as DurablePositionsTruthState)) return reject();
  if (!isNullableString(r.snapshot_id) || !isNullableString(r.captured_at_utc) || !isNullableString(r.run_id)) {
    return reject();
  }
  if (!Array.isArray(r.positions) || !r.positions.every(isValidPositionRow)) return reject();

  return {
    truth_state: r.truth_state,
    snapshot_id: r.snapshot_id as string | null,
    captured_at_utc: r.captured_at_utc as string | null,
    run_id: r.run_id as string | null,
    positions: r.positions as DurablePortfolioPositionRow[],
  };
}

function isValidSnapshotRow(v: unknown): v is DurablePortfolioSnapshotRow {
  if (!v || typeof v !== "object") return false;
  const s = v as Record<string, unknown>;
  return (
    isString(s.snapshot_id) &&
    isString(s.captured_at_utc) &&
    isString(s.deployment_mode) &&
    isString(s.source) &&
    typeof s.equity === "number" &&
    typeof s.cash === "number" &&
    isString(s.currency) &&
    isString(s.truth_state) &&
    isNullableString(s.run_id) &&
    isNullableString(s.operation_id)
  );
}

/** Parses a raw HTTP-200 body for GET durable-snapshots. Never throws. Order
 * is preserved exactly as received -- this validator never re-sorts. */
export function parseDurablePortfolioSnapshots(raw: unknown): DurablePortfolioSnapshotsResponse {
  const reject = unavailableDurablePortfolioSnapshots;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isString(r.truth_state)) return reject();
  if (!DURABLE_SNAPSHOTS_TRUTH_STATES.includes(r.truth_state as DurableSnapshotsTruthState)) return reject();
  if (!Array.isArray(r.snapshots) || !r.snapshots.every(isValidSnapshotRow)) return reject();

  return {
    truth_state: r.truth_state,
    snapshots: r.snapshots as DurablePortfolioSnapshotRow[],
  };
}

// ---------------------------------------------------------------------------
// Cross-response run-scoping check
// ---------------------------------------------------------------------------

/**
 * When both the summary and positions responses claim an active/proven
 * truth_state AND both carry a non-null run_id, those run_ids must match --
 * a mismatch means the two fetches landed on different runs (e.g. a race
 * against a newly-started run) and positions must be rejected rather than
 * displayed alongside a summary for a different run.
 *
 * Returns the (possibly downgraded) positions response; never mutates its
 * input.
 */
export function enforceRunScopeConsistency(
  summary: DurablePortfolioSummary,
  positions: DurablePortfolioPositionsResponse,
): DurablePortfolioPositionsResponse {
  if (
    summary.run_id != null &&
    positions.run_id != null &&
    summary.run_id !== positions.run_id &&
    summary.truth_state === "active" &&
    positions.truth_state === "active"
  ) {
    return {
      truth_state: "query_failed",
      snapshot_id: null,
      captured_at_utc: null,
      run_id: positions.run_id,
      positions: [],
    };
  }
  return positions;
}
