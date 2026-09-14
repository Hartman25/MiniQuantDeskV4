// GUI-LAYOUT-06: monitor-loss/recovery for an actual OS window (as opposed
// to monitorModel.ts's pure placement decision, which this wraps with the
// live Tauri calls). Re-validates a window's current geometry against the
// CURRENT monitor topology and relocates it back on-screen if needed —
// covers the "laptop undocked while a window was on an external monitor"
// case at the two points a window becomes visible again: its own startup,
// and being re-shown by ensureWindow.
//
// Every step is try/caught and silently no-ops outside Tauri — this is
// best-effort recovery, never a hard requirement for the window to function.

import { reconcileWindowGeometry, type MonitorDescriptor, type WindowGeometry } from "./monitorModel";

function toMonitorDescriptor(m: {
  name: string | null;
  position: { x: number; y: number };
  size: { width: number; height: number };
  scaleFactor: number;
}): MonitorDescriptor {
  return { name: m.name, x: m.position.x, y: m.position.y, width: m.size.width, height: m.size.height, scaleFactor: m.scaleFactor };
}

/**
 * Checks the CURRENT (already-open) window's on-screen position against the
 * live monitor topology and relocates it if it has drifted off every
 * available monitor. No-ops (including outside Tauri) if the window is
 * already fully or mostly visible — this never moves a window the operator
 * placed deliberately, only recovers one that monitor loss stranded.
 */
export async function recoverWindowIfStranded(target: {
  outerPosition(): Promise<{ x: number; y: number }>;
  outerSize(): Promise<{ width: number; height: number }>;
  setPosition(pos: unknown): Promise<void>;
  setSize(size: unknown): Promise<void>;
}): Promise<void> {
  try {
    const [{ availableMonitors }, { PhysicalPosition, PhysicalSize }] = await Promise.all([
      import("@tauri-apps/api/window"),
      import("@tauri-apps/api/dpi"),
    ]);

    const rawMonitors = await availableMonitors();
    if (rawMonitors.length === 0) return;
    const monitors = rawMonitors.map(toMonitorDescriptor);

    const position = await target.outerPosition();
    const size = await target.outerSize();
    const current: WindowGeometry = { x: position.x, y: position.y, width: size.width, height: size.height };

    const reconciled = reconcileWindowGeometry(current, monitors, current);
    if (reconciled.x === current.x && reconciled.y === current.y && reconciled.width === current.width && reconciled.height === current.height) {
      return; // already visible — never move a deliberately-placed window
    }

    await target.setPosition(new PhysicalPosition(reconciled.x, reconciled.y));
    await target.setSize(new PhysicalSize(reconciled.width, reconciled.height));
  } catch {
    // Not in a Tauri context, or the monitor/window query failed — leave the window exactly as-is.
  }
}

/** Convenience wrapper for the current window, resolving getCurrentWindow() lazily so this stays a no-op import outside Tauri. */
export async function recoverCurrentWindowIfStranded(): Promise<void> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await recoverWindowIfStranded(getCurrentWindow());
  } catch {
    // Not in a Tauri context.
  }
}
