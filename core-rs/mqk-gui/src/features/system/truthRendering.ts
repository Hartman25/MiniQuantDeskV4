import type { CorePanelKey, SystemModel } from "./types.ts";

export type TruthRenderState = "unimplemented" | "unavailable" | "stale" | "no_snapshot" | "degraded";

type PanelTruthRequirement = {
  hints: string[];
  missingMode?: "all" | "any";
};

// Maps a panel key to top-level probe path fragments required for operator truth.
// Rules: top-level probes only — no per-order/per-id fragments;
// paths must match what fetchOperatorModel() actually probes.
const PANEL_TRUTH_REQUIREMENTS: Partial<Record<CorePanelKey, PanelTruthRequirement>> = {
  // execution_orders (HTTP 503) is the definitive "no OMS truth" signal.
  // execution_summary can return HTTP 200 with has_snapshot=false and zero counts —
  // those zeros are honest (there are zero active orders because no loop is running).
  // But an empty orders list is ambiguous: it could mean "genuinely none" or "no snapshot".
  // The 503→missingEndpoints path resolves that ambiguity; only execution_orders being
  // absent should fire no_snapshot.  A single-item hint collapses every() to a simple
  // "is this endpoint missing?" check.
  execution: { hints: ["/execution/orders"] },
  // risk_denials IIFE returns ok: false when truth_state === "no_snapshot"
  // (execution loop not running), landing /risk/denials in missingEndpoints.
  // /risk/summary always returns HTTP 200 (even has_snapshot=false), so it
  // never lands in missingEndpoints and cannot drive this gate.
  // A single-item hint collapses every() to a simple "is this endpoint missing?" check.
  risk: { hints: ["/risk/denials"] },
  // Daemon mounts /reconcile/status — not /reconcile/summary.
  reconcile: { hints: ["/reconcile/status", "/reconcile/mismatches"], missingMode: "any" },
  // Portfolio row truth is gated on /portfolio/positions, not /portfolio/summary.
  // portfolio/summary returns HTTP 200 even when broker_snapshot is absent (has_snapshot:false),
  // so it never appears in missingEndpoints and cannot drive the no_snapshot gate.
  // The /portfolio/positions IIFE in api.ts returns ok:false when snapshot_state === "no_snapshot",
  // landing the endpoint in missingEndpoints and firing this gate.
  // Authoritative empty rows ("active" + []) are NOT caught here — only missing truth blocks.
  portfolio: { hints: ["/portfolio/positions"] },
  // Strategy armed/health state is operator-critical runtime truth.
  strategy: { hints: ["/strategy/summary"] },
  // Session state drives trading-window decisions.
  session: { hints: ["/system/session"] },
  // Config fingerprint is the runtime identity anchor.
  config: { hints: ["/system/config-fingerprint"] },
  // Runtime leadership is a mixed-derived daemon surface (runtime status plus
  // durable run/reconcile evidence when available). Missing endpoint still blocks
  // the runtime panel because generation/recovery truth is operator-critical.
  runtime: { hints: ["/system/runtime-leadership"] },
  // Active alert feed is deferred but absence must not show silent zero to operator.
  alerts: { hints: ["/alerts/active"] },
  // Audit actions are DB truth; empty list on missing endpoint is misleading.
  audit: { hints: ["/audit/operator-actions"] },
  // Artifact registry is DB truth; false zero on missing endpoint must not pass.
  artifacts: { hints: ["/audit/artifacts"] },
  // Operator timeline is the durable DB audit log; empty view is misleading.
  operatorTimeline: { hints: ["/ops/operator-timeline"] },
  // Transport queue depth is deferred; false zero masks stuck orders.
  transport: { hints: ["/execution/transport"] },
  // Incident list is deferred; false zero hides open operator cases.
  incidents: { hints: ["/incidents"] },
  // Market data quality is deferred; false "good" health is dangerous during live trading.
  marketData: { hints: ["/market-data/quality"] },
  // Topology is deferred; empty service map is misleading about system health.
  topology: { hints: ["/system/topology"] },
  // Metrics are deferred; empty charts must not look like real zeros.
  metrics: { hints: ["/metrics/dashboards"] },
  // Ops is the mode-change and action surface; must not render on stale or disconnected truth.
  ops: { hints: ["/system/status"] },
};

// AP-09: Panels whose OMS-level truth depends on proven external broker WS event
// continuity.  When broker_snapshot_source is "external" (Alpaca) but
// alpaca_ws_continuity is not "live", these panels must not render as healthy:
// their truth (OMS order state for execution; cross-comparison for reconcile)
// is derived from the WS trade-updates stream and may be missing events from
// the continuity gap window.
//
// Portfolio is intentionally excluded: portfolio positions come from the Alpaca
// REST snapshot fetch, which is independent of WS event continuity.
const EXTERNAL_BROKER_GATED_PANELS = new Set<CorePanelKey>(["execution", "reconcile"]);

function hasExternalBrokerContinuityGap(model: SystemModel): boolean {
  const source = model.status.broker_snapshot_source;
  const continuity = model.status.alpaca_ws_continuity;
  // Non-external source (paper, legacy-status undefined) never fires.
  if (source !== "external") return false;
  // "not_applicable" cannot coexist with "external" source but guard defensively.
  // Only "live" indicates proven continuity — all other states fail-closed.
  return continuity !== "live" && continuity !== "not_applicable";
}

function isMissingPanelTruth(model: SystemModel, panel: CorePanelKey): boolean {
  const requirement = PANEL_TRUTH_REQUIREMENTS[panel];
  if (!requirement || model.dataSource.state !== "partial") return false;

  const missingChecks = requirement.hints.map((hint) => model.dataSource.missingEndpoints.some((endpoint) => endpoint.includes(hint)));
  if (requirement.missingMode === "any") {
    return missingChecks.some(Boolean);
  }
  return missingChecks.every(Boolean);
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
  // AP-09: External broker WS continuity gate.
  // Execution and reconcile panels require proven WS event continuity when the
  // broker is external (Alpaca).  cold_start_unproven and gap_detected both
  // indicate that OMS state may be missing trade events — fail to no_snapshot.
  if (EXTERNAL_BROKER_GATED_PANELS.has(panel) && hasExternalBrokerContinuityGap(model)) return "no_snapshot";
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
