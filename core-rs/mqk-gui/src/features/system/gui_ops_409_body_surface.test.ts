// core-rs/mqk-gui/src/features/system/gui_ops_409_body_surface.test.ts
//
// GUI-OPERATOR-ACTION-409-BODY-SURFACE-01
//
// Confirmed defect: postJson (http.ts) collapsed every non-2xx daemon
// response to a bare `HTTP <status>` string, discarding the daemon's
// structured OperatorActionResponse body (accepted/disposition/blockers)
// on POST /api/v1/ops/action. invokeOperatorAction (actions.ts) then could
// never surface the real refusal reason for a 409 (e.g. clear-halted-run
// refused because of an active runtime lease or a still-live local
// execution-loop task -- see mqk-daemon/src/routes/control_plane.rs).
//
// These tests exercise the real fetch -> postJson -> invokeOperatorAction
// path end-to-end (globalThis.fetch mocked; no other code path shadowed),
// proving:
//   B1: 409 + blockers -> blockers visible verbatim in blocking_failures.
//   B2: malformed JSON body -> no crash, truthful failure.
//   B3: 409 with no body -> truthful failure, not reinterpreted as success.
//   B4: 500 with a text/html body -> truthful failure, body never parsed as JSON.
//   B5: 409 with an unrecognized-but-valid JSON shape -> no fabricated blocker.
//   Positive control: 200 success path is unchanged.

import test from "node:test";
import assert from "node:assert/strict";
import { invokeOperatorAction } from "./actions.ts";

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name: string) => (name.toLowerCase() === "content-type" ? "application/json" : null) },
    async json() {
      return body;
    },
  } as unknown as Response;
}

function textResponse(body: string, status: number): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name: string) => (name.toLowerCase() === "content-type" ? "text/html" : null) },
    async json() {
      throw new SyntaxError("Unexpected token < in JSON");
    },
    async text() {
      return body;
    },
  } as unknown as Response;
}

function emptyBodyResponse(status: number): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: () => null },
    async json() {
      throw new SyntaxError("Unexpected end of JSON input");
    },
  } as unknown as Response;
}

function malformedJsonResponse(status: number): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name: string) => (name.toLowerCase() === "content-type" ? "application/json" : null) },
    async json() {
      throw new SyntaxError("Unexpected token in JSON at position 0");
    },
  } as unknown as Response;
}

async function withMockFetch<T>(handler: typeof fetch, fn: () => Promise<T>): Promise<T> {
  const original = globalThis.fetch;
  globalThis.fetch = handler;
  try {
    return await fn();
  } finally {
    globalThis.fetch = original;
  }
}

// ---------------------------------------------------------------------------
// B1: 409 + structured blockers -- must be visible verbatim.
// ---------------------------------------------------------------------------
test("B1: 409 with blockers surfaces the real daemon refusal reason verbatim", async () => {
  const receipt = await withMockFetch(
    (async () =>
      jsonResponse(
        {
          requested_action: "clear-halted-run",
          accepted: false,
          disposition: "active_runtime_lease",
          blockers: [
            "runtime.clear_halted.active_runtime_lease: run abc-123 still has an unexpired runtime leader lease",
          ],
          warnings: [],
          environment: "paper",
        },
        409,
      )) as typeof fetch,
    () => invokeOperatorAction("clear-halted-run", {}),
  );

  assert.equal(receipt.ok, false);
  assert.equal(receipt.result_state, "active_runtime_lease");
  assert.equal(receipt.blocking_failures.length, 1);
  assert.match(receipt.blocking_failures[0], /active_runtime_lease/);
  assert.match(receipt.blocking_failures[0], /unexpired runtime leader lease/);
});

// ---------------------------------------------------------------------------
// B2: malformed JSON body -- no crash, truthful failure.
// ---------------------------------------------------------------------------
test("B2: malformed JSON body on a 409 does not crash and reports a truthful failure", async () => {
  const receipt = await withMockFetch(
    (async () => malformedJsonResponse(409)) as typeof fetch,
    () => invokeOperatorAction("arm-execution", {}),
  );

  assert.equal(receipt.ok, false);
  assert.notEqual(receipt.result_state, "accepted");
  assert.ok(receipt.blocking_failures.length > 0, "must report at least one truthful blocking failure");
});

// ---------------------------------------------------------------------------
// B3: 409 without a body -- truthful failure, never reinterpreted as success.
// ---------------------------------------------------------------------------
test("B3: 409 without a body is a truthful failure, not a fabricated success", async () => {
  const receipt = await withMockFetch(
    (async () => emptyBodyResponse(409)) as typeof fetch,
    () => invokeOperatorAction("disarm-execution", {}),
  );

  assert.equal(receipt.ok, false);
  assert.notEqual(receipt.result_state, "accepted");
  assert.ok(receipt.blocking_failures.length > 0);
});

// ---------------------------------------------------------------------------
// B4: 500 with a text/html body -- truthful failure, body never parsed as JSON.
// ---------------------------------------------------------------------------
test("B4: 500 with a text/html error body is a truthful failure and is never parsed as JSON", async () => {
  const receipt = await withMockFetch(
    (async () => textResponse("<html><body>Internal Server Error</body></html>", 500)) as typeof fetch,
    () => invokeOperatorAction("kill-switch", {}),
  );

  assert.equal(receipt.ok, false);
  assert.notEqual(receipt.result_state, "accepted");
  assert.ok(receipt.blocking_failures.length > 0);
});

// ---------------------------------------------------------------------------
// B5: 409 with a well-formed but unrecognized JSON shape -- no fabricated blocker.
// ---------------------------------------------------------------------------
test("B5: 409 with an unrecognized JSON shape never fabricates a blocker from unknown fields", async () => {
  const receipt = await withMockFetch(
    (async () => jsonResponse({ some_unrelated_field: "value", nested: { x: 1 } }, 409)) as typeof fetch,
    () => invokeOperatorAction("flatten-paper-positions", {}),
  );

  assert.equal(receipt.ok, false);
  // The fallback message must be generic (derived from status/actionKey), never
  // invented from "some_unrelated_field" or "nested".
  for (const line of receipt.blocking_failures) {
    assert.doesNotMatch(line, /some_unrelated_field/);
    assert.doesNotMatch(line, /nested/);
  }
});

// ---------------------------------------------------------------------------
// Positive control: 200 success path is unchanged.
// ---------------------------------------------------------------------------
test("positive control: 200 success with accepted:true and blockers:[] is unaffected", async () => {
  const receipt = await withMockFetch(
    (async () =>
      jsonResponse(
        {
          requested_action: "arm-execution",
          accepted: true,
          disposition: "armed",
          blockers: [],
          warnings: [],
          environment: "paper",
        },
        200,
      )) as typeof fetch,
    () => invokeOperatorAction("arm-execution", {}),
  );

  assert.equal(receipt.ok, true);
  assert.equal(receipt.result_state, "armed");
  assert.equal(receipt.blocking_failures.length, 0);
});
