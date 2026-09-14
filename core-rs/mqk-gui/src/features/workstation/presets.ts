// GUI-LAYOUT-04: Single/Dual/Triple workstation presets + saved custom
// layouts — pure, unit-tested. These are STARTING ARRANGEMENTS, not
// prisons: applying one seeds panel placement, nothing more. This module
// never touches broker/order/authority/runtime state — a preset or custom
// layout document contains panel ids and a dockview arrangement blob only,
// exactly like layoutModel.ts's per-window persistence.

import type { DeskMode, DeskRole } from "../../app/shellTypes";
import { filterKnownPanelIds } from "./panelRegistry";
import type { ScreenKey } from "../screens/screenRegistry";
import {
  LAYOUT_SCHEMA_VERSION,
  isPersistedWorkstationLayoutShape,
  sanitizeLayoutSummary,
  type PersistedWorkstationLayoutV1,
} from "./layoutModel";

export type PresetId = "single" | "dual" | "triple";

export interface PresetWindowSpec {
  role: DeskRole;
  panelIds: ScreenKey[];
}

export interface WorkstationPreset {
  id: PresetId;
  label: string;
  deskMode: DeskMode;
  windows: PresetWindowSpec[];
}

// Sensible, documented defaults per the mission's suggested layouts. These
// are adjustable starting points, not a frozen contract — panel choice here
// is a UX judgment call, not a safety invariant.
export const BUILTIN_PRESETS: Record<PresetId, WorkstationPreset> = {
  single: {
    id: "single",
    label: "Single — Operations",
    deskMode: "single",
    windows: [{ role: "control", panelIds: ["controlStation"] }],
  },
  dual: {
    id: "dual",
    label: "Dual — Operations / Analysis",
    deskMode: "two",
    windows: [
      { role: "control", panelIds: ["controlStation", "portfolio", "risk", "incidents", "dailyOperations"] },
      { role: "execution", panelIds: ["backtests", "marketData", "strategyScanner", "artifacts"] },
    ],
  },
  triple: {
    id: "triple",
    label: "Triple — Research",
    deskMode: "three",
    windows: [
      { role: "control", panelIds: ["controlStation", "risk", "reconcile", "incidents"] },
      { role: "execution", panelIds: ["backtests", "strategyScanner", "marketData"] },
      { role: "oversight", panelIds: ["portfolio", "execution", "artifacts"] },
    ],
  },
};

export function presetForId(id: string): WorkstationPreset | null {
  return id === "single" || id === "dual" || id === "triple" ? BUILTIN_PRESETS[id] : null;
}

export function panelIdsForRole(preset: WorkstationPreset, role: DeskRole): ScreenKey[] {
  return preset.windows.find((w) => w.role === role)?.panelIds ?? [];
}

export function rolesUsedByPreset(preset: WorkstationPreset): DeskRole[] {
  return preset.windows.map((w) => w.role);
}

// ---------------------------------------------------------------------------
// Pending-preset seed: written by the control window for a not-yet-open
// target window (execution/oversight) BEFORE that window is created, so its
// first mount can pick up the intended panel set deterministically instead
// of racing a live cross-window event against React mount order.
// ---------------------------------------------------------------------------

export function pendingPresetStorageKey(role: DeskRole): string {
  return `mqd.workstation.pendingPreset.${role}`;
}

export function writePendingPresetPayload(panelIds: readonly ScreenKey[]): string {
  return JSON.stringify({ schemaVersion: LAYOUT_SCHEMA_VERSION, panelIds: [...panelIds] });
}

/** Fails closed to null on malformed/foreign JSON or an empty/all-unknown panel list. */
export function readPendingPresetPanelIds(raw: string | null): ScreenKey[] | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const value = parsed as Record<string, unknown>;
  if (value.schemaVersion !== LAYOUT_SCHEMA_VERSION || !Array.isArray(value.panelIds)) return null;
  const known = filterKnownPanelIds(value.panelIds.filter((id): id is string => typeof id === "string"));
  return known.length > 0 ? known : null;
}

export const APPLY_PRESET_EVENT = "mqd:workstation:apply-preset";

// ---------------------------------------------------------------------------
// User-saved custom layouts — a named snapshot of one window's full dockview
// arrangement (same document shape as the auto-persisted layout).
// ---------------------------------------------------------------------------

export interface CustomLayout {
  id: string;
  name: string;
  savedAt: string;
  layout: PersistedWorkstationLayoutV1;
}

/** localStorage key for a role's saved-layouts list. I/O itself stays at the call site (AppShell/Workstation), matching layoutModel.ts's pattern — this module stays pure and DOM-free. */
export function customLayoutsStorageKey(role: DeskRole): string {
  return `mqd.workstation.customLayouts.${role}`;
}

function isCustomLayoutShape(value: unknown): value is CustomLayout {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.savedAt === "string" &&
    isPersistedWorkstationLayoutShape(v.layout)
  );
}

/** Parses a role's saved-layouts list from its raw stored string. Drops individual malformed entries rather than discarding the whole list — one corrupted entry must not take down every other saved layout. Malformed/foreign JSON (or no value at all) yields an empty list. */
export function parseCustomLayouts(raw: string | null | undefined): CustomLayout[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter(isCustomLayoutShape);
}

export function serializeCustomLayouts(layouts: readonly CustomLayout[]): string {
  return JSON.stringify(layouts);
}

/** Adds a new named snapshot, or overwrites the existing one with the same name. Returns the updated list — does not touch storage. */
export function withSavedCustomLayout(
  existing: readonly CustomLayout[],
  name: string,
  layout: PersistedWorkstationLayoutV1,
): CustomLayout[] {
  const trimmed = name.trim();
  const withoutSameName = existing.filter((l) => l.name !== trimmed);
  const entry: CustomLayout = {
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    name: trimmed,
    savedAt: new Date().toISOString(),
    layout,
  };
  return [...withoutSameName, entry];
}

export function withRenamedCustomLayout(existing: readonly CustomLayout[], id: string, newName: string): CustomLayout[] {
  const trimmed = newName.trim();
  return existing.map((l) => (l.id === id ? { ...l, name: trimmed } : l));
}

export function withoutCustomLayout(existing: readonly CustomLayout[], id: string): CustomLayout[] {
  return existing.filter((l) => l.id !== id);
}

/** Sanitizes a stored custom layout's panel references the same way a persisted auto-layout is sanitized — a saved layout referencing a since-removed panel must load safely, never crash. */
export function sanitizeCustomLayout(layout: PersistedWorkstationLayoutV1) {
  return sanitizeLayoutSummary(layout);
}
