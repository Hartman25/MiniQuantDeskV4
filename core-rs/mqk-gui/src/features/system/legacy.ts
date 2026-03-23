// core-rs/mqk-gui/src/features/system/legacy.ts
//
// Legacy protocol adapters, daemon response wrapper shapes, and data normalizers.
//
// Contains:
//   - Private interfaces for legacy daemon API shapes (LegacyTrading*, Daemon*)
//   - Canonical portfolio/risk/reconcile response wrappers (snapshot_state pattern)
//   - String parser/normalizer functions (parseNumber, normalizeSide, etc.)
//   - Legacy-to-GUI mapping functions (mapLegacy*, deriveExecutionSummaryFromOrders)
//   - Daemon catalog mapper (mapDaemonCatalog)
//   - Data source derivation (deriveDataSourceDetail)

import type { EndpointFetchResult } from "./http";
import type {
  ConfigDiffRow,
  DataSourceDetail,
  ExecutionOrderRow,
  ExecutionSummary,
  FillRow,
  OpenOrderRow,
  OperatorActionDefinition,
  PortfolioSummary,
  PositionRow,
  ReconcileMismatchRow,
  RiskDenialRow,
  StrategyRow,
  StrategySuppressionRow,
  SystemStatus,
} from "./types";

// ---------------------------------------------------------------------------
// Legacy daemon API shapes
// ---------------------------------------------------------------------------

export interface LegacyDaemonStatusSnapshot {
  daemon_uptime_secs: number;
  active_run_id: string | null;
  state: string;
  notes?: string | null;
  integrity_armed: boolean;
}

