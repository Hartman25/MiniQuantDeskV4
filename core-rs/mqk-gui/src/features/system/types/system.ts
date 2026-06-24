// core-rs/mqk-gui/src/features/system/types/system.ts
//
// Top-level system status, preflight, metadata, and the composite SystemModel.
// Also owns DEFAULT_STATUS and DEFAULT_PREFLIGHT constants.

import type { DataSourceDetail, ExplicitSurfaceTruth, HealthState, PanelSourceMap, RuntimeStatus, EnvironmentMode } from "./core";
import type { ExecutionOrderRow, ExecutionOutboxSurface, ExecutionSummary, FillQualityRow, FillQualitySurface, OmsOverview, OrderCausalityResponse, OrderChartResponse, OrderReplayResponse, OrderTimelineSurface, OrderTraceResponse, ReconcileSummary } from "./execution";
import type { ArtifactRegistrySummary, ConfigFingerprintSummary, MarketDataQualitySummary, RuntimeLeadershipSummary, ServiceTopology, SessionStateSummary, SystemMetrics, TransportSummary } from "./infra";
import type { AuditActionRow, AlertTriageRow, FeedEvent, IncidentCase, OperatorActionDefinition, OperatorAlert, OperatorTimelineEvent, PaperJournalAdmissionRow, PaperJournalTruthState, ReplaceCancelChainRow } from "./ops";
import type { FillRow, OpenOrderRow, PortfolioSummary, PositionRow, ReconcileMismatchRow, RiskDenialRow, RiskSummary } from "./portfolio";
import type { ConfigDiffRow, DryRunStrategyStatusSurface, MultiSymbolDispatchSummarySurface, StrategyDecisionDiagnostics, StrategyRow, StrategySuppressionRow } from "./strategy";

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
  /**
   * PT-AUTO-03: Count of signals admitted (Gate 7 Ok(true)) this run.
   * Null when ExternalSignalIngestion is not configured (not paper+alpaca).
   */
  autonomous_signal_count: number | null;
  /**
   * PT-AUTO-03: Whether the day signal intake limit has been hit.
   * Null when not applicable. True means Gate 1d is blocking further signals.
   */
  autonomous_signal_limit_hit: boolean | null;
  /**
   * B8: Canonical asset-class scope for this execution path.
   * Always "equity_only". Options, futures, crypto, and FX are not wired.
   * Strategy signals carrying a non-equity asset_class are rejected at Gate 0.
   */
  asset_class_scope: string;
  /**
   * C1: Parity evidence state derived from parity_evidence.json evaluation.
   * "not_configured" | "absent" | "invalid" | "incomplete" | "complete" | "unavailable".
   * "incomplete" means evidence is present but live_trust_complete=false (all current builds).
   * "complete" is unreachable in current builds — explicit ceiling.
   * Null is never a positive trust claim.
   */
  parity_evidence_state: string;
  /**
   * C1: Whether live-trust is complete enough for live-capital execution.
   * Non-null only when parity_evidence_state is "incomplete" or "complete".
   * Always false in current builds. Null elsewhere is not a positive trust claim.
   */
  live_trust_complete: boolean | null;
  /**
   * Deadman heartbeat status from the daemon's self-watchdog loop.
   * "ok" — heartbeat is current; "expired" — timer fired (daemon may have been stuck or restarted);
   * "unknown" — state not yet determined.
   * "expired" does not necessarily mean the system is unsafe — it indicates the watchdog
   * timer lapsed and the operator should verify continuity before resuming.
   */
  deadman_status: string;
  /**
   * UTC timestamp of the last recorded deadman heartbeat, or null if never set / not applicable.
   */
  deadman_last_heartbeat_utc: string | null;
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
  // AUTON-TRUTH-02: Autonomous-paper readiness fields.
  // Populated only for Paper+Alpaca. Absent/false for all other deployments.
  /** True only for Paper+Alpaca; controls whether the autonomous checks below apply. */
  autonomous_readiness_applicable?: boolean;
  /** WS continuity proven: true only when alpaca_ws_continuity == "live". Null when not paper+alpaca. */
  ws_continuity_ready?: boolean | null;
  /** Reconcile not dirty/stale. Null when not paper+alpaca. */
  reconcile_ready?: boolean | null;
  /**
   * Autonomous arm state: "armed" | "arm_pending" | "halted" | "not_applicable".
   * "arm_pending" = disarmed in memory but not halted; controller will auto-arm from DB.
   */
  autonomous_arm_state?: string;
  /** Exact autonomous-paper blockers in gate order. Empty when not applicable or all checks pass. */
  autonomous_blockers?: string[];
  /** Whether the current wall-clock is inside the autonomous session window. Null when not paper+alpaca. */
  session_in_window?: boolean | null;
  // OBS-SESSION-DISCORD-01: Session-window diagnostics merged from /autonomous/readiness.
  // Populated only when truth_state === "active" (paper+alpaca). Absent otherwise.
  /** RFC 3339 UTC timestamp of when the readiness response was generated. */
  now_utc?: string | null;
  /** Effective session start time as "HH:MM UTC" when a fixed env-window is active; null for NYSE seam. */
  session_start_utc?: string | null;
  /** Effective session stop time as "HH:MM UTC" when a fixed env-window is active; null for NYSE seam. */
  session_stop_utc?: string | null;
  /** "env" when session window is from env vars; "default" when absent/unparseable (NYSE seam). */
  session_window_source?: string | null;
  /** "in_window" | "outside_window". Null when not applicable. */
  session_window_state?: string | null;
}

