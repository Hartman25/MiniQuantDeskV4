import type { IncidentCase } from "./types";

export function parseIncidents(value: unknown): IncidentCase[] | null {
  if (!value || typeof value !== "object") return null;
  const wrapper = value as Record<string, unknown>;
  if (wrapper.truth_state !== "active" || !Array.isArray(wrapper.rows)) return null;
  const rows: IncidentCase[] = [];
  const ids = new Set<string>();
  for (const value of wrapper.rows) {
    if (!value || typeof value !== "object") return null;
    const row = value as Record<string, unknown>;
    if (typeof row.incident_id !== "string" || !row.incident_id || ids.has(row.incident_id) ||
        typeof row.opened_at_utc !== "string" || !Number.isFinite(Date.parse(row.opened_at_utc)) ||
        typeof row.title !== "string" || typeof row.opened_by !== "string" ||
        !["info", "warning", "critical"].includes(String(row.severity)) ||
        !["open", "resolved"].includes(String(row.status)) ||
        !(row.linked_alert_id === null || typeof row.linked_alert_id === "string")) return null;
    ids.add(row.incident_id);
    rows.push({
      incident_id: row.incident_id,
      severity: row.severity as IncidentCase["severity"],
      title: row.title,
      status: row.status as IncidentCase["status"],
      opened_at: row.opened_at_utc,
      opened_by: row.opened_by,
      linked_alert_id: row.linked_alert_id,
      detail_authority: "summary_only",
      updated_at: null,
      impacted_orders: [], impacted_strategies: [], impacted_subsystems: [],
      alerts: row.linked_alert_id === null ? [] : [row.linked_alert_id],
      reconcile_case_ids: [], operator_actions_taken: [], final_disposition: "",
    });
  }
  return rows.sort((a, b) => Date.parse(b.opened_at) - Date.parse(a.opened_at) ||
    (a.incident_id < b.incident_id ? -1 : a.incident_id > b.incident_id ? 1 : 0));
}
