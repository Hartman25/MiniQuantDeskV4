// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION
//
// Integration proof over fetchOperatorModel() for the two accepted E4
// canonical routes plus the resulting model's projection into
// AutonomousDailyOperationsScreen. Server-side rendering via
// react-dom/server works fine under this repo's `tsx --test` harness
// (no DOM required) — the same pattern src/features/system/api.test.ts
// already uses throughout.

import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "../../system/api.ts";
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS } from "../../system/types.ts";
import type { AutonomousDailyOperationApiRow } from "../../system/types.ts";
import { AutonomousDailyOperationsScreen } from "../AutonomousDailyOperationsScreen.tsx";

const SINGLE_ROUTE = "/api/v1/autonomous/daily-operation";
const HISTORY_ROUTE = "/api/v1/autonomous/daily-operations";

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    async json() {
      return body;
    },
  } as Response;
}

function notFoundResponse(): Response {
  return jsonResponse({ error: "not found" }, 404);
}

function baseRow(overrides: Partial<AutonomousDailyOperationApiRow> = {}): AutonomousDailyOperationApiRow {
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

async function withMockedFetch<T>(
  routes: Record<string, () => Response>,
  fn: () => Promise<T>,
): Promise<T> {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;
    const handler = routes[path];
    if (handler) return handler();
    return notFoundResponse();
  }) as typeof fetch;
  try {
    return await fn();
  } finally {
    globalThis.fetch = originalFetch;
  }
}

function minimalRoutes(extra: Record<string, () => Response> = {}): Record<string, () => Response> {
  return {
    "/api/v1/system/status": () =>
      jsonResponse({ ...DEFAULT_STATUS, runtime_status: "running", daemon_reachable: true }),
    "/api/v1/system/preflight": () => jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true }),
    ...extra,
  };
}

// F1.11 item 1: both canonical routes are requested as part of the tracked
// probe assembly.
test("fetchOperatorModel requests both canonical daily-operation routes", async () => {
  const seen: string[] = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;
    seen.push(path);
    if (path === SINGLE_ROUTE) {
      return jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "not_found", operation: null, message: "none" });
    }
    if (path === HISTORY_ROUTE) {
      return jsonResponse({
        canonical_route: HISTORY_ROUTE,
        truth_state: "active",
        requested_limit: 20,
        effective_limit: 20,
        rows: [],
        message: null,
      });
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    await fetchOperatorModel();
    assert.ok(seen.includes(SINGLE_ROUTE), "single-operation route must be requested");
    assert.ok(seen.includes(HISTORY_ROUTE), "history route must be requested");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// F1.11 item 2: successful not_found is authoritative, not relabeled.
test("not_found stays authoritative — realEndpoints includes the route, truth_state is not_found", async () => {
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({
          canonical_route: SINGLE_ROUTE,
          truth_state: "not_found",
          operation: null,
          message: "no autonomous daily operation exists yet",
        }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "available");
  assert.equal(model.autonomousDailyOperation.truth_state, "not_found");
  assert.equal(model.autonomousDailyOperation.operation, null);
  assert.ok(model.dataSource.realEndpoints.includes(SINGLE_ROUTE));

  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /No autonomous daily operation exists for the current canonical slot\./);
});

// F1.11 item 3/4: backend_unavailable and query_failed stay distinct from
// each other and from not_found/endpoint_unavailable.
test("backend_unavailable and query_failed remain distinct truth states", async () => {
  const backendUnavailableModel = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "backend_unavailable", operation: null, message: "no db" }),
    }),
    fetchOperatorModel,
  );
  assert.equal(backendUnavailableModel.autonomousDailyOperation.truth_state, "backend_unavailable");

  const queryFailedModel = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "query_failed", operation: null, message: "read failed" }),
    }),
    fetchOperatorModel,
  );
  assert.equal(queryFailedModel.autonomousDailyOperation.truth_state, "query_failed");
  assert.notEqual(
    backendUnavailableModel.autonomousDailyOperation.truth_state,
    queryFailedModel.autonomousDailyOperation.truth_state,
  );
});

// F1.11 item 5: a network/HTTP failure becomes the GUI-only endpoint_unavailable
// sentinel — it must never be reported as the daemon's own not_found.
test("endpoint/network failure maps to GUI-only endpoint_unavailable, never daemon not_found", async () => {
  const model = await withMockedFetch(minimalRoutes(), fetchOperatorModel);
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
  assert.equal(model.autonomousDailyOperations.transport_state, "endpoint_unavailable");
  assert.ok(model.dataSource.missingEndpoints.includes(SINGLE_ROUTE));
  assert.ok(model.dataSource.missingEndpoints.some((e) => e.startsWith(HISTORY_ROUTE)));
});

