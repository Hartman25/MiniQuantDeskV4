export type EnvironmentMode = "paper" | "live" | "backtest";
export type RuntimeStatus = "idle" | "starting" | "running" | "paused" | "degraded" | "halted";
export type HealthState = "ok" | "warning" | "critical" | "disconnected" | "unknown";
export type Severity = "info" | "warning" | "critical";
export type ActionLevel = 0 | 1 | 2 | 3;
export type OmsState = "open" | "partially_filled" | "filled" | "cancelled" | "rejected";
export type OperatorTimelineCategory = "alert" | "operator_action" | "mode_transition" | "runtime_restart" | "config_change" | "incident" | "reconcile";

export type DataSourceState = "real" | "partial" | "mock" | "disconnected";

export type SourceAuthority = "db_truth" | "runtime_memory" | "broker_snapshot" | "placeholder" | "mixed" | "unknown";

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

export interface ExecutionSummary {
  active_orders: number;
  pending_orders: number;
  dispatching_orders: number;
  reject_count_today: number;
  cancel_replace_count_today: number;
  avg_ack_latency_ms: number | null;
  stuck_orders: number;
}

export interface ExecutionOrderRow {
  internal_order_id: string;
  broker_order_id: string | null;
  symbol: string;
  strategy_id: string;
  side: "buy" | "sell";
  order_type: "market" | "limit" | "stop" | "stop_limit";
  requested_qty: number;
  filled_qty: number;
  current_status: string;
  current_stage: string;
  age_ms: number;
  has_warning: boolean;
  has_critical: boolean;
  updated_at: string;
}

export type TimelineStageStatus = "not_reached" | "active" | "completed" | "failed" | "late" | "missing" | "skipped";

export interface TimelineStage {
  stage_key: string;
  stage_label: string;
  sequence: number;
  status: TimelineStageStatus;
  started_at: string | null;
  completed_at: string | null;
  duration_ms: number | null;
  age_ms: number | null;
  expected: boolean;
  present: boolean;
  source_system: string;
  source_ref: string | null;
  details: string;
  severity: Severity;
  is_current: boolean;
  is_terminal_stage: boolean;
}

export interface TimelineIncident {
  incident_id: string;
  incident_type:
    | "delayed_ack"
    | "stuck_in_state"
    | "duplicate_broker_event"
    | "late_fill_after_restart"
    | "replace_attempt"
    | "cancel_attempt"
    | "cancel_reject"
    | "reconcile_mismatch"
    | "missing_expected_stage"
    | "runtime_restart_detected"
    | "broker_disconnect_during_lifecycle"
    | "risk_denial"
    | "integrity_halt"
    | "unknown_transition";
  severity: Severity;
  message: string;
  at: string;
}

export interface TimelineEventRow {
  event_id: string;
  timestamp: string;
  event_type: string;
  source_system: string;
  severity: Severity;
  order_id: string;
  broker_order_id: string | null;
  stage_key: string | null;
  message: string;
  payload_json: string;
  is_duplicate: boolean;
  is_replayed: boolean;
  is_operator_visible: boolean;
}

export interface ReconcileSummary {
  status: HealthState;
  last_run_at: string | null;
  mismatched_positions: number;
  mismatched_orders: number;
  mismatched_fills: number;
  unmatched_broker_events: number;
}

export interface ExecutionTimeline {
  timeline_id: string;
  environment: EnvironmentMode;
  account_id: string | null;
  symbol: string;
  strategy_id: string;
  internal_order_id: string;
  broker_order_id: string | null;
  parent_intent_id: string | null;
  side: "buy" | "sell";
  order_type: string;
  requested_qty: number;
  filled_qty: number;
  current_status: string;
  current_stage: string;
  opened_at: string;
  last_updated_at: string;
  is_terminal: boolean;
  has_warning: boolean;
  has_critical: boolean;
  stages: TimelineStage[];
  incident_events: TimelineIncident[];
  event_rows: TimelineEventRow[];
  reconcile_summary: ReconcileSummary;
}

