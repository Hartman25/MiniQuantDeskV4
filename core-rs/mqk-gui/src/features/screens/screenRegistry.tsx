import type { ReactElement } from "react";
import { AuditScreen } from "../audit/AuditScreen";
import { DashboardScreen } from "../dashboard/DashboardScreen";
import { ExecutionScreen } from "../execution/ExecutionScreen";
import { AlertsScreen } from "../alerts/AlertsScreen";
import { ConfigScreen } from "../config/ConfigScreen";
import { ArtifactsScreen } from "../artifacts/ArtifactsScreen";
import { MarketDataScreen } from "../marketData/MarketDataScreen";
import { RuntimeScreen } from "../runtime/RuntimeScreen";
import { IncidentsScreen } from "../incidents/IncidentsScreen";
import { MetricsScreen } from "../metrics/MetricsScreen";
import { OpsScreen } from "../ops/OpsScreen";
import { PortfolioScreen } from "../portfolio/PortfolioScreen";
import { ReconcileScreen } from "../reconcile/ReconcileScreen";
import { RiskScreen } from "../risk/RiskScreen";
import { SettingsScreen } from "../settings/SettingsScreen";
import { StrategyScreen } from "../strategy/StrategyScreen";
import { SessionScreen } from "../session/SessionScreen";
import { TopologyScreen } from "../topology/TopologyScreen";
import { TransportScreen } from "../transport/TransportScreen";
import { OperatorTimelineScreen } from "../operatorTimeline/OperatorTimelineScreen";
import type { OperatorActionDefinition, SystemModel } from "../system/types";

export type ScreenKey =
  | "dashboard"
  | "metrics"
  | "execution"
  | "risk"
  | "portfolio"
  | "reconcile"
  | "strategy"
  | "audit"
  | "ops"
  | "settings"
  | "topology"
  | "transport"
  | "incidents"
  | "alerts"
  | "session"
  | "config"
  | "marketData"
  | "runtime"
  | "artifacts"
  | "operatorTimeline";

/**
 * Which monitor this screen is designed to occupy.
 *
 * operator    — control window (monitor 1): decision surfaces, arm/disarm, portfolio, reconcile
 * execution   — execution window (monitor 2): OMS state, traces, timeline drill-down
 * diagnostics — oversight window (monitor 3): audit, forensics, incidents, alerts, supervision
 *
 * MONITOR_GROUPS exports the ordered placement zones so AppShell and DiagnosticsNav can
 * derive defaults and nav lists from a single source of truth rather than hardcoded arrays.
 */
export type MonitorGroup = "operator" | "execution" | "diagnostics";

export interface ScreenRenderContext {
  model: SystemModel;
  selectTimeline: (internalOrderId: string) => void;
  timelineLoading: boolean;
  runAction: (action: OperatorActionDefinition) => void;
}

export interface ScreenDefinition {
  title: string;
  description: string;
  /** Placement zone for three-monitor expansion. See MonitorGroup. */
  monitorGroup: MonitorGroup;
  render: (ctx: ScreenRenderContext) => ReactElement;
}

/**
 * Ordered placement zones for three-monitor expansion.
 * diagnostics[0] is the default screen for the oversight (monitor 3) window.
 * LeftCommandRail primary/secondary split is derived from operator vs diagnostics groups.
 */
export const MONITOR_GROUPS: Record<MonitorGroup, readonly ScreenKey[]> = {
  operator:    ["dashboard", "ops", "portfolio", "reconcile", "strategy", "session", "config", "marketData", "settings"],
  execution:   ["execution"],
  diagnostics: ["audit", "incidents", "alerts", "operatorTimeline", "runtime", "metrics", "topology", "transport", "artifacts", "risk"],
};

/**
 * Curated screen set for each secondary-window role.
 * Source of truth for RoleCommandStrip — control window uses LeftCommandRail (full set).
 *
 * execution: OMS state + outbox/inbox transport supervision + restart boundaries
 * oversight: full diagnostics group (audit, forensics, incidents, alerts, supervision)
 */
export const ROLE_SCREENS: Record<"execution" | "oversight", readonly ScreenKey[]> = {
  execution: ["execution", "transport", "runtime"],
  oversight: MONITOR_GROUPS.diagnostics,
};

