import { getDaemonUrl } from "../../config";
import { MOCK_MODEL } from "./mockData";
import type {
  AuditActionRow,
  CausalityTrace,
  ExecutionChartModel,
  ExecutionOrderRow,
  ExecutionReplay,
  ExecutionSummary,
  ExecutionTimeline,
  ExecutionTrace,
  FeedEvent,
  FillRow,
  ConfigDiffRow,
  ConfigFingerprintSummary,
  IncidentCase,
  MarketDataQualitySummary,
  MetadataSummary,
  ReplaceCancelChainRow,
  RuntimeLeadershipSummary,
  AlertTriageRow,
  ServiceTopology,
  SessionStateSummary,
  TransportSummary,
  OmsOverview,
  OpenOrderRow,
  OperatorActionDefinition,
  OperatorActionReceipt,
  OperatorAlert,
  OperatorTimelineEvent,
  PortfolioSummary,
  PositionRow,
  PreflightStatus,
  ReconcileMismatchRow,
  ReconcileSummary,
  RiskDenialRow,
  RiskSummary,
  StrategyRow,
  StrategySuppressionRow,
  SystemMetrics,
  ArtifactRegistrySummary,
  SystemModel,
  SystemStatus,
} from "./types";

async function tryFetchJson<T>(path: string): Promise<T | null> {
  return tryFetchJsonCandidates<T>([path]);
}

