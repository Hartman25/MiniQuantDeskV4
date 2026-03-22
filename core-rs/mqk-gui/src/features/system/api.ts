import { getDaemonUrl } from "../../config";
import { withClassifiedPanelSources } from "./sourceAuthority";
import type {
  AlertTriageRow,
  ArtifactRegistrySummary,
  ArtifactRow,
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
  ExplicitSurfaceTruth,
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
  OperatorTimelineCategory,
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

// Canonical portfolio surfaces (Cluster 2).  snapshot_state discriminates
// "active broker snapshot" from "no broker snapshot loaded" without HTTP
// status string matching.  GUI checks the typed field, not an error string.
interface PortfolioPositionsResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: PositionRow[];
}

interface PortfolioOpenOrdersResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: OpenOrderRow[];
}

interface PortfolioFillsResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: FillRow[];
}

// Canonical risk denial truth surface.
//
// truth_state values:
//   "no_snapshot"         — execution loop not running and no historical rows in DB;
//                           denial truth entirely absent. GUI IIFE emits ok:false →
//                           endpoint in missingEndpoints → panel blocks.
//   "active"              — execution loop running AND DB pool available. denials contains
//                           ONLY rows durably stored in sys_risk_denial_events. Restart-safe.
//                           denials: [] means no denial has ever been recorded in this deployment.
//   "active_session_only" — execution loop running but no DB pool (test environments only;
//                           never returned in production). denials from in-memory ring buffer
//                           only. NOT restart-safe.
//   "durable_history"     — execution loop not running but DB has historical rows from a prior
//                           session. denials is durably sourced. Restart-safe.
//   "not_wired"           — defensive guard only; not returned by current daemon but handled
//                           fail-closed in case of future partial-wiring edge cases.
interface RiskDenialsResponse {
  truth_state: "active" | "active_session_only" | "no_snapshot" | "not_wired" | "durable_history";
  snapshot_at_utc: string | null;
  denials: RiskDenialRow[];
}


// Canonical reconcile mismatch detail surface.
//
// `rows` are live derived reconcile diffs, not durable mismatch-table records.
// The daemon only exposes them when current execution snapshot + broker snapshot
// detail truth is authoritative and consistent with reconcile status.
interface ReconcileMismatchesResponse {
  truth_state: "active" | "no_snapshot" | "stale";
  snapshot_at_utc: string | null;
  rows: ReconcileMismatchRow[];
}

// Config-diff truth wrapper.
// "not_wired" = no durable config-diff persistence exists; rows is empty and not authoritative.
// "active"    = reserved for when durable tracking is wired (not currently returned).
interface ConfigDiffsWrapper {
  truth_state: "not_wired" | "active";
  backend?: string | null;
  rows: ConfigDiffRow[];
}

// Strategy suppression truth wrapper (CC-02: now durable).
// "no_db"  = DB pool not configured; rows is empty and not authoritative.
//            GUI renders "unavailable" notice rather than "not wired" notice.
// "active" = DB present; rows are authoritative (empty = no suppressions).
interface StrategySuppressionsWrapper {
  canonical_route?: string | null;
  backend?: string | null;
  truth_state: "no_db" | "active";
  rows: StrategySuppressionRow[];
}

