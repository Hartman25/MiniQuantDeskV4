// core-rs/mqk-gui/src/features/system/durablePortfolio.ts
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL closure repair (Repair H, hardened by the
// B4-FINAL-COHERENCE-AND-ACCEPTANCE-PROOF Phase C): runtime contract
// hardening for the three durable-portfolio routes (durable-summary,
// durable-positions, durable-snapshots).
//
// The daemon responses already carry an explicit truth_state field per
// route (B4-A/B4-E contract, extended by the Phase B shared provenance
// classifier), but api.ts previously trusted an HTTP 200 body's shape
// unconditionally (`result.data as DurablePortfolioSummary`). A malformed
// 200 body, an unrecognized truth_state, a numeric field that is NaN/
// Infinity/fractional-where-integral-is-required, an "active" response
// missing the fields that state requires, or a summary/positions pair
// naming two different runs OR two different snapshots must all fail
// closed instead of rendering as if they were proven active truth — this
// module is the single seam that decides that.

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

// Shared by accounting_truth_state AND realized_pnl_truth_state -- the
// daemon derives both from the identical Phase B classifier verdict
// (routes/portfolio_provenance.rs::PortfolioProvenanceState), so they are
// always the same string in every response this route can produce.
export const DURABLE_ACCOUNTING_TRUTH_STATES = [
  "active",
  "fill_history_incomplete",
  "accounting_epoch_unavailable",
  "accounting_snapshot_mismatch",
  "not_found",
  "db_unavailable",
  "query_failed",
  "unsupported_source",
] as const;
export type DurableAccountingTruthState = (typeof DURABLE_ACCOUNTING_TRUTH_STATES)[number];

export const DURABLE_UNREALIZED_PNL_TRUTH_STATES = [
  "active",
  "snapshot_unavailable",
  "db_unavailable",
  "mark_unavailable",
] as const;
export type DurableUnrealizedPnlTruthState = (typeof DURABLE_UNREALIZED_PNL_TRUTH_STATES)[number];

export const DURABLE_DAILY_PNL_TRUTH_STATES = [
  "active",
  "snapshot_unavailable",
  "db_unavailable",
  "baseline_unavailable",
  "stale_baseline",
] as const;
export type DurableDailyPnlTruthState = (typeof DURABLE_DAILY_PNL_TRUTH_STATES)[number];

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

// Each individual snapshot-history row's own `truth_state` column -- the
// daemon (mqk-db::insert_or_confirm_paper_portfolio_snapshot) only ever
// writes `"active"` for this column; any other value is unrecognized.
export const DURABLE_SNAPSHOT_ROW_TRUTH_STATES = ["active"] as const;
export type DurableSnapshotRowTruthState = (typeof DURABLE_SNAPSHOT_ROW_TRUTH_STATES)[number];

const ACTIVE_OR_STALE_SNAPSHOT_STATES: readonly string[] = ["active", "snapshot_stale"];

// ---------------------------------------------------------------------------
// Numeric validation helpers
// ---------------------------------------------------------------------------

function isString(v: unknown): v is string {
  return typeof v === "string";
}
function isNullableString(v: unknown): v is string | null {
  return v === null || typeof v === "string";
}
/** A finite JS number -- rejects NaN, Infinity, -Infinity, and non-numbers. */
function isFiniteNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}
/** `null` (unavailable) or a finite number -- never NaN/Infinity/-Infinity. */
function isNullableFiniteNumber(v: unknown): v is number | null {
  return v === null || isFiniteNumber(v);
}
/** `null` or a finite, safe, NONNEGATIVE integer (a watermark can never be negative). */
function isNullableNonNegativeSafeInteger(v: unknown): v is number | null {
  return v === null || (typeof v === "number" && Number.isSafeInteger(v) && v >= 0);
}
function isEnumValue<T extends string>(v: unknown, allowed: readonly T[]): v is T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v);
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
// Validators — every required field is checked; an unrecognized truth_state,
// a non-finite/non-integral number, or a state-specific missing/
// contradictory field fails closed rather than being coerced or rendered.
// ---------------------------------------------------------------------------

