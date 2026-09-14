// GUI-LAYOUT-02: workstation layout persistence model — pure, unit-tested.
//
// UI-ONLY STATE: this module ever carries panel placement/arrangement, never
// broker truth, positions, orders, authority, secrets, or runtime safety
// state. A hidden/moved/lost panel never changes what is true, only what is
// currently displayed.
//
// The persisted document is versioned (LAYOUT_SCHEMA_VERSION). Any malformed
// or unrecognized-shape input parses to `null`, and any known-shape document
// referencing removed panel ids is sanitized rather than rejected outright —
// callers fall back to a known-good default layout on either failure mode
// (see Workstation.tsx), so a corrupted or stale localStorage value can never
// crash startup or strand the operator without Tier-2 panels.

import { filterKnownPanelIds, isKnownPanelId, type PanelMetadata } from "./panelRegistry";
import type { ScreenKey } from "../screens/screenRegistry";

export const LAYOUT_SCHEMA_VERSION = 1 as const;

/**
 * `dockviewLayout` is the opaque result of dockview's own `api.toJSON()` —
 * treated as a black box here. `panelIds`/`activePanelId` are OUR redundant,
 * registry-checkable summary of the same state, kept in lockstep by
 * Workstation.tsx on every layout change, so this module can validate
 * against PANEL_REGISTRY without knowing dockview's internal shape.
 */
export interface PersistedWorkstationLayoutV1 {
  schemaVersion: 1;
  panelIds: string[];
  activePanelId: string | null;
  dockviewLayout: unknown;
}

export function isPersistedWorkstationLayoutShape(value: unknown): value is PersistedWorkstationLayoutV1 {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    v.schemaVersion === LAYOUT_SCHEMA_VERSION &&
    Array.isArray(v.panelIds) &&
    v.panelIds.every((id) => typeof id === "string") &&
    (v.activePanelId === null || typeof v.activePanelId === "string") &&
    typeof v.dockviewLayout === "object" &&
    v.dockviewLayout !== null
  );
}

/** Parses a raw persisted string. Any parse error or shape mismatch (including a foreign/future schema version) yields `null` — never a half-formed object. */
export function parsePersistedLayout(raw: string | null | undefined): PersistedWorkstationLayoutV1 | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  return isPersistedWorkstationLayoutShape(parsed) ? parsed : null;
}

export interface SanitizedLayoutSummary {
  panelIds: ScreenKey[];
  activePanelId: ScreenKey | null;
}

/** Drops unknown/removed panel ids from a shape-valid layout and re-derives a safe active panel. Never throws. */
export function sanitizeLayoutSummary(layout: PersistedWorkstationLayoutV1): SanitizedLayoutSummary {
  const panelIds = filterKnownPanelIds(layout.panelIds);
  const requestedActive = layout.activePanelId;
  const activePanelId =
    typeof requestedActive === "string" && isKnownPanelId(requestedActive) && panelIds.includes(requestedActive)
      ? requestedActive
      : (panelIds[0] ?? null);
  return { panelIds, activePanelId };
}

export function serializeLayout(
  panelIds: readonly ScreenKey[],
  activePanelId: ScreenKey | null,
  dockviewLayout: unknown,
): PersistedWorkstationLayoutV1 {
  return {
    schemaVersion: LAYOUT_SCHEMA_VERSION,
    panelIds: [...panelIds],
    activePanelId,
    dockviewLayout,
  };
}

/**
 * Every panel id dockview's own serialized layout references must resolve in
 * PANEL_REGISTRY. Used as a second, independent check before trusting
 * dockview's `fromJSON` — if this ever disagrees with our own `panelIds`
 * summary (e.g. a hand-edited localStorage value), the whole restore is
 * rejected rather than partially applied.
 */
export function allDockviewPanelIdsKnown(dockviewPanelIds: readonly string[]): boolean {
  return dockviewPanelIds.every(isKnownPanelId);
}

/**
 * Pulls the panel-id keys out of dockview's own serialized `panels` map
 * (`SerializedDockview.panels: Record<string, GroupviewPanelState>`) without
 * assuming anything else about dockview's internal shape. Returns `null` if
 * the blob doesn't even have a `panels` object, so the caller can fail
 * closed on a foreign/corrupted value.
 */
export function extractDockviewPanelIds(dockviewLayout: unknown): string[] | null {
  if (typeof dockviewLayout !== "object" || dockviewLayout === null) return null;
  const panels = (dockviewLayout as Record<string, unknown>).panels;
  if (typeof panels !== "object" || panels === null) return null;
  return Object.keys(panels);
}

/**
 * True when two id lists contain exactly the same set of ids (order
 * independent). Used to catch a hand-edited/foreign localStorage value where
 * our own `panelIds` summary and dockview's own blob disagree — such a
 * document is rejected outright rather than partially trusted.
 */
export function sameIdSet(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false;
  const setA = new Set(a);
  for (const id of b) {
    if (!setA.has(id)) return false;
  }
  return true;
}

/** True single-screen default: exactly one panel open, matching the current desk role's prior single-active-screen default. */
export function defaultLayoutPanelIds(initial: ScreenKey): ScreenKey[] {
  return [initial];
}

export function panelMinSizeStyle(meta: Pick<PanelMetadata, "minWidth" | "minHeight">): { minWidth: number; minHeight: number } {
  return { minWidth: meta.minWidth, minHeight: meta.minHeight };
}