// Strategy summary truth wrapper.
// "not_wired" = no real strategy-fleet registry exists; rows is empty and not authoritative.
//   The former synthetic "daemon_integrity_gate" surrogate row has been removed at the
//   daemon layer; this wrapper prevents any future bare-array regression from silently
//   re-introducing fake strategy rows.
// "active"    = reserved for when a real strategy-fleet source is wired.
interface StrategySummaryWrapper {
  canonical_route?: string | null;
  backend?: string | null;
  truth_state: "not_wired" | "active";
  rows: StrategyRow[];
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

// Durable operator-history daemon wrapper types.
// These three endpoints return {canonical_route, truth_state, backend, rows} —
// not direct arrays or GUI-typed objects. The fetch/map layer below unwraps
// the wrapper and maps daemon field names to GUI type field names. Only fields
// provably present in the daemon DB sources are populated; no values are fabricated.

interface DaemonAuditActionRow {
  audit_event_id: string;
  ts_utc: string;
  requested_action: string;
  disposition: string;
  run_id: string | null;
  runtime_transition: string | null;
  provenance_ref: string;
}

type DurableHistoryTruthState = "active" | "backend_unavailable";

interface DaemonAuditActionsWrapper {
  canonical_route: string;
  truth_state: DurableHistoryTruthState;
  backend: string;
  rows: DaemonAuditActionRow[];
}

interface DaemonArtifactRow {
  artifact_id: string;
  artifact_type: string;
  run_id: string;
  created_at_utc: string;
  provenance_ref: string;
}

interface DaemonArtifactsWrapper {
  canonical_route: string;
  truth_state: DurableHistoryTruthState;
  backend: string;
  rows: DaemonArtifactRow[];
}

interface DaemonTimelineRow {
  ts_utc: string;
  kind: string;
  run_id: string | null;
  detail: string;
  provenance_ref: string;
}

interface DaemonOperatorTimelineWrapper {
  canonical_route: string;
  truth_state: DurableHistoryTruthState;
  backend: string;
  rows: DaemonTimelineRow[];
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
    // AP-09: Legacy daemon (/v1/status) does not send external-broker truth fields.
    // Default to synthetic/not_applicable (paper-only assumption) so the external
    // broker continuity gate in truthRendering does not fire on legacy status.
    broker_snapshot_source: "synthetic" as const,
    alpaca_ws_continuity: "not_applicable" as const,
    deployment_start_allowed: false,
    daemon_mode: "paper",
    adapter_id: "paper",
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
      // Legacy path: side sourced from execution order row; fall back to "unknown" if absent.
      side: order.side ?? "unknown",
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
  const stuckOrders = activeOrders.filter((order) => (order.age_ms ?? 0) >= 300_000);

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
    // Execution orders: two-step fetch to distinguish "no snapshot" from "not mounted".
    // HTTP 503 = OMS loop has no snapshot → keep canonical path as missing (record in
    //   missingEndpoints) so isMissingPanelTruth fires and the execution panel blocks.
    //   Do NOT fall through to legacy broker orders — that would hide the missing truth.
    // HTTP 404 / network error = canonical not mounted → fall through to /v1/trading/orders.
    (async (): Promise<EndpointFetchResult<ExecutionOrderRow[] | LegacyTradingOrdersResponse>> => {
      const canonical = await fetchJsonCandidate<ExecutionOrderRow[]>("/api/v1/execution/orders");
      if (canonical.ok) return canonical;
      // 503 = explicit no-snapshot signal; keep canonical as the failed probe result.
      if (canonical.error === "HTTP 503") return canonical;
      // Any other failure (404 = unmounted, network error) → try legacy path.
      return fetchJsonCandidate<LegacyTradingOrdersResponse>("/v1/trading/orders");
    })(),
    fetchJsonCandidates<OmsOverview>(["/api/v1/oms/overview"]),
    fetchJsonCandidates<SystemMetrics>(["/api/v1/metrics/dashboards"]),
    fetchJsonCandidates<PortfolioSummary | LegacyTradingAccountResponse>(["/api/v1/portfolio/summary", "/v1/trading/account"]),
    // Portfolio positions: canonical route returns snapshot_state wrapper.
    // "active" → rows are real broker truth; "no_snapshot" → broker snapshot absent.
    // no_snapshot is returned as a failed probe (ok: false) so the endpoint lands in
    // missingEndpoints and isMissingPanelTruth fires, blocking the portfolio panel.
    // HTTP failure (404 = unmounted, network error) → fall through to legacy.
    (async (): Promise<EndpointFetchResult<PortfolioPositionsResponse | LegacyTradingPositionsResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioPositionsResponse>("/api/v1/portfolio/positions");
      if (canonical.ok) {
        if ((canonical.data as PortfolioPositionsResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingPositionsResponse>("/v1/trading/positions");
    })(),
    // Portfolio open orders: same snapshot_state pattern as positions.
    (async (): Promise<EndpointFetchResult<PortfolioOpenOrdersResponse | LegacyTradingOrdersResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioOpenOrdersResponse>("/api/v1/portfolio/orders/open");
      if (canonical.ok) {
        if ((canonical.data as PortfolioOpenOrdersResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingOrdersResponse>("/v1/trading/orders");
    })(),
    // Portfolio fills: same snapshot_state pattern as positions.
    (async (): Promise<EndpointFetchResult<PortfolioFillsResponse | LegacyTradingFillsResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioFillsResponse>("/api/v1/portfolio/fills");
      if (canonical.ok) {
        if ((canonical.data as PortfolioFillsResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingFillsResponse>("/v1/trading/fills");
    })(),
    fetchJsonCandidates<RiskSummary>(["/api/v1/risk/summary"]),
    // Risk denials: truth_state === "no_snapshot" means the execution loop is not
    // running — denial truth is unavailable and must not render as "zero denials."
    // The IIFE returns ok: false in that case so the endpoint lands in
    // missingEndpoints and isMissingPanelTruth fires for the risk panel.
    // No legacy fallback: there is no pre-canonical denial surface.
    (async (): Promise<EndpointFetchResult<RiskDenialRow[]>> => {
      const canonical = await fetchJsonCandidate<RiskDenialsResponse>("/api/v1/risk/denials");
      if (!canonical.ok) {
        // HTTP failure (404 = unmounted, network error).
        return { ok: false, endpoint: canonical.endpoint, error: canonical.error ?? "fetch_failed" };
      }
      const data = canonical.data as RiskDenialsResponse;
      if (data.truth_state === "no_snapshot" || data.truth_state === "not_wired") {
        // "no_snapshot": execution loop not running — denial truth entirely absent.
        // "not_wired":   execution loop running but denial accumulator not yet
        //   implemented; [] would falsely claim authoritative zero denials.
        // Both states are fail-closed: emit as failed probe so endpoint lands in
        // missingEndpoints and isMissingPanelTruth fires → risk panel blocks.
        return { ok: false, endpoint: canonical.endpoint, error: "no_denial_truth" };
      }
      // Only "active" (future: denial accumulator wired and proven) passes through.
      return { ok: true, endpoint: canonical.endpoint, data: data.denials };
    })(),
    fetchJsonCandidates<ReconcileSummary>(["/api/v1/reconcile/status"]),
    (async (): Promise<EndpointFetchResult<ReconcileMismatchRow[]>> => {
      const canonical = await fetchJsonCandidate<ReconcileMismatchesResponse>("/api/v1/reconcile/mismatches");
      if (!canonical.ok) {
        return { ok: false, endpoint: canonical.endpoint, error: canonical.error ?? "fetch_failed" };
      }
      const data = canonical.data as ReconcileMismatchesResponse;
      // Reconcile mismatch rows are a live derived truth surface, not a durable
      // table read. `no_snapshot` / `stale` must therefore fail closed so empty
      // rows never masquerade as authoritative zero mismatches.
      if (data.truth_state === "no_snapshot" || data.truth_state === "stale") {
        return { ok: false, endpoint: canonical.endpoint, error: "no_reconcile_detail_truth" };
      }
      return { ok: true, endpoint: canonical.endpoint, data: data.rows };
    })(),
    // strategy/summary: preserve the daemon wrapper so the GUI can distinguish
    // fail-closed "not_wired" truth from authoritative active-empty rows.
    (async (): Promise<EndpointFetchResult<StrategySummaryWrapper>> => {
      const r = await fetchJsonCandidate<StrategySummaryWrapper>("/api/v1/strategy/summary");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      return { ok: true, endpoint: r.endpoint, data: r.data as StrategySummaryWrapper };
    })(),
    fetchJsonCandidates<OperatorAlert[]>(["/api/v1/alerts/active"]),
    fetchJsonCandidates<FeedEvent[]>(["/api/v1/events/feed"]),
    // audit/operator-actions: daemon returns {canonical_route, truth_state, backend, rows}.
    // "backend_unavailable" means durable operator-action history is unavailable and
    // must fail closed rather than render as authoritative empty history.
    (async (): Promise<EndpointFetchResult<AuditActionRow[]>> => {
      const r = await fetchJsonCandidate<DaemonAuditActionsWrapper>("/api/v1/audit/operator-actions");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonAuditActionsWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_history_backend_unavailable" };
      }

      const rows: AuditActionRow[] = wrapper.rows.map((row) => ({
        audit_ref: row.audit_event_id,
        at: row.ts_utc,
        action_key: row.requested_action,
        result_state: row.disposition,
        // target_scope: use run_id when present (durable run reference), else provenance_ref.
        target_scope: row.run_id ?? row.provenance_ref,
        warnings: [],
        // actor and environment are not available from audit_events per-row; omitted.
      }));
      return { ok: true, endpoint: r.endpoint, data: rows };
    })(),
    fetchJsonCandidates<ServiceTopology>(["/api/v1/system/topology"]),
    fetchJsonCandidates<TransportSummary>(["/api/v1/execution/transport"]),
    fetchJsonCandidates<IncidentCase[]>(["/api/v1/incidents"]),
    fetchJsonCandidates<ReplaceCancelChainRow[]>(["/api/v1/execution/replace-cancel-chains"]),
    fetchJsonCandidates<AlertTriageRow[]>(["/api/v1/alerts/triage"]),
    fetchJsonCandidates<SessionStateSummary>(["/api/v1/system/session"]),
    fetchJsonCandidates<ConfigFingerprintSummary>(["/api/v1/system/config-fingerprint"]),
    fetchJsonCandidates<MarketDataQualitySummary>(["/api/v1/market-data/quality"]),
    fetchJsonCandidates<RuntimeLeadershipSummary>(["/api/v1/system/runtime-leadership"]),
    // audit/artifacts: daemon returns {canonical_route, truth_state, backend, rows}
    // where each row is one run from the runs table. "backend_unavailable" means
    // durable artifact history is unavailable and must fail closed.
    (async (): Promise<EndpointFetchResult<ArtifactRegistrySummary>> => {
      const r = await fetchJsonCandidate<DaemonArtifactsWrapper>("/api/v1/audit/artifacts");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonArtifactsWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_artifact_backend_unavailable" };
      }

      const artifacts: ArtifactRow[] = wrapper.rows.map((row) => ({
        artifact_id: row.artifact_id,
        artifact_type: row.artifact_type as ArtifactRow["artifact_type"],
        created_at: row.created_at_utc,
        status: "ready" as const,
        linked_order_id: null,
        linked_incident_id: null,
        linked_run_id: row.run_id,
        // storage_path and note are not available from the runs-table artifact source.
      }));

      // last_updated_at: newest artifact created_at (rows are already desc by started_at_utc).
      const lastUpdatedAt = artifacts.length > 0 ? artifacts[0].created_at : null;
      const summary: ArtifactRegistrySummary = {
        last_updated_at: lastUpdatedAt,
        ready_count: artifacts.length,
        pending_count: 0,
        failed_count: 0,
        artifacts,
      };
      return { ok: true, endpoint: r.endpoint, data: summary };
    })(),
    // strategy/suppressions: preserve the daemon wrapper so the GUI can render
    // mounted-but-not-wired truth distinctly from authoritative active-empty rows.
    (async (): Promise<EndpointFetchResult<StrategySuppressionsWrapper>> => {
      const r = await fetchJsonCandidate<StrategySuppressionsWrapper>("/api/v1/strategy/suppressions");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      return { ok: true, endpoint: r.endpoint, data: r.data as StrategySuppressionsWrapper };
    })(),
    // system/config-diffs: preserve the daemon wrapper so the GUI can render
    // mounted-but-not-wired truth distinctly from authoritative active-empty rows.
    (async (): Promise<EndpointFetchResult<ConfigDiffsWrapper>> => {
      const r = await fetchJsonCandidate<ConfigDiffsWrapper>("/api/v1/system/config-diffs");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      return { ok: true, endpoint: r.endpoint, data: r.data as ConfigDiffsWrapper };
    })(),
    // ops/operator-timeline: daemon returns {canonical_route, truth_state, backend, rows}
    // where each row is a runtime lifecycle transition or operator action from runs +
    // audit_events. "backend_unavailable" means durable operator timeline truth is
    // unavailable and must fail closed.
    (async (): Promise<EndpointFetchResult<OperatorTimelineEvent[]>> => {
      const r = await fetchJsonCandidate<DaemonOperatorTimelineWrapper>("/api/v1/ops/operator-timeline");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonOperatorTimelineWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_timeline_backend_unavailable" };
      }

      const events: OperatorTimelineEvent[] = wrapper.rows.map((row) => ({
        // provenance_ref is the durable DB reference (e.g. "runs:{id}:started_at_utc"
        // or "audit_events:{id}"); use as the stable event identity.
        timeline_event_id: row.provenance_ref,
        at: row.ts_utc,
        // "runtime_transition" and "operator_action" are the two kinds emitted by the daemon.
        category: row.kind as OperatorTimelineCategory,
        // "info" is the correct baseline severity for lifecycle and operator-action events;
        // no severity escalation data exists in the durable DB sources.
        severity: "info" as const,
        title: row.detail,
        summary: row.detail,
        // actor: not available in runs or audit_events per-row; omitted (optional field).
        linked_incident_id: null,
        linked_order_id: null,
        linked_strategy_id: null,
        linked_action_key: null,
        linked_config_diff_id: null,
        linked_runtime_generation_id: row.run_id ?? null,
      }));
      return { ok: true, endpoint: r.endpoint, data: events };
    })(),
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

  // HTTP 503 from /api/v1/execution/orders = explicit no-snapshot signal.
  // In that case the probe stays as { ok: false, endpoint: "/api/v1/execution/orders" }
  // so the endpoint lands in missingEndpoints, isMissingPanelTruth fires, and the
  // execution panel blocks.  Do NOT use legacy orders to fill this gap.
  const executionOrdersIsNoSnapshot = !executionOrdersR.ok && executionOrdersR.error === "HTTP 503";
  const executionOrdersCanonical = executionOrdersR.ok && Array.isArray(executionOrdersR.data);
  const executionOrders = executionOrdersCanonical
    ? (executionOrdersR.data as ExecutionOrderRow[])
    : executionOrdersIsNoSnapshot
      ? []  // No-snapshot signal: return empty rather than misleading legacy broker orders.
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
  // executionOrders non-canonical in two cases:
  //   1. Legacy path used (/v1/trading/orders): fabricated strategy_id/stage → "mixed" authority.
  //   2. No-snapshot signal (HTTP 503): canonical endpoint in missingEndpoints; "executionOrders"
  //      in mockSections. Combined with missingEndpoints, isMissingPanelTruth fires for the
  //      execution panel → no_snapshot gate blocks the screen.
  if (!executionOrdersCanonical) usedMockSections.push("executionOrders");

  const durableOperatorHistoryKeys = new Set(["auditActions", "artifactRegistry", "operatorTimeline"]);

  const explicitSurfaceTruthOrUnknown = (
    result: EndpointFetchResult<{ truth_state: "active" | "not_wired" | "no_db"; backend?: string | null }>,
  ): ExplicitSurfaceTruth => {
    if (!result.ok || result.data == null) {
      return { truth_state: "unknown", backend: null };
    }
    return {
      truth_state: result.data.truth_state,
      backend: result.data.backend ?? null,
    };
  };

  const useObject = <T,>(key: string, result: EndpointFetchResult<T>, fallback: T): T => {
    if (result.ok && result.data !== undefined) return result.data;

    // Durable operator-history endpoints are mounted but can explicitly report
    // backend_unavailable. That condition must surface through missingEndpoints
    // as unavailable durable truth, not be relabeled as placeholder/mock wiring.
    if (!durableOperatorHistoryKeys.has(key)) {
      usedMockSections.push(key);
    }

    return fallback;
  };
  const useArray = <T,>(key: string, result: EndpointFetchResult<T[]>, fallback: T[]): T[] => {
    if (result.ok && Array.isArray(result.data)) return result.data;

    // Same rule as useObject above: missing durable operator history is a
    // fail-closed truth gap, not a placeholder surface.
    if (!durableOperatorHistoryKeys.has(key)) {
      usedMockSections.push(key);
    }

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

  // Canonical portfolio routes return a structured wrapper with snapshot_state.
  // "active" = broker snapshot present; rows are authoritative (may be empty).
  // "no_snapshot" = broker snapshot absent; rows are empty, must not be read as truth.
  // The check is on the typed snapshot_state field — not HTTP status string matching.
  const positionsIsCanonical =
    positionsR.ok && positionsR.endpoint === "/api/v1/portfolio/positions";
  const positionsIsActive =
    positionsIsCanonical &&
    (positionsR.data as PortfolioPositionsResponse).snapshot_state === "active";
  const positions = positionsIsActive
    ? (positionsR.data as PortfolioPositionsResponse).rows
    : mapLegacyPositionsResponse(legacyPositionsResponse);
  if (!positionsIsActive) usedMockSections.push("positions");

  const openOrdersIsCanonical =
    openOrdersR.ok && openOrdersR.endpoint === "/api/v1/portfolio/orders/open";
  const openOrdersIsActive =
    openOrdersIsCanonical &&
    (openOrdersR.data as PortfolioOpenOrdersResponse).snapshot_state === "active";
  const openOrders = openOrdersIsActive
    ? (openOrdersR.data as PortfolioOpenOrdersResponse).rows
    : mapLegacyTradingOrdersToOpenOrders(legacyOrdersResponse);
  if (!openOrdersIsActive) usedMockSections.push("openOrders");

  const fillsIsCanonical =
    fillsR.ok && fillsR.endpoint === "/api/v1/portfolio/fills";
  const fillsIsActive =
    fillsIsCanonical &&
    (fillsR.data as PortfolioFillsResponse).snapshot_state === "active";
  const fills = fillsIsActive
    ? (fillsR.data as PortfolioFillsResponse).rows
    : mapLegacyTradingFillsToRows(legacyFillsResponse);
  if (!fillsIsActive) usedMockSections.push("fills");

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

  const strategySummaryTruth = explicitSurfaceTruthOrUnknown(strategiesR as EndpointFetchResult<StrategySummaryWrapper>);
  const strategySuppressionsTruth = explicitSurfaceTruthOrUnknown(strategySuppressionsR as EndpointFetchResult<StrategySuppressionsWrapper>);
  const configDiffsTruth = explicitSurfaceTruthOrUnknown(configDiffsR as EndpointFetchResult<ConfigDiffsWrapper>);

  const strategies = strategiesR.ok && strategiesR.data !== undefined
    ? (strategiesR.data as StrategySummaryWrapper).rows
    : [];
  const strategySuppressions = strategySuppressionsR.ok && strategySuppressionsR.data !== undefined
    ? (strategySuppressionsR.data as StrategySuppressionsWrapper).rows
    : [];
  const configDiffs = configDiffsR.ok && configDiffsR.data !== undefined
    ? (configDiffsR.data as ConfigDiffsWrapper).rows
    : [];
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
    // AP-09: deployment identity fields are absent when daemon session truth is
    // unavailable.  Leave optional fields undefined rather than asserting stale
    // values — the caller must not infer mode/adapter from unavailable state.
    deployment_start_allowed: false,
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
    restart_count_24h: null, // unavailable: daemon not reachable, no authoritative count
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
    strategies,
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
    strategySummaryTruth,
    strategySuppressionsTruth,
    configDiffsTruth,
    strategySuppressions,
    configDiffs,
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