/** Parses a raw HTTP-200 body for GET durable-summary. Never throws. */
export function parseDurablePortfolioSummary(raw: unknown): DurablePortfolioSummary {
  const reject = () => unavailableDurablePortfolioSummary("malformed durable-summary response");
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, DURABLE_SUMMARY_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.snapshot_truth_state, DURABLE_SUMMARY_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.accounting_truth_state, DURABLE_ACCOUNTING_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.realized_pnl_truth_state, DURABLE_ACCOUNTING_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.unrealized_pnl_truth_state, DURABLE_UNREALIZED_PNL_TRUTH_STATES)) return reject();
  if (!isEnumValue(r.daily_pnl_truth_state, DURABLE_DAILY_PNL_TRUTH_STATES)) return reject();
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
  if (!isNullableFiniteNumber(r.account_equity)) return reject();
  if (!isNullableFiniteNumber(r.cash)) return reject();
  if (!isNullableNonNegativeSafeInteger(r.last_applied_inbox_id)) return reject();
  if (!isNullableFiniteNumber(r.realized_pnl)) return reject();
  if (!isNullableFiniteNumber(r.fees)) return reject();
  if (!isNullableFiniteNumber(r.cumulative_cash_movement)) return reject();
  if (!isNullableFiniteNumber(r.unrealized_pnl)) return reject();
  if (!isNullableFiniteNumber(r.daily_pnl)) return reject();

  // C3: state invariants. An "active"/"snapshot_stale" snapshot truth must
  // carry real, internally-consistent snapshot identity/account fields --
  // never a null/mismatched field papered over by an otherwise-valid shape.
  if (ACTIVE_OR_STALE_SNAPSHOT_STATES.includes(r.snapshot_truth_state as string)) {
    if (
      !isString(r.snapshot_id) ||
      !isString(r.captured_at_utc) ||
      r.source !== "external_alpaca" ||
      r.deployment_mode !== "paper" ||
      r.currency !== "USD" ||
      !isString(r.run_id) ||
      !isFiniteNumber(r.account_equity) ||
      !isFiniteNumber(r.cash)
    ) {
      return reject();
    }
  }

  // C3: an "active" accounting/P&L truth must carry a proven, matching
  // snapshot provenance and a finite realized P&L -- this is the exact
  // invariant the B4 closure repair's shared classifier guarantees
  // server-side; the GUI must never trust "active" without re-checking it.
  if (r.accounting_truth_state === "active") {
    if (
      r.accounting_epoch !== "complete" ||
      !isString(r.accounting_source_snapshot_id) ||
      r.accounting_source_snapshot_id !== r.snapshot_id ||
      !isFiniteNumber(r.realized_pnl)
    ) {
      return reject();
    }
  }
  if (r.realized_pnl_truth_state === "active" && !isFiniteNumber(r.realized_pnl)) {
    return reject();
  }

  return {
    truth_state: r.truth_state as DurableSummaryTruthState,
    snapshot_truth_state: r.snapshot_truth_state as DurableSummaryTruthState,
    snapshot_id: r.snapshot_id as string | null,
    captured_at_utc: r.captured_at_utc as string | null,
    source: r.source as string | null,
    deployment_mode: r.deployment_mode as string | null,
    account_equity: r.account_equity as number | null,
    cash: r.cash as number | null,
    currency: r.currency as string | null,
    run_id: r.run_id as string | null,
    operation_id: r.operation_id as string | null,
    accounting_truth_state: r.accounting_truth_state as DurableAccountingTruthState,
    accounting_epoch: r.accounting_epoch as string | null,
    accounting_epoch_reason: r.accounting_epoch_reason as string | null,
    accounting_source_snapshot_id: r.accounting_source_snapshot_id as string | null,
    last_applied_inbox_id: r.last_applied_inbox_id as number | null,
    realized_pnl: r.realized_pnl as number | null,
    realized_pnl_truth_state: r.realized_pnl_truth_state as DurableAccountingTruthState,
    realized_pnl_unavailable_reason: r.realized_pnl_unavailable_reason as string | null,
    fees: r.fees as number | null,
    cumulative_cash_movement: r.cumulative_cash_movement as number | null,
    unrealized_pnl: r.unrealized_pnl as number | null,
    unrealized_pnl_truth_state: r.unrealized_pnl_truth_state as DurableUnrealizedPnlTruthState,
    unrealized_pnl_unavailable_reason: r.unrealized_pnl_unavailable_reason as string | null,
    daily_pnl: r.daily_pnl as number | null,
    daily_pnl_truth_state: r.daily_pnl_truth_state as DurableDailyPnlTruthState,
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
    Number.isSafeInteger(p.qty_signed) &&
    isFiniteNumber(p.avg_entry_price) &&
    isString(p.provenance)
  );
}

