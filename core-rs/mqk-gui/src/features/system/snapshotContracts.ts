import type { MarketDataQualitySummary, OmsOverview, TransportSummary } from "./types";

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object";
const count = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;

export function parseOmsOverview(value: unknown): OmsOverview | null {
  if (!object(value) || value.canonical_route !== "/api/v1/oms/overview" || typeof value.execution_has_snapshot !== "boolean" ||
      !count(value.execution_active_orders) || !count(value.execution_pending_orders)) return null;
  return { truth_state: value.execution_has_snapshot ? "active" : "no_snapshot", detail_authority: "snapshot_counts_only",
    total_active_orders: value.execution_has_snapshot ? value.execution_active_orders : null,
    stuck_orders: null, missing_transition_orders: null, state_nodes: [], transition_edges: [], orders: [] };
}

export function parseTransport(value: unknown): TransportSummary | null {
  if (!object(value) || value.truth_state !== "active" || !Array.isArray(value.queues)) return null;
  for (const key of ["outbox_depth", "inbox_depth", "max_claim_age_ms", "dispatch_retries", "orphaned_claims"]) if (!count(value[key])) return null;
  for (const row of value.queues) {
    if (!object(row) || typeof row.queue_id !== "string" || !(row.direction === "outbox" || row.direction === "inbox") ||
        typeof row.status !== "string" || typeof row.notes !== "string") return null;
    for (const key of ["depth", "oldest_age_ms", "retry_count", "orphaned_claims"]) if (!count(row[key])) return null;
  }
  return { ...value, duplicate_inbox_events: null, queues: value.queues.map((row) => ({ ...row, duplicate_events: null })) } as unknown as TransportSummary;
}

export function parseMarketDataQuality(value: unknown): MarketDataQualitySummary | null {
  if (!object(value) || value.truth_state !== "active" || !["ok", "warning", "critical", "not_configured"].includes(String(value.overall_health)) ||
      typeof value.market_data_source !== "string" || typeof value.ws_continuity !== "string" || !Array.isArray(value.venues) || !Array.isArray(value.issues)) return null;
  return { ...value, detail_authority: "transport_only" } as unknown as MarketDataQualitySummary;
}
