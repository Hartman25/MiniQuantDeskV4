import test from "node:test";
import assert from "node:assert/strict";
import {
  centeredDefaultGeometry,
  clampGeometryToMonitor,
  findContainingMonitor,
  isGeometryVisible,
  overlapArea,
  reconcileWindowGeometry,
  toLogicalWindowGeometry,
  type MonitorDescriptor,
  type WindowGeometry,
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

// ---------------------------------------------------------------------------
// WAVE-02-FINAL-REPAIR-01 R4: physical -> logical coordinate-space
// normalization. Tauri's Monitor.position/size (and therefore every
// MonitorDescriptor/WindowGeometry this module's own math produces) are
// physical pixels; WebviewWindow/Window creation options are logical.
// toLogicalWindowGeometry is the one conversion boundary — these tests prove
// it (and the geometryForSlot/detachPanel call sites that use it) actually
// center a new window inside its INTENDED monitor's own logical bounds
// across a range of scale factors, negative monitor coordinates, and
// mixed-DPI multi-monitor layouts.
// ---------------------------------------------------------------------------

function logicalBounds(m: MonitorDescriptor): { x: number; y: number; width: number; height: number } {
  return { x: m.x / m.scaleFactor, y: m.y / m.scaleFactor, width: m.width / m.scaleFactor, height: m.height / m.scaleFactor };
}

/** Simulates the geometryForSlot/detachPanel pattern: center a LOGICAL desired size on a PHYSICAL monitor, then convert back to logical. */
function centeredLogicalGeometryOnMonitor(monitor: MonitorDescriptor, desiredWidthLogical: number, desiredHeightLogical: number): WindowGeometry {
  const physical = centeredDefaultGeometry(monitor, desiredWidthLogical * monitor.scaleFactor, desiredHeightLogical * monitor.scaleFactor);
  return toLogicalWindowGeometry(physical, monitor.scaleFactor);
}

function assertCenteredWithinLogicalBounds(geometry: WindowGeometry, monitor: MonitorDescriptor, expectedWidth: number, expectedHeight: number) {
  const bounds = logicalBounds(monitor);
  assert.equal(geometry.width, expectedWidth);
  assert.equal(geometry.height, expectedHeight);
  // Fully within the monitor's own LOGICAL bounds — the defect this repairs
  // is a window centered using the wrong (physical) numbers landing on the
  // wrong monitor or partly off-screen once interpreted as logical.
  assert.ok(geometry.x >= bounds.x - 1 && geometry.x + geometry.width <= bounds.x + bounds.width + 1, `x=${geometry.x} must be within monitor logical bounds [${bounds.x}, ${bounds.x + bounds.width}]`);
  assert.ok(geometry.y >= bounds.y - 1 && geometry.y + geometry.height <= bounds.y + bounds.height + 1, `y=${geometry.y} must be within monitor logical bounds [${bounds.y}, ${bounds.y + bounds.height}]`);
  // Centered: window's logical midpoint within 1px of the monitor's logical midpoint.
  const geoMidX = geometry.x + geometry.width / 2;
  const geoMidY = geometry.y + geometry.height / 2;
  const monitorMidX = bounds.x + bounds.width / 2;
  const monitorMidY = bounds.y + bounds.height / 2;
  assert.ok(Math.abs(geoMidX - monitorMidX) <= 1, `x center off by ${geoMidX - monitorMidX}`);
  assert.ok(Math.abs(geoMidY - monitorMidY) <= 1, `y center off by ${geoMidY - monitorMidY}`);
}

test("toLogicalWindowGeometry converts physical pixels to logical using the scale factor", () => {
  const physical: WindowGeometry = { x: 200, y: 100, width: 1600, height: 1000 };
  assert.deepEqual(toLogicalWindowGeometry(physical, 1), { x: 200, y: 100, width: 1600, height: 1000 });
  assert.deepEqual(toLogicalWindowGeometry(physical, 2), { x: 100, y: 50, width: 800, height: 500 });
});

test("toLogicalWindowGeometry falls back to scaleFactor 1 on non-positive input rather than dividing by zero/negative", () => {
  const physical: WindowGeometry = { x: 200, y: 100, width: 1600, height: 1000 };
  assert.deepEqual(toLogicalWindowGeometry(physical, 0), physical);
  assert.deepEqual(toLogicalWindowGeometry(physical, -1), physical);
});

for (const scaleFactor of [1, 1.25, 1.5, 2]) {
  test(`centering at ${scaleFactor * 100}% scale lands inside the monitor's logical bounds`, () => {
    const monitor: MonitorDescriptor = { name: "M", x: 0, y: 0, width: Math.round(1920 * scaleFactor), height: Math.round(1080 * scaleFactor), scaleFactor };
    const geometry = centeredLogicalGeometryOnMonitor(monitor, 1024, 768);
    assertCenteredWithinLogicalBounds(geometry, monitor, 1024, 768);
  });
}

test("negative monitor coordinates (a monitor positioned left of/above the primary) still center correctly in logical space", () => {
  // A secondary monitor to the left of and above the primary (x=0,y=0) at 150% scale.
  const monitor: MonitorDescriptor = { name: "Left", x: -2880, y: -1620, width: 2880, height: 1620, scaleFactor: 1.5 };
  const geometry = centeredLogicalGeometryOnMonitor(monitor, 1024, 768);
  assertCenteredWithinLogicalBounds(geometry, monitor, 1024, 768);
  assert.ok(geometry.x < 0, "must resolve to a negative logical x on a monitor left of the origin");
  assert.ok(geometry.y < 0, "must resolve to a negative logical y on a monitor above the origin");
});

test("mixed-DPI dual monitor: each monitor's own scale factor is used, never the other monitor's", () => {
  // Monitor A: 100% scale, physical == logical, 1920x1080 at x=0.
  const monitorA: MonitorDescriptor = { name: "A", x: 0, y: 0, width: 1920, height: 1080, scaleFactor: 1 };
  // Monitor B: 200% scale, physically 3840 wide (logical 1920), placed right after A.
  const monitorB: MonitorDescriptor = { name: "B", x: 1920, y: 0, width: 3840, height: 2160, scaleFactor: 2 };

  const geoA = centeredLogicalGeometryOnMonitor(monitorA, 1024, 768);
  assertCenteredWithinLogicalBounds(geoA, monitorA, 1024, 768);

  const geoB = centeredLogicalGeometryOnMonitor(monitorB, 1024, 768);
  assertCenteredWithinLogicalBounds(geoB, monitorB, 1024, 768);
  // Using monitor A's scale factor (1) instead of B's (2) would place this
  // window far outside B's logical bounds [1920, 3840) — assert it lands
  // strictly inside B's own logical region, not A's or off past B's right edge.
  const boundsB = logicalBounds(monitorB);
  assert.ok(geoB.x >= boundsB.x && geoB.x + geoB.width <= boundsB.x + boundsB.width);
});

test("mixed-DPI triple monitor: three different scale factors each center independently and correctly", () => {
  const monitorA: MonitorDescriptor = { name: "A", x: 0, y: 0, width: 1920, height: 1080, scaleFactor: 1 };
  const monitorB: MonitorDescriptor = { name: "B", x: 1920, y: 0, width: 2880, height: 1620, scaleFactor: 1.5 };
  const monitorC: MonitorDescriptor = { name: "C", x: 3840, y: 0, width: 3200, height: 2000, scaleFactor: 2 };

  for (const monitor of [monitorA, monitorB, monitorC]) {
    const geometry = centeredLogicalGeometryOnMonitor(monitor, 1500, 960);
    assertCenteredWithinLogicalBounds(geometry, monitor, 1500, 960);
  }
});
