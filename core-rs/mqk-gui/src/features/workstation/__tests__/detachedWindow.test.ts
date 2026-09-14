import test from "node:test";
import assert from "node:assert/strict";
import { panelWindowLabel, parsePanelIdFromLabel, readDetachedPanelBootstrap, detachPanel, resolveReattachCollision } from "../detachedWindow.ts";
import { PANEL_REGISTRY } from "../panelRegistry.ts";
import { parsePanelLinkStatePayload, pinPanel, type WorkspaceIdentity } from "../../workspace/workspaceModel.ts";

test("panelWindowLabel/parsePanelIdFromLabel round-trip for a known panel", () => {
  const label = panelWindowLabel("marketData");
  assert.equal(label, "panel-marketData");
  assert.equal(parsePanelIdFromLabel(label), "marketData");
});

test("parsePanelIdFromLabel fails closed on a non-panel label or an unknown panel id", () => {
  assert.equal(parsePanelIdFromLabel("execution"), null);
  assert.equal(parsePanelIdFromLabel("oversight"), null);
  assert.equal(parsePanelIdFromLabel("panel-not-a-real-panel"), null);
  assert.equal(parsePanelIdFromLabel(""), null);
});

test("readDetachedPanelBootstrap parses a well-formed query string with no pinned identity", () => {
  const bootstrap = readDetachedPanelBootstrap("?detachedPanel=portfolio&opener=control");
  assert.deepEqual(bootstrap, { panelId: "portfolio", openerLabel: "control", pinnedIdentity: null });
});

test("readDetachedPanelBootstrap fails closed to null on missing/unknown/malformed values", () => {
  assert.equal(readDetachedPanelBootstrap(""), null);
  assert.equal(readDetachedPanelBootstrap("?opener=control"), null, "missing detachedPanel");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=portfolio"), null, "missing opener");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=not-a-real-panel&opener=control"), null, "unknown panel id");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=&opener="), null, "empty values");
});

test("readDetachedPanelBootstrap parses a well-formed pinnedIdentity", () => {
  const identity = {
    symbol: "AAPL",
    timeframe: "5m",
    strategyId: null,
    runId: null,
    backtestJobId: null,
    artifactId: null,
    evaluationSlice: null,
    executionDomain: null,
  };
  const search = `?detachedPanel=marketData&opener=control&pinnedIdentity=${encodeURIComponent(JSON.stringify(identity))}`;
  const bootstrap = readDetachedPanelBootstrap(search);
  assert.deepEqual(bootstrap?.pinnedIdentity, identity);
});

test("readDetachedPanelBootstrap fails closed on a malformed/foreign pinnedIdentity but still resolves panelId/opener", () => {
  const search = `?detachedPanel=marketData&opener=control&pinnedIdentity=${encodeURIComponent(JSON.stringify({ symbol: "AAPL", armed: true }))}`;
  const bootstrap = readDetachedPanelBootstrap(search);
  assert.ok(bootstrap);
  assert.equal(bootstrap.pinnedIdentity, null, "a smuggled authority-shaped key must void the pinned identity, not the whole bootstrap");
  assert.equal(bootstrap.panelId, "marketData");

  const invalidJson = readDetachedPanelBootstrap("?detachedPanel=marketData&opener=control&pinnedIdentity=not-json");
  assert.deepEqual(invalidJson, { panelId: "marketData", openerLabel: "control", pinnedIdentity: null });
});

// WAVE-02-FINAL-REPAIR-01 R2: panel-ops bootstrap fails closed.
test("readDetachedPanelBootstrap rejects a non-detachable (operator-singleton) panel id even from a well-formed URL", () => {
  assert.equal(PANEL_REGISTRY.ops.detachable, false, "precondition: ops must be non-detachable");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=ops&opener=control"), null);
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=ops&opener=execution"), null, "even a stale/manual URL claiming a different opener must fail closed");
});

test("detachPanel refuses a non-detachable panel id even if called directly (defense in depth)", async () => {
  const result = await detachPanel("ops", "control", null);
  assert.deepEqual(result, { ok: false, reason: "error" });
});

// WAVE-02-FINAL-REPAIR-01 R3 collision repair: pendingLinkStateRef must
// never survive the exact panel creation it belongs to. These cover the
// collision-decision policy at resolveReattachCollision's own level (the
// pure choke point Workstation.tsx's REATTACH_EVENT listener routes
// through), matching Negative Controls A/C/D/E from the mission.

test("resolveReattachCollision replaces an already-open ordinary panel instead of leaving it in place (control A/C: collision must not orphan the staged linkState)", () => {
  assert.equal(PANEL_REGISTRY.marketData.detachable, true, "precondition: marketData is detachable/contextAware");
  assert.equal(PANEL_REGISTRY.marketData.authority, "read-only");
  assert.equal(resolveReattachCollision(true, "marketData"), "replace-existing");
  // Same decision regardless of whether the reattaching linkState is pinned
  // or Linked — the collision itself, not the payload's content, is what
  // determines replacement.
  assert.equal(resolveReattachCollision(true, "backtests"), "replace-existing");
});

test("resolveReattachCollision creates fresh when no local instance is open (the ordinary, non-colliding reattach path is unchanged)", () => {
  assert.equal(resolveReattachCollision(false, "marketData"), "create");
});

test("a well-formed pinned linkState survives the collision/replace path unchanged (control A: reattaching AAPL pin over a local duplicate)", () => {
  assert.equal(resolveReattachCollision(true, "marketData"), "replace-existing");
  const identity: WorkspaceIdentity = {
    symbol: "AAPL",
    timeframe: null,
    strategyId: null,
    runId: null,
    backtestJobId: null,
    artifactId: null,
    evaluationSlice: null,
    executionDomain: null,
  };
  const linkState = parsePanelLinkStatePayload({ pinned: true, pinnedIdentity: identity });
  assert.deepEqual(linkState, pinPanel(identity), "the validated pin must reach the replacement panel intact");
});

test("a malformed reattach linkState payload still fails closed on the collision/replace path (control D)", () => {
  assert.equal(resolveReattachCollision(true, "marketData"), "replace-existing");
  const linkState = parsePanelLinkStatePayload({ pinned: true, pinnedIdentity: { symbol: "AAPL", armed: true } });
  assert.equal(linkState.pinned, false, "a smuggled/foreign payload must fail closed to unpinned, not partially trust it, even when the reattach also collides locally");
});

test("resolveReattachCollision never replaces an operator-singleton panel, collision or not (control E: ops behavior unchanged)", () => {
  assert.equal(PANEL_REGISTRY.ops.authority, "operator-singleton");
  assert.equal(resolveReattachCollision(true, "ops"), "focus-existing-singleton");
  assert.equal(resolveReattachCollision(false, "ops"), "create");
});
