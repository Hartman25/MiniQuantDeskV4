// GUI-LAYOUT-03: detach a Tier-2 panel into its own Tauri window.
//
// Detach is always a MOVE, never a duplicate: the caller (Workstation.tsx)
// removes the panel from its own dockview before calling detachPanel(), and
// the detached window's only path back is reattachAndClose(), which asks the
// opener to re-add the panel and then closes itself. This keeps an
// operator-singleton panel (e.g. `ops`) from ever existing as two live
// authority surfaces at once, and is the same uniform rule applied to every
// panel rather than a per-panel special case.
//
// Fails honestly when not running inside Tauri (isDesktopShell() false) —
// never silently no-ops while claiming success, so a browser-only preview
// can't be mistaken for a proven Tauri detach.

import { isDesktopShell } from "../../desktop/bootstrap";
import { getPanelMetadata, isKnownPanelId } from "./panelRegistry";
import { centeredDefaultGeometry, reconcileWindowGeometry, type MonitorDescriptor, type WindowGeometry } from "./monitorModel";
import { parseWorkspaceIdentityPayload, type WorkspaceIdentity } from "../workspace/workspaceModel.ts";
import type { ScreenKey } from "../screens/screenRegistry";

const DETACHED_PANEL_LABEL_PREFIX = "panel-";
export const REATTACH_EVENT = "mqd:workstation:reattach-panel";

export function panelWindowLabel(id: ScreenKey): string {
  return `${DETACHED_PANEL_LABEL_PREFIX}${id}`;
}

export function parsePanelIdFromLabel(label: string): ScreenKey | null {
  if (!label.startsWith(DETACHED_PANEL_LABEL_PREFIX)) return null;
  const id = label.slice(DETACHED_PANEL_LABEL_PREFIX.length);
  return isKnownPanelId(id) ? id : null;
}

export interface DetachedPanelBootstrap {
  panelId: ScreenKey;
  openerLabel: string;
  /** GUI-LAYOUT-05: the panel's frozen identity at the moment it was detached, if it was pinned. Null means the detached window follows the global linked identity like any unpinned panel. */
  pinnedIdentity: WorkspaceIdentity | null;
}

/** Parses the detached-window URL query. Malformed/unknown values fail closed to null (for pinnedIdentity) or to the whole bootstrap (for panelId/opener) so the window falls back to normal rendering rather than crash. */
export function readDetachedPanelBootstrap(search: string): DetachedPanelBootstrap | null {
  const params = new URLSearchParams(search);
  const panelId = params.get("detachedPanel");
  const openerLabel = params.get("opener");
  if (!panelId || !openerLabel || !isKnownPanelId(panelId)) return null;

  const rawPinned = params.get("pinnedIdentity");
  let pinnedIdentity: WorkspaceIdentity | null = null;
  if (rawPinned) {
    try {
      pinnedIdentity = parseWorkspaceIdentityPayload(JSON.parse(rawPinned));
    } catch {
      pinnedIdentity = null;
    }
  }

  return { panelId, openerLabel, pinnedIdentity };
}

export type DetachResult = { ok: true } | { ok: false; reason: "tauri-unavailable" | "no-monitors" | "error" };

function toMonitorDescriptor(m: { name: string | null; position: { x: number; y: number }; size: { width: number; height: number }; scaleFactor: number }): MonitorDescriptor {
  return { name: m.name, x: m.position.x, y: m.position.y, width: m.size.width, height: m.size.height, scaleFactor: m.scaleFactor };
}

/** Opens (or focuses, if it already exists) a detached window for `id`. Never creates a second detached window for the same panel id. `pinnedIdentity` is carried across for a panel that was pinned at detach time — null for an unpinned (globally-linked) panel. */
export async function detachPanel(id: ScreenKey, openerLabel: string, pinnedIdentity: WorkspaceIdentity | null): Promise<DetachResult> {
  if (!isDesktopShell()) return { ok: false, reason: "tauri-unavailable" };

  try {
    const [{ WebviewWindow, getAllWebviewWindows }, { availableMonitors, primaryMonitor }] = await Promise.all([
      import("@tauri-apps/api/webviewWindow"),
      import("@tauri-apps/api/window"),
    ]);

    const label = panelWindowLabel(id);
    const existing = (await getAllWebviewWindows()).find((w) => w.label === label);
    if (existing) {
      await existing.setFocus();
      return { ok: true };
    }

    const meta = getPanelMetadata(id);
    const rawMonitors = await availableMonitors();
    if (rawMonitors.length === 0) return { ok: false, reason: "no-monitors" };

    const primary = await primaryMonitor();
    const primaryDescriptor = primary ? toMonitorDescriptor(primary) : null;
    const monitors = rawMonitors.map(toMonitorDescriptor);
    const orderedMonitors = primaryDescriptor
      ? [primaryDescriptor, ...monitors.filter((m) => !(m.x === primaryDescriptor.x && m.y === primaryDescriptor.y))]
      : monitors;

    const desiredWidth = Math.max((meta?.minWidth ?? 480) * 1.6, 640);
    const desiredHeight = Math.max((meta?.minHeight ?? 360) * 1.6, 480);
    const fallback: WindowGeometry = centeredDefaultGeometry(orderedMonitors[0], desiredWidth, desiredHeight);
    const geometry = reconcileWindowGeometry(null, orderedMonitors, fallback);

    let url = `index.html?detachedPanel=${encodeURIComponent(id)}&opener=${encodeURIComponent(openerLabel)}`;
    if (pinnedIdentity) {
      url += `&pinnedIdentity=${encodeURIComponent(JSON.stringify(pinnedIdentity))}`;
    }

    const win = new WebviewWindow(label, {
      url,
      title: meta?.title ?? id,
      x: geometry.x,
      y: geometry.y,
      width: geometry.width,
      height: geometry.height,
      minWidth: meta?.minWidth,
      minHeight: meta?.minHeight,
      resizable: true,
      visible: true,
    });

    return await new Promise<DetachResult>((resolve) => {
      win.once("tauri://created", () => resolve({ ok: true }));
      win.once("tauri://error", () => resolve({ ok: false, reason: "error" }));
    });
  } catch {
    return { ok: false, reason: "error" };
  }
}

/** Called from inside a detached panel window: asks the opener to re-add the panel, then closes this window. Never leaves the panel unreachable — if the opener has since closed, the panel is simply lost from the workstation until reopened from a live window's nav, same as any screen that isn't currently open anywhere. */
export async function reattachAndClose(bootstrap: DetachedPanelBootstrap): Promise<void> {
  if (!isDesktopShell()) return;
  try {
    const { emitTo } = await import("@tauri-apps/api/event");
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await emitTo(bootstrap.openerLabel, REATTACH_EVENT, { panelId: bootstrap.panelId });
    await getCurrentWindow().close();
  } catch {
    // Best-effort: if emit/close fails (e.g. opener window gone), the operator can still close this window manually via the OS chrome.
  }
}
