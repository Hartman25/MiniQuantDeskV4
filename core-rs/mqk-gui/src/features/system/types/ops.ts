// core-rs/mqk-gui/src/features/system/types/ops.ts
//
// Operator-facing types: alerts, feed events, audit, incidents, alert triage,
// operator timeline, and operator action types.

import type { ActionLevel, EnvironmentMode, OperatorTimelineCategory, Severity } from "./core";

export interface OperatorAlert {
  id: string;
  severity: Severity;
  title: string;
  message: string;
  domain: "system" | "execution" | "risk" | "reconcile" | "integrity" | "ops" | "portfolio" | "strategy" | "audit" | "metrics" | "oms";
  acknowledged?: boolean;
}

export interface FeedEvent {
  id: string;
  at: string;
  severity: Severity;
  source: string;
  text: string;
}

export interface AuditActionRow {
  audit_ref: string;
  at: string;
  // actor and environment are not returned by the current daemon backend
  // (audit_events table has no actor/mode column per row); omitted from
  // the DB-backed mapping layer.  Fields are optional so the GUI renders
  // only provably sourced data.
  actor?: string;
  action_key: string;
  environment?: EnvironmentMode;
  target_scope?: string;
  result_state: string;
  warnings: string[];
}

export interface IncidentCase {
  incident_id: string;
  severity: Severity;
  title: string;
  status: "open" | "investigating" | "contained" | "resolved";
  opened_at: string;
  updated_at: string;
  impacted_orders: string[];
  impacted_strategies: string[];
  impacted_subsystems: string[];
  alerts: string[];
  reconcile_case_ids: string[];
  operator_actions_taken: string[];
  final_disposition: string;
}

export interface ReplaceCancelChainRow {
  chain_id: string;
  root_order_id: string;
  current_order_id: string;
  broker_order_id: string | null;
  symbol: string;
  strategy_id: string;
  action_type: "replace" | "cancel";
  status: string;
  request_at: string;
  ack_at: string | null;
  target_order_id: string;
  notes: string;
}

export interface AlertTriageRow {
  alert_id: string;
  severity: Severity;
  status: "unacked" | "acked" | "silenced" | "escalated";
  title: string;
  domain: string;
  linked_incident_id: string | null;
  linked_order_id: string | null;
  linked_strategy_id: string | null;
  created_at: string;
  assigned_to: string | null;
}

export interface OperatorTimelineEvent {
  timeline_event_id: string;
  at: string;
  category: OperatorTimelineCategory;
  severity: Severity;
  title: string;
  summary: string;
  // actor is not returned by the daemon's durable sources (runs +
  // audit_events tables have no per-row actor column); optional so the
  // GUI mapping layer does not fabricate a value.
  actor?: string;
  linked_incident_id: string | null;
  linked_order_id: string | null;
  linked_strategy_id: string | null;
  linked_action_key: string | null;
  linked_config_diff_id: string | null;
  linked_runtime_generation_id: string | null;
}

export interface OperatorActionDefinition {
  // Only the action keys the daemon can actually execute via POST /api/v1/ops/action.
  // "change-system-mode" is intentionally excluded — it returns 409 (requires restart).
  // "arm-strategy" / "disarm-strategy" are daemon-accepted aliases for arm/disarm-execution
  // and may appear in legacy paths; they are included here for mapping completeness.
  action_key:
    | "arm-execution"
    | "arm-strategy"
    | "disarm-execution"
    | "disarm-strategy"
    | "start-system"
    | "stop-system"
    | "kill-switch";
  label: string;
  level: ActionLevel;
  description: string;
  requiresReason: boolean;
  confirmText: string;
  /** Whether this action is currently executable given daemon runtime state. */
  enabled: boolean;
  /** Populated when enabled is false; explains why the action is unavailable. */
  disabledReason?: string;
  /** @deprecated Use !enabled instead. Kept for backward compatibility with OpsScreen rendering. */
  disabled: boolean;
}

export interface OperatorActionReceipt {
  ok: boolean;
  action_key: string;
  environment: EnvironmentMode;
  live_routing_enabled: boolean;
  result_state: string;
  audit_reference: string | null;
  warnings: string[];
  blocking_failures: string[];
}
