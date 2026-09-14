import test from "node:test";
import assert from "node:assert/strict";
import {
  LAYOUT_SCHEMA_VERSION,
  allDockviewPanelIdsKnown,
  defaultLayoutPanelIds,
  extractDockviewPanelIds,
  isPersistedWorkstationLayoutShape,
  parsePersistedLayout,
  sameIdSet,
  sanitizeLayoutSummary,
  serializeLayout,
} from "../layoutModel.ts";

test("a well-formed layout document round-trips through serialize/parse", () => {
  const doc = serializeLayout(["dashboard", "portfolio"], "portfolio", { grid: {} });
  const raw = JSON.stringify(doc);
  const parsed = parsePersistedLayout(raw);
  assert.ok(parsed);
  assert.deepEqual(parsed, doc);
});

test("malformed JSON fails closed to null, not a throw", () => {
  assert.equal(parsePersistedLayout("{not json"), null);
  assert.equal(parsePersistedLayout(""), null);
  assert.equal(parsePersistedLayout(null), null);
  assert.equal(parsePersistedLayout(undefined), null);
});

test("valid JSON with the wrong shape fails closed to null", () => {
  assert.equal(parsePersistedLayout(JSON.stringify({ foo: "bar" })), null);
  assert.equal(parsePersistedLayout(JSON.stringify([1, 2, 3])), null);
  assert.equal(parsePersistedLayout(JSON.stringify("just a string")), null);
});

test("an unrecognized/future schema version fails closed to null", () => {
  const futureDoc = { schemaVersion: 999, panelIds: ["dashboard"], activePanelId: "dashboard", dockviewLayout: {} };
  assert.equal(parsePersistedLayout(JSON.stringify(futureDoc)), null);
});

test("a layout referencing an unknown/removed panel is sanitized, not rejected outright", () => {
  const doc = serializeLayout(
    ["dashboard", "not-a-real-panel" as never, "portfolio"],
    "not-a-real-panel",
    {},
  );
  const summary = sanitizeLayoutSummary(doc);
  assert.deepEqual(summary.panelIds, ["dashboard", "portfolio"]);
  // active panel was the dropped one, so it must fall back to the first remaining known panel.
  assert.equal(summary.activePanelId, "dashboard");
});

test("sanitizing a layout with zero known panels falls back to null active panel, not a crash", () => {
  const doc = serializeLayout(["bogus-a" as never, "bogus-b" as never], "bogus-a", {});
  const summary = sanitizeLayoutSummary(doc);
  assert.deepEqual(summary.panelIds, []);
  assert.equal(summary.activePanelId, null);
});

test("sanitizing keeps a valid activePanelId that is present in panelIds", () => {
  const doc = serializeLayout(["dashboard", "portfolio"], "dashboard", {});
  assert.equal(sanitizeLayoutSummary(doc).activePanelId, "dashboard");
});

test("isPersistedWorkstationLayoutShape rejects non-object and partially-shaped values", () => {
  assert.equal(isPersistedWorkstationLayoutShape(null), false);
  assert.equal(isPersistedWorkstationLayoutShape(42), false);
  assert.equal(isPersistedWorkstationLayoutShape({ schemaVersion: LAYOUT_SCHEMA_VERSION }), false);
  assert.equal(
    isPersistedWorkstationLayoutShape({
      schemaVersion: LAYOUT_SCHEMA_VERSION,
      panelIds: ["dashboard"],
      activePanelId: "dashboard",
      dockviewLayout: null,
    }),
    false,
    "dockviewLayout must be a non-null object",
  );
});

test("allDockviewPanelIdsKnown rejects a foreign/unknown panel id embedded in dockview's own blob", () => {
  assert.equal(allDockviewPanelIdsKnown(["dashboard", "portfolio"]), true);
  assert.equal(allDockviewPanelIdsKnown(["dashboard", "smuggled-authority-panel"]), false);
  assert.equal(allDockviewPanelIdsKnown([]), true);
});

test("defaultLayoutPanelIds yields a single-panel starting layout", () => {
  assert.deepEqual(defaultLayoutPanelIds("dashboard"), ["dashboard"]);
});

test("extractDockviewPanelIds reads keys of dockview's own panels map", () => {
  const ids = extractDockviewPanelIds({ panels: { dashboard: {}, portfolio: {} } });
  assert.ok(ids);
  assert.deepEqual([...ids].sort(), ["dashboard", "portfolio"]);
});

test("extractDockviewPanelIds fails closed to null on a foreign/malformed blob", () => {
  assert.equal(extractDockviewPanelIds(null), null);
  assert.equal(extractDockviewPanelIds(42), null);
  assert.equal(extractDockviewPanelIds({}), null);
  assert.equal(extractDockviewPanelIds({ panels: null }), null);
  assert.equal(extractDockviewPanelIds({ panels: "not-an-object" }), null);
});

test("sameIdSet compares as sets, ignoring order, and catches a disagreeing document", () => {
  assert.equal(sameIdSet(["a", "b"], ["b", "a"]), true);
  assert.equal(sameIdSet(["a", "b"], ["a"]), false);
  assert.equal(sameIdSet(["a", "b"], ["a", "c"]), false);
  assert.equal(sameIdSet([], []), true);
});