// ---------------------------------------------------------------------------
// ASSET-CAPABILITY-MATRIX-01 / ASSET-CAPABILITY-MATRIX-GUI-VISIBILITY-01:
// static, read-only asset-class capability matrix carried on
// /api/v1/system/metadata. Never used for order routing — operator
// visibility only.
// ---------------------------------------------------------------------------

export interface AssetCapabilityEntry {
  asset_class: string;
  enabled: boolean;
  paper_ready: boolean;
  live_ready: boolean;
  broker_adapter: string;
  notes: string;
}

export interface AssetCapabilityMatrix {
  schema_version: string;
  static_source: string;
  live_capital_ready: boolean;
  default_enabled_asset_classes: string[];
  disabled_asset_classes: string[];
  entries: AssetCapabilityEntry[];
}

export interface MetadataSummary {
  build_version: string;
  api_version: string;
  broker_adapter: string;
  endpoint_status: HealthState;
  /**
   * ASSET-CAPABILITY-MATRIX-01: present on every live `/api/v1/system/metadata`
   * response. Optional here (not on `unavailableMetadata`'s fallback in api.ts)
   * so an unreachable daemon reports the matrix as genuinely absent rather
   * than fabricating one.
   */
  asset_capability_matrix?: AssetCapabilityMatrix;
}

// ---------------------------------------------------------------------------
// ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED: status of the separate v2
// instrument registry source configured via MQK_INSTRUMENT_REGISTRY_V2_PATH.
// Operator visibility only; never proof of live/paper trading readiness.
// ---------------------------------------------------------------------------

export interface InstrumentRegistryV2EnabledCounts {
  enabled: number;
  paper_trading_enabled: number;
  live_trading_enabled: number;
}

export type InstrumentRegistryV2SourceTruthState =
  | "not_configured"
  | "configured_valid"
  | "registry_unavailable"
  | "validation_failed";

export interface InstrumentRegistryV2SourceStatusResponse {
  truth_state: InstrumentRegistryV2SourceTruthState | string;
  configured: boolean;
  path: string | null;
  source: string;
  schema_version: number | null;
  purpose: string;
  used_for_trading: boolean;
  enabled_for_live_trading: boolean;
  enabled_for_paper_trading: boolean;
  total_instruments: number;
  asset_class_counts: Record<string, number>;
  enabled_counts: InstrumentRegistryV2EnabledCounts;
  non_equity_present: boolean;
  non_equity_all_disabled: boolean;
  has_economics_metadata: boolean;
  sample_symbols: string[];
  validation_errors: string[];
  message: string;
}

// ---------------------------------------------------------------------------
// GUI-OPS-01: Paper journal surface — cross-module composite type.
// FillQualityRow (execution.ts) + PaperJournalAdmissionRow (ops.ts)
// are combined here because system.ts is the only module that imports both.
// ---------------------------------------------------------------------------

export interface PaperJournalSurface {
  run_id: string | null;
  fills_truth_state: PaperJournalTruthState;
  fills: FillQualityRow[];
  admissions_truth_state: PaperJournalTruthState;
  admissions: PaperJournalAdmissionRow[];
}

// ---------------------------------------------------------------------------
// GUI-PAPER-STATUS-VISIBILITY-01: Read-only surfaces for the autonomous paper
// status, watchlist status, and watchlist admission-check truth surfaces.
// These are visibility-only — no action invocation paths are derived here.
// ---------------------------------------------------------------------------

export type AutonomousPaperStatusTruthState = "active" | "not_applicable" | "unavailable";

export interface AutonomousPaperStatusSurface {
  truth_state: AutonomousPaperStatusTruthState;
  mode: string;
  live_routing_enabled: boolean;
  runtime_status: string;
  arm_state: string;
  kill_switch_active: boolean;
  deadman_status: string;
  ws_continuity: string;
  reconcile_status: string;
  mismatch_count: number | null;
  open_order_count: number | null;
  position_count: number | null;
  current_symbol: string | null;
  current_position_qty: number | null;
  target_qty: number | null;
  computed_delta_qty: number | null;
  no_order_reason: string | null;
  last_strategy_decision: string | null;
  flatten_available: boolean;
  flatten_blockers: string[];
  watchlist_outcome: string;
  watchlist_approved: boolean;
  readiness_classification: string;
  blockers: string[];
  next_operator_action: string | null;
  autonomous_session_state: string;
  now_utc: string | null;
}

export type WatchlistStatusTruthState = "active" | "unavailable";

