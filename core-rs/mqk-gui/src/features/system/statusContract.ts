import type { SystemStatus } from "./types";
import { isSourceTimestamp } from "./historyContract";

export function parseSystemStatus(value: unknown): SystemStatus | null {
  if (!value || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  const booleans = ["deployment_start_allowed", "has_warning", "has_critical", "strategy_armed", "execution_armed", "kill_switch_active", "risk_halt_active", "integrity_halt_active", "daemon_reachable"];
  if (booleans.some((key) => typeof row[key] !== "boolean") ||
      !(row.live_routing_enabled === null || typeof row.live_routing_enabled === "boolean") ||
      !(row.environment === null || ["paper", "live", "backtest", "unknown"].includes(String(row.environment))) ||
      !["idle", "starting", "running", "paused", "degraded", "halted", "unknown"].includes(String(row.runtime_status)) ||
      !["synthetic", "external"].includes(String(row.broker_snapshot_source)) ||
      !["not_applicable", "cold_start_unproven", "live", "gap_detected"].includes(String(row.alpaca_ws_continuity))) return null;
  for (const key of ["daemon_mode", "adapter_id", "broker_status", "db_status", "market_data_health", "reconcile_status", "integrity_status", "audit_writer_status", "deadman_status", "asset_class_scope", "parity_evidence_state"]) {
    if (typeof row[key] !== "string") return null;
  }
  for (const key of ["active_account_id", "config_profile"]) if (!(row[key] === null || typeof row[key] === "string")) return null;
  if (!(row.last_heartbeat === null || isSourceTimestamp(row.last_heartbeat)) ||
      !(row.loop_latency_ms === null || typeof row.loop_latency_ms === "number" && Number.isFinite(row.loop_latency_ms) && row.loop_latency_ms >= 0)) return null;
  return { ...row, environment: row.environment ?? "unknown" } as unknown as SystemStatus;
}