// F1.11 item 6: an active no-trade row maps without reinterpretation.
test("active no-trade row renders finalized/no_trade/complete verbatim", async () => {
  const row = baseRow();
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: row, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.operation?.outcome_class, "no_trade");
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /finalized/);
  assert.match(html, /no_trade/);
  assert.match(html, /complete/);
});

// F1.11 item 7: an active activity row maps without reinterpretation.
test("active with-activity row renders finalized/with_activity verbatim", async () => {
  const row = baseRow({
    state: "completed_with_activity",
    outcome_class: "with_activity",
    outcome_reason_code: "activity_fill_confirmed",
    order_activity_count: 2,
    fill_count: 1,
  });
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: row, message: null }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /with_activity/);
  assert.match(html, />1</); // fill_count rendered verbatim
});

// F1.11 item 8: awaiting-finalization truth is retained, not shown as finalized.
test("awaiting_finalization is retained as pending, not finalized", async () => {
  const row = baseRow({
    state: "stopping",
    finalization_status: "awaiting_finalization",
    outcome_class: null,
    outcome_reason_code: null,
    finalized_at_utc: null,
    evidence_state: "pending",
  });
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: row, message: null }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /awaiting_finalization/);
  assert.doesNotMatch(html, />finalized</);
});

// F1.11 item 9: evidence-degraded blockers are retained and visible.
test("evidence-degraded blockers are surfaced verbatim", async () => {
  const row = baseRow({
    state: "evidence_degraded",
    state_reason_code: "unknown_unresolved_dispatch_claim",
    finalization_status: "blocked_insufficient_evidence",
    outcome_class: null,
    outcome_reason_code: null,
    finalized_at_utc: null,
    evidence_state: "degraded",
    evidence_blockers: ["unknown_unresolved_dispatch_claim"],
  });
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: row, message: null }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /blocked_insufficient_evidence/);
  assert.match(html, /unknown_unresolved_dispatch_claim/);
});

// F1.11 item 10: generic completed remains generic, never relabeled as
// automatic no-trade/with-activity evidence-complete proof.
test("generic completed remains generic, evidence pending not complete", async () => {
  const row = baseRow({
    state: "completed",
    outcome_class: "completed",
    outcome_reason_code: null,
    evidence_state: "pending",
  });
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: row, message: null }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, />completed</);
  assert.match(html, />pending</);
});

// F1.11 item 13/14: history order is preserved verbatim; an authoritative
// empty history is active, not unavailable.
test("history rows preserve daemon order; empty active history is authoritative", async () => {
  const rowA = baseRow({ operation_id: "aaaa", market_date: "2026-07-20" });
  const rowB = baseRow({ operation_id: "bbbb", market_date: "2026-07-21" });
  const model = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "active",
          requested_limit: 20,
          effective_limit: 20,
          rows: [rowA, rowB],
          message: null,
        }),
    }),
    fetchOperatorModel,
  );
  assert.deepEqual(
    model.autonomousDailyOperations.rows.map((r) => r.operation_id),
    ["aaaa", "bbbb"],
  );

  const emptyModel = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "active",
          requested_limit: 20,
          effective_limit: 20,
          rows: [],
          message: null,
        }),
    }),
    fetchOperatorModel,
  );
  assert.equal(emptyModel.autonomousDailyOperations.truth_state, "active");
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model: emptyModel }));
  assert.match(html, /No autonomous daily operations recorded yet\./);
});

// F1.11 item 15: a top-level query_failed history response keeps its
// (bounded) rows visible but clearly labeled partial/unavailable.
test("query_failed history keeps rows but renders a partial notice", async () => {
  const row = baseRow({ strategy_evaluation_count: null, order_activity_count: null, fill_count: null, evidence_state: "unavailable" });
  const model = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "query_failed",
          requested_limit: 20,
          effective_limit: 20,
          rows: [row],
          message: "count read failed",
        }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /Partial — some rows unavailable/);
  assert.match(html, /Unavailable/); // null counts render as Unavailable, never 0
});

