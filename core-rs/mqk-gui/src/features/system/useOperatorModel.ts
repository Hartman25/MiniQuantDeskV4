import { useCallback, useEffect, useMemo, useState } from "react";
import { fetchCausalityTrace, fetchExecutionChart, fetchExecutionReplay, fetchExecutionTimeline, fetchExecutionTrace, fetchOperatorModel, invokeOperatorAction, requestSystemModeTransition } from "./api";
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS, type OperatorActionDefinition, type OperatorActionReceipt, type SystemModel } from "./types";

const FALLBACK_MODEL: SystemModel = {
  status: DEFAULT_STATUS,
  preflight: DEFAULT_PREFLIGHT,
  alerts: [],
  feed: [],
  executionSummary: {
    active_orders: 0,
    pending_orders: 0,
    dispatching_orders: 0,
    reject_count_today: 0,
    cancel_replace_count_today: 0,
    avg_ack_latency_ms: null,
    stuck_orders: 0,
  },
  executionOrders: [],
  selectedTimeline: null,
  omsOverview: {
    total_active_orders: 0,
    stuck_orders: 0,
    missing_transition_orders: 0,
    state_nodes: [],
    transition_edges: [],
    orders: [],
  },
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
  portfolioSummary: {
    account_equity: 0,
    cash: 0,
    long_market_value: 0,
    short_market_value: 0,
    daily_pnl: 0,
    buying_power: 0,
  },
  positions: [],
  openOrders: [],
  fills: [],
  riskSummary: {
    gross_exposure: 0,
    net_exposure: 0,
    concentration_pct: 0,
    daily_pnl: 0,
    drawdown_pct: 0,
    loss_limit_utilization_pct: 0,
    kill_switch_active: false,
    active_breaches: 0,
  },
  riskDenials: [],
  reconcileSummary: {
    status: "unknown",
    last_run_at: null,
    mismatched_positions: 0,
    mismatched_orders: 0,
    mismatched_fills: 0,
    unmatched_broker_events: 0,
  },
  mismatches: [],
  strategies: [],
  auditActions: [],
  metadata: {
    build_version: "unknown",
    api_version: "unknown",
    broker_adapter: "unknown",
    endpoint_status: "unknown",
  },
  topology: { updated_at: null as unknown as string, services: [] },
  transport: { outbox_depth: 0, inbox_depth: 0, max_claim_age_ms: 0, dispatch_retries: 0, orphaned_claims: 0, duplicate_inbox_events: 0, queues: [] },
  incidents: [],
  replaceCancelChains: [],
  alertTriage: [],
  sessionState: { market_session: "closed", exchange_calendar_state: "closed", system_trading_window: "disabled", strategy_allowed: false, next_session_change_at: null, notes: [] },
  configFingerprint: { config_hash: "unknown", risk_policy_version: "unknown", strategy_bundle_version: "unknown", build_version: "unknown", environment_profile: "unknown", runtime_generation_id: "unknown", last_restart_at: null },
  marketDataQuality: { overall_health: "unknown", freshness_sla_ms: 0, stale_symbol_count: 0, missing_bar_count: 0, venue_disagreement_count: 0, strategy_blocks: 0, venues: [], issues: [] },
  runtimeLeadership: { leader_node: "unknown", leader_lease_state: "lost", generation_id: "unknown", restart_count_24h: 0, last_restart_at: null, post_restart_recovery_state: "degraded", recovery_checkpoint: "unknown", checkpoints: [] },
  artifactRegistry: { last_updated_at: null, ready_count: 0, pending_count: 0, failed_count: 0, artifacts: [] },
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
  connected: false,
  lastUpdatedAt: null,
};

export function useOperatorModel(pollIntervalMs = 5000) {
  const [model, setModel] = useState<SystemModel>(FALLBACK_MODEL);
  const [loading, setLoading] = useState(true);
  const [actionReceipt, setActionReceipt] = useState<OperatorActionReceipt | null>(null);
  const [timelineLoading, setTimelineLoading] = useState(false);

  const refresh = useCallback(async () => {
    const next = await fetchOperatorModel();
    setModel(next);
    setLoading(false);
  }, []);

  useEffect(() => {
    let mounted = true;
    const guardedRefresh = async () => {
      const next = await fetchOperatorModel();
      if (!mounted) return;
      setModel(next);
      setLoading(false);
    };

    void guardedRefresh();
    const timer = window.setInterval(() => {
      void guardedRefresh();
    }, pollIntervalMs);

    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, [pollIntervalMs]);

  const selectTimeline = useCallback(async (internalOrderId: string) => {
    setTimelineLoading(true);
    const [timeline, executionTrace, executionReplay, executionChart, causalityTrace] = await Promise.all([
      fetchExecutionTimeline(internalOrderId),
      fetchExecutionTrace(internalOrderId),
      fetchExecutionReplay(internalOrderId),
      fetchExecutionChart(internalOrderId),
      fetchCausalityTrace(internalOrderId),
    ]);
    setModel((current) => ({ ...current, selectedTimeline: timeline, executionTrace, executionReplay, executionChart, causalityTrace }));
    setTimelineLoading(false);
  }, []);



  const requestModeChange = useCallback(
    async (targetMode: SystemModel["status"]["environment"], reason: string) => {
      const receipt = await requestSystemModeTransition(targetMode, reason);
      setActionReceipt(receipt);
      return receipt;
    },
    [model],
  );

  const runAction = useCallback(
    async (action: OperatorActionDefinition, args: { reason?: string; target_scope?: string; alert_id?: string }) => {
      const receipt = await invokeOperatorAction(action.action_key, args);
      setActionReceipt(receipt);
      return receipt;
    },
    [model],
  );

  return useMemo(
    () => ({ model, loading, refresh, selectTimeline, timelineLoading, actionReceipt, runAction, requestModeChange }),
    [actionReceipt, loading, model, refresh, runAction, requestModeChange, selectTimeline, timelineLoading],
  );
}
