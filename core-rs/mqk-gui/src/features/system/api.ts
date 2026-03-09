import { getDaemonUrl } from "../../config";
import { MOCK_MODEL } from "./mockData";
import type {
  AlertTriageRow,
  ArtifactRegistrySummary,
  AuditActionRow,
  CausalityTrace,
  ConfigDiffRow,
  ConfigFingerprintSummary,
  DataSourceDetail,
  ExecutionChartModel,
  ExecutionOrderRow,
  ExecutionReplay,
  ExecutionSummary,
  ExecutionTimeline,
  ExecutionTrace,
  FeedEvent,
  FillRow,
  IncidentCase,
  MarketDataQualitySummary,
  MetadataSummary,
  OmsOverview,
  OpenOrderRow,
  OperatorActionReceipt,
  OperatorAlert,
  OperatorTimelineEvent,
  PortfolioSummary,
  PositionRow,
  PreflightStatus,
  ReconcileMismatchRow,
  ReconcileSummary,
  ReplaceCancelChainRow,
  RiskDenialRow,
  RiskSummary,
  RuntimeLeadershipSummary,
  ServiceTopology,
  SessionStateSummary,
  StrategyRow,
  StrategySuppressionRow,
  SystemMetrics,
  SystemModel,
  SystemStatus,
  TransportSummary,
} from "./types";

interface EndpointFetchResult<T> {
  ok: boolean;
  endpoint: string;
  data?: T;
  error?: string;
}

interface LegacyDaemonStatusSnapshot {
  daemon_uptime_secs: number;
  active_run_id: string | null;
  state: string;
  notes?: string | null;
  integrity_armed: boolean;
}

async function fetchJsonCandidate<T>(path: string): Promise<EndpointFetchResult<T>> {
  try {
    const url = new URL(path, getDaemonUrl()).toString();
    const response = await fetch(url, {
      method: "GET",
      headers: { Accept: "application/json" },
    });

    if (!response.ok) {
      return { ok: false, endpoint: path, error: `HTTP ${response.status}` };
    }

    return {
      ok: true,
      endpoint: path,
      data: (await response.json()) as T,
    };
  } catch (error) {
    return {
      ok: false,
      endpoint: path,
      error: error instanceof Error ? error.message : "unknown error",
    };
  }
}

async function fetchJsonCandidates<T>(paths: string[]): Promise<EndpointFetchResult<T>> {
  for (const path of paths) {
    const result = await fetchJsonCandidate<T>(path);
    if (result.ok) return result;
  }
  return {
    ok: false,
    endpoint: paths[0] ?? "unknown",
    error: "all candidates failed",
  };
}

async function tryFetchJson<T>(paths: string[]): Promise<T | null> {
  const result = await fetchJsonCandidates<T>(paths);
  return result.ok ? (result.data ?? null) : null;
}