export interface LegacyTradingAccountResponse {
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

export interface LegacyTradingPositionsResponse {
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

export interface LegacyTradingOrdersResponse {
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

export interface LegacyTradingFillsResponse {
  // has_snapshot removed: DMON-04 contract.
  fills: LegacyTradingFill[];
}

// ---------------------------------------------------------------------------
// Canonical portfolio surfaces (snapshot_state pattern)
// ---------------------------------------------------------------------------

// Canonical portfolio surfaces (Cluster 2).  snapshot_state discriminates
// "active broker snapshot" from "no broker snapshot loaded" without HTTP
// status string matching.  GUI checks the typed field, not an error string.
export interface PortfolioPositionsResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: PositionRow[];
}

export interface PortfolioOpenOrdersResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: OpenOrderRow[];
}

export interface PortfolioFillsResponse {
  snapshot_state: "active" | "no_snapshot";
  captured_at_utc: string | null;
  rows: FillRow[];
}

// ---------------------------------------------------------------------------
// Canonical risk/reconcile response wrappers
// ---------------------------------------------------------------------------

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
export interface RiskDenialsResponse {
  truth_state: "active" | "active_session_only" | "no_snapshot" | "not_wired" | "durable_history";
  snapshot_at_utc: string | null;
  denials: RiskDenialRow[];
}


// Canonical reconcile mismatch detail surface.
//
// `rows` are live derived reconcile diffs, not durable mismatch-table records.
// The daemon only exposes them when current execution snapshot + broker snapshot
// detail truth is authoritative and consistent with reconcile status.
export interface ReconcileMismatchesResponse {
  truth_state: "active" | "no_snapshot" | "stale";
  snapshot_at_utc: string | null;
  rows: ReconcileMismatchRow[];
}

// Config-diff truth wrapper.
// "not_wired" = no durable config-diff persistence exists; rows is empty and not authoritative.
// "active"    = reserved for when durable tracking is wired (not currently returned).
export interface ConfigDiffsWrapper {
  truth_state: "not_wired" | "active";
  backend?: string | null;
  rows: ConfigDiffRow[];
}

// Strategy suppression truth wrapper (CC-02: now durable).
// "no_db"  = DB pool not configured; rows is empty and not authoritative.
//            GUI renders "unavailable" notice rather than "not wired" notice.
// "active" = DB present; rows are authoritative (empty = no suppressions).
export interface StrategySuppressionsWrapper {
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
export interface StrategySummaryWrapper {
  canonical_route?: string | null;
  backend?: string | null;
  truth_state: "not_wired" | "active";
  rows: StrategyRow[];
}

// ---------------------------------------------------------------------------
// Durable operator-history daemon wrapper types
// ---------------------------------------------------------------------------

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

export interface DaemonAuditActionsWrapper {
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

export interface DaemonArtifactsWrapper {
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

export interface DaemonOperatorTimelineWrapper {
  canonical_route: string;
  truth_state: DurableHistoryTruthState;
  backend: string;
  rows: DaemonTimelineRow[];
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

export interface DaemonActionCatalogResponse {
  canonical_route: string;
  actions: DaemonActionCatalogEntry[];
}

// ---------------------------------------------------------------------------
// Parser / normalizer functions
// ---------------------------------------------------------------------------

export function parseNumber(value: unknown): number {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

export function parseIsoTimestamp(value: unknown): string | null {
  if (typeof value !== "string" || !value) return null;
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

export function nowIso(): string {
  return new Date().toISOString();
}

export function ageMsFromTimestamp(value: unknown): number {
  const parsed = parseIsoTimestamp(value);
  if (!parsed) return 0;
  return Date.now() - new Date(parsed).getTime();
}

export function normalizeSide(value: unknown): "buy" | "sell" {
  if (value === "buy" || value === "long") return "buy";
  if (value === "sell" || value === "short") return "sell";
  return "buy";
}

export function normalizeOrderType(value: unknown): "market" | "limit" | "stop" | "stop_limit" {
  if (value === "market") return "market";
  if (value === "limit") return "limit";
  if (value === "stop") return "stop";
  if (value === "stop_limit") return "stop_limit";
  return "market";
}

export function normalizeOrderStatus(value: unknown): string {
  if (typeof value !== "string") return "unknown";
  return value.toLowerCase().replace(/[\s-]/g, "_");
}

export function isTerminalOrderStatus(status: string): boolean {
  return ["filled", "cancelled", "canceled", "rejected", "expired", "done_for_day", "replaced"].includes(status);
}

export function deriveExecutionStage(status: string): string {
  if (status.includes("new") || status.includes("pending") || status.includes("accepted")) return "Pending";
  if (status.includes("submit") || status.includes("dispatch")) return "Dispatching";
  if (status.includes("partial")) return "PartialFill";
  if (status.includes("fill")) return "Filled";
  if (status.includes("cancel")) return "Cancelled";
  if (status.includes("reject")) return "Rejected";
  return "Unknown";
}

export function mapLegacyStatusToSystemStatus(legacy: LegacyDaemonStatusSnapshot): SystemStatus {
  const stateStr = String(legacy.state ?? "").toLowerCase();
  const isRunning = stateStr.includes("run") || stateStr.includes("active");
  const isHalted = stateStr.includes("halt") || stateStr.includes("disarm");
  return {
    environment: "paper",
    runtime_status: isHalted ? "halted" : isRunning ? "running" : "idle",
    broker_status: "unknown",
    db_status: "unknown",
    market_data_health: "unknown",
    reconcile_status: "unknown",
    integrity_status: "unknown",
    audit_writer_status: "unknown",
    last_heartbeat: null,
    loop_latency_ms: null,
    active_account_id: legacy.active_run_id ?? null,
    config_profile: null,
    has_warning: false,
    has_critical: isHalted,
    strategy_armed: legacy.integrity_armed,
    execution_armed: legacy.integrity_armed,
    live_routing_enabled: false,
    kill_switch_active: false,
    risk_halt_active: false,
    integrity_halt_active: isHalted,
    daemon_reachable: true,
    broker_snapshot_source: "synthetic",
    alpaca_ws_continuity: "not_applicable",
    deployment_start_allowed: false,
    daemon_mode: "paper",
    adapter_id: "paper",
  };
}

export function legacyActionPaths(actionKey: string): string[] {
  switch (actionKey) {
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

// ---------------------------------------------------------------------------
// Legacy mapper functions
// ---------------------------------------------------------------------------

export function mapLegacyPositionsResponse(response: LegacyTradingPositionsResponse | null): PositionRow[] | null {
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

export function mapLegacyPortfolioSummary(
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

export function mapLegacyTradingOrdersToExecutionOrders(response: LegacyTradingOrdersResponse | null): ExecutionOrderRow[] | null {
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

export function mapLegacyTradingOrdersToOpenOrders(response: LegacyTradingOrdersResponse | null): OpenOrderRow[] | null {
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

export function mapLegacyTradingFillsToRows(response: LegacyTradingFillsResponse | null): FillRow[] | null {
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

export function deriveExecutionSummaryFromOrders(orders: ExecutionOrderRow[] | null): ExecutionSummary | null {
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

// ---------------------------------------------------------------------------
// Daemon catalog mapper
// ---------------------------------------------------------------------------

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

export function mapDaemonCatalog(response: DaemonActionCatalogResponse): OperatorActionDefinition[] {
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

// ---------------------------------------------------------------------------
// Data source derivation
// ---------------------------------------------------------------------------

export function deriveDataSourceDetail(args: {
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
