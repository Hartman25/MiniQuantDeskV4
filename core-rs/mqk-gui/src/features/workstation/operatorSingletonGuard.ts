// GUI-LAYOUT-WAVE-02-FINAL-REPAIR-01 R2: operator-singleton authority must
// hold ACROSS the whole workstation, not merely inside one Dockview API
// instance. `ops` (see panelRegistry.ts's OPERATOR_SINGLETON_PANEL_IDS) is
// now non-detachable, which structurally prevents the control window from
// losing local knowledge of it via detach — but every other path a panel id
// can enter a window (restored layout, custom layout, preset seed, live
// preset event, reattach) must independently reject an operator-singleton
// panel id for any window that is not the control/operator authority
// window, so a stale localStorage value or a hand-crafted payload can never
// grant a second authority surface either.
//
// Pure and DOM-free, same style as layoutModel.ts/presets.ts, so the policy
// is unit-testable without a Dockview/Tauri runtime.

import type { DeskRole } from "../../app/shellTypes";
import { OPERATOR_SINGLETON_PANEL_IDS } from "./panelRegistry";

/** The only desk role permitted to host an operator-singleton panel. */
export const OPERATOR_AUTHORITY_ROLE: DeskRole = "control";

export function isOperatorSingletonPanelId(id: string): boolean {
  return (OPERATOR_SINGLETON_PANEL_IDS as readonly string[]).includes(id);
}

/** True unless `id` is an operator-singleton panel being hosted outside the control/operator authority window. */
export function panelAllowedInRole(id: string, role: DeskRole): boolean {
  return role === OPERATOR_AUTHORITY_ROLE || !isOperatorSingletonPanelId(id);
}

/** Drops any operator-singleton panel id not permitted in `role`, preserving order. */
export function filterPanelIdsForRole<T extends string>(ids: readonly T[], role: DeskRole): T[] {
  return ids.filter((id) => panelAllowedInRole(id, role));
}

/** True if restoring `ids` into `role` would grant a second operator-singleton authority surface. */
export function containsDisallowedOperatorSingleton(ids: readonly string[], role: DeskRole): boolean {
  return ids.some((id) => !panelAllowedInRole(id, role));
}