/** Parses a raw HTTP-200 body for GET durable-positions. Never throws. */
export function parseDurablePortfolioPositions(raw: unknown): DurablePortfolioPositionsResponse {
  const reject = unavailableDurablePortfolioPositions;
  if (!raw || typeof raw !== "object") return reject();
  const r = raw as Record<string, unknown>;

  if (!isEnumValue(r.truth_state, DURABLE_POSITIONS_TRUTH_STATES)) return reject();
  if (!isNullableString(r.snapshot_id) || !isNullableString(r.captured_at_utc) || !isNullableString(r.run_id)) {
    return reject();
  }
  if (!Array.isArray(r.positions) || !r.positions.every(isValidPositionRow)) return reject();

  // C3: active/stale positions must carry real snapshot identity -- never a
  // null snapshot_id/captured_at_utc/run_id papered over by an otherwise
  // well-typed empty/partial shape.
  if (ACTIVE_OR_STALE_SNAPSHOT_STATES.includes(r.truth_state as string)) {
    if (!isString(r.snapshot_id) || !isString(r.captured_at_utc) || !isString(r.run_id)) {
      return reject();
    }
  }

  return {
    truth_state: r.truth_state as DurablePositionsTruthState,
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
    isFiniteNumber(s.equity) &&
    isFiniteNumber(s.cash) &&
    isString(s.currency) &&
    isEnumValue(s.truth_state, DURABLE_SNAPSHOT_ROW_TRUTH_STATES) &&
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

  if (!isEnumValue(r.truth_state, DURABLE_SNAPSHOTS_TRUTH_STATES)) return reject();
  if (!Array.isArray(r.snapshots) || !r.snapshots.every(isValidSnapshotRow)) return reject();

  return {
    truth_state: r.truth_state as DurableSnapshotsTruthState,
    snapshots: r.snapshots as DurablePortfolioSnapshotRow[],
  };
}

// ---------------------------------------------------------------------------
// Cross-response run/snapshot-scoping check
// ---------------------------------------------------------------------------

/**
 * Whenever both the summary and positions responses claim an active/proven
 * (`"active"` or `"snapshot_stale"`) truth_state AND both carry a non-null
 * `run_id`/`snapshot_id`, those identities must match on BOTH axes -- a
 * run_id-only mismatch means the two fetches landed on different runs (e.g.
 * a race against a newly-started run); a snapshot_id-only mismatch (same
 * run, different snapshot) means the two fetches landed on different
 * snapshot generations within the same run (e.g. a race against a fresh
 * snapshot persist between the two requests). Either kind of mismatch means
 * positions must be rejected rather than displayed alongside a summary that
 * does not actually describe the same underlying truth.
 *
 * Returns the (possibly downgraded) positions response; never mutates its
 * input.
 */
export function enforceRunScopeConsistency(
  summary: DurablePortfolioSummary,
  positions: DurablePortfolioPositionsResponse,
): DurablePortfolioPositionsResponse {
  const bothAuthoritative =
    ACTIVE_OR_STALE_SNAPSHOT_STATES.includes(summary.truth_state) &&
    ACTIVE_OR_STALE_SNAPSHOT_STATES.includes(positions.truth_state);
  if (!bothAuthoritative) return positions;

  const runMismatch =
    summary.run_id != null && positions.run_id != null && summary.run_id !== positions.run_id;
  const snapshotMismatch =
    summary.snapshot_id != null &&
    positions.snapshot_id != null &&
    summary.snapshot_id !== positions.snapshot_id;

  if (runMismatch || snapshotMismatch) {
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
