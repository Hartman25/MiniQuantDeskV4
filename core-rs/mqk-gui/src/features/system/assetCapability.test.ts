// core-rs/mqk-gui/src/features/system/assetCapability.test.ts
//
// ASSET-CAPABILITY-MATRIX-GUI-VISIBILITY-01: proof tests for the pure
// asset-capability-matrix render helpers.

import test from "node:test";
import assert from "node:assert/strict";
import {
  assetCapabilityMatrixStatusLabel,
  describeAssetCapabilityEntry,
  describeNonEquityAssetClassState,
  nonEquityAssetClassState,
} from "./assetCapability.ts";
import type { AssetCapabilityEntry, AssetCapabilityMatrix } from "./types.ts";

function equityEntry(overrides: Partial<AssetCapabilityEntry> = {}): AssetCapabilityEntry {
  return {
    asset_class: "us_equities",
    enabled: true,
    paper_ready: true,
    live_ready: false,
    broker_adapter: "alpaca",
    notes: "current Paper+Alpaca equity path; live locked pending future review",
    ...overrides,
  };
}

function disabledEntry(asset_class: string): AssetCapabilityEntry {
  return {
    asset_class,
    enabled: false,
    paper_ready: false,
    live_ready: false,
    broker_adapter: "none",
    notes: `BACKLOG — no adapter wired for ${asset_class}`,
  };
}

function realShapedMatrix(): AssetCapabilityMatrix {
  const entries = [
    equityEntry(),
    disabledEntry("crypto"),
    disabledEntry("futures"),
    disabledEntry("options"),
    disabledEntry("forex"),
  ];
  return {
    schema_version: "asset-capability-v1",
    static_source: "mqk-daemon/routes/system.rs::static_asset_capability_matrix",
    live_capital_ready: false,
    default_enabled_asset_classes: ["us_equities"],
    disabled_asset_classes: ["crypto", "futures", "options", "forex"],
    entries,
  };
}

// ---------------------------------------------------------------------------
// assetCapabilityMatrixStatusLabel
// ---------------------------------------------------------------------------

test("assetCapabilityMatrixStatusLabel reports unavailable truth when matrix is undefined", () => {
  assert.equal(
    assetCapabilityMatrixStatusLabel(undefined),
    "Asset capability matrix truth unavailable.",
  );
});

test("assetCapabilityMatrixStatusLabel summarizes the real production shape", () => {
  assert.equal(assetCapabilityMatrixStatusLabel(realShapedMatrix()), "1 of 5 asset classes enabled.");
});

// ---------------------------------------------------------------------------
// nonEquityAssetClassState
// ---------------------------------------------------------------------------

test("nonEquityAssetClassState is unknown when the matrix is undefined", () => {
  assert.equal(nonEquityAssetClassState(undefined), "unknown");
});

test("nonEquityAssetClassState is unknown when there are no non-equity entries to check", () => {
  const matrix = realShapedMatrix();
  matrix.entries = [equityEntry()];
  assert.equal(nonEquityAssetClassState(matrix), "unknown");
});

test("nonEquityAssetClassState is all_disabled for the real production shape", () => {
  assert.equal(nonEquityAssetClassState(realShapedMatrix()), "all_disabled");
});

// Adversarial fixture: proves the helper actually inspects `entries` rather
// than trusting the backend's own `disabled_asset_classes` array verbatim.
test("nonEquityAssetClassState is some_enabled when a non-equity entry is enabled, even if disabled_asset_classes claims otherwise", () => {
  const matrix = realShapedMatrix();
  matrix.entries = matrix.entries.map((e) =>
    e.asset_class === "crypto" ? { ...e, enabled: true } : e,
  );
  // disabled_asset_classes deliberately left stale/wrong to prove independence.
  assert.equal(nonEquityAssetClassState(matrix), "some_enabled");
});

// ---------------------------------------------------------------------------
// describeNonEquityAssetClassState
// ---------------------------------------------------------------------------

test("describeNonEquityAssetClassState renders a plain note for all_disabled", () => {
  assert.equal(
    describeNonEquityAssetClassState("all_disabled"),
    "All non-equity asset classes are disabled.",
  );
});

test("describeNonEquityAssetClassState renders a loud warning for some_enabled", () => {
  assert.match(describeNonEquityAssetClassState("some_enabled"), /WARNING/);
});

test("describeNonEquityAssetClassState renders an explicit unavailable note for unknown", () => {
  assert.match(describeNonEquityAssetClassState("unknown"), /unavailable/);
});

// ---------------------------------------------------------------------------
// describeAssetCapabilityEntry
// ---------------------------------------------------------------------------

test("describeAssetCapabilityEntry describes an enabled entry", () => {
  const label = describeAssetCapabilityEntry(equityEntry());
  assert.match(label, /us_equities/);
  assert.match(label, /enabled/);
  assert.match(label, /paper-ready/);
  assert.match(label, /live not ready/);
  assert.match(label, /alpaca/);
});

test("describeAssetCapabilityEntry describes a disabled entry", () => {
  const label = describeAssetCapabilityEntry(disabledEntry("crypto"));
  assert.match(label, /crypto/);
  assert.match(label, /disabled/);
  assert.match(label, /paper not ready/);
  assert.match(label, /adapter: none/);
});
