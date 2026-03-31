// core-rs/mqk-gui/src/features/system/gui_ops_surfaces.test.ts
//
// GUI-OPS-01/02 surface proof tests.
//
// These tests prove that:
//   1. mapExecutionOutboxWrapper canonicalizes truth_state correctly and preserves rows.
//   2. mapFillQualityWrapper canonicalizes truth_state correctly and preserves rows.
//   3. mapPaperJournalWrapper handles dual-lane truth_state independently.
//   4. executionOutboxNotice / fillQualityNotice / paperJournalLaneNotice return non-null
//      for every non-"active" state, proving the screen renders a notice rather than data.
//
// Screen-level fail-closed contract: a non-null notice from the notice function means
// the screen renders the notice string, NOT the data rows. This is enforced in the screen
// components by the guard `if (noticeFunction(surface)) { return <notice> }` pattern.
// These tests prove the notice functions always fire for unavailable states — so the
// screen cannot silently pass empty rows through as authoritative data.

import test from "node:test";
import assert from "node:assert/strict";
import {
  mapExecutionOutboxWrapper,
  mapFillQualityWrapper,
  mapPaperJournalWrapper,
  executionOutboxNotice,
  fillQualityNotice,
  paperJournalLaneNotice,
  type ExecutionOutboxWrapper,
  type FillQualityWrapper,
  type PaperJournalWrapper,
} from "./legacy.ts";

// ---------------------------------------------------------------------------
// mapExecutionOutboxWrapper
// ---------------------------------------------------------------------------

test("mapExecutionOutboxWrapper: null input → unavailable surface with empty rows", () => {
  const s = mapExecutionOutboxWrapper(null);
  assert.equal(s.truth_state, "unavailable");
  assert.equal(s.run_id, null);
  assert.equal(s.rows.length, 0);
});

test("mapExecutionOutboxWrapper: undefined input → unavailable surface", () => {
  const s = mapExecutionOutboxWrapper(undefined);
  assert.equal(s.truth_state, "unavailable");
});

test("mapExecutionOutboxWrapper: truth_state 'active' passes through with rows preserved", () => {
  const wrapper: ExecutionOutboxWrapper = {
    canonical_route: "/api/v1/execution/outbox",
    truth_state: "active",
    backend: "postgres.oms_outbox",
    run_id: "run-abc",
    rows: [
      {
        idempotency_key: "key-1",
        run_id: "run-abc",
        status: "sent",
        lifecycle_stage: "dispatched",
        symbol: "AAPL",
        side: "buy",
        qty: 100,
        order_type: "market",
        strategy_id: "strat-1",
        signal_source: "external",
        created_at_utc: "2026-03-30T09:00:00Z",
        claimed_at_utc: "2026-03-30T09:00:01Z",
        dispatching_at_utc: "2026-03-30T09:00:02Z",
        sent_at_utc: "2026-03-30T09:00:03Z",
      },
    ],
  };
  const s = mapExecutionOutboxWrapper(wrapper);
  assert.equal(s.truth_state, "active");
  assert.equal(s.run_id, "run-abc");
  assert.equal(s.rows.length, 1);
  assert.equal(s.rows[0].idempotency_key, "key-1");
  assert.equal(s.rows[0].symbol, "AAPL");
});

test("mapExecutionOutboxWrapper: truth_state 'no_active_run' passes through", () => {
  const wrapper: ExecutionOutboxWrapper = {
    canonical_route: "/api/v1/execution/outbox",
    truth_state: "no_active_run",
    backend: "postgres.oms_outbox",
    run_id: null,
    rows: [],
  };
  const s = mapExecutionOutboxWrapper(wrapper);
  assert.equal(s.truth_state, "no_active_run");
  assert.equal(s.run_id, null);
});

test("mapExecutionOutboxWrapper: unknown truth_state canonicalized to unavailable", () => {
  const wrapper = {
    canonical_route: "/api/v1/execution/outbox",
    truth_state: "some_future_state",
    backend: "postgres.oms_outbox",
    run_id: null,
    rows: [],
  } as unknown as ExecutionOutboxWrapper;
  const s = mapExecutionOutboxWrapper(wrapper);
  // Unknown states must be treated as unavailable — fail-closed.
  assert.equal(s.truth_state, "unavailable");
});

