import test from "node:test";
import assert from "node:assert/strict";
import {
  BUILTIN_PRESETS,
  panelIdsForRole,
  parseCustomLayouts,
  pendingPresetStorageKey,
  presetForId,
  readPendingPresetPanelIds,
  rolesUsedByPreset,
  serializeCustomLayouts,
  withRenamedCustomLayout,
  withSavedCustomLayout,
  withoutCustomLayout,
  writePendingPresetPayload,
} from "../presets.ts";
import { serializeLayout } from "../layoutModel.ts";
import { isKnownPanelId } from "../panelRegistry.ts";

test("presetForId resolves the three built-in ids and rejects anything else", () => {
  assert.equal(presetForId("single"), BUILTIN_PRESETS.single);
  assert.equal(presetForId("dual"), BUILTIN_PRESETS.dual);
  assert.equal(presetForId("triple"), BUILTIN_PRESETS.triple);
  assert.equal(presetForId("quad"), null);
  assert.equal(presetForId(""), null);
});

test("every built-in preset only references known panel ids and roles it actually uses", () => {
  for (const preset of Object.values(BUILTIN_PRESETS)) {
    assert.ok(preset.windows.length > 0, `${preset.id}: must define at least one window`);
    const roles = rolesUsedByPreset(preset);
    assert.equal(new Set(roles).size, roles.length, `${preset.id}: duplicate role in windows`);
    for (const win of preset.windows) {
      assert.ok(win.panelIds.length > 0, `${preset.id}/${win.role}: window has no panels`);
      for (const id of win.panelIds) {
        assert.ok(isKnownPanelId(id), `${preset.id}/${win.role}: unknown panel id ${id}`);
      }
    }
  }
});

test("panelIdsForRole returns the role's panel set, or empty for a role the preset doesn't use", () => {
  assert.deepEqual(panelIdsForRole(BUILTIN_PRESETS.single, "control"), ["controlStation"]);
  assert.deepEqual(panelIdsForRole(BUILTIN_PRESETS.single, "execution"), []);
  assert.ok(panelIdsForRole(BUILTIN_PRESETS.triple, "oversight").length > 0);
});

test("pendingPresetStorageKey is namespaced per role", () => {
  assert.equal(pendingPresetStorageKey("control"), "mqd.workstation.pendingPreset.control");
  assert.notEqual(pendingPresetStorageKey("control"), pendingPresetStorageKey("execution"));
});

test("writePendingPresetPayload/readPendingPresetPanelIds round-trip", () => {
  const payload = writePendingPresetPayload(["dashboard", "portfolio"]);
  assert.deepEqual(readPendingPresetPanelIds(payload), ["dashboard", "portfolio"]);
});

test("readPendingPresetPanelIds fails closed on malformed/foreign/empty input", () => {
  assert.equal(readPendingPresetPanelIds(null), null);
  assert.equal(readPendingPresetPanelIds(""), null);
  assert.equal(readPendingPresetPanelIds("{not json"), null);
  assert.equal(readPendingPresetPanelIds(JSON.stringify({ schemaVersion: 999, panelIds: ["dashboard"] })), null);
  assert.equal(readPendingPresetPanelIds(JSON.stringify({ schemaVersion: 1, panelIds: ["bogus-panel"] })), null);
  assert.equal(readPendingPresetPanelIds(JSON.stringify({ schemaVersion: 1, panelIds: [] })), null);
});

test("readPendingPresetPanelIds drops unknown ids but keeps known ones", () => {
  const raw = JSON.stringify({ schemaVersion: 1, panelIds: ["dashboard", "bogus", "portfolio"] });
  assert.deepEqual(readPendingPresetPanelIds(raw), ["dashboard", "portfolio"]);
});

test("withSavedCustomLayout/withRenamedCustomLayout/withoutCustomLayout compose as a pure pipeline", () => {
  const layout = serializeLayout(["dashboard"], "dashboard", { grid: {}, panels: { dashboard: {} } });

  let layouts = withSavedCustomLayout([], "My Layout", layout);
  assert.equal(layouts.length, 1);
  assert.equal(layouts[0].name, "My Layout");

  layouts = withRenamedCustomLayout(layouts, layouts[0].id, "Renamed");
  assert.equal(layouts[0].name, "Renamed");

  layouts = withoutCustomLayout(layouts, layouts[0].id);
  assert.equal(layouts.length, 0);
});

test("saving a custom layout under an existing name overwrites it rather than duplicating", () => {
  const layout = serializeLayout(["dashboard"], "dashboard", { grid: {}, panels: { dashboard: {} } });
  let layouts = withSavedCustomLayout([], "Dup", layout);
  layouts = withSavedCustomLayout(layouts, "Dup", layout);
  assert.equal(layouts.filter((l) => l.name === "Dup").length, 1);
});

test("parseCustomLayouts/serializeCustomLayouts round-trip and drop individually malformed entries", () => {
  const layout = serializeLayout(["dashboard"], "dashboard", { grid: {}, panels: { dashboard: {} } });
  const layouts = withSavedCustomLayout([], "Good", layout);
  const raw = serializeCustomLayouts(layouts);
  assert.deepEqual(parseCustomLayouts(raw), layouts);

  // one malformed entry alongside a valid one — only the malformed one is dropped
  const mixed = JSON.stringify([...layouts, { id: "x", name: "bad" /* missing savedAt/layout */ }]);
  assert.deepEqual(parseCustomLayouts(mixed), layouts);
});

test("parseCustomLayouts fails closed to an empty list on missing/corrupted/non-array input", () => {
  assert.deepEqual(parseCustomLayouts(null), []);
  assert.deepEqual(parseCustomLayouts(""), []);
  assert.deepEqual(parseCustomLayouts("{not valid json"), []);
  assert.deepEqual(parseCustomLayouts(JSON.stringify({ not: "an array" })), []);
});
