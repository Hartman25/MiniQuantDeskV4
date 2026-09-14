// GUI-LAYOUT-01: workstation panel registry and display-priority contract.
//
// This is an adapter over the existing SCREEN_REGISTRY (features/screens/
// screenRegistry.tsx), not a parallel truth: every panel id is a ScreenKey,
// title/description/category are read directly from SCREEN_REGISTRY, and
// rendering still goes through SCREEN_REGISTRY[key].render. This module only
// adds the metadata the workstation layout system (docking, detach, presets)
// needs to place, duplicate-guard, and fail closed on panels.
//
// Tier 0 (fixed safety truth: environment/runtime/broker/db/market-data/
// reconcile/integrity/audit health, kill-switch and live-routing truth,
// critical/warning incident-alert indication, and daemon reachability) is
// rendered directly by GlobalStatusBar from SystemStatus and is never a
// movable panel — TIER0_STATUS_FIELDS below is a compile-time-checked
// contract, not a registry entry.
//
// Every screen is a Tier 1 or Tier 2 *panel*:
//   tier1 — must remain reachable without a full workspace switch (e.g. via a
//           drawer/context surface) on single-screen layouts.
//   tier2 — contextual/movable workstation panel: dockable, resizable,
//           tabbable, detachable, hideable.

import { SCREEN_REGISTRY, type MonitorGroup, type ScreenKey } from "../screens/screenRegistry";
import type { SystemStatus } from "../system/types";

export type PanelTier = "tier1" | "tier2";

/**
 * "operator-singleton" panels expose guarded/emergency action surfaces (arm,
 * mode change, and other server-side operator actions routed through
 * runAction). They must never be duplicated into more than one open
 * instance — duplication would create a second authority path to the same
 * server-side action. Detaching a singleton panel MOVES it; it does not
 * create a second instance. All other panels are read-only and duplicable.
 */
export type PanelAuthorityClass = "read-only" | "operator-singleton";

interface RawPanelContract {
  tier: PanelTier;
  authority: PanelAuthorityClass;
  detachable: boolean;
  /** Whether this panel's content changes based on linked/pinned WorkspaceIdentity (see workspaceModel.ts). */
  contextAware: boolean;
  minWidth: number;
  minHeight: number;
}

export interface PanelMetadata {
  id: ScreenKey;
  title: string;
  description: string;
  category: MonitorGroup;
  tier: PanelTier;
  authority: PanelAuthorityClass;
  detachable: boolean;
  /** Derived from authority — never set independently. Only a read-only panel may be duplicable. */
  duplicable: boolean;
  contextAware: boolean;
  minWidth: number;
  minHeight: number;
}

