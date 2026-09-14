import test from "node:test";
import assert from "node:assert/strict";
import {
  centeredDefaultGeometry,
  clampGeometryToMonitor,
  findContainingMonitor,
  isGeometryVisible,
  overlapArea,
  reconcileWindowGeometry,
  type MonitorDescriptor,
} from "../monitorModel.ts";

const MONITOR_1: MonitorDescriptor = { name: "Monitor 1", x: 0, y: 0, width: 1920, height: 1080, scaleFactor: 1 };
const MONITOR_2_RIGHT: MonitorDescriptor = { name: "Monitor 2", x: 1920, y: 0, width: 1920, height: 1080, scaleFactor: 1 };
// Simulates a triple-monitor layout's third screen, positioned far to the right.
const MONITOR_3_FAR_RIGHT: MonitorDescriptor = { name: "Monitor 3", x: 3840, y: 0, width: 1600, height: 900, scaleFactor: 1 };

test("overlapArea is 0 for disjoint rectangles and correct for overlapping ones", () => {
  assert.equal(overlapArea({ x: 2000, y: 0, width: 100, height: 100 }, MONITOR_1), 0);
  assert.equal(overlapArea({ x: 0, y: 0, width: 100, height: 100 }, MONITOR_1), 10000);
  // straddles the boundary between monitor 1 and 2: only 100px of a 200px-wide window is on monitor 1
  assert.equal(overlapArea({ x: 1820, y: 0, width: 200, height: 100 }, MONITOR_1), 100 * 100);
});

test("findContainingMonitor picks the monitor with the most overlap, or null when none", () => {
  const geometry = { x: 1900, y: 0, width: 400, height: 400 }; // mostly on monitor 2
  assert.equal(findContainingMonitor(geometry, [MONITOR_1, MONITOR_2_RIGHT])?.name, "Monitor 2");
  assert.equal(findContainingMonitor({ x: 9000, y: 0, width: 100, height: 100 }, [MONITOR_1]), null);
});

test("isGeometryVisible requires a meaningful fraction on-screen, not a corner pixel", () => {
  assert.equal(isGeometryVisible({ x: 0, y: 0, width: 800, height: 600 }, [MONITOR_1]), true);
  // only a 5x5 corner overlaps — far below the visibility threshold
  assert.equal(isGeometryVisible({ x: 1915, y: 1075, width: 800, height: 600 }, [MONITOR_1]), false);
  assert.equal(isGeometryVisible({ x: 5000, y: 5000, width: 100, height: 100 }, [MONITOR_1]), false);
});

test("clampGeometryToMonitor keeps a fully-on-screen geometry unchanged", () => {
  const geometry = { x: 100, y: 100, width: 800, height: 600 };
  assert.deepEqual(clampGeometryToMonitor(geometry, MONITOR_1), geometry);
});

test("clampGeometryToMonitor pulls a partially off-right-edge window fully on-screen (acceptance test 9)", () => {
  const geometry = { x: 1800, y: 100, width: 400, height: 300 };
  const clamped = clampGeometryToMonitor(geometry, MONITOR_1);
  assert.equal(clamped.x + clamped.width <= MONITOR_1.x + MONITOR_1.width, true);
  assert.equal(clamped.width, 400);
  assert.equal(clamped.x, 1520); // shifted left just enough to fit fully within 0..1920
});

test("clampGeometryToMonitor shrinks a geometry larger than the monitor itself", () => {
  const geometry = { x: -200, y: -200, width: 3000, height: 2000 };
  const clamped = clampGeometryToMonitor(geometry, MONITOR_1);
  assert.equal(clamped.width, MONITOR_1.width);
  assert.equal(clamped.height, MONITOR_1.height);
  assert.equal(clamped.x, MONITOR_1.x);
  assert.equal(clamped.y, MONITOR_1.y);
});

test("reconcileWindowGeometry keeps a saved geometry that is still visible, clamped to its monitor", () => {
  const saved = { x: 100, y: 100, width: 800, height: 600 };
  const result = reconcileWindowGeometry(saved, [MONITOR_1, MONITOR_2_RIGHT], { x: 0, y: 0, width: 640, height: 480 });
  assert.deepEqual(result, saved);
});

test("reconcileWindowGeometry relocates onto the sole remaining monitor when the saved monitor is gone (acceptance test 8)", () => {
  // Saved geometry lived on a since-removed "Monitor 3" at x=3840; only Monitor 1 remains.
  const saved = { x: 3840, y: 0, width: 1600, height: 900 };
  const result = reconcileWindowGeometry(saved, [MONITOR_1], { x: 0, y: 0, width: 640, height: 480 });
  assert.equal(isGeometryVisible(result, [MONITOR_1]), true);
  assert.equal(result.x >= MONITOR_1.x && result.x + result.width <= MONITOR_1.x + MONITOR_1.width, true);
});

test("reconcileWindowGeometry falls back to the fallback geometry, clamped, when there is no saved geometry", () => {
  const fallback = { x: 5000, y: 5000, width: 640, height: 480 }; // deliberately bogus/off-screen fallback
  const result = reconcileWindowGeometry(null, [MONITOR_1], fallback);
  assert.equal(isGeometryVisible(result, [MONITOR_1]), true);
});

test("reconcileWindowGeometry trusts the caller's fallback verbatim when there is no monitor topology at all", () => {
  const fallback = { x: 10, y: 10, width: 640, height: 480 };
  assert.deepEqual(reconcileWindowGeometry(null, [], fallback), fallback);
});

test("reconcileWindowGeometry prefers monitors[0] (caller's most-preferred order) when relocating", () => {
  const saved = { x: 9999, y: 9999, width: 400, height: 300 }; // visible on neither monitor
  const result = reconcileWindowGeometry(saved, [MONITOR_2_RIGHT, MONITOR_1], { x: 0, y: 0, width: 400, height: 300 });
  assert.equal(isGeometryVisible(result, [MONITOR_2_RIGHT]), true);
});

test("centeredDefaultGeometry centers within the monitor and never exceeds its size", () => {
  const geo = centeredDefaultGeometry(MONITOR_1, 800, 600);
  assert.equal(geo.width, 800);
  assert.equal(geo.height, 600);
  assert.equal(geo.x, Math.floor((1920 - 800) / 2));
  assert.equal(geo.y, Math.floor((1080 - 600) / 2));

  const oversized = centeredDefaultGeometry(MONITOR_3_FAR_RIGHT, 4000, 3000);
  assert.equal(oversized.width, MONITOR_3_FAR_RIGHT.width);
  assert.equal(oversized.height, MONITOR_3_FAR_RIGHT.height);
});
