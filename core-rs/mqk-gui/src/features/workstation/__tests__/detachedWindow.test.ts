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

test("readDetachedPanelBootstrap parses a well-formed query string", () => {
  const bootstrap = readDetachedPanelBootstrap("?detachedPanel=portfolio&opener=control");
  assert.deepEqual(bootstrap, { panelId: "portfolio", openerLabel: "control" });
});

test("readDetachedPanelBootstrap fails closed to null on missing/unknown/malformed values", () => {
  assert.equal(readDetachedPanelBootstrap(""), null);
  assert.equal(readDetachedPanelBootstrap("?opener=control"), null, "missing detachedPanel");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=portfolio"), null, "missing opener");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=not-a-real-panel&opener=control"), null, "unknown panel id");
  assert.equal(readDetachedPanelBootstrap("?detachedPanel=&opener="), null, "empty values");
});
