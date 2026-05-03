import test from "node:test";
import assert from "node:assert/strict";
import {
  LEFT_RAIL_PRIMARY,
  LEFT_RAIL_SECONDARY,
} from "../../../components/layout/leftRailNav.ts";

const ALL_LEFT_RAIL = [...LEFT_RAIL_PRIMARY, ...LEFT_RAIL_SECONDARY];

// 1. Live left-rail secondary includes ingest.
test("LEFT_RAIL_SECONDARY includes ingest", () => {
  assert.ok(
    LEFT_RAIL_SECONDARY.includes("ingest"),
    "LEFT_RAIL_SECONDARY does not include 'ingest' — screen is unreachable from the left nav",
  );
});

// 2. ingest is adjacent to marketData in the secondary list.
test("ingest is adjacent to marketData in LEFT_RAIL_SECONDARY", () => {
  const mIdx = LEFT_RAIL_SECONDARY.indexOf("marketData");
  const iIdx = LEFT_RAIL_SECONDARY.indexOf("ingest");
  assert.ok(mIdx !== -1, "LEFT_RAIL_SECONDARY missing 'marketData'");
  assert.ok(iIdx !== -1, "LEFT_RAIL_SECONDARY missing 'ingest'");
  assert.ok(
    Math.abs(mIdx - iIdx) <= 2,
    `ingest (index ${iIdx}) should be within 2 positions of marketData (index ${mIdx})`,
  );
});

// 3. No duplicate keys in either left-rail list.
test("no duplicate keys in LEFT_RAIL_PRIMARY or LEFT_RAIL_SECONDARY", () => {
  const primarySet = new Set(LEFT_RAIL_PRIMARY);
  assert.equal(primarySet.size, LEFT_RAIL_PRIMARY.length, "LEFT_RAIL_PRIMARY has duplicate entries");

  const secondarySet = new Set(LEFT_RAIL_SECONDARY);
  assert.equal(secondarySet.size, LEFT_RAIL_SECONDARY.length, "LEFT_RAIL_SECONDARY has duplicate entries");
});

// 4. marketData and backtests remain in the nav (regression guard).
test("marketData and backtests are present in left-rail nav", () => {
  assert.ok(ALL_LEFT_RAIL.includes("marketData"), "marketData was removed from the left rail");
  assert.ok(ALL_LEFT_RAIL.includes("backtests"), "backtests was removed from the left rail");
});

// 5. dashboard is in primary (smoke check that primary list is intact).
test("dashboard is in LEFT_RAIL_PRIMARY", () => {
  assert.ok(LEFT_RAIL_PRIMARY.includes("dashboard"), "dashboard was removed from LEFT_RAIL_PRIMARY");
});
