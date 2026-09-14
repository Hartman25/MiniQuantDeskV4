// GUI-LAYOUT-03: monitor topology reconciliation — pure, unit-tested.
//
// A persisted window geometry is never trusted blindly: every restore is
// re-validated against the CURRENT monitor topology (queried fresh via
// Tauri's availableMonitors() at call time) so a window can never be
// stranded off-screen after a monitor is unplugged, a resolution changes, or
// a saved layout is replayed on different hardware. `MonitorDescriptor` is a
// plain-data flattening of Tauri's `Monitor` type so this module has no
// runtime dependency on the Tauri API and is fully testable with simulated
// descriptors.
//
// WAVE-02-FINAL-REPAIR-01 R4 — coordinate-space contract: every
// MonitorDescriptor and every WindowGeometry produced by this module's own
// math (overlapArea/findContainingMonitor/isGeometryVisible/
// clampGeometryToMonitor/reconcileWindowGeometry/centeredDefaultGeometry) is
// PHYSICAL pixels, matching what Tauri's own `Monitor.position`/`.size`
// report (per @tauri-apps/api's javascript/api/namespacewindow and
// namespacedpi reference: "Monitor properties such as size, position, and
// workArea are measured in physical pixels"). Callers that construct an
// actual `WebviewWindow`/`Window` MUST convert the final geometry to logical
// pixels with `toLogicalWindowGeometry` first — the `x`/`y`/`width`/`height`
// creation options on `WebviewOptions`/`WindowOptions` are logical, not
// physical. windowRecovery.ts is the one exception: it reads/writes an
// already-open window's geometry via `outerPosition()`/`outerSize()`/
// `PhysicalPosition`/`PhysicalSize`, which are physical end-to-end, so it
// intentionally never calls toLogicalWindowGeometry.

export interface MonitorDescriptor {
  name: string | null;
  x: number;
  y: number;
  width: number;
  height: number;
  scaleFactor: number;
}

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Converts a physical-pixel WindowGeometry (this module's native space) into
 * logical pixels for a WebviewWindow/Window creation options object. A
 * non-positive scaleFactor (malformed monitor data) falls back to 1 (i.e.
 * treats the input as already logical) rather than dividing by zero or a
 * negative number and producing a nonsensical/off-screen position.
 */
export function toLogicalWindowGeometry(geometry: WindowGeometry, scaleFactor: number): WindowGeometry {
  const factor = scaleFactor > 0 ? scaleFactor : 1;
  return {
    x: Math.round(geometry.x / factor),
    y: Math.round(geometry.y / factor),
    width: Math.round(geometry.width / factor),
    height: Math.round(geometry.height / factor),
  };
}

/** Fraction of a geometry's own area that must overlap the monitor union to count as "on-screen" — a one-pixel sliver does not. */
const VISIBILITY_THRESHOLD = 0.25;

function right(r: { x: number; width: number }): number {
  return r.x + r.width;
}

function bottom(r: { y: number; height: number }): number {
  return r.y + r.height;
}

/** Overlap area (px²) between a window geometry and a monitor's bounds. 0 when disjoint. */
export function overlapArea(geometry: WindowGeometry, monitor: MonitorDescriptor): number {
  const left = Math.max(geometry.x, monitor.x);
  const top = Math.max(geometry.y, monitor.y);
  const overlapRight = Math.min(right(geometry), right(monitor));
  const overlapBottom = Math.min(bottom(geometry), bottom(monitor));
  const width = overlapRight - left;
  const height = overlapBottom - top;
  return width > 0 && height > 0 ? width * height : 0;
}

/** The monitor a geometry overlaps most, or null if it doesn't meaningfully overlap any available monitor. */
export function findContainingMonitor(
  geometry: WindowGeometry,
  monitors: readonly MonitorDescriptor[],
): MonitorDescriptor | null {
  let best: MonitorDescriptor | null = null;
  let bestArea = 0;
  for (const monitor of monitors) {
    const area = overlapArea(geometry, monitor);
    if (area > bestArea) {
      bestArea = area;
      best = monitor;
    }
  }
  return bestArea > 0 ? best : null;
}

/** True only if a meaningful fraction of the geometry overlaps the current monitor topology — not just a corner pixel. */
export function isGeometryVisible(geometry: WindowGeometry, monitors: readonly MonitorDescriptor[]): boolean {
  const totalArea = geometry.width * geometry.height;
  if (totalArea <= 0) return false;
  const covered = monitors.reduce((sum, m) => sum + overlapArea(geometry, m), 0);
  return covered / totalArea >= VISIBILITY_THRESHOLD;
}

/** Clamps a geometry so it fits entirely within a monitor's bounds, shrinking it first if it's larger than the monitor. */
export function clampGeometryToMonitor(geometry: WindowGeometry, monitor: MonitorDescriptor): WindowGeometry {
  const width = Math.min(geometry.width, monitor.width);
  const height = Math.min(geometry.height, monitor.height);
  const maxX = monitor.x + monitor.width - width;
  const maxY = monitor.y + monitor.height - height;
  const x = Math.min(Math.max(geometry.x, monitor.x), maxX);
  const y = Math.min(Math.max(geometry.y, monitor.y), maxY);
  return { x, y, width, height };
}

/**
 * The core fail-closed placement decision. Given a possibly-stale saved
 * geometry and the CURRENT monitor topology, decides where a window should
 * actually appear — never off-screen, never on a monitor that no longer
 * exists. `monitors` should be ordered most-preferred-first (e.g. primary
 * monitor first) by the caller; that ordering is what's used when the saved
 * geometry must be relocated.
 */
export function reconcileWindowGeometry(
  saved: WindowGeometry | null,
  monitors: readonly MonitorDescriptor[],
  fallback: WindowGeometry,
): WindowGeometry {
  if (monitors.length === 0) return fallback;

  if (saved && isGeometryVisible(saved, monitors)) {
    const containing = findContainingMonitor(saved, monitors);
    return containing ? clampGeometryToMonitor(saved, containing) : saved;
  }

  return clampGeometryToMonitor(saved ?? fallback, monitors[0]);
}

/** A sensible centered default geometry on a given monitor, capped to the monitor's own size. */
export function centeredDefaultGeometry(monitor: MonitorDescriptor, width: number, height: number): WindowGeometry {
  const w = Math.min(width, monitor.width);
  const h = Math.min(height, monitor.height);
  return {
    x: monitor.x + Math.floor((monitor.width - w) / 2),
    y: monitor.y + Math.floor((monitor.height - h) / 2),
    width: w,
    height: h,
  };
}
