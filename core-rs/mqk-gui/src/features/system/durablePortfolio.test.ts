// core-rs/mqk-gui/src/features/system/durablePortfolio.test.ts
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL closure repair (Repair H): proof tests for
// the durable-portfolio runtime validators/fail-closed sentinels.

import test from "node:test";
import assert from "node:assert/strict";
import {
  enforceRunScopeConsistency,
  parseDurablePortfolioPositions,
  parseDurablePortfolioSnapshots,
  parseDurablePortfolioSummary,
  unavailableDurablePortfolioPositions,
} from "./durablePortfolio.ts";

function validSummaryRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    truth_state: "active",
    snapshot_truth_state: "active",
    snapshot_id: "snap-1",
    captured_at_utc: "2099-05-01T13:00:00Z",
    source: "external_alpaca",
    deployment_mode: "paper",
    account_equity: 100000,
    cash: 40000,
    currency: "USD",
    run_id: "run-a",
    operation_id: null,
    accounting_truth_state: "active",
    accounting_epoch: "complete",
    accounting_epoch_reason: null,
    accounting_source_snapshot_id: "snap-1",
    last_applied_inbox_id: 3,
    realized_pnl: 40,
    realized_pnl_truth_state: "active",
    realized_pnl_unavailable_reason: null,
    fees: 0,
    cumulative_cash_movement: -1500,
    unrealized_pnl: 0,
    unrealized_pnl_truth_state: "active",
    unrealized_pnl_unavailable_reason: null,
    daily_pnl: 0,
    daily_pnl_truth_state: "active",
    daily_pnl_unavailable_reason: null,
    blockers: [],
    ...overrides,
  };
}

function validPositionsRaw(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    truth_state: "active",
    snapshot_id: "snap-1",
    captured_at_utc: "2099-05-01T13:00:00Z",
    run_id: "run-a",
    positions: [{ symbol: "AAPL", qty_signed: 10, avg_entry_price: 150, provenance: "external_alpaca" }],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

test("parseDurablePortfolioSummary accepts a valid active response", () => {
  const parsed = parseDurablePortfolioSummary(validSummaryRaw());
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.run_id, "run-a");
  assert.equal(parsed.realized_pnl, 40);
  assert.equal(parsed.accounting_source_snapshot_id, "snap-1");
});

test("parseDurablePortfolioSummary fails closed on a malformed 200 body", () => {
  const parsed = parseDurablePortfolioSummary({ truth_state: "active" }); // missing everything else
  assert.equal(parsed.truth_state, "db_unavailable");
  assert.equal(parsed.run_id, null);
});

