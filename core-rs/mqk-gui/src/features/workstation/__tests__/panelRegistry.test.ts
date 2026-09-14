import test from "node:test";
import assert from "node:assert/strict";
import { SCREEN_REGISTRY } from "../../screens/screenRegistry.tsx";
import {
  PANEL_IDS,
  PANEL_REGISTRY,
  OPERATOR_SINGLETON_PANEL_IDS,
  TIER0_STATUS_FIELDS,
  TIER1_PANEL_IDS,
  TIER2_PANEL_IDS,
  filterKnownPanelIds,
  getPanelMetadata,
  isKnownPanelId,
} from "../panelRegistry.ts";

test("panel registry covers exactly the screen registry, with no duplicates", () => {
  const screenKeys = Object.keys(SCREEN_REGISTRY).sort();
  const panelIds = [...PANEL_IDS].sort();
  assert.deepEqual(panelIds, screenKeys, "PANEL_REGISTRY must be a 1:1 adapter over SCREEN_REGISTRY");
  assert.equal(new Set(PANEL_IDS).size, PANEL_IDS.length, "PANEL_IDS contains duplicate entries");
});

test("panel metadata is sourced from SCREEN_REGISTRY, not duplicated truth", () => {
  for (const id of PANEL_IDS) {
    const meta = PANEL_REGISTRY[id];
    assert.equal(meta.title, SCREEN_REGISTRY[id].title, `${id}: title drifted from SCREEN_REGISTRY`);
    assert.equal(meta.description, SCREEN_REGISTRY[id].description, `${id}: description drifted from SCREEN_REGISTRY`);
    assert.equal(meta.category, SCREEN_REGISTRY[id].monitorGroup, `${id}: category drifted from SCREEN_REGISTRY`);
  }
});

test("unknown/stale panel id is rejected, not fabricated", () => {
  assert.equal(getPanelMetadata("not-a-real-panel"), null);
  assert.equal(isKnownPanelId("not-a-real-panel"), false);
  assert.equal(isKnownPanelId("ops"), true);
});

test("a persisted layout referencing an unknown panel id fails closed by dropping it, not crashing", () => {
  const sanitized = filterKnownPanelIds(["ops", "not-a-real-panel", "portfolio", "also-bogus"]);
  assert.deepEqual(sanitized, ["ops", "portfolio"]);
});

test("filterKnownPanelIds preserves order and handles an all-unknown list", () => {
  assert.deepEqual(filterKnownPanelIds([]), []);
  assert.deepEqual(filterKnownPanelIds(["bogus-a", "bogus-b"]), []);
});

test("exactly one operator-authority singleton panel exists today: ops", () => {
  assert.deepEqual([...OPERATOR_SINGLETON_PANEL_IDS], ["ops"]);
  assert.equal(PANEL_REGISTRY.ops.authority, "operator-singleton");
});

test("operator-singleton panels are never duplicable — derived, not independently settable", () => {
  for (const id of OPERATOR_SINGLETON_PANEL_IDS) {
    assert.equal(PANEL_REGISTRY[id].duplicable, false, `${id}: operator-singleton panel must not be duplicable`);
  }
});

test("duplicable is always exactly (authority === read-only) for every panel", () => {
  for (const id of PANEL_IDS) {
    const meta = PANEL_REGISTRY[id];
    assert.equal(meta.duplicable, meta.authority === "read-only", `${id}: duplicable/authority mismatch`);
  }
});

test("every panel is classified into exactly one of tier1/tier2, and the split is a true partition", () => {
  const seen = new Set<string>();
  for (const id of PANEL_IDS) {
    assert.ok(["tier1", "tier2"].includes(PANEL_REGISTRY[id].tier), `${id}: invalid tier`);
    seen.add(id);
  }
  const tier1Set = new Set(TIER1_PANEL_IDS);
  const tier2Set = new Set(TIER2_PANEL_IDS);
  assert.equal(tier1Set.size + tier2Set.size, PANEL_IDS.length, "tier1/tier2 must partition all panels");
  for (const id of tier1Set) assert.ok(!tier2Set.has(id), `${id}: cannot be both tier1 and tier2`);
});

test("Tier-0 status fields are fixed, non-empty, and distinct — never a movable panel", () => {
  assert.ok(TIER0_STATUS_FIELDS.length > 0);
  assert.equal(new Set(TIER0_STATUS_FIELDS).size, TIER0_STATUS_FIELDS.length);
  for (const field of TIER0_STATUS_FIELDS) {
    assert.ok(!(PANEL_IDS as readonly string[]).includes(field), `${field}: Tier-0 field must not also be a panel id`);
  }
});

test("contextAware is only set for panels with current WorkspaceContext evidence (marketData, backtests)", () => {
  const contextAwareIds = PANEL_IDS.filter((id) => PANEL_REGISTRY[id].contextAware).sort();
  assert.deepEqual(contextAwareIds, ["backtests", "marketData"]);
});

test("every panel declares a positive minimum footprint", () => {
  for (const id of PANEL_IDS) {
    const meta = PANEL_REGISTRY[id];
    assert.ok(meta.minWidth > 0, `${id}: minWidth must be positive`);
    assert.ok(meta.minHeight > 0, `${id}: minHeight must be positive`);
  }
});
