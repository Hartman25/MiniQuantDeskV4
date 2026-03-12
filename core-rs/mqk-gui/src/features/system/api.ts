import { getDaemonUrl } from "../../config";
import { MOCK_MODEL } from "./mockData";
import { classifyPanelSources } from "./sourceAuthority";
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

interface EndpointPostResult<T> {
  ok: boolean;
  endpoint: string;
  status?: number;
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

interface LegacyHealthResponse {
  ok: boolean;
  service: string;
  version: string;
}

interface LegacyTradingAccountResponse {
  has_snapshot: boolean;
  account: {
    equity: string;
    cash: string;
    currency: string;
  };
}

interface LegacyTradingPosition {
  symbol: string;
  qty: string;
  avg_price: string;
}

interface LegacyTradingPositionsResponse {
  has_snapshot: boolean;
  positions: LegacyTradingPosition[];
}

interface LegacyTradingOrder {
  broker_order_id: string;
  client_order_id: string;
  symbol: string;
  side: string;
  type: string;
  status: string;
  qty: string;
  limit_price?: string | null;
  stop_price?: string | null;
  created_at_utc: string;
}

interface LegacyTradingOrdersResponse {
  has_snapshot: boolean;
  orders: LegacyTradingOrder[];
}

interface LegacyTradingFill {
  broker_fill_id: string;
  broker_order_id: string;
  client_order_id: string;
  symbol: string;
  side: string;
  qty: string;
  price: string;
  fee: string;
  ts_utc: string;
}

interface LegacyTradingFillsResponse {
  has_snapshot: boolean;
  fills: LegacyTradingFill[];
}

interface LegacyIntegrityResponse {
  armed: boolean;
  active_run_id: string | null;
  state: string;
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

async function postJson<T>(paths: string[], body: Record<string, unknown>): Promise<EndpointPostResult<T>> {
  let lastFailure: EndpointPostResult<T> = {
    ok: false,
    endpoint: paths[0] ?? "unknown",
    error: "all candidates failed",
  };

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

      if (!response.ok) {
        lastFailure = {
          ok: false,
          endpoint: path,
          status: response.status,
          error: `HTTP ${response.status}`,
        };
        continue;
      }

      const contentType = response.headers.get("content-type") ?? "";
      const data = contentType.includes("application/json") ? ((await response.json()) as T) : undefined;

      return {
        ok: true,
        endpoint: path,
        status: response.status,
        data,
      };
    } catch (error) {
      lastFailure = {
        ok: false,
        endpoint: path,
        error: error instanceof Error ? error.message : "unknown error",
      };
    }
  }

  return lastFailure;
}