export interface OmsStateNode {
  state: OmsState;
  active_count: number;
  warning_count: number;
  over_sla_count: number;
  avg_dwell_ms: number | null;
  p95_dwell_ms: number | null;
}

export interface OmsTransitionEdge {
  from_state: OmsState;
  to_state: OmsState;
  transition_count: number;
  median_latency_ms: number | null;
  anomaly_count: number;
}

export interface OmsOrderStateRow {
  internal_order_id: string;
  broker_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  requested_qty: number;
  filled_qty: number;
  oms_state: OmsState;
  execution_stage: string;
  entered_state_at: string;
  dwell_ms: number;
  sla_ms: number;
  is_stuck: boolean;
  severity: Severity;
}

export interface OmsOverview {
  total_active_orders: number;
  stuck_orders: number;
  missing_transition_orders: number;
  state_nodes: OmsStateNode[];
  transition_edges: OmsTransitionEdge[];
  orders: OmsOrderStateRow[];
}

export interface TraceCorrelationIds {
  internal_order_id: string;
  broker_order_id: string | null;
  parent_order_id: string | null;
  outbox_id: string | null;
  claim_token: string | null;
  dispatch_attempt_id: string | null;
  inbox_ids: string[];
  fill_ids: string[];
  reconcile_case_id: string | null;
  audit_chain_id: string | null;
}

export interface TraceEventRow {
  trace_event_id: string;
  timestamp: string;
  subsystem: "strategy" | "runtime" | "risk" | "db" | "execution" | "broker" | "reconcile" | "audit";
  event_type: string;
  before_state: string;
  after_state: string;
  latency_since_prev_ms: number | null;
  summary: string;
  payload_digest: string;
  anomaly_tags: string[];
}

export interface TraceBrokerMessageRow {
  message_id: string;
  timestamp: string;
  direction: "outbound" | "inbound";
  message_type: string;
  normalized_summary: string;
  raw_payload: string;
}

export interface TraceFillRow {
  fill_id: string;
  timestamp: string;
  qty: number;
  price: number;
  liquidity_flag: string | null;
  fee_estimate: number | null;
  fee_actual: number | null;
  cumulative_filled_qty: number;
  average_fill_price: number;
  slippage_bps: number | null;
}

export interface StateLadderRow {
  key: string;
  oms_state: string;
  execution_state: string;
  broker_state: string;
  reconcile_state: string;
  at: string;
}


export interface CausalityNode {
  node_key: string;
  node_type: "signal" | "intent" | "risk_gate" | "outbox" | "broker_event" | "oms_transition" | "portfolio_effect" | "reconcile_outcome";
  title: string;
  status: "ok" | "warning" | "critical" | "derived";
  subsystem: "strategy" | "runtime" | "risk" | "execution" | "broker" | "portfolio" | "reconcile" | "audit";
  linked_id: string | null;
  timestamp: string | null;
  elapsed_from_prev_ms: number | null;
  anomaly_tags: string[];
  summary: string;
}

export interface CausalityTimelineRow {
  row_id: string;
  timestamp: string;
  subsystem: "strategy" | "runtime" | "risk" | "execution" | "broker" | "oms" | "portfolio" | "reconcile" | "audit";
  event_type: string;
  correlation_id: string;
  before_state: string;
  after_state: string;
  latency_since_prev_ms: number | null;
  summary: string;
  anomaly_tags: string[];
}

export interface PortfolioEffectSummary {
  pre_position_qty: number;
  post_position_qty: number;
  net_position_delta: number;
  average_price_effect: number | null;
  realized_pnl_delta: number | null;
  unrealized_pnl_delta: number | null;
  cash_delta: number | null;
  buying_power_delta: number | null;
  exposure_delta: number | null;
  strategy_allocation_effect: string;
}

