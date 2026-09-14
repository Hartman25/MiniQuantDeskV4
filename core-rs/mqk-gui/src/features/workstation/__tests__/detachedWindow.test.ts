import test from "node:test";
import assert from "node:assert/strict";
import { panelWindowLabel, parsePanelIdFromLabel, readDetachedPanelBootstrap } from "../detachedWindow.ts";

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