// Evidence-based, not invented: `ops` is the sole screen whose render() is
// given `runAction` (see screenRegistry.tsx) — it is the only current
// operator-authority action surface. `contextAware: true` is set only for
// backtests and marketData, the only two screens that currently call
// useWorkspaceContext().
//
// WAVE-02-FINAL-REPAIR-01 R2: `ops` is non-detachable. Detaching it would
// remove it from the control window's local Dockview instance while leaving
// it live in a separate Tauri window; reopening it from control navigation
// would then create a second `ops` instance — two authority surfaces for the
// same guarded operator-action panel. See operatorSingletonGuard.ts for the
// rest of the cross-window enforcement (restored/custom layouts, presets,
// detached-window bootstrap all independently reject `ops` outside control).
const PANEL_CONTRACTS: Record<ScreenKey, RawPanelContract> = {
  controlStation:   { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 420, minHeight: 340 },
  dashboard:        { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 420, minHeight: 340 },
  metrics:          { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  execution:        { tier: "tier1", authority: "read-only",         detachable: true, contextAware: false, minWidth: 420, minHeight: 320 },
  risk:             { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  portfolio:        { tier: "tier1", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  reconcile:        { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  strategy:         { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  audit:            { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  ops:              { tier: "tier1", authority: "operator-singleton", detachable: false, contextAware: false, minWidth: 380, minHeight: 320 },
  settings:         { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  topology:         { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  transport:        { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  incidents:        { tier: "tier1", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  alerts:           { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  session:          { tier: "tier1", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  config:           { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  marketData:       { tier: "tier2", authority: "read-only",         detachable: true, contextAware: true,  minWidth: 480, minHeight: 360 },
  ingest:           { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  runtime:          { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  artifacts:        { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  operatorTimeline: { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  backtests:        { tier: "tier2", authority: "read-only",         detachable: true, contextAware: true,  minWidth: 480, minHeight: 360 },
  strategyScanner:  { tier: "tier2", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
  dailyOperations:  { tier: "tier1", authority: "read-only",         detachable: true, contextAware: false, minWidth: 360, minHeight: 280 },
};

function buildPanelMetadata(id: ScreenKey, contract: RawPanelContract): PanelMetadata {
  const screen = SCREEN_REGISTRY[id];
  return {
    id,
    title: screen.title,
    description: screen.description,
    category: screen.monitorGroup,
    tier: contract.tier,
    authority: contract.authority,
    detachable: contract.detachable,
    duplicable: contract.authority === "read-only",
    contextAware: contract.contextAware,
    minWidth: contract.minWidth,
    minHeight: contract.minHeight,
  };
}

export const PANEL_REGISTRY: Readonly<Record<ScreenKey, PanelMetadata>> = Object.freeze(
  Object.fromEntries(
    (Object.keys(PANEL_CONTRACTS) as ScreenKey[]).map((id) => [id, buildPanelMetadata(id, PANEL_CONTRACTS[id])]),
  ) as Record<ScreenKey, PanelMetadata>,
);

export const PANEL_IDS: readonly ScreenKey[] = Object.keys(PANEL_REGISTRY) as ScreenKey[];

export const TIER1_PANEL_IDS: readonly ScreenKey[] = PANEL_IDS.filter((id) => PANEL_REGISTRY[id].tier === "tier1");
export const TIER2_PANEL_IDS: readonly ScreenKey[] = PANEL_IDS.filter((id) => PANEL_REGISTRY[id].tier === "tier2");

export const OPERATOR_SINGLETON_PANEL_IDS: readonly ScreenKey[] = PANEL_IDS.filter(
  (id) => PANEL_REGISTRY[id].authority === "operator-singleton",
);

/** Fail-closed lookup: an unknown/stale panel id (e.g. from a persisted layout) resolves to null — never fabricated metadata. */
export function getPanelMetadata(id: string): PanelMetadata | null {
  return Object.prototype.hasOwnProperty.call(PANEL_REGISTRY, id) ? PANEL_REGISTRY[id as ScreenKey] : null;
}

export function isKnownPanelId(id: string): id is ScreenKey {
  return getPanelMetadata(id) !== null;
}

/** Drops unknown/stale ids from a persisted list instead of throwing — a removed panel must not crash startup. Order-preserving. */
export function filterKnownPanelIds(ids: readonly string[]): ScreenKey[] {
  return ids.filter(isKnownPanelId);
}

/**
 * GUI-LAYOUT-05 repair: which dockview panel-render mode a panel needs.
 * dockview's default ("onlyWhenVisible") destroys a panel's component
 * instance whenever its tab isn't the active one and recreates it fresh —
 * which silently erases any panel-local React state (e.g. pin/unpin) on
 * every tab switch. Only a contextAware panel owns that kind of local
 * state (see PanelHost in Workstation.tsx), so only it opts into "always"
 * (kept mounted, hidden via CSS) to survive being backgrounded; every other
 * panel keeps the default and its lower DOM/memory footprint. Returns a
 * bare string rather than dockview's own type so this module stays free of
 * a dockview import — Workstation.tsx passes the result straight through.
 */
export function panelRendererFor(id: string): "always" | undefined {
  return getPanelMetadata(id)?.contextAware ? "always" : undefined;
}

/**
 * Fixed Tier-0 safety-truth fields, always rendered by GlobalStatusBar
 * directly from SystemStatus. Not user-hideable, not part of the panel
 * registry, never movable/detachable/duplicable.
 *
 * kill_switch_active, live_routing_enabled, has_critical, has_warning, and
 * daemon_reachable were added by the WAVE-02-FINAL-REPAIR-01 R1 repair: the
 * original 9-field list omitted kill-switch truth, live-routing truth,
 * critical/warning incident-alert indication, and stale/offline/unavailable
 * indication, none of which may disappear when movable panels are hidden.
 */
export const TIER0_STATUS_FIELDS = [
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
] as const;

export type Tier0StatusField = (typeof TIER0_STATUS_FIELDS)[number];

// Compile-time contract: every TIER0_STATUS_FIELDS entry must be a real
// SystemStatus field. If a field is ever renamed/removed on SystemStatus
// without updating this list, `npm run build`'s tsc pass fails closed here
// rather than silently letting Tier-0 truth drift from the type.
type AssertTier0FieldsExist = Tier0StatusField extends keyof SystemStatus ? true : never;
const _tier0FieldsAssertion: AssertTier0FieldsExist = true;
void _tier0FieldsAssertion;