export interface ReconcileOutcomeSummary {
  reconcile_status: "matched_cleanly" | "corrected" | "unresolved" | "missing_fill_detected" | "unknown_broker_order_detected";
  drift_detected: boolean;
  drift_type: string | null;
  correction_applied: boolean;
  correction_timestamp: string | null;
  corrected_fields: string[];
  remaining_discrepancy: string | null;
  operator_escalation_status: string;
}

export interface CausalityTrace {
  incident_id: string;
  internal_order_id: string;
  broker_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  target_qty: number;
  filled_qty: number;
  current_oms_state: string;
  current_execution_state: string;
  current_reconcile_status: string;
  terminal_outcome: string;
  replay_available: boolean;
  audit_chain_id: string | null;
  severity: Severity;
  correlation: {
    signal_id: string | null;
    decision_id: string | null;
    intent_id: string | null;
    internal_order_id: string;
    broker_order_id: string | null;
    outbox_id: string | null;
    claim_token: string | null;
    dispatch_attempt_id: string | null;
    inbox_ids: string[];
    fill_ids: string[];
    portfolio_event_ids: string[];
    reconcile_case_id: string | null;
    audit_chain_id: string | null;
    run_id: string | null;
  };
  nodes: CausalityNode[];
  timeline: CausalityTimelineRow[];
  portfolio_effects: PortfolioEffectSummary;
  reconcile_outcome: ReconcileOutcomeSummary;
  anomalies: string[];
}

export interface ExecutionTrace {
  internal_order_id: string;
  broker_order_id: string | null;
  parent_order_id: string | null;
  strategy_id: string;
  symbol: string;
  side: "buy" | "sell";
  qty: number;
  current_execution_state: string;
  current_oms_state: string;
  submit_time: string;
  terminal_time: string | null;
  replay_available: boolean;
  correlation: TraceCorrelationIds;
  timeline: TraceEventRow[];
  broker_messages: TraceBrokerMessageRow[];
  state_ladder: StateLadderRow[];
  fills: TraceFillRow[];
}

export interface ReplayFrame {
  frame_id: string;
  timestamp: string;
  subsystem: string;
  event_type: string;
  state_delta: string;
  message_digest: string;
  order_execution_state: string;
  oms_state: string;
  filled_qty: number;
  open_qty: number;
  risk_state: string;
  reconcile_state: string;
  queue_status: string;
  anomaly_tags: string[];
  boundary_tags: string[];
}

export interface ExecutionReplay {
  replay_id: string;
  replay_scope: "single_order" | "incident" | "time_window";
  source: "audit_log" | "execution_events" | "reconcile_case" | "full_correlated";
  title: string;
  current_frame_index: number;
  frames: ReplayFrame[];
}


