import { getDaemonUrl } from "../../config";
import { withClassifiedPanelSources } from "./sourceAuthority";
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
  OperatorActionDefinition,
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
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS } from "./types";

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

interface LegacyTradingAccountResponse {
  // has_snapshot removed: DMON-04 contract replaced it with snapshot_state + snapshot_captured_at_utc.
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
  // has_snapshot removed: DMON-04 contract.
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
  // has_snapshot removed: DMON-04 contract.
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
  // has_snapshot removed: DMON-04 contract.
  fills: LegacyTradingFill[];
}

interface LegacyIntegrityResponse {
  armed: boolean;
  active_run_id: string | null;
  state: string;
}

interface DaemonOperatorActionResponse {
  requested_action: string;
  accepted: boolean;
  disposition: string;
  warnings?: string[];
  environment?: SystemStatus["environment"];
  audit?: {
    audit_event_id?: string | null;
  };
}

// Matches the daemon's ActionCatalogEntry shape (snake_case from JSON).
interface DaemonActionCatalogEntry {
  action_key: string;
  label: string;
  level: number;
  description: string;
  requires_reason: boolean;
  confirm_text: string;
  enabled: boolean;
  disabled_reason?: string | null;
}

interface DaemonActionCatalogResponse {
  canonical_route: string;
  actions: DaemonActionCatalogEntry[];
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
  const base = { ...DEFAULT_STATUS };
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
      return ["/control/arm", "/v1/integrity/arm"];
    case "disarm-execution":
    case "disarm-strategy":
      return ["/control/disarm", "/v1/integrity/disarm"];
    default:
      return [];
  }
}

function objectOrFallback<T>(value: unknown, fallback: T): T {
  return value && typeof value === "object" ? (value as T) : fallback;
}

// Supported action keys the daemon can execute (matches ops_action dispatcher).
const DAEMON_SUPPORTED_ACTION_KEYS = new Set([
  "arm-execution",
  "arm-strategy",
  "disarm-execution",
  "disarm-strategy",
  "start-system",
  "stop-system",
  "kill-switch",
]);

function mapDaemonCatalog(response: DaemonActionCatalogResponse): OperatorActionDefinition[] {
  return response.actions
    .filter((entry) => DAEMON_SUPPORTED_ACTION_KEYS.has(entry.action_key))
    .map((entry) => ({
      action_key: entry.action_key as OperatorActionDefinition["action_key"],
      label: entry.label,
      level: Math.min(3, Math.max(0, entry.level)) as 0 | 1 | 2 | 3,
      description: entry.description,
      requiresReason: entry.requires_reason,
      confirmText: entry.confirm_text,
      enabled: entry.enabled,
      disabledReason: entry.disabled_reason ?? undefined,
      disabled: !entry.enabled,
    }));
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
          ? "Connected, but no tracked backend truth endpoints resolved."
          : state === "partial"
            ? "Mixed resolved and unresolved backend truth across panels."
            : "All tracked surfaces resolved from daemon endpoints.",
  };
}