// F1.11 item 20: existing dashboard/session/preflight truth is unaffected by
// the new probes being inserted into the tracked probe array.
test("existing preflight/status truth is unaffected by the new daily-operation probes", async () => {
  const model = await withMockedFetch(minimalRoutes(), fetchOperatorModel);
  assert.equal(model.connected, true);
  assert.equal(model.preflight.daemon_reachable, true);
  assert.equal(model.status.runtime_status, "running");
});

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01
// ---------------------------------------------------------------------------

// Repair item 1: "active" + a missing/null operation must fail closed, and
// must never render as the neutral not_found copy.
test("active with null operation maps to endpoint_unavailable, never authoritative not_found", async () => {
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: null, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.doesNotMatch(html, /No autonomous daily operation exists for the current canonical slot\./);
});

// Repair item 2: "not_found" + a non-null operation must fail closed.
test("not_found with a non-null operation maps to endpoint_unavailable", async () => {
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "not_found", operation: baseRow(), message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
});

// Repair item 3: an active row missing evidence_blockers must fail closed.
test("active row missing evidence_blockers maps to endpoint_unavailable", async () => {
  const malformedRow = baseRow() as unknown as Record<string, unknown>;
  delete malformedRow.evidence_blockers;
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: malformedRow, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
});

// Repair item 4: an active row with a non-array blocker field must fail closed.
test("active row with non-array evidence_blockers maps to endpoint_unavailable", async () => {
  const malformedRow = { ...baseRow(), evidence_blockers: "not_an_array" };
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: malformedRow, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
});

// Repair item 5: an active row with an unknown finalization_status must fail closed.
test("active row with unknown finalization_status maps to endpoint_unavailable", async () => {
  const malformedRow = { ...baseRow(), finalization_status: "some_future_unknown_status" };
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: malformedRow, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
});

// Repair item 6: an active row with an unknown evidence_state must fail closed.
test("active row with unknown evidence_state maps to endpoint_unavailable", async () => {
  const malformedRow = { ...baseRow(), evidence_state: "some_future_unknown_state" };
  const model = await withMockedFetch(
    minimalRoutes({
      [SINGLE_ROUTE]: () =>
        jsonResponse({ canonical_route: SINGLE_ROUTE, truth_state: "active", operation: malformedRow, message: null }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperation.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperation.truth_state, null);
});

// Repair item 7: an active history response missing `rows` must fail closed,
// never render as an authoritative empty history.
test("active history response missing rows maps to endpoint_unavailable, never authoritative empty history", async () => {
  const model = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "active",
          requested_limit: 20,
          effective_limit: 20,
          message: null,
        }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperations.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperations.truth_state, null);
  assert.deepEqual(model.autonomousDailyOperations.rows, []);
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.doesNotMatch(html, /No autonomous daily operations recorded yet\./);
});

// Repair item 8: one malformed row in an otherwise-valid history response
// demotes the whole surface to endpoint_unavailable.
test("history containing one malformed row maps the whole surface to endpoint_unavailable", async () => {
  const validRow = baseRow({ operation_id: "cccc" });
  const malformedRow = { ...baseRow({ operation_id: "dddd" }), evidence_state: "some_future_unknown_state" };
  const model = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "active",
          requested_limit: 20,
          effective_limit: 20,
          rows: [validRow, malformedRow],
          message: null,
        }),
    }),
    fetchOperatorModel,
  );
  assert.equal(model.autonomousDailyOperations.transport_state, "endpoint_unavailable");
  assert.equal(model.autonomousDailyOperations.truth_state, null);
  assert.deepEqual(model.autonomousDailyOperations.rows, []);
});

// Repair item 11: every evidence blocker on a history row is rendered, not
// just the first, and not deduplicated.
test("a history row with two evidence blockers renders both blocker strings", async () => {
  const row = baseRow({
    evidence_state: "degraded",
    evidence_blockers: ["unknown_unresolved_dispatch_claim", "unknown_missing_evaluation_evidence"],
  });
  const model = await withMockedFetch(
    minimalRoutes({
      [HISTORY_ROUTE]: () =>
        jsonResponse({
          canonical_route: HISTORY_ROUTE,
          truth_state: "active",
          requested_limit: 20,
          effective_limit: 20,
          rows: [row],
          message: null,
        }),
    }),
    fetchOperatorModel,
  );
  const html = renderToStaticMarkup(React.createElement(AutonomousDailyOperationsScreen, { model }));
  assert.match(html, /unknown_unresolved_dispatch_claim/);
  assert.match(html, /unknown_missing_evaluation_evidence/);
});
