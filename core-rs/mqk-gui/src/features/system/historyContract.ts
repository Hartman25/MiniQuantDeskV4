import type { FeedEvent, OperatorTimelineEvent } from "./types";

const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object";
const nullableString = (value: unknown) => value === null || typeof value === "string";
export function isSourceTimestamp(value: unknown): value is string {
  return typeof value === "string" && /(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value));
}

/** Exact repeats collapse; conflicting evidence for one identity fails closed. */
export function canonicalHistory<T>(rows: T[], id: (row: T) => string, at: (row: T) => string): T[] | null {
  const unique = new Map<string, T>();
  for (const row of rows) {
    const previous = unique.get(id(row));
    if (previous && JSON.stringify(previous) !== JSON.stringify(row)) return null;
    unique.set(id(row), row);
  }
  return [...unique.values()].sort((a, b) => Date.parse(at(b)) - Date.parse(at(a)) ||
    (id(a) < id(b) ? -1 : id(a) > id(b) ? 1 : 0));
}

export function parseEventFeed(value: unknown): FeedEvent[] | null {
  if (!record(value) || value.truth_state !== "active" || !Array.isArray(value.rows)) return null;
  const rows: FeedEvent[] = [];
  for (const row of value.rows) {
    if (!record(row) || typeof row.event_id !== "string" || !row.event_id || !isSourceTimestamp(row.ts_utc) ||
        typeof row.kind !== "string" || typeof row.detail !== "string" || !nullableString(row.run_id) ||
        !(row.audit_event_id === undefined || nullableString(row.audit_event_id))) return null;
    rows.push({ id: row.event_id, at: row.ts_utc, source: row.kind, text: row.detail,
      severity: row.kind === "orchestrator_halt" ? "critical" : "info", run_id: row.run_id,
      audit_event_id: row.audit_event_id as string | null | undefined ?? null });
  }
  return canonicalHistory(rows, (row) => row.id, (row) => row.at);
}

export function parseOperatorTimeline(value: unknown): OperatorTimelineEvent[] | null {
  if (!record(value) || value.truth_state !== "active" || !Array.isArray(value.rows)) return null;
  const rows: OperatorTimelineEvent[] = [];
  for (const row of value.rows) {
    if (!record(row) || typeof row.provenance_ref !== "string" || !row.provenance_ref || !isSourceTimestamp(row.ts_utc) ||
        !(row.kind === "runtime_transition" || row.kind === "operator_action") || typeof row.detail !== "string" ||
        !nullableString(row.run_id) || !(row.audit_event_id === undefined || nullableString(row.audit_event_id))) return null;
    rows.push({ timeline_event_id: row.provenance_ref, at: row.ts_utc, category: row.kind,
      severity: "info", title: row.detail, summary: row.detail,
      audit_event_id: row.audit_event_id as string | null | undefined ?? null,
      linked_incident_id: null, linked_order_id: null, linked_strategy_id: null, linked_action_key: null,
      linked_config_diff_id: null, linked_runtime_generation_id: row.run_id });
  }
  return canonicalHistory(rows, (row) => row.timeline_event_id, (row) => row.at);
}
