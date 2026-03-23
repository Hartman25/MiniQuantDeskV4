// core-rs/mqk-gui/src/features/system/types/core.ts
//
// Primitive type aliases and cross-cutting concepts shared by all other
// domain type modules. No imports from sibling type modules.

export type EnvironmentMode = "paper" | "live" | "backtest";
export type RuntimeStatus = "idle" | "starting" | "running" | "paused" | "degraded" | "halted";
export type HealthState = "ok" | "warning" | "critical" | "disconnected" | "unknown";
export type Severity = "info" | "warning" | "critical";
export type ActionLevel = 0 | 1 | 2 | 3;
export type OmsState = "open" | "partially_filled" | "filled" | "cancelled" | "rejected";
export type OperatorTimelineCategory = "alert" | "operator_action" | "mode_transition" | "runtime_restart" | "config_change" | "incident" | "reconcile" | "runtime_transition";

export type DataSourceState = "real" | "partial" | "mock" | "disconnected";

export type SourceAuthority = "db_truth" | "runtime_memory" | "broker_snapshot" | "placeholder" | "mixed" | "unknown";

export type ExplicitSurfaceTruthState = "unknown" | "active" | "not_wired" | "no_db";

export interface ExplicitSurfaceTruth {
  truth_state: ExplicitSurfaceTruthState;
  backend: string | null;
}

export const CORE_PANEL_KEYS = [
  "dashboard",
  "metrics",
  "execution",
  "risk",
  "portfolio",
  "reconcile",
  "strategy",
  "audit",
  "ops",
  "settings",
  "topology",
  "transport",
  "incidents",
  "alerts",
  "session",
  "config",
  "marketData",
  "runtime",
  "artifacts",
  "operatorTimeline",
] as const;

export type CorePanelKey = (typeof CORE_PANEL_KEYS)[number];
export type PanelSourceMap = Record<CorePanelKey, SourceAuthority>;

export interface DataSourceDetail {
  state: DataSourceState;
  reachable: boolean;
  realEndpoints: string[];
  missingEndpoints: string[];
  mockSections: string[];
  message?: string;
}