// ---------------------------------------------------------------------------
// mapFillQualityWrapper
// ---------------------------------------------------------------------------

test("mapFillQualityWrapper: null input → unavailable surface", () => {
  const s = mapFillQualityWrapper(null);
  assert.equal(s.truth_state, "unavailable");
  assert.equal(s.rows.length, 0);
});

test("mapFillQualityWrapper: truth_state 'active' passes through with rows", () => {
  const wrapper: FillQualityWrapper = {
    canonical_route: "/api/v1/execution/fill-quality",
    truth_state: "active",
    backend: "postgres.fill_quality_telemetry",
    rows: [
      {
        telemetry_id: "tel-1",
        run_id: "run-abc",
        internal_order_id: "ord-1",
        broker_order_id: "brk-1",
        symbol: "TSLA",
        side: "sell",
        ordered_qty: 50,
        fill_qty: 50,
        fill_price_micros: 250_000_000,
        reference_price_micros: 249_500_000,
        slippage_bps: 2,
        fill_kind: "full",
        fill_received_at_utc: "2026-03-30T10:00:00Z",
        submit_to_fill_ms: 123,
      },
    ],
  };
  const s = mapFillQualityWrapper(wrapper);
  assert.equal(s.truth_state, "active");
  assert.equal(s.rows.length, 1);
  assert.equal(s.rows[0].symbol, "TSLA");
  assert.equal(s.rows[0].fill_price_micros, 250_000_000);
});

test("mapFillQualityWrapper: unknown truth_state canonicalized to unavailable", () => {
  const wrapper = {
    canonical_route: "/api/v1/execution/fill-quality",
    truth_state: "experimental",
    backend: "postgres.fill_quality_telemetry",
    rows: [],
  } as unknown as FillQualityWrapper;
  const s = mapFillQualityWrapper(wrapper);
  assert.equal(s.truth_state, "unavailable");
});

// ---------------------------------------------------------------------------
// mapPaperJournalWrapper
// ---------------------------------------------------------------------------

test("mapPaperJournalWrapper: null input → both lanes unavailable", () => {
  const s = mapPaperJournalWrapper(null);
  assert.equal(s.fills_truth_state, "unavailable");
  assert.equal(s.admissions_truth_state, "unavailable");
  assert.equal(s.fills.length, 0);
  assert.equal(s.admissions.length, 0);
  assert.equal(s.run_id, null);
});

test("mapPaperJournalWrapper: both lanes active — rows and run_id preserved", () => {
  const wrapper: PaperJournalWrapper = {
    canonical_route: "/api/v1/paper/journal",
    run_id: "run-xyz",
    fills_lane: {
      truth_state: "active",
      backend: "postgres.fill_quality_telemetry",
      rows: [
        {
          telemetry_id: "t1",
          run_id: "run-xyz",
          internal_order_id: "o1",
          broker_order_id: null,
          symbol: "NVDA",
          side: "buy",
          ordered_qty: 10,
          fill_qty: 10,
          fill_price_micros: 800_000_000,
          reference_price_micros: null,
          slippage_bps: null,
          fill_kind: "full",
          fill_received_at_utc: "2026-03-30T11:00:00Z",
          submit_to_fill_ms: null,
        },
      ],
    },
    admissions_lane: {
      truth_state: "active",
      backend: "postgres.audit_events",
      rows: [
        {
          event_id: "evt-1",
          ts_utc: "2026-03-30T11:00:00Z",
          signal_id: "sig-1",
          strategy_id: "strat-1",
          symbol: "NVDA",
          side: "buy",
          qty: 10,
          run_id: "run-xyz",
        },
      ],
    },
  };
  const s = mapPaperJournalWrapper(wrapper);
  assert.equal(s.run_id, "run-xyz");
  assert.equal(s.fills_truth_state, "active");
  assert.equal(s.admissions_truth_state, "active");
  assert.equal(s.fills.length, 1);
  assert.equal(s.admissions.length, 1);
  assert.equal(s.fills[0].symbol, "NVDA");
  assert.equal(s.admissions[0].signal_id, "sig-1");
});