async function tryFetchJsonCandidates<T>(paths: string[]): Promise<T | null> {
  for (const path of paths) {
    try {
      const url = new URL(path, getDaemonUrl()).toString();
      const response = await fetch(url, {
        method: "GET",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) continue;
      return (await response.json()) as T;
    } catch {
      // try next candidate
    }
  }

  return null;
}

async function postJson<T>(path: string, body: Record<string, unknown>): Promise<T | null> {
  return postJsonCandidates<T>([path], body);
}

async function postJsonCandidates<T>(paths: string[], body: Record<string, unknown>): Promise<T | null> {
  for (const path of paths) {
    try {
      const url = new URL(path, getDaemonUrl()).toString();
      const response = await fetch(url, {
        method: "POST",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        body: JSON.stringify(body),
      });
      if (!response.ok) continue;
      return (await response.json()) as T;
    } catch {
      // try next candidate
    }
  }

  return null;
}

interface LegacyDaemonStatusSnapshot {
  daemon_uptime_secs: number;
  active_run_id: string | null;
  state: string;
  notes?: string | null;
  integrity_armed: boolean;
}

function mapLegacyStatusToSystemStatus(legacy: LegacyDaemonStatusSnapshot): SystemStatus {
  const base = { ...MOCK_MODEL.status };
  const state = String(legacy.state ?? '').toLowerCase();
  const runtimeStatus: SystemStatus["runtime_status"] =
    state.includes('halt') ? 'halted' :
    state.includes('start') ? 'starting' :
    state.includes('run') ? 'running' :
    state.includes('degrad') ? 'degraded' :
    state.includes('pause') ? 'paused' : 'idle';

  return {
    ...base,
    runtime_status: runtimeStatus,
    integrity_status: legacy.integrity_armed ? 'ok' : 'warning',
    last_heartbeat: new Date().toISOString(),
    active_account_id: legacy.active_run_id,
    has_warning: base.has_warning || !legacy.integrity_armed,
    strategy_armed: legacy.integrity_armed,
    execution_armed: legacy.integrity_armed,
    kill_switch_active: runtimeStatus === 'halted',
    integrity_halt_active: !legacy.integrity_armed,
    daemon_reachable: true,
  };
}

function deriveLegacyPreflight(status: LegacyDaemonStatusSnapshot): PreflightStatus {
  return {
    ...MOCK_MODEL.preflight,
    daemon_reachable: true,
    runtime_idle: !String(status.state ?? '').toLowerCase().includes('run'),
    strategy_disarmed: !status.integrity_armed,
    execution_disarmed: !status.integrity_armed,
    blockers: [],
  };
}

function legacyActionPaths(actionKey: string): string[] {
  switch (actionKey) {
    case 'start-system':
      return ['/v1/run/start'];
    case 'stop-system':
      return ['/v1/run/stop'];
    case 'kill-switch':
      return ['/v1/run/halt'];
    case 'arm-execution':
    case 'arm-strategy':
      return ['/v1/integrity/arm'];
    case 'disarm-execution':
    case 'disarm-strategy':
      return ['/v1/integrity/disarm'];
    default:
      return [];
  }
}

function arrayOrFallback<T>(value: unknown, fallback: T[]): T[] {
  return Array.isArray(value) ? (value as T[]) : fallback;
}

function objectOrFallback<T>(value: unknown, fallback: T): T {
  return value && typeof value === "object" ? (value as T) : fallback;
}

export async function fetchOperatorModel(): Promise<SystemModel> {
  const legacyStatus = await tryFetchJsonCandidates<LegacyDaemonStatusSnapshot>(["/v1/status"]);

  const [
    status,
    preflight,
    executionSummary,
    executionOrders,
    omsOverview,
    metrics,
    portfolioSummary,
    positions,
    openOrders,
    fills,
    riskSummary,
    riskDenials,
    reconcileSummary,
    mismatches,
    strategies,
    alerts,
    feed,
    auditActions,
    metadata,
    topology,
    transport,
    incidents,
    replaceCancelChains,
    alertTriage,
    sessionState,
    configFingerprint,
    marketDataQuality,
    runtimeLeadership,
    artifactRegistry,
    strategySuppressions,
    configDiffs,
    operatorTimeline,
  ] = await Promise.all([
    tryFetchJson<SystemStatus>("/api/v1/system/status"),
    tryFetchJson<PreflightStatus>("/api/v1/system/preflight"),
    tryFetchJson<ExecutionSummary>("/api/v1/execution/summary"),
    tryFetchJson<ExecutionOrderRow[]>("/api/v1/execution/orders"),
    tryFetchJson<OmsOverview>("/api/v1/oms/overview"),
    tryFetchJson<SystemMetrics>("/api/v1/metrics/dashboards"),
    tryFetchJson<PortfolioSummary>("/api/v1/portfolio/summary"),
    tryFetchJson<PositionRow[]>("/api/v1/portfolio/positions"),
    tryFetchJson<OpenOrderRow[]>("/api/v1/portfolio/orders/open"),
    tryFetchJson<FillRow[]>("/api/v1/portfolio/fills"),
    tryFetchJson<RiskSummary>("/api/v1/risk/summary"),
    tryFetchJson<RiskDenialRow[]>("/api/v1/risk/denials"),
    tryFetchJson<ReconcileSummary>("/api/v1/reconcile/status"),
    tryFetchJson<ReconcileMismatchRow[]>("/api/v1/reconcile/mismatches"),
    tryFetchJson<StrategyRow[]>("/api/v1/strategy/summary"),
    tryFetchJson<OperatorAlert[]>("/api/v1/alerts/active"),
    tryFetchJson<FeedEvent[]>("/api/v1/events/feed"),
    tryFetchJson<AuditActionRow[]>("/api/v1/audit/operator-actions"),
    tryFetchJson<MetadataSummary>("/api/v1/system/metadata"),
    tryFetchJson<ServiceTopology>("/api/v1/system/topology"),
    tryFetchJson<TransportSummary>("/api/v1/execution/transport"),
    tryFetchJson<IncidentCase[]>("/api/v1/incidents"),
    tryFetchJson<ReplaceCancelChainRow[]>("/api/v1/execution/replace-cancel-chains"),
    tryFetchJson<AlertTriageRow[]>("/api/v1/alerts/triage"),
    tryFetchJson<SessionStateSummary>("/api/v1/system/session"),
    tryFetchJson<ConfigFingerprintSummary>("/api/v1/system/config-fingerprint"),
    tryFetchJson<MarketDataQualitySummary>("/api/v1/market-data/quality"),
    tryFetchJson<RuntimeLeadershipSummary>("/api/v1/system/runtime-leadership"),
    tryFetchJson<ArtifactRegistrySummary>("/api/v1/audit/artifacts"),
    tryFetchJson<StrategySuppressionRow[]>("/api/v1/strategy/suppressions"),
    tryFetchJson<ConfigDiffRow[]>("/api/v1/system/config-diffs"),
    tryFetchJson<OperatorTimelineEvent[]>("/api/v1/ops/operator-timeline"),
  ]);

  const connected = Boolean(status || legacyStatus);
  const firstOrderId = arrayOrFallback(executionOrders, MOCK_MODEL.executionOrders)[0]?.internal_order_id;
  const [selectedTimeline, executionTrace, executionReplay, executionChart, causalityTrace] = firstOrderId
    ? await Promise.all([
        tryFetchJson<ExecutionTimeline>(`/api/v1/execution/timeline/${firstOrderId}`),
        tryFetchJson<ExecutionTrace>(`/api/v1/execution/trace/${firstOrderId}`),
        tryFetchJson<ExecutionReplay>(`/api/v1/execution/replay/${firstOrderId}`),
        tryFetchJson<ExecutionChartModel>(`/api/v1/execution/chart/${firstOrderId}`),
        tryFetchJson<CausalityTrace>(`/api/v1/execution/causality/${firstOrderId}`),
      ])
    : [null, null, null, null, null];

  return {
    status: objectOrFallback(
      status ?? (legacyStatus ? mapLegacyStatusToSystemStatus(legacyStatus) : null),
      connected || Boolean(legacyStatus) ? MOCK_MODEL.status : { ...MOCK_MODEL.status, daemon_reachable: false },
    ),
    preflight: objectOrFallback(
      preflight ?? (legacyStatus ? deriveLegacyPreflight(legacyStatus) : null),
      connected || Boolean(legacyStatus) ? MOCK_MODEL.preflight : { ...MOCK_MODEL.preflight, daemon_reachable: false, blockers: ["Daemon unreachable"] },
    ),
    executionSummary: objectOrFallback(executionSummary, MOCK_MODEL.executionSummary),
    executionOrders: arrayOrFallback(executionOrders, MOCK_MODEL.executionOrders),
    selectedTimeline: objectOrFallback(selectedTimeline, MOCK_MODEL.selectedTimeline),
    omsOverview: objectOrFallback(omsOverview, MOCK_MODEL.omsOverview),
    executionTrace: objectOrFallback(executionTrace, MOCK_MODEL.executionTrace),
    executionReplay: objectOrFallback(executionReplay, MOCK_MODEL.executionReplay),
    executionChart: objectOrFallback(executionChart, MOCK_MODEL.executionChart),
    causalityTrace: objectOrFallback(causalityTrace, MOCK_MODEL.causalityTrace),
    metrics: objectOrFallback(metrics, MOCK_MODEL.metrics),
    portfolioSummary: objectOrFallback(portfolioSummary, MOCK_MODEL.portfolioSummary),
    positions: arrayOrFallback(positions, MOCK_MODEL.positions),
    openOrders: arrayOrFallback(openOrders, MOCK_MODEL.openOrders),
    fills: arrayOrFallback(fills, MOCK_MODEL.fills),
    riskSummary: objectOrFallback(riskSummary, MOCK_MODEL.riskSummary),
    riskDenials: arrayOrFallback(riskDenials, MOCK_MODEL.riskDenials),
    reconcileSummary: objectOrFallback(reconcileSummary, MOCK_MODEL.reconcileSummary),
    mismatches: arrayOrFallback(mismatches, MOCK_MODEL.mismatches),
    strategies: arrayOrFallback(strategies, MOCK_MODEL.strategies),
    alerts: arrayOrFallback(alerts, MOCK_MODEL.alerts),
    feed: arrayOrFallback(feed, MOCK_MODEL.feed),
    auditActions: arrayOrFallback(auditActions, MOCK_MODEL.auditActions),
    metadata: objectOrFallback(metadata, MOCK_MODEL.metadata),
    topology: objectOrFallback(topology, MOCK_MODEL.topology),
    transport: objectOrFallback(transport, MOCK_MODEL.transport),
    incidents: arrayOrFallback(incidents, MOCK_MODEL.incidents),
    replaceCancelChains: arrayOrFallback(replaceCancelChains, MOCK_MODEL.replaceCancelChains),
    alertTriage: arrayOrFallback(alertTriage, MOCK_MODEL.alertTriage),
    sessionState: objectOrFallback(sessionState, MOCK_MODEL.sessionState),
    configFingerprint: objectOrFallback(configFingerprint, MOCK_MODEL.configFingerprint),
    marketDataQuality: objectOrFallback(marketDataQuality, MOCK_MODEL.marketDataQuality),
    runtimeLeadership: objectOrFallback(runtimeLeadership, MOCK_MODEL.runtimeLeadership),
    artifactRegistry: objectOrFallback(artifactRegistry, MOCK_MODEL.artifactRegistry),
    strategySuppressions: arrayOrFallback(strategySuppressions, MOCK_MODEL.strategySuppressions),
    configDiffs: arrayOrFallback(configDiffs, MOCK_MODEL.configDiffs),
    operatorTimeline: arrayOrFallback(operatorTimeline, MOCK_MODEL.operatorTimeline),
    actionCatalog: MOCK_MODEL.actionCatalog,
    connected,
    lastUpdatedAt: new Date().toISOString(),
  };
}

export async function fetchExecutionTimeline(internalOrderId: string): Promise<ExecutionTimeline> {
  const response = await tryFetchJson<ExecutionTimeline>(`/api/v1/execution/timeline/${internalOrderId}`);
  if (response) return response;
  return { ...MOCK_MODEL.selectedTimeline!, internal_order_id: internalOrderId, timeline_id: `TL-${internalOrderId}` };
}

export async function fetchExecutionTrace(internalOrderId: string): Promise<ExecutionTrace> {
  const response = await tryFetchJson<ExecutionTrace>(`/api/v1/execution/trace/${internalOrderId}`);
  if (response) return response;
  return { ...MOCK_MODEL.executionTrace!, internal_order_id: internalOrderId };
}

export async function fetchExecutionReplay(internalOrderId: string): Promise<ExecutionReplay> {
  const response = await tryFetchJson<ExecutionReplay>(`/api/v1/execution/replay/${internalOrderId}`);
  if (response) return response;
  return { ...MOCK_MODEL.executionReplay!, replay_id: `RPL-${internalOrderId}`, title: `${internalOrderId} replay` };
}

export async function fetchCausalityTrace(internalOrderId: string): Promise<CausalityTrace> {
  const response = await tryFetchJson<CausalityTrace>(`/api/v1/execution/causality/${internalOrderId}`);
  if (response) return response;
  return { ...MOCK_MODEL.causalityTrace!, internal_order_id: internalOrderId, incident_id: `INC-${internalOrderId}` };
}

export async function invokeOperatorAction(
  action: OperatorActionDefinition,
  args: { reason?: string; target_scope?: string; alert_id?: string },
  current: SystemModel,
): Promise<OperatorActionReceipt> {
  const body = {
    environment: current.status.environment,
    reason: args.reason ?? null,
    target_scope: args.target_scope ?? "runtime",
    alert_id: args.alert_id ?? null,
    live_routing_enabled: current.status.live_routing_enabled,
  };

  const result = await postJson<OperatorActionReceipt>(`/api/v1/ops/${action.action_key}`, body);
  if (result) return result;

  const legacyPaths = legacyActionPaths(action.action_key);
  if (legacyPaths.length > 0) {
    const legacyResult = await postJsonCandidates<Record<string, unknown>>(legacyPaths, body);
    if (legacyResult !== null) {
      return {
        ok: true,
        action_key: action.action_key,
        environment: current.status.environment,
        live_routing_enabled: current.status.live_routing_enabled,
        result_state: `${action.action_key} accepted (legacy daemon endpoint)`,
        audit_reference: `LEGACY-${Date.now()}`,
        warnings: ["Action was routed through a legacy daemon endpoint for compatibility."],
        blocking_failures: [],
        simulated: false,
      };
    }
  }

  const warnings: string[] = [];
  if (current.status.environment === "live") {
    warnings.push("GUI is using simulated operator action fallback because the daemon endpoint was unavailable.");
  }

  return {
    ok: true,
    action_key: action.action_key,
    environment: current.status.environment,
    live_routing_enabled: current.status.live_routing_enabled,
    result_state: `${action.action_key} accepted (simulated)`,
    audit_reference: `SIM-${Date.now()}`,
    warnings,
    blocking_failures: [],
    simulated: true,
  };
}


export async function requestSystemModeTransition(
  targetMode: SystemStatus["environment"],
  current: SystemModel,
  reason: string,
): Promise<OperatorActionReceipt> {
  const body = {
    current_mode: current.status.environment,
    target_mode: targetMode,
    reason,
    live_routing_enabled: current.status.live_routing_enabled,
    strategy_armed: current.status.strategy_armed,
    execution_armed: current.status.execution_armed,
  };

  const result = await postJson<OperatorActionReceipt>("/api/v1/ops/change-system-mode", body);
  if (result) return result;

  const legacyStop = await postJsonCandidates<Record<string, unknown>>(["/v1/run/stop"], body);
  if (legacyStop !== null) {
    return {
      ok: true,
      action_key: "change-system-mode",
      environment: current.status.environment,
      live_routing_enabled: current.status.live_routing_enabled,
      result_state: `legacy daemon accepted stop request; complete the ${targetMode} mode restart out-of-band`,
      audit_reference: `LEGACY-MODE-${Date.now()}`,
      warnings: [
        "Legacy daemon contract does not expose a first-class mode-transition endpoint.",
        "The GUI issued a compatible stop request only. Complete the controlled restart into the target mode separately.",
      ],
      blocking_failures: [],
      simulated: false,
    };
  }

  const warnings = [
    "GUI is using simulated mode-transition fallback because the daemon endpoint was unavailable.",
    "Treat this as a scaffold receipt only; real mode changes must perform a controlled daemon restart.",
  ];

  return {
    ok: true,
    action_key: "change-system-mode",
    environment: targetMode,
    live_routing_enabled: targetMode === "live" ? false : false,
    result_state: `controlled mode transition queued: ${current.status.environment} -> ${targetMode} (simulated)`,
    audit_reference: `SIM-MODE-${Date.now()}`,
    warnings,
    blocking_failures: [],
    simulated: true,
  };
}


export async function fetchExecutionChart(internalOrderId: string): Promise<ExecutionChartModel> {
  const response = await tryFetchJson<ExecutionChartModel>(`/api/v1/execution/chart/${internalOrderId}`);
  if (response) return response;
  return { ...MOCK_MODEL.executionChart!, order_id: internalOrderId };
}
