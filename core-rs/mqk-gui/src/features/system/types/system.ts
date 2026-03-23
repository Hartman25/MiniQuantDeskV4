// core-rs/mqk-gui/src/features/system/types/system.ts
//
// Top-level system status, preflight, metadata, and the composite SystemModel.
// Also owns DEFAULT_STATUS and DEFAULT_PREFLIGHT constants.

import type { DataSourceDetail, ExplicitSurfaceTruth, HealthState, PanelSourceMap, RuntimeStatus, EnvironmentMode } from "./core";
import type { CausalityTrace, ExecutionChartModel, ExecutionOrderRow, ExecutionReplay, ExecutionSummary, ExecutionTimeline, ExecutionTrace, OmsOverview, ReconcileSummary } from "./execution";
import type { ArtifactRegistrySummary, ConfigFingerprintSummary, MarketDataQualitySummary, RuntimeLeadershipSummary, ServiceTopology, SessionStateSummary, SystemMetrics, TransportSummary } from "./infra";
import type { AuditActionRow, AlertTriageRow, FeedEvent, IncidentCase, OperatorActionDefinition, OperatorAlert, OperatorTimelineEvent, ReplaceCancelChainRow } from "./ops";
import type { FillRow, OpenOrderRow, PortfolioSummary, PositionRow, ReconcileMismatchRow, RiskDenialRow, RiskSummary } from "./portfolio";
import type { ConfigDiffRow, StrategyRow, StrategySuppressionRow } from "./strategy";

export interface SystemStatus {
  environment: EnvironmentMode;
  runtime_status: RuntimeStatus;
  broker_status: HealthState;
  db_status: HealthState;
  market_data_health: HealthState;
  reconcile_status: HealthState;
  integrity_status: HealthState;
  audit_writer_status: HealthState;
  last_heartbeat: string | null;
  loop_latency_ms: number | null;
  active_account_id: string | null;
  config_profile: string | null;
  has_warning: boolean;
  has_critical: boolean;
  strategy_armed: boolean;
  execution_armed: boolean;
  live_routing_enabled: boolean;
  kill_switch_active: boolean;
  risk_halt_active: boolean;
  integrity_halt_active: boolean;
  daemon_reachable: boolean;
  /**
   * AP-04: How the broker snapshot is sourced for this adapter.
   * "synthetic" = paper (local OMS synthesis); "external" = Alpaca REST fetch.
   * Independent of market_data_health and strategy feed policy.
   */
  broker_snapshot_source: "synthetic" | "external";
  /**
   * AP-05: Alpaca WebSocket trade-updates continuity truth.
   * "not_applicable" for Paper; for Alpaca: "cold_start_unproven" | "live" | "gap_detected".
   * Only "live" indicates proven continuity — all other states are fail-closed.
   */
  alpaca_ws_continuity: "not_applicable" | "cold_start_unproven" | "live" | "gap_detected";
  /**
   * AP-08: Whether the configured (deployment_mode, adapter) pair may be started.
   * False when the mode/adapter combination is blocked or unrecognised.
   */
  deployment_start_allowed: boolean;
  /**
   * Deployment mode label from the daemon ("paper" | "live-shadow" | "live-capital" | "backtest").
   * Distinguishes paper+alpaca from live-shadow+alpaca and live-capital+alpaca.
   */
  daemon_mode: string;
  /** Broker adapter identifier ("paper" | "alpaca"). */
  adapter_id: string;
}

export interface PreflightStatus {
  daemon_reachable: boolean;
  db_reachable: boolean;
  broker_config_present: boolean;
  market_data_config_present: boolean;
  audit_writer_ready: boolean;
  runtime_idle: boolean;
  strategy_disarmed: boolean;
  execution_disarmed: boolean;
  live_routing_disabled: boolean;
  warnings: string[];
  blockers: string[];
}

export interface MetadataSummary {
  build_version: string;
  api_version: string;
  broker_adapter: string;
  endpoint_status: HealthState;
}

export interface SystemModel {
  status: SystemStatus;
  preflight: PreflightStatus;
  alerts: OperatorAlert[];
  feed: FeedEvent[];
  executionSummary: ExecutionSummary;
  executionOrders: ExecutionOrderRow[];
  selectedTimeline: ExecutionTimeline | null;
  omsOverview: OmsOverview;
  executionTrace: ExecutionTrace | null;
  causalityTrace: CausalityTrace | null;
  executionReplay: ExecutionReplay | null;
  executionChart: ExecutionChartModel | null;
  metrics: SystemMetrics;
  portfolioSummary: PortfolioSummary;
  positions: PositionRow[];
  openOrders: OpenOrderRow[];
  fills: FillRow[];
  riskSummary: RiskSummary;
  riskDenials: RiskDenialRow[];
  reconcileSummary: ReconcileSummary;
  mismatches: ReconcileMismatchRow[];
  strategies: StrategyRow[];
  auditActions: AuditActionRow[];
  metadata: MetadataSummary;
  topology: ServiceTopology;
  transport: TransportSummary;
  incidents: IncidentCase[];
  replaceCancelChains: ReplaceCancelChainRow[];
  alertTriage: AlertTriageRow[];
  sessionState: SessionStateSummary;
  configFingerprint: ConfigFingerprintSummary;
  marketDataQuality: MarketDataQualitySummary;
  runtimeLeadership: RuntimeLeadershipSummary;
  artifactRegistry: ArtifactRegistrySummary;
  strategySummaryTruth: ExplicitSurfaceTruth;
  strategySuppressionsTruth: ExplicitSurfaceTruth;
  configDiffsTruth: ExplicitSurfaceTruth;
  strategySuppressions: StrategySuppressionRow[];
  configDiffs: ConfigDiffRow[];
  operatorTimeline: OperatorTimelineEvent[];
  actionCatalog: OperatorActionDefinition[];
  dataSource: DataSourceDetail;
  panelSources: PanelSourceMap;
  connected: boolean;
  lastUpdatedAt: string | null;
}

export const DEFAULT_STATUS: SystemStatus = {
  environment: "paper",
  runtime_status: "idle",
  broker_status: "disconnected",
  db_status: "unknown",
  market_data_health: "unknown",
  reconcile_status: "unknown",
  integrity_status: "unknown",
  audit_writer_status: "unknown",
  last_heartbeat: null,
  loop_latency_ms: null,
  active_account_id: null,
  config_profile: null,
  has_warning: false,
  has_critical: true,
  strategy_armed: false,
  execution_armed: false,
  live_routing_enabled: false,
  kill_switch_active: false,
  risk_halt_active: false,
  integrity_halt_active: false,
  daemon_reachable: false,
  // AP-09: Fail-closed defaults for external broker truth fields.
  // When the daemon is unreachable the safest assumption is synthetic paper
  // (not external) so the continuity gate in truthRendering does not fire on
  // stale or absent status.  Actual values are populated once the canonical
  // /api/v1/system/status response is received.
  broker_snapshot_source: "synthetic",
  alpaca_ws_continuity: "not_applicable",
  deployment_start_allowed: false,
  daemon_mode: "paper",
  adapter_id: "paper",
};

export const DEFAULT_PREFLIGHT: PreflightStatus = {
  daemon_reachable: false,
  db_reachable: false,
  broker_config_present: false,
  market_data_config_present: false,
  audit_writer_ready: false,
  runtime_idle: true,
  strategy_disarmed: true,
  execution_disarmed: true,
  live_routing_disabled: true,
  warnings: [],
  blockers: ["Daemon unreachable"],
};