export const SCREEN_REGISTRY: Record<ScreenKey, ScreenDefinition> = {
  dashboard: {
    title: "Dashboard",
    description: "Answer in seconds whether the system is alive, safe, and behaving correctly.",
    monitorGroup: "operator",
    render: ({ model }) => <DashboardScreen model={model} />,
  },
  ops: {
    title: "Operator Actions",
    description: "Explicit action catalog with guarded and emergency controls.",
    monitorGroup: "operator",
    render: ({ model, runAction }) => <OpsScreen model={model} onRunAction={runAction} />,
  },
  portfolio: {
    title: "Portfolio",
    description: "Show what the system actually owns, what is working, and what recently changed.",
    monitorGroup: "operator",
    render: ({ model }) => <PortfolioScreen model={model} />,
  },
  reconcile: {
    title: "Reconcile",
    description: "Prove or disprove that broker truth matches internal truth.",
    monitorGroup: "operator",
    render: ({ model }) => <ReconcileScreen model={model} />,
  },
  strategy: {
    title: "Strategy",
    description: "Monitor strategy engines without turning the GUI into manual trading software.",
    monitorGroup: "operator",
    render: ({ model }) => <StrategyScreen model={model} />,
  },
  session: {
    title: "Session",
    description: "Market-state and trading-window visibility for safe operator context.",
    monitorGroup: "operator",
    render: ({ model }) => <SessionScreen model={model} />,
  },
  config: {
    title: "Config",
    description: "Build, policy, profile, and runtime fingerprint visibility.",
    monitorGroup: "operator",
    render: ({ model }) => <ConfigScreen model={model} />,
  },
  marketData: {
    title: "Market Data",
    description: "Feed freshness, venue disagreement, and strategy-blocking data quality issues.",
    monitorGroup: "operator",
    render: ({ model }) => <MarketDataScreen model={model} />,
  },
  settings: {
    title: "Settings / Operations",
    description: "Low-frequency operational metadata and endpoint configuration.",
    monitorGroup: "operator",
    render: ({ model }) => <SettingsScreen model={model} />,
  },
  execution: {
    title: "Execution",
    description: "Primary operational debugging surface for OMS state, traces, replay, and timeline drill-down.",
    monitorGroup: "execution",
    render: ({ model, selectTimeline, timelineLoading }) => (
      <ExecutionScreen model={model} onSelectTimeline={selectTimeline} timelineLoading={timelineLoading} />
    ),
  },
  audit: {
    title: "Logs / Audit",
    description: "Structured event visibility, replay, and operator forensics.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <AuditScreen model={model} />,
  },
  incidents: {
    title: "Incidents",
    description: "Case workspace for grouping alerts, orders, reconcile cases, and operator actions.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <IncidentsScreen model={model} />,
  },
  alerts: {
    title: "Alerts",
    description: "Alert triage board with ack/escalation workflow and incident linkage.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <AlertsScreen model={model} />,
  },
  operatorTimeline: {
    title: "Operator Timeline",
    description: "Chronological record of alerts, operator actions, restarts, config changes, and incidents.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <OperatorTimelineScreen model={model} />,
  },
  runtime: {
    title: "Runtime",
    description: "Leadership, restart boundaries, generation state, and recovery checkpoints.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <RuntimeScreen model={model} />,
  },
  metrics: {
    title: "Metrics",
    description: "Institution-style time-series dashboards for runtime, execution, fill quality, risk, and reconcile pressure.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <MetricsScreen model={model} />,
  },
  topology: {
    title: "Topology",
    description: "Dependency map for daemon, runtime, broker, data, reconcile, audit, strategy, and risk.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <TopologyScreen model={model} />,
  },
  transport: {
    title: "Transport",
    description: "Outbox/inbox transport supervision with claim age, lag, retries, and duplicates.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <TransportScreen model={model} />,
  },
  artifacts: {
    title: "Artifacts",
    description: "Trace, replay, incident, reconcile, and operator evidence bundles.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <ArtifactsScreen model={model} />,
  },
  risk: {
    title: "Risk",
    description: "Make risk posture obvious and hard to ignore.",
    monitorGroup: "diagnostics",
    render: ({ model }) => <RiskScreen model={model} />,
  },
};
