// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION
//
// Registration, mutation-free, and fallback-truth proofs for
// AutonomousDailyOperationsScreen.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MONITOR_GROUPS, SCREEN_REGISTRY } from "../../screens/screenRegistry.tsx";
import { LEFT_RAIL_PRIMARY, LEFT_RAIL_SECONDARY } from "../../../components/layout/leftRailNav.ts";
import { mapAutonomousDailyOperationResponse, mapAutonomousDailyOperationsResponse } from "../../system/legacy.ts";
import type { SystemModel } from "../../system/types.ts";
import { AutonomousDailyOperationsScreen } from "../AutonomousDailyOperationsScreen.tsx";

const __dirname = dirname(fileURLToPath(import.meta.url));
const screenSource = readFileSync(join(__dirname, "..", "AutonomousDailyOperationsScreen.tsx"), "utf-8");

// F1.11 item 16: registered under the operator monitor group, reachable from the left rail.
test("dailyOperations is registered as an operator-group screen and reachable from the left rail", () => {
  assert.equal(SCREEN_REGISTRY.dailyOperations.title, "Daily Operations");
  assert.equal(SCREEN_REGISTRY.dailyOperations.monitorGroup, "operator");
  assert.ok(MONITOR_GROUPS.operator.includes("dailyOperations"), "dailyOperations must be in MONITOR_GROUPS.operator");
  const allLeftRail = [...LEFT_RAIL_PRIMARY, ...LEFT_RAIL_SECONDARY];
  assert.ok(allLeftRail.includes("dailyOperations"), "dailyOperations must be reachable from the left rail nav");
});

// F1.11 item 17: no action/mutation controls exist on this screen — source-text
// proof (no button elements, no postJson/invokeOperatorAction/onRunAction
// reference) plus a rendered-output proof (no <button> in the static markup).
test("AutonomousDailyOperationsScreen source contains no mutating call or control", () => {
  const forbidden = [
    "postJson",
    "invokeOperatorAction",
    "onRunAction",
    "<button",
    "onClick",
    "fetch(",
    "await fetch",
  ];
  for (const term of forbidden) {
    assert.ok(!screenSource.includes(term), `AutonomousDailyOperationsScreen.tsx must not contain '${term}'`);
  }
});

test("rendered markup contains no button or form element for any surface state", () => {
  const emptyModelBase = {
    autonomousDailyOperation: {
      transport_state: "endpoint_unavailable" as const,
      canonical_route: null,
      truth_state: null,
      operation: null,
      message: null,
    },
    autonomousDailyOperations: {
      transport_state: "endpoint_unavailable" as const,
      canonical_route: null,
      truth_state: null,
      requested_limit: null,
      effective_limit: null,
      rows: [],
      message: null,
    },
  };
  const html = renderToStaticMarkup(
    React.createElement(AutonomousDailyOperationsScreen, { model: emptyModelBase as unknown as SystemModel }),
  );
  assert.doesNotMatch(html, /<button/);
  assert.doesNotMatch(html, /<form/);
  assert.doesNotMatch(html, /<input/);
});

// F1.11 item 19: the fallback/unavailable mapping never fabricates healthy
// daily-operation truth from a null/malformed wrapper.
test("mapAutonomousDailyOperationResponse(null) fabricates no healthy truth", () => {
  const surface = mapAutonomousDailyOperationResponse(null);
  assert.equal(surface.transport_state, "endpoint_unavailable");
  assert.equal(surface.truth_state, null);
  assert.equal(surface.operation, null);
});

test("mapAutonomousDailyOperationsResponse(null) fabricates no healthy history", () => {
  const surface = mapAutonomousDailyOperationsResponse(null);
  assert.equal(surface.transport_state, "endpoint_unavailable");
  assert.equal(surface.truth_state, null);
  assert.deepEqual(surface.rows, []);
});

// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01
// Repair item 4: the screen itself stays defensive even if a malformed
// surface reached it directly (bypassing the mapper) — an "active" +
// null-operation surface must render the malformed notice, never the
// neutral not_found copy.
test("screen renders a malformed notice, not the neutral not_found copy, for active truth_state with null operation", () => {
  const modelBase = {
    autonomousDailyOperation: {
      transport_state: "available" as const,
      canonical_route: "/api/v1/autonomous/daily-operation",
      truth_state: "active" as const,
      operation: null,
      message: null,
    },
    autonomousDailyOperations: {
      transport_state: "available" as const,
      canonical_route: "/api/v1/autonomous/daily-operations",
      truth_state: "active" as const,
      requested_limit: 20,
      effective_limit: 20,
      rows: [],
      message: null,
    },
  };
  const html = renderToStaticMarkup(
    React.createElement(AutonomousDailyOperationsScreen, { model: modelBase as unknown as SystemModel }),
  );
  assert.match(html, /Malformed daily-operation response/);
  assert.doesNotMatch(html, /No autonomous daily operation exists for the current canonical slot\./);
});

test("an unrecognized truth_state never falls through as authoritative", () => {
  const surface = mapAutonomousDailyOperationResponse({
    canonical_route: "/api/v1/autonomous/daily-operation",
    truth_state: "some_future_unknown_state",
    operation: null,
    message: null,
  });
  assert.equal(surface.transport_state, "endpoint_unavailable");
  assert.equal(surface.truth_state, null);
});