export interface ExecutionChartBar {
  ts: string;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

export type ExecutionOverlayKind =
  | "signal"
  | "intent"
  | "risk_pass"
  | "order_sent"
  | "broker_ack"
  | "partial_fill"
  | "fill"
  | "replace"
  | "cancel"
  | "reconcile"
  | "portfolio"
  | "expected_price";

export interface ExecutionOverlayEvent {
  overlay_id: string;
  kind: ExecutionOverlayKind;
  ts: string;
  price: number;
  label: string;
  detail: string;
  severity: Severity;
  linked_frame_id: string | null;
}

export interface ExecutionChartModel {
  order_id: string;
  symbol: string;
  timeframe: "1m" | "5m" | "15m" | "1h";
  bars: ExecutionChartBar[];
  overlays: ExecutionOverlayEvent[];
  reference_price: number | null;
}

export interface MetricPoint {
  ts: string;
  value: number;
}

export interface MetricSeries {
  key: string;
  label: string;
  unit: "count" | "ms" | "pct" | "rate" | "usd";
  window: "5m" | "15m" | "1h" | "4h" | "1d";
  points: MetricPoint[];
  current_value: number;
  threshold_warning: number | null;
  threshold_critical: number | null;
}

export interface MetricsSection {
  key: string;
  title: string;
  description: string;
  series: MetricSeries[];
}

export interface SystemMetrics {
  runtime: MetricsSection;
  execution: MetricsSection;
  fillQuality: MetricsSection;
  reconciliation: MetricsSection;
  riskSafety: MetricsSection;
}

export interface PositionRow {
  symbol: string;
  strategy_id: string;
  qty: number;
  avg_price: number;
  mark_price: number;
  unrealized_pnl: number;
  realized_pnl_today: number;
  broker_qty: number;
  drift: boolean;
}

export interface OpenOrderRow {
  internal_order_id: string;
  symbol: string;
  strategy_id: string;
  side: string;
  status: string;
  broker_order_id: string | null;
  requested_qty: number;
  filled_qty: number;
  entered_at: string;
}

export interface FillRow {
  fill_id: string;
  internal_order_id: string;
  symbol: string;
  strategy_id: string;
  side: string;
  qty: number;
  price: number;
  broker_exec_id: string;
  applied: boolean;
  at: string;
}

export interface PortfolioSummary {
  account_equity: number;
  cash: number;
  long_market_value: number;
  short_market_value: number;
  daily_pnl: number;
  buying_power: number;
}

export interface RiskSummary {
  gross_exposure: number;
  net_exposure: number;
  concentration_pct: number;
  daily_pnl: number;
  drawdown_pct: number;
  loss_limit_utilization_pct: number;
  kill_switch_active: boolean;
  active_breaches: number;
}

export interface RiskDenialRow {
  id: string;
  at: string;
  strategy_id: string;
  symbol: string;
  rule: string;
  message: string;
  severity: Severity;
}

export interface ReconcileMismatchRow {
  id: string;
  domain: "position" | "order" | "fill" | "cash" | "event";
  symbol: string;
  internal_value: string;
  broker_value: string;
  status: Severity;
  note: string;
}

export interface StrategyRow {
  strategy_id: string;
  enabled: boolean;
  armed: boolean;
  health: HealthState;
  universe: string;
  pending_intents: number;
  open_positions: number;
  today_pnl: number;
  drawdown_pct: number;
  regime: string;
  throttle_state: string;
  last_decision_time: string | null;
}

export interface AuditActionRow {
  audit_ref: string;
  at: string;
  actor: string;
  action_key: string;
  environment: EnvironmentMode;
  target_scope: string;
  result_state: string;
  warnings: string[];
}

export interface ServiceDependencyNode {
  service_key: string;
  label: string;
  layer: "runtime" | "execution" | "data" | "broker" | "reconcile" | "audit" | "strategy" | "risk";
  health: HealthState;
  role: string;
  dependency_keys: string[];
  failure_impact: string;
  last_heartbeat: string | null;
  latency_ms: number | null;
  notes: string;
}

export interface ServiceTopology {
  updated_at: string;
  services: ServiceDependencyNode[];
}

export interface TransportQueueRow {
  queue_id: string;
  direction: "outbox" | "inbox";
  status: string;
  depth: number;
  oldest_age_ms: number;
  retry_count: number;
  duplicate_events: number;
  orphaned_claims: number;
  lag_ms: number | null;
  last_activity_at: string | null;
  notes: string;
}

export interface TransportSummary {
  outbox_depth: number;
  inbox_depth: number;
  max_claim_age_ms: number;
  dispatch_retries: number;
  orphaned_claims: number;
  duplicate_inbox_events: number;
  queues: TransportQueueRow[];
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

export interface SessionStateSummary {
  market_session: "premarket" | "regular" | "after_hours" | "closed";
  exchange_calendar_state: "open" | "halted" | "closed" | "holiday";
  system_trading_window: "enabled" | "disabled" | "exit_only";
  strategy_allowed: boolean;
  next_session_change_at: string | null;
  notes: string[];
}

export interface ConfigFingerprintSummary {
  config_hash: string;
  risk_policy_version: string;
  strategy_bundle_version: string;
  build_version: string;
  environment_profile: string;
  runtime_generation_id: string;
  last_restart_at: string | null;
}

export interface MarketDataIssueRow {
  issue_id: string;
  severity: Severity;
  scope: "symbol" | "venue" | "pipeline";
  symbol: string | null;
  venue: string | null;
  issue_type: string;
  freshness_lag_ms: number | null;
  affected_strategies: string[];
  status: string;
  note: string;
  detected_at: string;
}

export interface MarketDataVenueRow {
  venue_key: string;
  label: string;
  health: HealthState;
  freshness_lag_ms: number | null;
  symbols_degraded: number;
  missing_updates: number;
  disagreement_count: number;
  last_good_at: string | null;
  note: string;
}

export interface MarketDataQualitySummary {
  overall_health: HealthState;
  freshness_sla_ms: number;
  stale_symbol_count: number;
  missing_bar_count: number;
  venue_disagreement_count: number;
  strategy_blocks: number;
  venues: MarketDataVenueRow[];
  issues: MarketDataIssueRow[];
}

export interface RuntimeCheckpointRow {
  checkpoint_id: string;
  checkpoint_type: "restart" | "leader_acquired" | "leader_lost" | "recovery_complete" | "snapshot_refresh";
  timestamp: string;
  generation_id: string;
  leader_node: string;
  status: "ok" | "warning" | "critical";
  note: string;
}

export interface RuntimeLeadershipSummary {
  leader_node: string;
  leader_lease_state: "held" | "contested" | "lost";
  generation_id: string;
  restart_count_24h: number;
  last_restart_at: string | null;
  post_restart_recovery_state: "complete" | "in_progress" | "degraded";
  recovery_checkpoint: string;
  checkpoints: RuntimeCheckpointRow[];
}

export interface ArtifactRow {
  artifact_id: string;
  artifact_type: "run_bundle" | "trace_export" | "replay_export" | "reconcile_report" | "operator_receipt" | "incident_bundle";
  created_at: string;
  status: "ready" | "pending" | "failed";
  linked_order_id: string | null;
  linked_incident_id: string | null;
  linked_run_id: string | null;
  storage_path: string;
  note: string;
}

export interface ArtifactRegistrySummary {
  last_updated_at: string | null;
  ready_count: number;
  pending_count: number;
  failed_count: number;
  artifacts: ArtifactRow[];
}

export interface StrategySuppressionRow {
  suppression_id: string;
  strategy_id: string;
  state: "active" | "cleared";
  trigger_domain: "risk" | "market_data" | "runtime" | "reconcile" | "operator";
  trigger_reason: string;
  started_at: string;
  cleared_at: string | null;
  note: string;
}

export interface ConfigDiffRow {
  diff_id: string;
  changed_at: string;
  changed_domain: "config" | "risk" | "strategy_bundle" | "runtime";
  before_version: string;
  after_version: string;
  summary: string;
}


export interface OperatorTimelineEvent {
  timeline_event_id: string;
  at: string;
  category: OperatorTimelineCategory;
  severity: Severity;
  title: string;
  summary: string;
  actor: string;
  linked_incident_id: string | null;
  linked_order_id: string | null;
  linked_strategy_id: string | null;
  linked_action_key: string | null;
  linked_config_diff_id: string | null;
  linked_runtime_generation_id: string | null;
}

export interface MetadataSummary {
  build_version: string;
  api_version: string;
  broker_adapter: string;
  endpoint_status: HealthState;
}

export interface OperatorActionDefinition {
  action_key:
    | "start-system"
    | "stop-system"
    | "arm-strategy"
    | "disarm-strategy"
    | "arm-execution"
    | "disarm-execution"
    | "enable-live-routing"
    | "disable-live-routing"
    | "pause-new-entries"
    | "resume-new-entries"
    | "reconcile-now"
    | "refresh-broker-snapshot"
    | "flatten-all"
    | "cancel-all-open-orders"
    | "kill-switch"
    | "resume-after-halt"
    | "ack-alert"
    | "change-system-mode";
  label: string;
  level: ActionLevel;
  description: string;
  requiresReason: boolean;
  confirmText: string;
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