export async function fetchOperatorModel(): Promise<SystemModel> {
  const statusProbe = await fetchJsonCandidates<SystemStatus | LegacyDaemonStatusSnapshot>(["/api/v1/system/status", "/v1/status"]);
  const healthProbe = await fetchJsonCandidates<MetadataSummary>(["/api/v1/system/metadata"]);

  // Extract legacy status only when the statusProbe itself resolved via the
  // legacy path.  Do NOT fire a second fetch — canonical is preferred and if
  // canonical failed the probe already tried the legacy path as a fallback.
  const statusCanonical = statusProbe.ok && statusProbe.endpoint === "/api/v1/system/status";
  const legacyStatusFromProbe: LegacyDaemonStatusSnapshot | null =
    statusProbe.ok && statusProbe.endpoint === "/v1/status"
      ? (statusProbe.data as LegacyDaemonStatusSnapshot)
      : null;

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
    // Canonical Action Catalog: daemon-authoritative action availability.
    fetchJsonCandidates<DaemonActionCatalogResponse>(["/api/v1/ops/catalog"]),
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
    actionCatalogR,
  ] = probes;

  const daemonReachable = statusProbe.ok || healthProbe.ok || Boolean(legacyStatusFromProbe) || probes.some((p) => p.ok);
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

  const executionOrdersCanonical = executionOrdersR.ok && Array.isArray(executionOrdersR.data);
  const executionOrders = executionOrdersCanonical
    ? (executionOrdersR.data as ExecutionOrderRow[])
    : mapLegacyTradingOrdersToExecutionOrders(legacyOrdersResponse) ?? [];

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

  // When system status resolved via legacy (/v1/status) instead of canonical
  // (/api/v1/system/status), mark it as non-canonical. "status" is in the ops
  // panel's placeholder evidence hints — this push ensures the ops authority
  // classification degrades to "placeholder" (or "mixed") so the truth gate
  // fires and the action catalog is not shown on approximate legacy data.
  if (!statusCanonical) usedMockSections.push("status");
  // executionOrders from legacy (/v1/trading/orders) has fabricated strategy_id and
  // derived execution_stage. Mark as non-canonical so the execution panel's authority
  // degrades to "mixed" rather than "runtime_memory".
  if (!executionOrdersCanonical) usedMockSections.push("executionOrders");

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
  // Push when canonical failed (even if derived summary is non-null from legacy orders).
  if (!executionSummary || !executionSummaryR.ok) usedMockSections.push("executionSummary");

  const portfolioSummaryCanonical =
    portfolioSummaryR.ok &&
    portfolioSummaryR.endpoint === "/api/v1/portfolio/summary" &&
    portfolioSummaryR.data !== undefined;
  const portfolioSummary = portfolioSummaryCanonical
    ? (portfolioSummaryR.data as PortfolioSummary)
    : mapLegacyPortfolioSummary(legacyAccountResponse);
  if (!portfolioSummary || !portfolioSummaryCanonical) usedMockSections.push("portfolioSummary");

  const positionsCanonical = positionsR.ok && Array.isArray(positionsR.data);
  const positions = positionsCanonical
    ? (positionsR.data as PositionRow[])
    : mapLegacyPositionsResponse(legacyPositionsResponse);
  if (!positions || !positionsCanonical) usedMockSections.push("positions");

  const openOrdersCanonical = openOrdersR.ok && Array.isArray(openOrdersR.data);
  const openOrders = openOrdersCanonical
    ? (openOrdersR.data as OpenOrderRow[])
    : mapLegacyTradingOrdersToOpenOrders(legacyOrdersResponse);
  if (!openOrders || !openOrdersCanonical) usedMockSections.push("openOrders");

  const fillsCanonical = fillsR.ok && Array.isArray(fillsR.data);
  const fills = fillsCanonical
    ? (fillsR.data as FillRow[])
    : mapLegacyTradingFillsToRows(legacyFillsResponse);
  if (!fills || !fillsCanonical) usedMockSections.push("fills");

  const metadata =
    healthProbe.ok && healthProbe.data !== undefined
      ? (healthProbe.data as MetadataSummary)
      : null;
  if (!metadata) usedMockSections.push("metadata");

  if (!selectedTimeline) usedMockSections.push("selectedTimeline");
  if (!executionTrace) usedMockSections.push("executionTrace");
  if (!executionReplay) usedMockSections.push("executionReplay");
  if (!executionChart) usedMockSections.push("executionChart");
  if (!causalityTrace) usedMockSections.push("causalityTrace");

  // Resolve action catalog BEFORE dataSource computation so a catalog endpoint failure
  // is visible in dataSource.mockSections and properly degrades the ops panel authority.
  const catalogResult = (() => {
    if (actionCatalogR.ok && actionCatalogR.data !== undefined) {
      const raw = actionCatalogR.data as DaemonActionCatalogResponse;
      if (raw.actions && Array.isArray(raw.actions)) {
        return mapDaemonCatalog(raw);
      }
    }
    return null;
  })();
  // null means the endpoint was unreachable or returned an invalid shape.
  // An empty array is valid and means no actions are currently available (e.g., all halted).
  if (catalogResult === null) {
    usedMockSections.push("actionCatalog");
  }
  const resolvedActionCatalog: OperatorActionDefinition[] = catalogResult ?? [];

  const dataSource = deriveDataSourceDetail({
    probeResults: [statusProbe, healthProbe, ...probes],
    usedMockSections,
    daemonReachable,
  });

  const unavailableStatus: SystemStatus = {
    ...DEFAULT_STATUS,
    daemon_reachable: connected,
  };
  const unavailablePreflight: PreflightStatus = {
    ...DEFAULT_PREFLIGHT,
    daemon_reachable: connected,
    blockers: connected ? ["Backend preflight truth unavailable"] : ["Daemon unreachable"],
  };
  const unavailableExecutionSummary: ExecutionSummary = {
    active_orders: 0,
    pending_orders: 0,
    dispatching_orders: 0,
    reject_count_today: 0,
    cancel_replace_count_today: 0,
    avg_ack_latency_ms: null,
    stuck_orders: 0,
  };
  const unavailableOmsOverview: OmsOverview = {
    total_active_orders: 0,
    stuck_orders: 0,
    missing_transition_orders: 0,
    state_nodes: [],
    transition_edges: [],
    orders: [],
  };
  const unavailableMetrics: SystemMetrics = {
    runtime: { key: "runtime", title: "Runtime", description: "Backend truth unavailable", series: [] },
    execution: { key: "execution", title: "Execution", description: "Backend truth unavailable", series: [] },
    fillQuality: { key: "fill_quality", title: "Fill Quality", description: "Backend truth unavailable", series: [] },
    reconciliation: { key: "reconciliation", title: "Reconciliation", description: "Backend truth unavailable", series: [] },
    riskSafety: { key: "risk_safety", title: "Risk/Safety", description: "Backend truth unavailable", series: [] },
  };
  const unavailablePortfolioSummary: PortfolioSummary = {
    account_equity: 0,
    cash: 0,
    long_market_value: 0,
    short_market_value: 0,
    daily_pnl: 0,
    buying_power: 0,
  };
  const unavailableRiskSummary: RiskSummary = {
    gross_exposure: 0,
    net_exposure: 0,
    concentration_pct: 0,
    daily_pnl: 0,
    drawdown_pct: 0,
    loss_limit_utilization_pct: 0,
    kill_switch_active: false,
    active_breaches: 0,
  };
  const unavailableReconcileSummary: ReconcileSummary = {
    status: "unknown",
    last_run_at: null,
    mismatched_positions: 0,
    mismatched_orders: 0,
    mismatched_fills: 0,
    unmatched_broker_events: 0,
  };
  const unavailableMetadata: MetadataSummary = {
    build_version: "unknown",
    api_version: "unknown",
    broker_adapter: "unknown",
    endpoint_status: "unknown",
  };
  const unavailableTopology: ServiceTopology = { updated_at: nowIso(), services: [] };
  const unavailableTransport: TransportSummary = {
    outbox_depth: 0,
    inbox_depth: 0,
    max_claim_age_ms: 0,
    dispatch_retries: 0,
    orphaned_claims: 0,
    duplicate_inbox_events: 0,
    queues: [],
  };
  const unavailableSessionState: SessionStateSummary = {
    market_session: "closed",
    exchange_calendar_state: "closed",
    system_trading_window: "disabled",
    strategy_allowed: false,
    next_session_change_at: null,
    notes: connected ? ["Backend session truth unavailable"] : ["Daemon unreachable"],
  };
  const unavailableConfigFingerprint: ConfigFingerprintSummary = {
    config_hash: "unknown",
    risk_policy_version: "unknown",
    strategy_bundle_version: "unknown",
    build_version: "unknown",
    environment_profile: "unknown",
    runtime_generation_id: "unknown",
    last_restart_at: null,
  };
  const unavailableMarketDataQuality: MarketDataQualitySummary = {
    overall_health: "unknown",
    freshness_sla_ms: 0,
    stale_symbol_count: 0,
    missing_bar_count: 0,
    venue_disagreement_count: 0,
    strategy_blocks: 0,
    venues: [],
    issues: [],
  };
  const unavailableRuntimeLeadership: RuntimeLeadershipSummary = {
    leader_node: "unknown",
    leader_lease_state: "lost",
    generation_id: "unknown",
    restart_count_24h: 0,
    last_restart_at: null,
    // Use "in_progress" not "degraded": "degraded" in panelTruthRenderState
    // triggers a system-wide degraded overlay on every panel.  The fallback
    // represents missing truth, not a real degraded recovery state.
    post_restart_recovery_state: "in_progress",
    recovery_checkpoint: "unknown",
    checkpoints: [],
  };
  const unavailableArtifactRegistry: ArtifactRegistrySummary = {
    last_updated_at: null,
    ready_count: 0,
    pending_count: 0,
    failed_count: 0,
    artifacts: [],
  };

  const resolvedStatus: SystemStatus = objectOrFallback(
    statusProbe.ok && statusProbe.endpoint === "/api/v1/system/status"
      ? statusProbe.data
      // statusProbe already tried /v1/status as a fallback before giving up;
      // if it resolved there, use the legacy mapping.  No additional fetch.
      : legacyStatusFromProbe
        ? mapLegacyStatusToSystemStatus(legacyStatusFromProbe)
        : null,
    unavailableStatus,
  );

  return withClassifiedPanelSources({
    status: resolvedStatus,
    // Preflight is fail-closed: if the canonical endpoint did not return a
    // valid response, surface explicit unavailable state rather than a
    // silently-derived fake preflight with no blockers.
    preflight: objectOrFallback(
      preflightR.ok ? preflightR.data : null,
      unavailablePreflight,
    ),
    executionSummary: executionSummary ?? unavailableExecutionSummary,
    executionOrders,
    selectedTimeline,
    omsOverview: useObject("omsOverview", omsOverviewR, unavailableOmsOverview),
    executionTrace,
    executionReplay,
    executionChart,
    causalityTrace,
    metrics: useObject("metrics", metricsR, unavailableMetrics),
    portfolioSummary: portfolioSummary ?? unavailablePortfolioSummary,
    positions: positions ?? [],
    openOrders: openOrders ?? [],
    fills: fills ?? [],
    riskSummary: useObject("riskSummary", riskSummaryR, unavailableRiskSummary),
    riskDenials: useArray("riskDenials", riskDenialsR, []),
    reconcileSummary: useObject("reconcileSummary", reconcileSummaryR, unavailableReconcileSummary),
    mismatches: useArray("mismatches", mismatchesR, []),
    strategies: useArray("strategies", strategiesR, []),
    alerts: useArray("alerts", alertsR, []),
    feed: useArray("feed", feedR, []),
    auditActions: useArray("auditActions", auditActionsR, []),
    metadata: metadata ?? unavailableMetadata,
    topology: useObject("topology", topologyR, unavailableTopology),
    transport: useObject("transport", transportR, unavailableTransport),
    incidents: useArray("incidents", incidentsR, []),
    replaceCancelChains: useArray("replaceCancelChains", replaceCancelChainsR, []),
    alertTriage: useArray("alertTriage", alertTriageR, []),
    sessionState: useObject("sessionState", sessionStateR, unavailableSessionState),
    configFingerprint: useObject("configFingerprint", configFingerprintR, unavailableConfigFingerprint),
    marketDataQuality: useObject("marketDataQuality", marketDataQualityR, unavailableMarketDataQuality),
    runtimeLeadership: useObject("runtimeLeadership", runtimeLeadershipR, unavailableRuntimeLeadership),
    artifactRegistry: useObject("artifactRegistry", artifactRegistryR, unavailableArtifactRegistry),
    strategySuppressions: useArray("strategySuppressions", strategySuppressionsR, []),
    configDiffs: useArray("configDiffs", configDiffsR, []),
    operatorTimeline: useArray("operatorTimeline", operatorTimelineR, []),
    // Daemon-authoritative Action Catalog from GET /api/v1/ops/catalog.
    // Resolved before dataSource so catalog failures reach dataSource.mockSections
    // and degrade the ops panel truth authority correctly.
    actionCatalog: resolvedActionCatalog,
    dataSource,
    connected,
    lastUpdatedAt: nowIso(),
  });
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

  const payload = response.data as
    | Partial<OperatorActionReceipt & LegacyDaemonStatusSnapshot & LegacyIntegrityResponse>
    | DaemonOperatorActionResponse
    | undefined;
  if (!payload || typeof payload !== "object") {
    return {
      ok: true,
      action_key: actionKey,
      environment: "paper",
      live_routing_enabled: false,
      result_state: "accepted",
      warnings: ["Operator action completed but returned no JSON payload."],
      audit_reference: null,
      blocking_failures: [],
    };
  }

  if ("requested_action" in payload || "disposition" in payload) {
    const operatorPayload = payload as DaemonOperatorActionResponse;
    return {
      ok: operatorPayload.accepted ?? true,
      action_key: operatorPayload.requested_action ?? actionKey,
      environment: operatorPayload.environment ?? "paper",
      live_routing_enabled: false,
      result_state: operatorPayload.disposition ?? "accepted",
      warnings: operatorPayload.warnings ?? [],
      audit_reference: operatorPayload.audit?.audit_event_id ?? null,
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
      audit_reference: payload.audit_reference ?? null,
      blocking_failures: payload.blocking_failures ?? [],
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
      audit_reference: null,
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

  if (failure.status === 401) {
    resultState = "unauthorized";
    blockingFailures.push("Daemon refused operator action: valid Bearer token required.");
  } else if (failure.status === 403) {
    resultState = "refused";
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
    audit_reference: null,
    blocking_failures: blockingFailures,
  };
}

export async function invokeOperatorAction(
  actionKey: string,
  params: Record<string, unknown>,
): Promise<OperatorActionReceipt> {
  // Try the canonical dispatcher first. A 400/403/409 from canonical is a
  // definitive daemon decision and MUST NOT fall through to legacy paths.
  // Only fall back to legacy when canonical was unreachable (network error,
  // status === undefined) or explicitly absent (status === 404).
  const canonicalResult = await postJson<Partial<OperatorActionReceipt> | LegacyDaemonStatusSnapshot | LegacyIntegrityResponse>(
    ["/api/v1/ops/action"],
    { action_key: actionKey, ...params },
  );

  const canonicalDefinitive = canonicalResult.ok || (canonicalResult.status !== undefined && canonicalResult.status !== 404);
  if (canonicalDefinitive) {
    const mapped = mapLegacyOperatorActionResponse(actionKey, canonicalResult);
    if (mapped) return mapped;
    return failedOperatorActionReceipt(actionKey, canonicalResult);
  }

  // Canonical was not found (404) or was unreachable (no status = network error).
  // Fall back to legacy action paths for older daemon versions.
  const legacyPaths = legacyActionPaths(actionKey);
  if (legacyPaths.length === 0) {
    return failedOperatorActionReceipt(actionKey, canonicalResult);
  }

  const legacyResult = await postJson<Partial<OperatorActionReceipt> | LegacyDaemonStatusSnapshot | LegacyIntegrityResponse>(
    legacyPaths,
    { action_key: actionKey, ...params },
  );

  const mapped = mapLegacyOperatorActionResponse(actionKey, legacyResult);
  if (mapped) return mapped;
  return failedOperatorActionReceipt(actionKey, legacyResult);
}

// requestSystemModeTransition was removed (H-7 / PC-1):
// /api/v1/ops/change-mode is intentionally NOT mounted on the daemon.
// Mode transitions require a controlled restart with configuration reload.
// The change-system-mode action key returns 409 from /api/v1/ops/action
// as a defense-in-depth rejection. Callers were removed in H-7.
