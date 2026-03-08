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

export interface ScreenRenderContext {
  model: SystemModel;
  selectTimeline: (internalOrderId: string) => void;
  timelineLoading: boolean;
  runAction: (action: OperatorActionDefinition) => void;
  changeMode: (targetMode: SystemModel["status"]["environment"]) => void;
}

export interface ScreenDefinition {
  title: string;
  description: string;
  render: (ctx: ScreenRenderContext) => ReactElement;
}

export const SCREEN_REGISTRY: Record<ScreenKey, ScreenDefinition> = {
  dashboard: {
    title: "Dashboard",
    description: "Answer in seconds whether the system is alive, safe, and behaving correctly.",
    render: ({ model }) => <DashboardScreen model={model} />,
  },
  metrics: {
    title: "Metrics",
    description: "Institution-style time-series dashboards for runtime, execution, fill quality, risk, and reconcile pressure.",
    render: ({ model }) => <MetricsScreen model={model} />,
  },
  topology: {
    title: "Topology",
    description: "Dependency map for daemon, runtime, broker, data, reconcile, audit, strategy, and risk.",
    render: ({ model }) => <TopologyScreen model={model} />,
  },
  transport: {
    title: "Transport",
    description: "Outbox/inbox transport supervision with claim age, lag, retries, and duplicates.",
    render: ({ model }) => <TransportScreen model={model} />,
  },
  incidents: {
    title: "Incidents",
    description: "Case workspace for grouping alerts, orders, reconcile cases, and operator actions.",
    render: ({ model }) => <IncidentsScreen model={model} />,
  },
  alerts: {
    title: "Alerts",
    description: "Alert triage board with ack/escalation workflow and incident linkage.",
    render: ({ model }) => <AlertsScreen model={model} />,
  },
  operatorTimeline: {
    title: "Operator Timeline",
    description: "Chronological record of alerts, operator actions, restarts, config changes, and incidents.",
    render: ({ model }) => <OperatorTimelineScreen model={model} />,
  },
  session: {
    title: "Session",
    description: "Market-state and trading-window visibility for safe operator context.",
    render: ({ model }) => <SessionScreen model={model} />,
  },
  config: {
    title: "Config",
    description: "Build, policy, profile, and runtime fingerprint visibility.",
    render: ({ model }) => <ConfigScreen model={model} />,
  },
  runtime: {
    title: "Runtime",
    description: "Leadership, restart boundaries, generation state, and recovery checkpoints.",
    render: ({ model }) => <RuntimeScreen model={model} />,
  },
  marketData: {
    title: "Market Data",
    description: "Feed freshness, venue disagreement, and strategy-blocking data quality issues.",
    render: ({ model }) => <MarketDataScreen model={model} />,
  },
  execution: {
    title: "Execution",
    description: "Primary operational debugging surface for OMS state, traces, replay, and timeline drill-down.",
    render: ({ model, selectTimeline, timelineLoading }) => (
      <ExecutionScreen model={model} onSelectTimeline={selectTimeline} timelineLoading={timelineLoading} />
    ),
  },
  risk: {
    title: "Risk",
    description: "Make risk posture obvious and hard to ignore.",
    render: ({ model }) => <RiskScreen model={model} />,
  },
  portfolio: {
    title: "Portfolio",
    description: "Show what the system actually owns, what is working, and what recently changed.",
    render: ({ model }) => <PortfolioScreen model={model} />,
  },
  reconcile: {
    title: "Reconcile",
    description: "Prove or disprove that broker truth matches internal truth.",
    render: ({ model }) => <ReconcileScreen model={model} />,
  },
  strategy: {
    title: "Strategy",
    description: "Monitor strategy engines without turning the GUI into manual trading software.",
    render: ({ model }) => <StrategyScreen model={model} />,
  },
  audit: {
    title: "Logs / Audit",
    description: "Structured event visibility, replay, and operator forensics.",
    render: ({ model }) => <AuditScreen model={model} />,
  },
  artifacts: {
    title: "Artifacts",
    description: "Trace, replay, incident, reconcile, and operator evidence bundles.",
    render: ({ model }) => <ArtifactsScreen model={model} />,
  },
  ops: {
    title: "Operator Actions",
    description: "Explicit action catalog with guarded and emergency controls.",
    render: ({ model, runAction, changeMode }) => <OpsScreen model={model} onRunAction={runAction} onChangeMode={changeMode} />,
  },
  settings: {
    title: "Settings / Operations",
    description: "Low-frequency operational metadata and endpoint configuration.",
    render: ({ model }) => <SettingsScreen model={model} />,
  },
};