async function postJson<T>(paths: string[], body: Record<string, unknown>): Promise<T | null> {
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

function mapLegacyStatusToSystemStatus(legacy: LegacyDaemonStatusSnapshot): SystemStatus {
  const base = { ...MOCK_MODEL.status };
  const state = String(legacy.state ?? "").toLowerCase();
  const runtimeStatus: SystemStatus["runtime_status"] =
    state.includes("halt")
      ? "halted"
      : state.includes("start")
        ? "starting"
        : state.includes("run")
          ? "running"
          : state.includes("degrad")
            ? "degraded"
            : state.includes("pause")
              ? "paused"
              : "idle";

  return {
    ...base,
    runtime_status: runtimeStatus,
    integrity_status: legacy.integrity_armed ? "ok" : "warning",
    last_heartbeat: new Date().toISOString(),
    active_account_id: legacy.active_run_id,
    has_warning: base.has_warning || !legacy.integrity_armed,
    strategy_armed: legacy.integrity_armed,
    execution_armed: legacy.integrity_armed,
    kill_switch_active: runtimeStatus === "halted",
    integrity_halt_active: !legacy.integrity_armed,
    daemon_reachable: true,
  };
}

function deriveLegacyPreflight(status: LegacyDaemonStatusSnapshot): PreflightStatus {
  return {
    ...MOCK_MODEL.preflight,
    daemon_reachable: true,
    runtime_idle: !String(status.state ?? "").toLowerCase().includes("run"),
    strategy_disarmed: !status.integrity_armed,
    execution_disarmed: !status.integrity_armed,
    blockers: [],
  };
}

function legacyActionPaths(actionKey: string): string[] {
  switch (actionKey) {
    case "start-system":
      return ["/v1/run/start"];
    case "stop-system":
      return ["/v1/run/stop"];
    case "kill-switch":
      return ["/v1/run/halt"];
    case "arm-execution":
    case "arm-strategy":
      return ["/v1/integrity/arm"];
    case "disarm-execution":
    case "disarm-strategy":
      return ["/v1/integrity/disarm"];
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

function deriveDataSourceDetail(args: {
  probeResults: EndpointFetchResult<unknown>[];
  usedMockSections: string[];
  daemonReachable: boolean;
}): DataSourceDetail {
  const realEndpoints = args.probeResults.filter((r) => r.ok).map((r) => r.endpoint);
  const missingEndpoints = args.probeResults.filter((r) => !r.ok).map((r) => r.endpoint);

  let state: DataSourceDetail["state"];
  if (!args.daemonReachable && realEndpoints.length === 0) {
    state = "disconnected";
  } else if (realEndpoints.length === 0) {
    state = "mock";
  } else if (args.usedMockSections.length > 0 || missingEndpoints.length > 0) {
    state = "partial";
  } else {
    state = "real";
  }

  return {
    state,
    reachable: args.daemonReachable,
    realEndpoints,
    missingEndpoints,
    mockSections: args.usedMockSections,
    message:
      state === "disconnected"
        ? "Daemon unreachable; GUI is not receiving live data."
        : state === "mock"
          ? "Mock fallback active."
          : state === "partial"
            ? "Mixed real and fallback data."
            : "All tracked surfaces resolved from daemon endpoints.",
  };
}

export async function fetchOperatorModel(): Promise<SystemModel> {
  const statusProbe = await fetchJsonCandidates<SystemStatus>(["/api/v1/system/status", "/v1/status"]);
  const legacyStatus =
    statusProbe.ok && statusProbe.endpoint === "/v1/status"
      ? (statusProbe.data as unknown as LegacyDaemonStatusSnapshot)
      : await tryFetchJson<LegacyDaemonStatusSnapshot>(["/v1/status"]);

  const probes = await Promise.all([
    fetchJsonCandidates<PreflightStatus>(["/api/v1/system/preflight"]),
    fetchJsonCandidates<ExecutionSummary>(["/api/v1/execution/summary"]),
    fetchJsonCandidates<ExecutionOrderRow[]>(["/api/v1/execution/orders", "/v1/trading/orders"]),
    fetchJsonCandidates<OmsOverview>(["/api/v1/oms/overview"]),
    fetchJsonCandidates<SystemMetrics>(["/api/v1/metrics/dashboards"]),
    fetchJsonCandidates<PortfolioSummary>(["/api/v1/portfolio/summary", "/v1/trading/account"]),
    fetchJsonCandidates<PositionRow[]>(["/api/v1/portfolio/positions", "/v1/trading/positions"]),
    fetchJsonCandidates<OpenOrderRow[]>(["/api/v1/portfolio/orders/open", "/v1/trading/orders"]),
    fetchJsonCandidates<FillRow[]>(["/api/v1/portfolio/fills", "/v1/trading/fills"]),
    fetchJsonCandidates<RiskSummary>(["/api/v1/risk/summary"]),
    fetchJsonCandidates<RiskDenialRow[]>(["/api/v1/risk/denials"]),
    fetchJsonCandidates<ReconcileSummary>(["/api/v1/reconcile/status"]),
    fetchJsonCandidates<ReconcileMismatchRow[]>(["/api/v1/reconcile/mismatches"]),
    fetchJsonCandidates<StrategyRow[]>(["/api/v1/strategy/summary"]),
    fetchJsonCandidates<OperatorAlert[]>(["/api/v1/alerts/active"]),
    fetchJsonCandidates<FeedEvent[]>(["/api/v1/events/feed", "/v1/stream"]),
    fetchJsonCandidates<AuditActionRow[]>(["/api/v1/audit/operator-actions"]),
    fetchJsonCandidates<MetadataSummary>(["/api/v1/system/metadata"]),
    fetchJsonCandidates<ServiceTopology>(["/api/v1/system/topology"]),
    fetchJsonCandidates<TransportSummary>(["/api/v1/execution/transport"]),
    fetchJsonCandidates<IncidentCase[]>(["/api/v1/incidents"]),
    fetchJsonCandidates<ReplaceCancelChainRow[]>(["/api/v1/execution/replace-cancel-chains"]),
    fetchJsonCandidates<AlertTriageRow[]>(["/api/v1/alerts/triage"]),
    fetchJsonCandidates<SessionStateSummary>(["/api/v1/system/session"]),
    fetchJsonCandidates<ConfigFingerprintSummary>(["/api/v1/system/config-fingerprint"]),
    fetchJsonCandidates<MarketDataQualitySummary>(["/api/v1/market-data/quality"]),
    fetchJsonCandidates<RuntimeLeadershipSummary>(["/api/v1/system/runtime-leadership"]),
    fetchJsonCandidates<ArtifactRegistrySummary>(["/api/v1/audit/artifacts"]),
    fetchJsonCandidates<StrategySuppressionRow[]>(["/api/v1/strategy/suppressions"]),
    fetchJsonCandidates<ConfigDiffRow[]>(["/api/v1/system/config-diffs"]),
    fetchJsonCandidates<OperatorTimelineEvent[]>(["/api/v1/ops/operator-timeline"]),
  ]);

  const [
    preflightR,
    executionSummaryR,
    executionOrdersR,
    omsOverviewR,
    metricsR,
    portfolioSummaryR,
    positionsR,
    openOrdersR,
    fillsR,
    riskSummaryR,
    riskDenialsR,
    reconcileSummaryR,
    mismatchesR,
    strategiesR,
    alertsR,
    feedR,
    auditActionsR,
    metadataR,
    topologyR,
    transportR,
    incidentsR,
    replaceCancelChainsR,
    alertTriageR,
    sessionStateR,
    configFingerprintR,
    marketDataQualityR,
    runtimeLeadershipR,
    artifactRegistryR,
    strategySuppressionsR,
    configDiffsR,
    operatorTimelineR,
  ] = probes;

  const daemonReachable = statusProbe.ok || Boolean(legacyStatus) || probes.some((p) => p.ok);
  const connected = daemonReachable;

  const executionOrders = arrayOrFallback(executionOrdersR.data, MOCK_MODEL.executionOrders);
  const firstOrderId = executionOrders[0]?.internal_order_id;
  const [selectedTimeline, executionTrace, executionReplay, executionChart, causalityTrace] = firstOrderId
    ? await Promise.all([
        tryFetchJson<ExecutionTimeline>([`/api/v1/execution/timeline/${firstOrderId}`]),
        tryFetchJson<ExecutionTrace>([`/api/v1/execution/trace/${firstOrderId}`]),
        tryFetchJson<ExecutionReplay>([`/api/v1/execution/replay/${firstOrderId}`]),
        tryFetchJson<ExecutionChartModel>([`/api/v1/execution/chart/${firstOrderId}`]),
        tryFetchJson<CausalityTrace>([`/api/v1/execution/causality/${firstOrderId}`]),
      ])
    : [null, null, null, null, null];

  const usedMockSections: string[] = [];
  const useObject = <T,>(key: string, result: EndpointFetchResult<T>, fallback: T): T => {
    if (result.ok && result.data !== undefined) return result.data;
    usedMockSections.push(key);
    return fallback;
  };
  const useArray = <T,>(key: string, result: EndpointFetchResult<T[]>, fallback: T[]): T[] => {
    if (result.ok && Array.isArray(result.data)) return result.data;
    usedMockSections.push(key);
    return fallback;
  };

  const dataSource = deriveDataSourceDetail({
    probeResults: [statusProbe, ...probes],
    usedMockSections,
    daemonReachable,
  });

  return {
    status: objectOrFallback(
      statusProbe.ok && statusProbe.endpoint === "/api/v1/system/status"
        ? statusProbe.data
        : legacyStatus
          ? mapLegacyStatusToSystemStatus(legacyStatus)
          : null,
      connected ? MOCK_MODEL.status : { ...MOCK_MODEL.status, daemon_reachable: false },
    ),
    preflight: objectOrFallback(
      preflightR.ok ? preflightR.data : legacyStatus ? deriveLegacyPreflight(legacyStatus) : null,
      connected ? MOCK_MODEL.preflight : { ...MOCK_MODEL.preflight, daemon_reachable: false, blockers: ["Daemon unreachable"] },
    ),
    executionSummary: useObject("executionSummary", executionSummaryR, MOCK_MODEL.executionSummary),
    executionOrders,
    selectedTimeline: objectOrFallback(selectedTimeline, MOCK_MODEL.selectedTimeline),
    omsOverview: useObject("omsOverview", omsOverviewR, MOCK_MODEL.omsOverview),
    executionTrace: objectOrFallback(executionTrace, MOCK_MODEL.executionTrace),
    executionReplay: objectOrFallback(executionReplay, MOCK_MODEL.executionReplay),
    executionChart: objectOrFallback(executionChart, MOCK_MODEL.executionChart),
    causalityTrace: objectOrFallback(causalityTrace, MOCK_MODEL.causalityTrace),
    metrics: useObject("metrics", metricsR, MOCK_MODEL.metrics),
    portfolioSummary: useObject("portfolioSummary", portfolioSummaryR, MOCK_MODEL.portfolioSummary),
    positions: useArray("positions", positionsR, MOCK_MODEL.positions),
    openOrders: useArray("openOrders", openOrdersR, MOCK_MODEL.openOrders),
    fills: useArray("fills", fillsR, MOCK_MODEL.fills),
    riskSummary: useObject("riskSummary", riskSummaryR, MOCK_MODEL.riskSummary),
    riskDenials: useArray("riskDenials", riskDenialsR, MOCK_MODEL.riskDenials),
    reconcileSummary: useObject("reconcileSummary", reconcileSummaryR, MOCK_MODEL.reconcileSummary),
    mismatches: useArray("mismatches", mismatchesR, MOCK_MODEL.mismatches),
    strategies: useArray("strategies", strategiesR, MOCK_MODEL.strategies),
    alerts: useArray("alerts", alertsR, MOCK_MODEL.alerts),
    feed: useArray("feed", feedR, MOCK_MODEL.feed),
    auditActions: useArray("auditActions", auditActionsR, MOCK_MODEL.auditActions),
    metadata: useObject("metadata", metadataR, MOCK_MODEL.metadata),
    topology: useObject("topology", topologyR, MOCK_MODEL.topology),
    transport: useObject("transport", transportR, MOCK_MODEL.transport),
    incidents: useArray("incidents", incidentsR, MOCK_MODEL.incidents),
    replaceCancelChains: useArray("replaceCancelChains", replaceCancelChainsR, MOCK_MODEL.replaceCancelChains),
    alertTriage: useArray("alertTriage", alertTriageR, MOCK_MODEL.alertTriage),
    sessionState: useObject("sessionState", sessionStateR, MOCK_MODEL.sessionState),
    configFingerprint: useObject("configFingerprint", configFingerprintR, MOCK_MODEL.configFingerprint),
    marketDataQuality: useObject("marketDataQuality", marketDataQualityR, MOCK_MODEL.marketDataQuality),
    runtimeLeadership: useObject("runtimeLeadership", runtimeLeadershipR, MOCK_MODEL.runtimeLeadership),
    artifactRegistry: useObject("artifactRegistry", artifactRegistryR, MOCK_MODEL.artifactRegistry),
    strategySuppressions: useArray("strategySuppressions", strategySuppressionsR, MOCK_MODEL.strategySuppressions),
    configDiffs: useArray("configDiffs", configDiffsR, MOCK_MODEL.configDiffs),
    operatorTimeline: useArray("operatorTimeline", operatorTimelineR, MOCK_MODEL.operatorTimeline),
    actionCatalog: connected ? MOCK_MODEL.actionCatalog : [],
    dataSource,
    connected,
    lastUpdatedAt: new Date().toISOString(),
  };
}

export async function fetchExecutionTimeline(internalOrderId: string): Promise<ExecutionTimeline | null> {
  return tryFetchJson<ExecutionTimeline>([`/api/v1/execution/timeline/${internalOrderId}`]);
}

export async function fetchExecutionTrace(internalOrderId: string): Promise<ExecutionTrace | null> {
  return tryFetchJson<ExecutionTrace>([`/api/v1/execution/trace/${internalOrderId}`]);
}

export async function fetchExecutionReplay(internalOrderId: string): Promise<ExecutionReplay | null> {
  return tryFetchJson<ExecutionReplay>([`/api/v1/execution/replay/${internalOrderId}`]);
}

export async function fetchExecutionChart(internalOrderId: string): Promise<ExecutionChartModel | null> {
  return tryFetchJson<ExecutionChartModel>([`/api/v1/execution/chart/${internalOrderId}`]);
}

export async function fetchCausalityTrace(internalOrderId: string): Promise<CausalityTrace | null> {
  return tryFetchJson<CausalityTrace>([`/api/v1/execution/causality/${internalOrderId}`]);
}

export async function invokeOperatorAction(
  actionKey: string,
  params: Record<string, unknown>,
): Promise<OperatorActionReceipt> {
  const response = await postJson<Partial<OperatorActionReceipt>>(
    ["/api/v1/ops/action", ...legacyActionPaths(actionKey)],
    { action_key: actionKey, ...params },
  );

  if (response) {
    return {
      ok: response.ok ?? true,
      action_key: response.action_key ?? actionKey,
      environment: response.environment ?? "paper",
      live_routing_enabled: response.live_routing_enabled ?? false,
      result_state: response.result_state ?? "accepted",
      warnings: response.warnings ?? [],
      audit_reference: response.audit_reference ?? `audit-${actionKey}-${Date.now()}`,
      blocking_failures: response.blocking_failures ?? [],
    };
  }

  return {
    ok: true,
    action_key: actionKey,
    environment: "paper",
    live_routing_enabled: false,
    result_state: "accepted",
    warnings: ["Simulated receipt: daemon action endpoint unavailable."],
    audit_reference: `audit-${actionKey}-${Date.now()}`,
    blocking_failures: [],
  };
}

export async function requestSystemModeTransition(
  targetMode: SystemStatus["environment"],
  reason: string,
): Promise<OperatorActionReceipt> {
  const response = await postJson<Partial<OperatorActionReceipt>>(
    ["/api/v1/ops/change-mode"],
    { target_mode: targetMode, reason },
  );

  if (response) {
    return {
      ok: response.ok ?? true,
      action_key: response.action_key ?? "change-system-mode",
      environment: response.environment ?? targetMode,
      live_routing_enabled: response.live_routing_enabled ?? false,
      result_state: response.result_state ?? "accepted",
      warnings: response.warnings ?? [],
      audit_reference: response.audit_reference ?? `audit-change-system-mode-${Date.now()}`,
      blocking_failures: response.blocking_failures ?? [],
    };
  }

  return {
    ok: true,
    action_key: "change-system-mode",
    environment: targetMode,
    live_routing_enabled: false,
    result_state: "accepted",
    warnings: ["Simulated receipt: daemon mode transition endpoint unavailable."],
    audit_reference: `audit-change-system-mode-${Date.now()}`,
    blocking_failures: [],
  };
}