test("mapPaperJournalWrapper: independent lanes — fills active, admissions no_db", () => {
  const wrapper: PaperJournalWrapper = {
    canonical_route: "/api/v1/paper/journal",
    run_id: "run-xyz",
    fills_lane: {
      truth_state: "active",
      backend: "postgres.fill_quality_telemetry",
      rows: [],
    },
    admissions_lane: {
      truth_state: "no_db",
      backend: "postgres.audit_events",
      rows: [],
    },
  };
  const s = mapPaperJournalWrapper(wrapper);
  assert.equal(s.fills_truth_state, "active");
  assert.equal(s.admissions_truth_state, "no_db");
});

test("mapPaperJournalWrapper: unknown truth_state in either lane → unavailable for that lane", () => {
  const wrapper = {
    canonical_route: "/api/v1/paper/journal",
    run_id: null,
    fills_lane: { truth_state: "unknown_fills_state", backend: "x", rows: [] },
    admissions_lane: { truth_state: "active", backend: "y", rows: [] },
  } as unknown as PaperJournalWrapper;
  const s = mapPaperJournalWrapper(wrapper);
  assert.equal(s.fills_truth_state, "unavailable");
  assert.equal(s.admissions_truth_state, "active");
});

// ---------------------------------------------------------------------------
// Notice helpers — screen fail-closed proof
//
// For every non-"active" truth_state the notice function returns a non-null string.
// This is the contract that prevents the screen from rendering data rows as authoritative
// when truth is compromised: screen components check `if (notice) { render notice }`
// before ever touching the rows array.
// ---------------------------------------------------------------------------

test("executionOutboxNotice: returns null for active (data may render)", () => {
  assert.equal(executionOutboxNotice({ truth_state: "active", run_id: null, rows: [] }), null);
});

test("executionOutboxNotice: returns non-null for no_active_run (fail-closed)", () => {
  assert.notEqual(executionOutboxNotice({ truth_state: "no_active_run", run_id: null, rows: [] }), null);
});

test("executionOutboxNotice: returns non-null for no_db (fail-closed)", () => {
  assert.notEqual(executionOutboxNotice({ truth_state: "no_db", run_id: null, rows: [] }), null);
});

test("executionOutboxNotice: returns non-null for unavailable (fail-closed)", () => {
  assert.notEqual(executionOutboxNotice({ truth_state: "unavailable", run_id: null, rows: [] }), null);
});

test("fillQualityNotice: returns null for active (data may render)", () => {
  assert.equal(fillQualityNotice({ truth_state: "active", rows: [] }), null);
});

test("fillQualityNotice: returns non-null for no_active_run (fail-closed)", () => {
  assert.notEqual(fillQualityNotice({ truth_state: "no_active_run", rows: [] }), null);
});

test("fillQualityNotice: returns non-null for no_db (fail-closed)", () => {
  assert.notEqual(fillQualityNotice({ truth_state: "no_db", rows: [] }), null);
});

test("fillQualityNotice: returns non-null for unavailable (fail-closed)", () => {
  assert.notEqual(fillQualityNotice({ truth_state: "unavailable", rows: [] }), null);
});

test("paperJournalLaneNotice: returns null for active (data may render)", () => {
  assert.equal(paperJournalLaneNotice("active"), null);
});

test("paperJournalLaneNotice: returns non-null for no_active_run (fail-closed)", () => {
  assert.notEqual(paperJournalLaneNotice("no_active_run"), null);
});

test("paperJournalLaneNotice: returns non-null for no_db (fail-closed)", () => {
  assert.notEqual(paperJournalLaneNotice("no_db"), null);
});

test("paperJournalLaneNotice: returns non-null for unavailable (fail-closed)", () => {
  assert.notEqual(paperJournalLaneNotice("unavailable"), null);
});
