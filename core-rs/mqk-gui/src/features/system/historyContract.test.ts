import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "./api";
import { parseEventFeed, parseOperatorTimeline } from "./historyContract";
import { OperatorTimelineScreen } from "../operatorTimeline/OperatorTimelineScreen";
import { DEFAULT_STATUS } from "./types";
import { formatDateTime, formatNumber } from "../../lib/format";

const a = { provenance_ref: "audit_events:a", ts_utc: "2026-10-10T10:00:00Z", kind: "operator_action", detail: "stop-system", run_id: "run-1", audit_event_id: "a" };
const b = { ...a, provenance_ref: "audit_events:b", audit_event_id: "b" };
const c = { ...a, provenance_ref: "audit_events:c", audit_event_id: "c", ts_utc: "2026-10-10T10:00:01Z" };
test("chronology preserves event identity across duplicate, late and tied delivery; conflicting evidence is rejected", () => {
  const first = parseOperatorTimeline({ truth_state: "active", rows: [b, a, c, a] })!;
  const second = parseOperatorTimeline({ truth_state: "active", rows: [a, c, b, c] })!;
  assert.deepEqual(first, second);
  assert.deepEqual(first.map((row) => row.timeline_event_id), ["audit_events:c", "audit_events:a", "audit_events:b"]);
  assert.equal(first[0].audit_event_id, "c");
  assert.equal(parseOperatorTimeline({ truth_state: "active", rows: [a, { ...a, detail: "start-system" }] }), null);
  for (const body of [{ truth_state: "future", rows: [] }, { truth_state: "active", rows: [{}] },
    { truth_state: "active", rows: [{ ...a, ts_utc: "2026-10-10T10:00:00" }] }]) assert.equal(parseOperatorTimeline(body), null);
  const halt = { event_id: "audit_events:halt", ts_utc: a.ts_utc, kind: "orchestrator_halt", detail: "Risk limit", run_id: "run-1", audit_event_id: "halt" };
  const feed = parseEventFeed({ truth_state: "active", rows: [halt, halt] })!;
  assert.equal(feed.length, 1);
  assert.equal(feed[0].severity, "critical");
  assert.equal(parseEventFeed({ truth_state: "future", rows: [] }), null);
  assert.match(formatDateTime(a.ts_utc), /UTC/);
  assert.equal(formatNumber(Infinity), "—");
});

test("production timeline renders bounded history with source identities and duplicate-free ordering", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const path = new URL(String(input)).pathname;
    if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
    if (path === "/api/v1/ops/operator-timeline") return Response.json({ truth_state: "active", rows: [b, c, a, a] });
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.operatorTimeline.length, 3);
    const html = renderToStaticMarkup(React.createElement(OperatorTimelineScreen, { model }));
    assert.match(html, /Older events may be omitted/);
    assert.match(html, /audit_events:c/);
    assert.match(html, /UTC/);
    assert.doesNotMatch(html, /complete chronological record/);
  } finally { globalThis.fetch = original; }
});
