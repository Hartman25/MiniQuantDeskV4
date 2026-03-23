// core-rs/mqk-gui/src/features/system/types/infra.ts
//
// Infrastructure types: service topology, transport, market data quality,
// runtime leadership, artifact registry, session state, config fingerprint,
// and metrics.

import type { HealthState, Severity } from "./core";

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

export interface SessionStateSummary {
  market_session: "premarket" | "regular" | "after_hours" | "closed";
  exchange_calendar_state: "open" | "halted" | "closed" | "holiday";
  system_trading_window: "enabled" | "disabled" | "exit_only";
  strategy_allowed: boolean;
  next_session_change_at: string | null;
  /** Stable identifier for the calendar spec driving this response.
   *  "always_on" (paper/backtest) or "nyse_weekdays" (live/shadow). */
  calendar_spec_id?: string;
  notes: string[];
  /**
   * AP-09: Deployment mode label from daemon session truth.
   * "PAPER" | "LIVE-SHADOW" | "LIVE-CAPITAL" | "BACKTEST".
   * Distinguishes paper+alpaca from live-shadow+alpaca and live-capital+alpaca
   * without requiring a separate API call.
   */
  daemon_mode?: string;
  /** AP-09: Broker adapter identifier ("paper" | "alpaca"). */
  adapter_id?: string;
  /** AP-09: Whether the configured (mode, adapter) pair may be started. */
  deployment_start_allowed?: boolean;
  /** AP-09: Blocker explanation when deployment_start_allowed is false. */
  deployment_blocker?: string | null;
  /**
   * AP-09: Operator auth mode from daemon.
   * "token_required" | "explicit_dev_no_token" | "missing_token_fail_closed".
   * Explicit dev-no-token mode is visible here so capital mode restrictions
   * are honest on operator surfaces.
   */
  operator_auth_mode?: string;
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
  /** DB-backed count of run starts in last 24 h; null when daemon has no DB pool. */
  restart_count_24h: number | null;
  last_restart_at: string | null;
  post_restart_recovery_state: "complete" | "in_progress" | "degraded";
  recovery_checkpoint: string;
  checkpoints: RuntimeCheckpointRow[];
}

export interface ArtifactRow {
  artifact_id: string;
  // "run_config" is the artifact_type emitted by the daemon's audit/artifacts
  // handler (one entry per run from the runs table).  Other types are planned
  // for future artifact sources and are retained for forward compatibility.
  artifact_type: "run_bundle" | "trace_export" | "replay_export" | "reconcile_report" | "operator_receipt" | "incident_bundle" | "run_config";
  created_at: string;
  status?: "ready" | "pending" | "failed";
  linked_order_id: string | null;
  linked_incident_id: string | null;
  linked_run_id: string | null;
  // storage_path and note are not returned by the current daemon artifact source
  // (runs table has no file path or note column); optional until a durable
  // artifact store is wired.
  storage_path?: string;
  note?: string;
}

export interface ArtifactRegistrySummary {
  last_updated_at: string | null;
  ready_count: number;
  pending_count: number;
  failed_count: number;
  artifacts: ArtifactRow[];
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