export interface WatchlistStatusSurface {
  truth_state: WatchlistStatusTruthState;
  configured_path: string | null;
  status: string;
  approved_for_autonomous_paper: boolean;
  approved_for_live: boolean;
  symbols: string[];
  top_symbol: string | null;
  strategy_assignment_count: number | null;
  max_symbols_to_trade: number | null;
  max_concurrent_positions: number | null;
  failure_reasons: string[];
  checked_at_utc: string | null;
}

/**
 * "not_checked" — no safe symbol/strategy pair available; admission-check was not invoked.
 * "checked" — backend returned an admission-check result for a real symbol/strategy pair.
 * "unavailable" — backend was reachable but returned an unusable/malformed response.
 */
export type AdmissionCheckState = "not_checked" | "checked" | "unavailable";

export interface AdmissionCheckSurface {
  state: AdmissionCheckState;
  reason_unchecked: string | null;
  symbol: string | null;
  strategy_id: string | null;
  allowed: boolean | null;
  reason_code: string | null;
  status: string | null;
  approved_for_autonomous_paper: boolean | null;
  approved_for_live: boolean | null;
  note: string | null;
  checked_at_utc: string | null;
}

export interface SystemModel {
  status: SystemStatus;
  preflight: PreflightStatus;
  alerts: OperatorAlert[];
  feed: FeedEvent[];
  executionSummary: ExecutionSummary;
  executionOrders: ExecutionOrderRow[];
  selectedTimeline: OrderTimelineSurface | null;
  omsOverview: OmsOverview;
  executionTrace: OrderTraceResponse | null;
  causalityTrace: OrderCausalityResponse | null;
  executionReplay: OrderReplayResponse | null;
  executionChart: OrderChartResponse | null;
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
  /** GUI-OPS-02: Durable execution outbox — intent timeline for the active run. */
  executionOutbox: ExecutionOutboxSurface;
  /** GUI-OPS-02: Fill quality telemetry for the active run. */
  fillQualityTelemetry: FillQualitySurface;
  /** GUI-OPS-01: Paper journal — fills and signal admissions for the active run. */
  paperJournal: PaperJournalSurface;
  /** GUI-PAPER-STATUS-VISIBILITY-01: Autonomous paper status truth surface (read-only). */
  autonomousPaperStatus: AutonomousPaperStatusSurface;
  /** GUI-PAPER-STATUS-VISIBILITY-01: Watchlist status truth surface (read-only). */
  watchlistStatus: WatchlistStatusSurface;
  /** GUI-PAPER-STATUS-VISIBILITY-01: Watchlist admission-check dry-run surface (read-only, advisory only). */
  admissionCheck: AdmissionCheckSurface;
  /** MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01: Multi-symbol dispatch summary surface (read-only). */
  multiSymbolDispatchSummary: MultiSymbolDispatchSummarySurface;
  /**
   * MULTI-STRATEGY-DRY-RUN-GUI-01: Multi-strategy dry-run diagnostics surface
   * (read-only). Dry-run strategies never submit orders — `submitted` is
   * always `false` on every row. Source: GET /api/v1/strategy/dry-run/status.
   */
  dryRunStrategyStatus: DryRunStrategyStatusSurface;
  /**
   * STRATEGY-DECISION-OBSERVABILITY-01: Read-only decision diagnostics from the most
   * recent native strategy bar dispatch. Null when not paper+alpaca or no bar dispatched.
   * Source: GET /api/v1/autonomous/readiness → strategy_decision_diagnostics.
   */
  strategyDecisionDiagnostics: StrategyDecisionDiagnostics | null;
  /** Bar tick dispatch count this session. Null when not paper+alpaca. */
  autonomousBarTickCount: number | null;
  /** Signal qty from the last bar dispatch. Null when no bar dispatched. */
  autonomousLastSignalQty: number | null;
  /** Bar context source: "db_loaded" | "stub_no_price" | "no_dispatch_yet" | "not_applicable". */
  autonomousBarContextSource: string | null;
  /** Autonomous readiness blockers. Empty when not applicable or all gates pass. */
  autonomousBlockers: string[];
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
  // PT-AUTO-03: null = not applicable (not paper+alpaca); populated from daemon response.
  autonomous_signal_count: null,
  autonomous_signal_limit_hit: null,
  // B8: fail-closed default; daemon always returns "equity_only".
  asset_class_scope: "equity_only",
  // C1: fail-closed defaults. "not_configured" when daemon is unreachable; null trust.
  parity_evidence_state: "not_configured",
  live_trust_complete: null,
  // DEADMAN-GUI-01: fail-closed default. "unknown" when daemon is unreachable — do not
  // render "ok" before the daemon has confirmed the watchdog state.
  deadman_status: "unknown",
  deadman_last_heartbeat_utc: null,
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
  // AUTON-TRUTH-02: autonomous fields absent by default (daemon unreachable).
  autonomous_readiness_applicable: false,
  ws_continuity_ready: null,
  reconcile_ready: null,
  autonomous_arm_state: "not_applicable",
  autonomous_blockers: [],
  session_in_window: null,
};
