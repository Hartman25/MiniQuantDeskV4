// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01
//
// Focused unit proofs for isAutonomousDailyOperationApiRow, isolated from the
// fetchOperatorModel/screen integration proofs in api.test.ts.

import test from "node:test";
import assert from "node:assert/strict";
import { isAutonomousDailyOperationApiRow } from "../../system/types/autonomousDailyOperations.ts";
import type { AutonomousDailyOperationApiRow } from "../../system/types.ts";

function validRow(overrides: Partial<AutonomousDailyOperationApiRow> = {}): AutonomousDailyOperationApiRow {
  return {
    operation_id: "11111111-1111-1111-1111-111111111111",
    market_date: "2026-07-22",
    deployment_mode: "paper",
    adapter_id: "alpaca",
    state: "completed_no_trade",
    state_reason_code: null,
    finalization_status: "finalized",
    outcome_class: "no_trade",
    outcome_reason_code: "no_trade_strategy_evaluated_no_signal",
    finalized_at_utc: "2026-07-22T20:15:00Z",
    run_id: "22222222-2222-2222-2222-222222222222",
    bars_observed: 12,
    bars_dispatched: 12,
    last_completed_bar_ts: 1700000000,
    last_dispatched_bar_ts: 1700000000,
    strategy_evaluation_count: 12,
    order_activity_count: 0,
    fill_count: 0,
    evidence_state: "complete",
    evidence_blockers: [],
    created_at_utc: "2026-07-22T13:30:00Z",
    updated_at_utc: "2026-07-22T20:15:00Z",
    ...overrides,
  };
}

// F1.11 item 13: a complete, valid no-trade row is accepted.
test("isAutonomousDailyOperationApiRow accepts a complete valid no-trade row", () => {
  assert.equal(isAutonomousDailyOperationApiRow(validRow()), true);
});

// F1.11 item 14: nullable count fields are accepted verbatim as null, never
// converted to 0 or dropped.
test("isAutonomousDailyOperationApiRow accepts nullable count fields without converting them", () => {
  const row = validRow({
    strategy_evaluation_count: null,
    order_activity_count: null,
    fill_count: null,
    last_completed_bar_ts: null,
    last_dispatched_bar_ts: null,
  });
  assert.equal(isAutonomousDailyOperationApiRow(row), true);
  assert.equal(row.strategy_evaluation_count, null);
  assert.equal(row.order_activity_count, null);
  assert.equal(row.fill_count, null);
});

// F1.11 item 15: an undefined count field is rejected, not silently
// converted to null or zero.
test("isAutonomousDailyOperationApiRow rejects an undefined count field rather than converting it", () => {
  const row = validRow() as unknown as Record<string, unknown>;
  delete row.strategy_evaluation_count;
  assert.equal(isAutonomousDailyOperationApiRow(row), false);
});

test("isAutonomousDailyOperationApiRow rejects NaN/infinite numeric fields", () => {
  assert.equal(isAutonomousDailyOperationApiRow(validRow({ bars_observed: Number.NaN })), false);
  assert.equal(isAutonomousDailyOperationApiRow(validRow({ bars_dispatched: Number.POSITIVE_INFINITY })), false);
  assert.equal(
    isAutonomousDailyOperationApiRow({ ...validRow(), strategy_evaluation_count: Number.NaN }),
    false,
  );
});

test("isAutonomousDailyOperationApiRow rejects a non-array evidence_blockers field", () => {
  assert.equal(
    isAutonomousDailyOperationApiRow({ ...validRow(), evidence_blockers: "not_an_array" }),
    false,
  );
});

test("isAutonomousDailyOperationApiRow rejects a non-string entry inside evidence_blockers", () => {
  assert.equal(
    isAutonomousDailyOperationApiRow({ ...validRow(), evidence_blockers: ["ok", 7] }),
    false,
  );
});

test("isAutonomousDailyOperationApiRow rejects an unrecognized finalization_status/outcome_class/evidence_state", () => {
  assert.equal(isAutonomousDailyOperationApiRow(validRow({ finalization_status: "unknown_status" as never })), false);
  assert.equal(isAutonomousDailyOperationApiRow(validRow({ outcome_class: "unknown_class" as never })), false);
  assert.equal(isAutonomousDailyOperationApiRow(validRow({ evidence_state: "unknown_state" as never })), false);
});

test("isAutonomousDailyOperationApiRow rejects non-object and null values", () => {
  assert.equal(isAutonomousDailyOperationApiRow(null), false);
  assert.equal(isAutonomousDailyOperationApiRow(undefined), false);
  assert.equal(isAutonomousDailyOperationApiRow("row"), false);
  assert.equal(isAutonomousDailyOperationApiRow([]), false);
});
