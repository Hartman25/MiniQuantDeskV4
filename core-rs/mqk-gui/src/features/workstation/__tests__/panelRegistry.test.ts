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
  panelRendererFor,
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

// WAVE-02-FINAL-REPAIR-01 R2: an operator-singleton panel must never be
// detachable — detaching would remove it from the authority window's local
// Dockview instance while it stays live in a separate Tauri window, letting
// the authority window's navigation create a second instance on reopen.
test("operator-singleton panels are never detachable", () => {
  for (const id of OPERATOR_SINGLETON_PANEL_IDS) {
    assert.equal(PANEL_REGISTRY[id].detachable, false, `${id}: operator-singleton panel must not be detachable`);
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

// WAVE-02-FINAL-REPAIR-01 R1: the fixed Tier-0 surface must expose
// kill-switch truth, live-routing truth, critical/warning incident-alert
// indication, and daemon reachability alongside the original health fields
// — these may not silently disappear when the field list is edited again.
test("Tier-0 status fields include the full required fixed safety surface", () => {
  const required = [
    "environment",
    "runtime_status",
    "broker_status",
    "alpaca_ws_continuity",
    "db_status",
    "market_data_health",
    "reconcile_status",
    "integrity_status",
    "audit_writer_status",
    "kill_switch_active",
    "live_routing_enabled",
    "has_critical",
    "has_warning",
    "daemon_reachable",
  ];
  for (const field of required) {
    assert.ok(
      (TIER0_STATUS_FIELDS as readonly string[]).includes(field),
      `${field}: required fixed Tier-0 safety truth is missing from TIER0_STATUS_FIELDS`,
    );
  }
});

test("contextAware is only set for panels with current WorkspaceContext evidence (marketData, backtests, evidence)", () => {
  const contextAwareIds = PANEL_IDS.filter((id) => PANEL_REGISTRY[id].contextAware).sort();
  assert.deepEqual(contextAwareIds, ["backtests", "evidence", "marketData"]);
});

test("every panel declares a positive minimum footprint", () => {
  for (const id of PANEL_IDS) {
    const meta = PANEL_REGISTRY[id];
    assert.ok(meta.minWidth > 0, `${id}: minWidth must be positive`);
    assert.ok(meta.minHeight > 0, `${id}: minHeight must be positive`);
  }
});

// GUI-LAYOUT-05 repair: dockview's default "onlyWhenVisible" render mode
// destroys a backgrounded panel's React instance, wiping local state like
// pin/unpin. panelRendererFor opts a panel into "always" (stays mounted)
// specifically to survive that — this must apply to exactly the
// contextAware class (the only panels with pin/unpin local state) and to
// no one else, or every other panel would pay the DOM/memory cost of
// staying mounted in the background for no reason.
test("panelRendererFor requests 'always' for exactly the contextAware panels, and the default for every other panel", () => {
  for (const id of PANEL_IDS) {
    const meta = PANEL_REGISTRY[id];
    const renderer = panelRendererFor(id);
    if (meta.contextAware) {
      assert.equal(renderer, "always", `${id}: contextAware panel must opt into "always" so its pin state survives a tab switch`);
    } else {
      assert.equal(renderer, undefined, `${id}: non-contextAware panel must keep dockview's default renderer, not "always"`);
    }
  }
  // Anchors the exact expected set so this test would fail if contextAware
  // classification ever silently drifted (see the contextAware test above).
  const alwaysRenderedIds = PANEL_IDS.filter((id) => panelRendererFor(id) === "always").sort();
  assert.deepEqual(alwaysRenderedIds, ["backtests", "evidence", "marketData"]);
});

test("panelRendererFor fails closed (default renderer, not 'always') for an unknown panel id", () => {
  assert.equal(panelRendererFor("not-a-real-panel"), undefined);
});

// R2-EV-09: the Unified Evidence panel exposes no operator/trading authority — read-only, and detachable like any other read-only panel.
test("R2-EV-09: the evidence panel is read-only, duplicable, and detachable — never operator-singleton authority", () => {
  const meta = PANEL_REGISTRY.evidence;
  assert.equal(meta.authority, "read-only");
  assert.equal(meta.duplicable, true);
  assert.equal(meta.detachable, true);
  assert.equal(meta.contextAware, true);
});
