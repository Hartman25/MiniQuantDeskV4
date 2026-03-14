import type { CorePanelKey, SystemModel } from "./types.ts";

export type TruthRenderState = "unimplemented" | "unavailable" | "stale" | "no_snapshot" | "degraded";

const PANEL_TRUTH_ENDPOINTS: Partial<Record<CorePanelKey, string[]>> = {
  execution: ["/execution/summary", "/execution/orders", "/execution/timeline"],
  risk: ["/risk/summary"],
  reconcile: ["/reconcile/summary", "/reconcile/mismatches"],
};

function isMissingPanelTruth(model: SystemModel, panel: CorePanelKey): boolean {
  const requiredHints = PANEL_TRUTH_ENDPOINTS[panel];
  if (!requiredHints || model.dataSource.state !== "partial") return false;
  return requiredHints.every((hint) => model.dataSource.missingEndpoints.some((endpoint) => endpoint.includes(hint)));
}

function hasStaleHeartbeat(model: SystemModel): boolean {
  if (!model.connected || !model.status.last_heartbeat) return false;
  const heartbeatMs = Date.parse(model.status.last_heartbeat);
  if (Number.isNaN(heartbeatMs)) return false;
  return Date.now() - heartbeatMs > 120_000;
}

export function panelTruthRenderState(model: SystemModel, panel: CorePanelKey): TruthRenderState | null {
  if (!model.connected || !model.dataSource.reachable || model.dataSource.state === "disconnected") return "unavailable";
  if (model.panelSources[panel] === "placeholder" || model.dataSource.state === "mock") return "unimplemented";
  if (model.status.runtime_status === "degraded" || model.runtimeLeadership.post_restart_recovery_state === "degraded") return "degraded";
  if (hasStaleHeartbeat(model)) return "stale";
  if (isMissingPanelTruth(model, panel)) return "no_snapshot";
  return null;
}

export function truthStateCopy(state: TruthRenderState): { title: string; detail: string } {
  switch (state) {
    case "unimplemented":
      return {
        title: "Unimplemented",
        detail: "This panel is still wired to placeholder data and must not be read as live truth.",
      };
    case "unavailable":
      return {
        title: "Unavailable",
        detail: "Live truth is currently unreachable. Do not treat displayed values as authoritative.",
      };
    case "stale":
      return {
        title: "Stale",
        detail: "Latest heartbeat is outside freshness limits; values may lag current system truth.",
      };
    case "no_snapshot":
      return {
        title: "No snapshot",
        detail: "Required truth snapshot endpoints are missing for this panel.",
      };
    case "degraded":
      return {
        title: "Degraded",
        detail: "Runtime recovery is degraded; treat this panel as partial truth until recovery is complete.",
      };
  }
}
