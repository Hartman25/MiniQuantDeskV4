import type { AlertTriageRow, OperatorAlert } from "./types";
import { mapActiveAlertsResponse, type ActiveAlertsWrapper } from "./legacy";

const severity = (value: unknown) => ["info", "warning", "critical"].includes(String(value));
const nullableString = (value: unknown) => value === null || typeof value === "string";
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object";

export function parseActiveAlerts(value: unknown): OperatorAlert[] | null {
  if (!object(value) || value.truth_state !== "active" || !Array.isArray(value.rows) || value.alert_count !== value.rows.length) return null;
  const ids = new Set<string>();
  for (const row of value.rows) {
    if (!object(row) || typeof row.alert_id !== "string" || !row.alert_id || ids.has(row.alert_id) ||
        !severity(row.severity) || typeof row.class !== "string" || typeof row.summary !== "string" ||
        !nullableString(row.detail) || typeof row.source !== "string") return null;
    ids.add(row.alert_id);
  }
  return mapActiveAlertsResponse(value as unknown as ActiveAlertsWrapper);
}

export interface AlertTriageSnapshot {
  truth_state: "active" | "no_db";
  note: string;
  rows: AlertTriageRow[];
}

export function parseAlertTriage(value: unknown): AlertTriageSnapshot | null {
  if (!object(value) || !["active", "no_db"].includes(String(value.truth_state)) || !Array.isArray(value.rows) || typeof value.triage_note !== "string") return null;
  const rows: AlertTriageRow[] = [];
  const ids = new Set<string>();
  for (const row of value.rows) {
    if (!object(row) || typeof row.alert_id !== "string" || !row.alert_id || ids.has(row.alert_id) ||
        !severity(row.severity) || !["acked", "unacked"].includes(String(row.status)) ||
        typeof row.title !== "string" || typeof row.domain !== "string" ||
        !nullableString(row.linked_incident_id) || !nullableString(row.linked_order_id) || !nullableString(row.linked_strategy_id) ||
        !nullableString(row.assigned_to) || !nullableString(row.created_at) ||
        (row.created_at !== null && !Number.isFinite(Date.parse(row.created_at as string))) ||
        !(row.linked_incident_status === null || row.linked_incident_status === "open" || row.linked_incident_status === "resolved")) return null;
    if (row.status === "acked" && (value.truth_state !== "active" || row.created_at === null)) return null;
    ids.add(row.alert_id);
    rows.push(row as unknown as AlertTriageRow);
  }
  return { truth_state: value.truth_state as AlertTriageSnapshot["truth_state"], note: value.triage_note, rows };
}