test("parseDurablePortfolioSummary fails closed on an unrecognized truth_state", () => {
  const parsed = parseDurablePortfolioSummary(validSummaryRaw({ truth_state: "totally_fine_trust_me" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseDurablePortfolioSummary fails closed on an unrecognized accounting_truth_state", () => {
  const parsed = parseDurablePortfolioSummary(validSummaryRaw({ accounting_truth_state: "made_up" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

test("parseDurablePortfolioSummary preserves null vs. zero distinction", () => {
  const withZero = parseDurablePortfolioSummary(validSummaryRaw({ realized_pnl: 0 }));
  assert.equal(withZero.realized_pnl, 0);
  const withNull = parseDurablePortfolioSummary(validSummaryRaw({ realized_pnl: null, realized_pnl_truth_state: "not_found" }));
  assert.equal(withNull.realized_pnl, null);
});

test("parseDurablePortfolioSummary preserves fill_history_incomplete", () => {
  const parsed = parseDurablePortfolioSummary(
    validSummaryRaw({
      accounting_truth_state: "fill_history_incomplete",
      realized_pnl: null,
      realized_pnl_truth_state: "fill_history_incomplete",
      accounting_epoch: "incomplete",
      accounting_epoch_reason: "position_quantity_mismatch:AAPL",
    }),
  );
  assert.equal(parsed.accounting_truth_state, "fill_history_incomplete");
  assert.equal(parsed.accounting_epoch_reason, "position_quantity_mismatch:AAPL");
  assert.equal(parsed.realized_pnl, null);
});

test("parseDurablePortfolioSummary preserves query_failed", () => {
  const parsed = parseDurablePortfolioSummary(
    validSummaryRaw({
      truth_state: "query_failed",
      snapshot_truth_state: "query_failed",
      accounting_truth_state: "query_failed",
      realized_pnl_truth_state: "query_failed",
      unrealized_pnl_truth_state: "query_failed",
      daily_pnl_truth_state: "query_failed",
      blockers: ["a durable portfolio query failed"],
    }),
  );
  assert.equal(parsed.truth_state, "query_failed");
});

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

test("parseDurablePortfolioPositions accepts a valid active response", () => {
  const parsed = parseDurablePortfolioPositions(validPositionsRaw());
  assert.equal(parsed.truth_state, "active");
  assert.equal(parsed.positions.length, 1);
  assert.equal(parsed.positions[0].symbol, "AAPL");
});

test("parseDurablePortfolioPositions fails closed on a malformed row", () => {
  const parsed = parseDurablePortfolioPositions(
    validPositionsRaw({ positions: [{ symbol: "AAPL", qty_signed: "ten" }] }),
  );
  assert.equal(parsed.truth_state, "db_unavailable");
  assert.deepEqual(parsed.positions, []);
});

test("parseDurablePortfolioPositions fails closed on an unrecognized truth_state", () => {
  const parsed = parseDurablePortfolioPositions(validPositionsRaw({ truth_state: "fabricated" }));
  assert.equal(parsed.truth_state, "db_unavailable");
});

// ---------------------------------------------------------------------------
// Snapshots (history order preserved)
// ---------------------------------------------------------------------------

test("parseDurablePortfolioSnapshots preserves history order", () => {
  const raw = {
    truth_state: "active",
    snapshots: [
      { snapshot_id: "s2", captured_at_utc: "t2", deployment_mode: "paper", source: "external_alpaca", equity: 2, cash: 2, currency: "USD", truth_state: "active", run_id: null, operation_id: null },
      { snapshot_id: "s1", captured_at_utc: "t1", deployment_mode: "paper", source: "external_alpaca", equity: 1, cash: 1, currency: "USD", truth_state: "active", run_id: null, operation_id: null },
    ],
  };
  const parsed = parseDurablePortfolioSnapshots(raw);
  assert.deepEqual(parsed.snapshots.map((s) => s.snapshot_id), ["s2", "s1"]);
});

test("parseDurablePortfolioSnapshots fails closed on malformed rows", () => {
  const parsed = parseDurablePortfolioSnapshots({ truth_state: "active", snapshots: [{ snapshot_id: "s1" }] });
  assert.equal(parsed.truth_state, "db_unavailable");
});

// ---------------------------------------------------------------------------
// Cross-response run-scoping
// ---------------------------------------------------------------------------

test("enforceRunScopeConsistency rejects positions naming a different run than an active summary", () => {
  const summary = parseDurablePortfolioSummary(validSummaryRaw({ run_id: "run-a" }));
  const positions = parseDurablePortfolioPositions(validPositionsRaw({ run_id: "run-b" }));
  const result = enforceRunScopeConsistency(summary, positions);
  assert.equal(result.truth_state, "query_failed");
  assert.deepEqual(result.positions, []);
});

test("enforceRunScopeConsistency passes through matching run_ids", () => {
  const summary = parseDurablePortfolioSummary(validSummaryRaw({ run_id: "run-a" }));
  const positions = parseDurablePortfolioPositions(validPositionsRaw({ run_id: "run-a" }));
  const result = enforceRunScopeConsistency(summary, positions);
  assert.equal(result.truth_state, "active");
  assert.equal(result.positions.length, 1);
});

test("enforceRunScopeConsistency does not reject when either side is not active", () => {
  const summary = parseDurablePortfolioSummary(validSummaryRaw({ run_id: "run-a", truth_state: "snapshot_stale" }));
  const positions = parseDurablePortfolioPositions(validPositionsRaw({ run_id: "run-b" }));
  const result = enforceRunScopeConsistency(summary, positions);
  // Not both "active", so the mismatch guard does not fire -- positions
  // pass through unchanged (the summary side is already flagged stale).
  assert.deepEqual(result, positions);
});

test("unavailableDurablePortfolioPositions sentinel has null run_id and empty rows", () => {
  const sentinel = unavailableDurablePortfolioPositions();
  assert.equal(sentinel.run_id, null);
  assert.deepEqual(sentinel.positions, []);
});
