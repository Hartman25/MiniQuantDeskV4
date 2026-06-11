// core-rs/mqk-gui/src/features/system/haltSummary.ts
//
// GUI-HALT-CAUSE-TIMELINE-SURFACE-01: derives a single, honest "what halted,
// why, and when" summary from data the operator model already carries.
//
// Read-only derivation. No new backend routes, no fabricated state. If the
// model is disconnected, status is "unknown" — never a fake "no_halt".

import type { FeedEvent, Severity, SystemModel } from "./types.ts";

export type HaltSummaryStatus = "active_halt" | "recent_halt" | "no_halt" | "unknown";

export const ORCHESTRATOR_HALT_SOURCE = "orchestrator_halt";

export interface HaltSummary {
  status: HaltSummaryStatus;
  reason: string | null;
  run_id: string | null;
  timestamp: string | null;
  audit_event_id: string | null;
  source_event: string | null;
  severity: Severity;
  live_routing_enabled: boolean | null;
  reconcile_status: string | null;
  kill_switch_active: boolean | null;
  integrity_halt_active: boolean | null;
  risk_halt_active: boolean | null;
  deadman_status: string | null;
  next_operator_action: string | null;
}

const UNKNOWN_SUMMARY: HaltSummary = {
  status: "unknown",
  reason: null,
  run_id: null,
  timestamp: null,
  audit_event_id: null,
  source_event: null,
  severity: "warning",
  live_routing_enabled: null,
  reconcile_status: null,
  kill_switch_active: null,
  integrity_halt_active: null,
  risk_halt_active: null,
  deadman_status: null,
  next_operator_action: null,
};

export function deriveLatestHaltSummary(model: SystemModel): HaltSummary {
  if (!model.connected) {
    return UNKNOWN_SUMMARY;
  }

  const { status, autonomousPaperStatus, feed } = model;

  const killSwitchActive = status.kill_switch_active;
  const integrityHaltActive = status.integrity_halt_active;
  const riskHaltActive = status.risk_halt_active;
  const activeHalt = killSwitchActive || integrityHaltActive || riskHaltActive || status.runtime_status === "halted";

  const latestHaltEvent = (feed ?? [])
    .filter((event) => event.source === ORCHESTRATOR_HALT_SOURCE)
    .reduce<(typeof feed)[number] | null>((latest, event) => {
      if (!latest) return event;
      return Date.parse(event.at) > Date.parse(latest.at) ? event : latest;
    }, null);

  let summaryStatus: HaltSummaryStatus;
  let reason: string | null;
  if (activeHalt) {
    summaryStatus = "active_halt";
    reason = latestHaltEvent ? latestHaltEvent.text : "unknown";
  } else if (latestHaltEvent) {
    summaryStatus = "recent_halt";
    reason = latestHaltEvent.text;
  } else {
    summaryStatus = "no_halt";
    reason = null;
  }

  const liveRoutingEnabled = status.live_routing_enabled;
  const reconcileStatus = autonomousPaperStatus.reconcile_status;

  let severity: Severity = "info";
  if (activeHalt) {
    severity = "critical";
  } else if (summaryStatus === "recent_halt") {
    severity = "warning";
  }
  // Live routing enabled is anomalous regardless of halt state — never hide it.
  if (liveRoutingEnabled) {
    severity = "critical";
  }
  // Dirty/stale reconcile is unsafe regardless of halt state — never hide it.
  if ((reconcileStatus === "dirty" || reconcileStatus === "stale") && severity !== "critical") {
    severity = "warning";
  }

  return {
    status: summaryStatus,
    reason,
    run_id: latestHaltEvent?.run_id ?? null,
    timestamp: latestHaltEvent?.at ?? null,
    audit_event_id: latestHaltEvent?.audit_event_id ?? null,
    source_event: latestHaltEvent?.source ?? null,
    severity,
    live_routing_enabled: liveRoutingEnabled,
    reconcile_status: reconcileStatus,
    kill_switch_active: killSwitchActive,
    integrity_halt_active: integrityHaltActive,
    risk_halt_active: riskHaltActive,
    deadman_status: status.deadman_status,
    next_operator_action: autonomousPaperStatus.next_operator_action,
  };
}

const DEFAULT_HALT_HISTORY_LIMIT = 10;

function haltEventTimestamp(event: FeedEvent): number {
  const parsed = Date.parse(event.at);
  return Number.isNaN(parsed) ? 0 : parsed;
}

// GUI-ALERTS-HALT-HISTORY-FILTER-SURFACE-01: selects orchestrator_halt rows
// from the events feed, newest first, for the Alerts halt history panel.
// Read-only — does not mutate or reinterpret event fields.
export function selectHaltEvents(feed: FeedEvent[] | null | undefined, limit = DEFAULT_HALT_HISTORY_LIMIT): FeedEvent[] {
  return (feed ?? [])
    .filter((event) => event.source === ORCHESTRATOR_HALT_SOURCE)
    .slice()
    .sort((a, b) => haltEventTimestamp(b) - haltEventTimestamp(a))
    .slice(0, limit);
}
