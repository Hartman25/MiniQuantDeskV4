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
