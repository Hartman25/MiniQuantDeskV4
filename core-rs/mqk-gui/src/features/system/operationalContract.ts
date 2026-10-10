import { hasRows } from "./rowContract";

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object";
const strings = (value: unknown) => Array.isArray(value) && value.every((entry) => typeof entry === "string");
const count = (value: unknown) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const nullableNumber = (value: unknown) => value === null || typeof value === "number" && Number.isFinite(value);
const nullableTime = (value: unknown) => value === null || typeof value === "string" && /(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value));
const stringFields = (value: Record<string, unknown>, keys: string[]) => keys.every((key) => typeof value[key] === "string");

/** Structural guards for existing canonical read models. Unknown or incomplete bodies are absent truth. */
export function validOperationalPayload(kind: "executionSummary" | "executionOrders" | "session" | "config" | "metadata" | "runtime" | "topology", value: unknown): boolean {
  if (kind === "executionOrders") return Array.isArray(value) && hasRows({ rows: value }, "rows", ["internal_order_id", "symbol", "current_status", "current_stage", "updated_at"]) &&
    value.every((row) => object(row) && (row.requested_qty === null || count(row.requested_qty)) && (row.filled_qty === null || count(row.filled_qty)) &&
      typeof row.has_warning === "boolean" && typeof row.has_critical === "boolean" && nullableTime(row.updated_at));
  if (!object(value)) return false;
  switch (kind) {
    case "executionSummary": return typeof value.has_snapshot === "boolean" &&
      ["active_orders", "pending_orders", "dispatching_orders", "reject_count_today", "stuck_orders"].every((key) => count(value[key])) &&
      (value.cancel_replace_count_today === null || count(value.cancel_replace_count_today)) && nullableNumber(value.avg_ack_latency_ms);
    case "session": return ["premarket", "regular", "after_hours", "closed"].includes(String(value.market_session)) &&
      ["open", "halted", "closed", "holiday"].includes(String(value.exchange_calendar_state)) &&
      ["enabled", "disabled", "exit_only"].includes(String(value.system_trading_window)) && typeof value.strategy_allowed === "boolean" &&
      nullableTime(value.next_session_change_at) && strings(value.notes) &&
      (value.supported_session_profiles === undefined || strings(value.supported_session_profiles)) &&
      ["calendar_spec_id", "session_profile", "session_authority", "session_profile_reason_code", "session_profile_message", "daemon_mode", "adapter_id", "operator_auth_mode"].every((key) => value[key] === undefined || typeof value[key] === "string") &&
      (value.session_profile_is_open === undefined || value.session_profile_is_open === null || typeof value.session_profile_is_open === "boolean") &&
      (value.deployment_start_allowed === undefined || typeof value.deployment_start_allowed === "boolean");
    case "config": return stringFields(value, ["config_hash", "risk_policy_version", "strategy_bundle_version", "build_version", "environment_profile", "runtime_generation_id"]) && nullableTime(value.last_restart_at);
    case "metadata": return stringFields(value, ["build_version", "api_version", "broker_adapter"]) && ["ok", "warning", "critical", "disconnected", "unknown"].includes(String(value.endpoint_status));
    case "runtime": return stringFields(value, ["leader_node", "generation_id", "recovery_checkpoint"]) &&
      ["held", "contested", "lost"].includes(String(value.leader_lease_state)) && ["complete", "in_progress", "degraded"].includes(String(value.post_restart_recovery_state)) &&
      (value.restart_count_24h === null || count(value.restart_count_24h)) && nullableTime(value.last_restart_at) &&
      hasRows(value, "checkpoints", ["checkpoint_id", "checkpoint_type", "timestamp", "generation_id", "leader_node", "status", "note"]);
    case "topology": return value.truth_state === "active" && nullableTime(value.updated_at) &&
      hasRows(value, "services", ["service_key", "label", "layer", "health", "role", "failure_impact", "notes"]) &&
      (value.services as Record<string, unknown>[]).every((row) => strings(row.dependency_keys) && nullableTime(row.last_heartbeat) && nullableNumber(row.latency_ms));
  }
}