function parseNumber(value: unknown): number {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

function parseIsoTimestamp(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function nowIso(): string {
  return new Date().toISOString();
}

function ageMsFromTimestamp(value: unknown): number {
  const iso = parseIsoTimestamp(value);
  if (!iso) return 0;
  const timestamp = Date.parse(iso);
  if (Number.isNaN(timestamp)) return 0;
  return Math.max(0, Date.now() - timestamp);
}

function normalizeSide(side: unknown): "buy" | "sell" {
  return String(side ?? "").toLowerCase() === "sell" ? "sell" : "buy";
}

function normalizeOrderType(orderType: unknown): "market" | "limit" | "stop" | "stop_limit" {
  const normalized = String(orderType ?? "").toLowerCase().replace(/\s+/g, "_");
  if (normalized === "limit") return "limit";
  if (normalized === "stop") return "stop";
  if (normalized === "stop_limit" || normalized === "stoplimit") return "stop_limit";
  return "market";
}

function normalizeOrderStatus(status: unknown): string {
  const normalized = String(status ?? "unknown").trim().toLowerCase().replace(/\s+/g, "_");
  return normalized || "unknown";
}

function isTerminalOrderStatus(status: string): boolean {
  return ["filled", "cancelled", "canceled", "rejected", "expired", "done_for_day"].includes(status);
}

function deriveExecutionStage(status: string): string {
  if (status.includes("partial")) return "Partial Fill";
  if (status.includes("fill")) return "Filled";
  if (status.includes("cancel")) return "Cancelled";
  if (status.includes("reject")) return "Rejected";
  if (status.includes("pending") || status.includes("new") || status.includes("accepted")) return "Pending";
  if (status.includes("submit")) return "Dispatching";
  return "Broker Snapshot";
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
    last_heartbeat: null,
    active_account_id: null,
    config_profile: null,
    has_warning: base.has_warning || !legacy.integrity_armed || Boolean(legacy.notes),
    strategy_armed: legacy.integrity_armed,
    execution_armed: legacy.integrity_armed,
    live_routing_enabled: false,
    kill_switch_active: runtimeStatus === "halted",
    risk_halt_active: false,
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
    live_routing_disabled: true,
    warnings: ["Derived from legacy /v1/status; canonical preflight endpoint unavailable."],
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

function objectOrFallback<T>(value: unknown, fallback: T): T {
  return value && typeof value === "object" ? (value as T) : fallback;
}

function mapLegacyPositionsResponse(response: LegacyTradingPositionsResponse | null): PositionRow[] | null {
  if (!response) return null;
  return response.positions.map((position) => {
    const qty = parseNumber(position.qty);
    const avgPrice = parseNumber(position.avg_price);
    return {
      symbol: position.symbol,
      strategy_id: "broker_snapshot",
      qty,
      avg_price: avgPrice,
      mark_price: avgPrice,
      unrealized_pnl: 0,
      realized_pnl_today: 0,
      broker_qty: qty,
      drift: false,
    };
  });
}

function mapLegacyPortfolioSummary(
  accountResponse: LegacyTradingAccountResponse | null,
): PortfolioSummary | null {
  if (!accountResponse) return null;

  const equity = parseNumber(accountResponse.account?.equity);
  const cash = parseNumber(accountResponse.account?.cash);

  return {
    account_equity: equity,
    cash,
    long_market_value: 0,
    short_market_value: 0,
    daily_pnl: 0,
    buying_power: cash,
  };
}

function mapLegacyTradingOrdersToExecutionOrders(response: LegacyTradingOrdersResponse | null): ExecutionOrderRow[] | null {
  if (!response) return null;

  return response.orders.map((order) => {
    const status = normalizeOrderStatus(order.status);
    const ageMs = ageMsFromTimestamp(order.created_at_utc);
    const hasCritical = status.includes("reject");
    const hasWarning = !hasCritical && !isTerminalOrderStatus(status) && ageMs >= 300_000;

    return {
      internal_order_id: order.client_order_id || order.broker_order_id,
      broker_order_id: order.broker_order_id || null,
      symbol: order.symbol,
      strategy_id: "broker_snapshot",
      side: normalizeSide(order.side),
      order_type: normalizeOrderType(order.type),
      requested_qty: parseNumber(order.qty),
      filled_qty: 0,
      current_status: status,
      current_stage: deriveExecutionStage(status),
      age_ms: ageMs,
      has_warning: hasWarning,
      has_critical: hasCritical,
      updated_at: parseIsoTimestamp(order.created_at_utc) ?? nowIso(),
    };
  });
}

function mapLegacyTradingOrdersToOpenOrders(response: LegacyTradingOrdersResponse | null): OpenOrderRow[] | null {
  const rows = mapLegacyTradingOrdersToExecutionOrders(response);
  if (!rows) return null;

  return rows
    .filter((order) => !isTerminalOrderStatus(order.current_status))
    .map((order) => ({
      internal_order_id: order.internal_order_id,
      symbol: order.symbol,
      strategy_id: order.strategy_id,
      side: order.side,
      status: order.current_status,
      broker_order_id: order.broker_order_id,
      requested_qty: order.requested_qty,
      filled_qty: order.filled_qty,
      entered_at: order.updated_at,
    }));
}

function mapLegacyTradingFillsToRows(response: LegacyTradingFillsResponse | null): FillRow[] | null {
  if (!response) return null;

  return response.fills.map((fill) => ({
    fill_id: fill.broker_fill_id,
    internal_order_id: fill.client_order_id || fill.broker_order_id,
    symbol: fill.symbol,
    strategy_id: "broker_snapshot",
    side: normalizeSide(fill.side),
    qty: parseNumber(fill.qty),
    price: parseNumber(fill.price),
    broker_exec_id: fill.broker_fill_id,
    applied: true,
    at: parseIsoTimestamp(fill.ts_utc) ?? nowIso(),
  }));
}

function deriveExecutionSummaryFromOrders(orders: ExecutionOrderRow[] | null): ExecutionSummary | null {
  if (!orders) return null;

  const activeOrders = orders.filter((order) => !isTerminalOrderStatus(order.current_status));
  const pendingOrders = orders.filter((order) => {
    const status = order.current_status;
    return status.includes("new") || status.includes("pending") || status.includes("accepted");
  });
  const dispatchingOrders = orders.filter((order) => order.current_status.includes("submit") || order.current_stage === "Dispatching");
  const rejectedOrders = orders.filter((order) => order.current_status.includes("reject"));
  const stuckOrders = activeOrders.filter((order) => order.age_ms >= 300_000);

  return {
    active_orders: activeOrders.length,
    pending_orders: pendingOrders.length,
    dispatching_orders: dispatchingOrders.length,
    reject_count_today: rejectedOrders.length,
    cancel_replace_count_today: 0,
    avg_ack_latency_ms: null,
    stuck_orders: stuckOrders.length,
  };
}

function mapLegacyHealthToMetadata(health: LegacyHealthResponse | null): MetadataSummary | null {
  if (!health) return null;
  return {
    build_version: health.version,
    api_version: "v1",
    broker_adapter: "unknown",
    endpoint_status: health.ok ? "ok" : "warning",
  };
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
  const statusProbe = await fetchJsonCandidates<SystemStatus | LegacyDaemonStatusSnapshot>(["/api/v1/system/status", "/v1/status"]);
  const healthProbe = await fetchJsonCandidates<MetadataSummary | LegacyHealthResponse>(["/api/v1/system/metadata", "/v1/health"]);

  const legacyStatus =
    statusProbe.ok && statusProbe.endpoint === "/v1/status"
      ? (statusProbe.data as LegacyDaemonStatusSnapshot)
      : await tryFetchJson<LegacyDaemonStatusSnapshot>(["/v1/status"]);

  const probes = await Promise.all([
    fetchJsonCandidates<PreflightStatus>(["/api/v1/system/preflight"]),
    fetchJsonCandidates<ExecutionSummary>(["/api/v1/execution/summary"]),
    fetchJsonCandidates<ExecutionOrderRow[] | LegacyTradingOrdersResponse>(["/api/v1/execution/orders", "/v1/trading/orders"]),
    fetchJsonCandidates<OmsOverview>(["/api/v1/oms/overview"]),
    fetchJsonCandidates<SystemMetrics>(["/api/v1/metrics/dashboards"]),
    fetchJsonCandidates<PortfolioSummary | LegacyTradingAccountResponse>(["/api/v1/portfolio/summary", "/v1/trading/account"]),
    fetchJsonCandidates<PositionRow[] | LegacyTradingPositionsResponse>(["/api/v1/portfolio/positions", "/v1/trading/positions"]),
    fetchJsonCandidates<OpenOrderRow[] | LegacyTradingOrdersResponse>(["/api/v1/portfolio/orders/open", "/v1/trading/orders"]),
    fetchJsonCandidates<FillRow[] | LegacyTradingFillsResponse>(["/api/v1/portfolio/fills", "/v1/trading/fills"]),
    fetchJsonCandidates<RiskSummary>(["/api/v1/risk/summary"]),
    fetchJsonCandidates<RiskDenialRow[]>(["/api/v1/risk/denials"]),
    fetchJsonCandidates<ReconcileSummary>(["/api/v1/reconcile/status"]),
    fetchJsonCandidates<ReconcileMismatchRow[]>(["/api/v1/reconcile/mismatches"]),
    fetchJsonCandidates<StrategyRow[]>(["/api/v1/strategy/summary"]),
    fetchJsonCandidates<OperatorAlert[]>(["/api/v1/alerts/active"]),
    fetchJsonCandidates<FeedEvent[]>(["/api/v1/events/feed"]),
    fetchJsonCandidates<AuditActionRow[]>(["/api/v1/audit/operator-actions"]),
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

  const daemonReachable = statusProbe.ok || healthProbe.ok || Boolean(legacyStatus) || probes.some((p) => p.ok);
  const connected = daemonReachable;

  const legacyOrdersResponse = executionOrdersR.ok && executionOrdersR.endpoint === "/v1/trading/orders"
    ? (executionOrdersR.data as LegacyTradingOrdersResponse)
    : openOrdersR.ok && openOrdersR.endpoint === "/v1/trading/orders"
      ? (openOrdersR.data as LegacyTradingOrdersResponse)
      : null;

  const legacyPositionsResponse = positionsR.ok && positionsR.endpoint === "/v1/trading/positions"
    ? (positionsR.data as LegacyTradingPositionsResponse)
    : null;

  const legacyAccountResponse = portfolioSummaryR.ok && portfolioSummaryR.endpoint === "/v1/trading/account"
    ? (portfolioSummaryR.data as LegacyTradingAccountResponse)
    : null;

  const legacyFillsResponse = fillsR.ok && fillsR.endpoint === "/v1/trading/fills"
    ? (fillsR.data as LegacyTradingFillsResponse)
    : null;

  const legacyHealthResponse = healthProbe.ok && healthProbe.endpoint === "/v1/health"
    ? (healthProbe.data as LegacyHealthResponse)
    : null;

  const executionOrders = Array.isArray(executionOrdersR.data)
    ? executionOrdersR.data
    : mapLegacyTradingOrdersToExecutionOrders(legacyOrdersResponse) ?? MOCK_MODEL.executionOrders;

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

  const executionSummary =
    executionSummaryR.ok && executionSummaryR.data !== undefined
      ? executionSummaryR.data
      : deriveExecutionSummaryFromOrders(executionOrders);
  if (!executionSummary) usedMockSections.push("executionSummary");

  const portfolioSummary =
    portfolioSummaryR.ok && portfolioSummaryR.endpoint === "/api/v1/portfolio/summary" && portfolioSummaryR.data !== undefined
      ? (portfolioSummaryR.data as PortfolioSummary)
      : mapLegacyPortfolioSummary(legacyAccountResponse);
  if (!portfolioSummary) usedMockSections.push("portfolioSummary");

  const positions =
    positionsR.ok && Array.isArray(positionsR.data)
      ? (positionsR.data as PositionRow[])
      : mapLegacyPositionsResponse(legacyPositionsResponse);
  if (!positions) usedMockSections.push("positions");

  const openOrders =
    openOrdersR.ok && Array.isArray(openOrdersR.data)
      ? (openOrdersR.data as OpenOrderRow[])
      : mapLegacyTradingOrdersToOpenOrders(legacyOrdersResponse);
  if (!openOrders) usedMockSections.push("openOrders");

  const fills =
    fillsR.ok && Array.isArray(fillsR.data)
      ? (fillsR.data as FillRow[])
      : mapLegacyTradingFillsToRows(legacyFillsResponse);
  if (!fills) usedMockSections.push("fills");

  const metadata =
    healthProbe.ok && healthProbe.endpoint === "/api/v1/system/metadata" && healthProbe.data !== undefined
      ? (healthProbe.data as MetadataSummary)
      : mapLegacyHealthToMetadata(legacyHealthResponse);
  if (!metadata) usedMockSections.push("metadata");

  if (!selectedTimeline) usedMockSections.push("selectedTimeline");
  if (!executionTrace) usedMockSections.push("executionTrace");
  if (!executionReplay) usedMockSections.push("executionReplay");
  if (!executionChart) usedMockSections.push("executionChart");
  if (!causalityTrace) usedMockSections.push("causalityTrace");

  const dataSource = deriveDataSourceDetail({
    probeResults: [statusProbe, healthProbe, ...probes],
    usedMockSections,
    daemonReachable,
  });
  const panelSources = classifyPanelSources(dataSource);

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
    executionSummary: executionSummary ?? MOCK_MODEL.executionSummary,
    executionOrders,
    selectedTimeline: objectOrFallback(selectedTimeline, MOCK_MODEL.selectedTimeline),
    omsOverview: useObject("omsOverview", omsOverviewR, MOCK_MODEL.omsOverview),
    executionTrace: objectOrFallback(executionTrace, MOCK_MODEL.executionTrace),
    executionReplay: objectOrFallback(executionReplay, MOCK_MODEL.executionReplay),
    executionChart: objectOrFallback(executionChart, MOCK_MODEL.executionChart),
    causalityTrace: objectOrFallback(causalityTrace, MOCK_MODEL.causalityTrace),
    metrics: useObject("metrics", metricsR, MOCK_MODEL.metrics),
    portfolioSummary: portfolioSummary ?? MOCK_MODEL.portfolioSummary,
    positions: positions ?? MOCK_MODEL.positions,
    openOrders: openOrders ?? MOCK_MODEL.openOrders,
    fills: fills ?? MOCK_MODEL.fills,
    riskSummary: useObject("riskSummary", riskSummaryR, MOCK_MODEL.riskSummary),
    riskDenials: useArray("riskDenials", riskDenialsR, MOCK_MODEL.riskDenials),
    reconcileSummary: useObject("reconcileSummary", reconcileSummaryR, MOCK_MODEL.reconcileSummary),
    mismatches: useArray("mismatches", mismatchesR, MOCK_MODEL.mismatches),
    strategies: useArray("strategies", strategiesR, MOCK_MODEL.strategies),
    alerts: useArray("alerts", alertsR, MOCK_MODEL.alerts),
    feed: useArray("feed", feedR, MOCK_MODEL.feed),
    auditActions: useArray("auditActions", auditActionsR, MOCK_MODEL.auditActions),
    metadata: metadata ?? MOCK_MODEL.metadata,
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
    panelSources,
    connected,
    lastUpdatedAt: nowIso(),
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

function mapLegacyOperatorActionResponse(
  actionKey: string,
  response: EndpointPostResult<unknown>,
): OperatorActionReceipt | null {
  if (!response.ok) return null;

  const payload = response.data as Partial<OperatorActionReceipt & LegacyDaemonStatusSnapshot & LegacyIntegrityResponse> | undefined;
  if (!payload || typeof payload !== "object") {
    return {
      ok: true,
      action_key: actionKey,
      environment: "paper",
      live_routing_enabled: false,
      result_state: "accepted",
      warnings: ["Operator action completed but returned no JSON payload."],
      audit_reference: `${actionKey}-${Date.now()}`,
      blocking_failures: [],
    };
  }

  if ("action_key" in payload || "result_state" in payload) {
    return {
      ok: payload.ok ?? true,
      action_key: payload.action_key ?? actionKey,
      environment: payload.environment ?? "paper",
      live_routing_enabled: payload.live_routing_enabled ?? false,
      result_state: payload.result_state ?? "accepted",
      warnings: payload.warnings ?? [],
      audit_reference: payload.audit_reference ?? `${actionKey}-${Date.now()}`,
      blocking_failures: payload.blocking_failures ?? [],
      simulated: payload.simulated,
    };
  }

  if ("armed" in payload || "active_run_id" in payload || "state" in payload) {
    return {
      ok: true,
      action_key: actionKey,
      environment: "paper",
      live_routing_enabled: false,
      result_state: String(payload.state ?? "accepted"),
      warnings: [],
      audit_reference: `${actionKey}-${Date.now()}`,
      blocking_failures: [],
    };
  }

  return null;
}

function failedOperatorActionReceipt(
  actionKey: string,
  failure: EndpointPostResult<unknown>,
  targetEnvironment: SystemStatus["environment"] = "paper",
): OperatorActionReceipt {
  const blockingFailures: string[] = [];
  const warnings: string[] = [];
  let resultState = "unavailable";
  let simulated = true;

  if (failure.status === 401) {
    resultState = "unauthorized";
    simulated = false;
    blockingFailures.push("Daemon refused operator action: valid Bearer token required.");
  } else if (failure.status === 403) {
    resultState = "refused";
    simulated = false;
    blockingFailures.push("Daemon refused operator action at the gate.");
  } else if (failure.status === 404) {
    blockingFailures.push(`Operator action endpoint missing for ${actionKey}.`);
  } else if (failure.error) {
    blockingFailures.push(`Operator action failed: ${failure.error}`);
  } else {
    blockingFailures.push(`Operator action failed for ${actionKey}.`);
  }

  warnings.push(`Last attempted endpoint: ${failure.endpoint}`);

  return {
    ok: false,
    action_key: actionKey,
    environment: targetEnvironment,
    live_routing_enabled: false,
    result_state: resultState,
    warnings,
    audit_reference: `${actionKey}-${Date.now()}`,
    blocking_failures: blockingFailures,
    simulated,
  };
}

export async function invokeOperatorAction(
  actionKey: string,
  params: Record<string, unknown>,
): Promise<OperatorActionReceipt> {
  const response = await postJson<Partial<OperatorActionReceipt> | LegacyDaemonStatusSnapshot | LegacyIntegrityResponse>(
    ["/api/v1/ops/action", ...legacyActionPaths(actionKey)],
    { action_key: actionKey, ...params },
  );

  const mapped = mapLegacyOperatorActionResponse(actionKey, response);
  if (mapped) return mapped;

  return failedOperatorActionReceipt(actionKey, response);
}

export async function requestSystemModeTransition(
  targetMode: SystemStatus["environment"],
  reason: string,
): Promise<OperatorActionReceipt> {
  const response = await postJson<Partial<OperatorActionReceipt>>(
    ["/api/v1/ops/change-mode"],
    { target_mode: targetMode, reason },
  );

  if (response.ok && response.data) {
    const payload = response.data;
    return {
      ok: payload.ok ?? true,
      action_key: payload.action_key ?? "change-system-mode",
      environment: payload.environment ?? targetMode,
      live_routing_enabled: payload.live_routing_enabled ?? false,
      result_state: payload.result_state ?? "accepted",
      warnings: payload.warnings ?? [],
      audit_reference: payload.audit_reference ?? `audit-change-system-mode-${Date.now()}`,
      blocking_failures: payload.blocking_failures ?? [],
      simulated: payload.simulated,
    };
  }

  return failedOperatorActionReceipt("change-system-mode", response, targetMode);
}