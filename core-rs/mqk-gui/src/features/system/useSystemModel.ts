import { useEffect, useMemo, useState } from "react";
import { fetchOperatorModel } from "./api";
import { classifyPanelSources } from "./sourceAuthority";
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS, type SystemModel } from "./types";

const FALLBACK_MODEL: SystemModel = {
  status: DEFAULT_STATUS,
  preflight: DEFAULT_PREFLIGHT,
  alerts: [],
  feed: [],
  executionSummary: { active_orders: 0, pending_orders: 0, dispatching_orders: 0, reject_count_today: 0, cancel_replace_count_today: 0, avg_ack_latency_ms: null, stuck_orders: 0 },
  executionOrders: [],
  selectedTimeline: null,
  omsOverview: { total_active_orders: 0, stuck_orders: 0, missing_transition_orders: 0, state_nodes: [], transition_edges: [], orders: [] },
  executionTrace: null,
  executionReplay: null,
  executionChart: null,
  causalityTrace: null,
  metrics: {
    runtime: { key: "runtime", title: "Runtime", description: "", series: [] },
    execution: { key: "execution", title: "Execution", description: "", series: [] },
    fillQuality: { key: "fill_quality", title: "Fill Quality", description: "", series: [] },
    reconciliation: { key: "reconciliation", title: "Reconciliation", description: "", series: [] },
    riskSafety: { key: "risk_safety", title: "Risk/Safety", description: "", series: [] },
  },
  portfolioSummary: { account_equity: 0, cash: 0, long_market_value: 0, short_market_value: 0, daily_pnl: 0, buying_power: 0 },
  positions: [],
  openOrders: [],
  fills: [],
  riskSummary: { gross_exposure: 0, net_exposure: 0, concentration_pct: 0, daily_pnl: 0, drawdown_pct: 0, loss_limit_utilization_pct: 0, kill_switch_active: false, active_breaches: 0 },
  riskDenials: [],
  reconcileSummary: { status: "unknown", last_run_at: null, mismatched_positions: 0, mismatched_orders: 0, mismatched_fills: 0, unmatched_broker_events: 0 },
  mismatches: [],
  strategies: [],
  auditActions: [],
  metadata: { build_version: "unknown", api_version: "unknown", broker_adapter: "unknown", endpoint_status: "unknown" },
  topology: { updated_at: new Date(0).toISOString(), services: [] },
  transport: { outbox_depth: 0, inbox_depth: 0, max_claim_age_ms: 0, dispatch_retries: 0, orphaned_claims: 0, duplicate_inbox_events: 0, queues: [] },
  incidents: [],
  replaceCancelChains: [],
  alertTriage: [],
  sessionState: { market_session: "closed", exchange_calendar_state: "closed", system_trading_window: "disabled", strategy_allowed: false, next_session_change_at: null, notes: [] },
  configFingerprint: { config_hash: "unknown", risk_policy_version: "unknown", strategy_bundle_version: "unknown", build_version: "unknown", environment_profile: "unknown", runtime_generation_id: "unknown", last_restart_at: null },
  marketDataQuality: { overall_health: "unknown", freshness_sla_ms: 0, stale_symbol_count: 0, missing_bar_count: 0, venue_disagreement_count: 0, strategy_blocks: 0, venues: [], issues: [] },
  // "in_progress" not "degraded": "degraded" triggers a system-wide degraded overlay.
  // The fallback represents missing truth (no daemon data yet), not a real degraded recovery state.
  runtimeLeadership: { leader_node: "unknown", leader_lease_state: "lost", generation_id: "unknown", restart_count_24h: null, last_restart_at: null, post_restart_recovery_state: "in_progress", recovery_checkpoint: "unknown", checkpoints: [] },
  artifactRegistry: { last_updated_at: null, ready_count: 0, pending_count: 0, failed_count: 0, artifacts: [] },
  strategySummaryTruth: { truth_state: "unknown", backend: null },
  strategySuppressionsTruth: { truth_state: "unknown", backend: null },
  configDiffsTruth: { truth_state: "unknown", backend: null },
  strategySuppressions: [],
  configDiffs: [],
  operatorTimeline: [],
  actionCatalog: [],
  executionOutbox: { truth_state: "unavailable" as const, run_id: null, rows: [] },
  fillQualityTelemetry: { truth_state: "unavailable" as const, rows: [] },
  paperJournal: { run_id: null, fills_truth_state: "unavailable" as const, fills: [], admissions_truth_state: "unavailable" as const, admissions: [] },
  autonomousPaperStatus: {
    truth_state: "unavailable" as const,
    mode: "unknown",
    live_routing_enabled: false,
    runtime_status: "unknown",
    arm_state: "unknown",
    kill_switch_active: false,
    deadman_status: "unknown",
    ws_continuity: "unknown",
    reconcile_status: "unknown",
    mismatch_count: null,
    open_order_count: null,
    position_count: null,
    current_symbol: null,
    current_position_qty: null,
    target_qty: null,
    computed_delta_qty: null,
    no_order_reason: null,
    last_strategy_decision: null,
    flatten_available: false,
    flatten_blockers: [],
    watchlist_outcome: "unknown",
    watchlist_approved: false,
    readiness_classification: "unknown",
    blockers: [],
    next_operator_action: null,
    autonomous_session_state: "unknown",
    now_utc: null,
  },
  watchlistStatus: {
    truth_state: "unavailable" as const,
    configured_path: null,
    status: "unknown",
    approved_for_autonomous_paper: false,
    approved_for_live: false,
    symbols: [],
    top_symbol: null,
    strategy_assignment_count: null,
    max_symbols_to_trade: null,
    max_concurrent_positions: null,
    failure_reasons: [],
    checked_at_utc: null,
  },
  admissionCheck: {
    state: "not_checked" as const,
    reason_unchecked: "no daemon connection established yet",
    symbol: null,
    strategy_id: null,
    allowed: null,
    reason_code: null,
    status: null,
    approved_for_autonomous_paper: null,
    approved_for_live: null,
    note: null,
    checked_at_utc: null,
  },
  multiSymbolDispatchSummary: {
    truth_state: "unavailable" as const,
    backend: "unknown",
    runtime_execution_mode: "unknown",
    configured_symbol_count: 0,
    per_symbol: [],
  },
  dryRunStrategyStatus: {
    truth_state: "unavailable" as const,
    backend: "unknown",
    configured_dry_run_strategy_ids: [],
    dry_run_strategy_diagnostics: [],
  },
  strategyDecisionDiagnostics: null,
  autonomousBarTickCount: null,
  autonomousLastSignalQty: null,
  autonomousBarContextSource: null,
  autonomousBlockers: [],
  dataSource: {
    state: "disconnected",
    reachable: false,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: [],
    message: "No daemon connection established yet",
  },
  panelSources: classifyPanelSources({
    state: "disconnected",
    reachable: false,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: [],
    message: "No daemon connection established yet",
  }, false),
  connected: false,
  lastUpdatedAt: null,
};

export function useSystemModel(pollIntervalMs = 4000) {
  const [model, setModel] = useState<SystemModel>(FALLBACK_MODEL);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let mounted = true;

    const refresh = async () => {
      const next = await fetchOperatorModel();
      if (!mounted) return;
      setModel(next);
      setLoading(false);
    };

    void refresh();
    const timer = window.setInterval(() => {
      void refresh();
    }, pollIntervalMs);

    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, [pollIntervalMs]);

  return useMemo(() => ({ model, loading }), [loading, model]);
}
