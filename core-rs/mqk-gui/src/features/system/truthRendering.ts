import type { CorePanelKey, SystemModel } from "./types.ts";

export type TruthRenderState = "unimplemented" | "unavailable" | "stale" | "no_snapshot" | "degraded";

// Maps a panel key to top-level probe path fragments that must ALL be absent
// from dataSource.realEndpoints before no_snapshot fires.
// Rules: top-level probes only — no per-order/per-id fragments;
// paths must match what fetchOperatorModel() actually probes.
const PANEL_TRUTH_ENDPOINTS: Partial<Record<CorePanelKey, string[]>> = {
  // execution_orders (HTTP 503) is the definitive "no OMS truth" signal.
  // execution_summary can return HTTP 200 with has_snapshot=false and zero counts —
  // those zeros are honest (there are zero active orders because no loop is running).
  // But an empty orders list is ambiguous: it could mean "genuinely none" or "no snapshot".
  // The 503→missingEndpoints path resolves that ambiguity; only execution_orders being
  // absent should fire no_snapshot.  A single-item hint collapses every() to a simple
  // "is this endpoint missing?" check.
  execution: ["/execution/orders"],
  risk: ["/risk/summary"],
  // Daemon mounts /reconcile/status — not /reconcile/summary.
  reconcile: ["/reconcile/status", "/reconcile/mismatches"],
  // Portfolio row truth is gated on /portfolio/positions, not /portfolio/summary.
  // portfolio/summary returns HTTP 200 even when broker_snapshot is absent (has_snapshot:false),
  // so it never appears in missingEndpoints and cannot drive the no_snapshot gate.
  // The /portfolio/positions IIFE in api.ts returns ok:false when snapshot_state === "no_snapshot",
  // landing the endpoint in missingEndpoints and firing this gate.
  // Authoritative empty rows ("active" + []) are NOT caught here — only missing truth blocks.
  portfolio: ["/portfolio/positions"],
  // Strategy armed/health state is operator-critical runtime truth.
  strategy: ["/strategy/summary"],
  // Session state drives trading-window decisions.
  session: ["/system/session"],
  // Config fingerprint is the runtime identity anchor.
  config: ["/system/config-fingerprint"],
  // Runtime leadership tracks daemon generation and recovery state.
  runtime: ["/system/runtime-leadership"],
  // Active alert feed is deferred but absence must not show silent zero to operator.
  alerts: ["/alerts/active"],
  // Audit actions are DB truth; empty list on missing endpoint is misleading.
  audit: ["/audit/operator-actions"],
  // Artifact registry is DB truth; false zero on missing endpoint must not pass.
  artifacts: ["/audit/artifacts"],
  // Operator timeline is the durable DB audit log; empty view is misleading.
  operatorTimeline: ["/ops/operator-timeline"],
  // Transport queue depth is deferred; false zero masks stuck orders.
  transport: ["/execution/transport"],
  // Incident list is deferred; false zero hides open operator cases.
  incidents: ["/incidents"],
  // Market data quality is deferred; false "good" health is dangerous during live trading.
  marketData: ["/market-data/quality"],
  // Topology is deferred; empty service map is misleading about system health.
  topology: ["/system/topology"],
  // Metrics are deferred; empty charts must not look like real zeros.
  metrics: ["/metrics/dashboards"],
  // Ops is the mode-change and action surface; must not render on stale or disconnected truth.
  ops: ["/system/status"],
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
