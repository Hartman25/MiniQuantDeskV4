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
  AdmissionCheckSurface,
  AutonomousPaperStatusSurface,
  ConfigDiffRow,
  DataSourceDetail,
  ExecutionOrderRow,
  ExecutionOutboxSurface,
  ExecutionSummary,
  FeedEvent,
  FillQualitySurface,
  FillRow,
  ModeChangeGuidanceResponse,
  OpenOrderRow,
  OperatorActionDefinition,
  OperatorAlert,
  PaperJournalSurface,
  PaperJournalTruthState,
  PortfolioSummary,
  PositionRow,
  ReconcileMismatchRow,
  RiskDenialRow,
  Severity,
  StrategyRow,
  StrategySuppressionRow,
  SystemStatus,
  WatchlistStatusSurface,
} from "./types";
import type { OrderTimelineSurface } from "./types/execution";

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

// Strategy summary truth wrapper (B2B: fleet × registry cross-reference).
// "no_db"     = DB unavailable; rows is empty and not authoritative (fail closed).
// "registry"  = DB present; rows are authoritative.  Includes synthetic rows for
//               fleet entries with no registry record (admission_state="blocked_not_registered").
// Legacy values retained for forward-compat guard in case of old daemon:
// "not_wired" | "active" — treated as stale; GUI falls back to fail-closed.
export interface StrategySummaryWrapper {
  canonical_route?: string | null;
  backend?: string | null;
  truth_state: "no_db" | "registry" | "not_wired" | "active";
  /** B2B: "single_strategy" | "fleet_not_configured" | "fleet" | "unknown" */
  runtime_execution_mode?: string | null;
  /** B2B: number of configured fleet entries; null when truth_state == "no_db" */
  configured_fleet_size?: number | null;
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
    autonomous_signal_count: null,
    autonomous_signal_limit_hit: null,
    asset_class_scope: "equity_only",
    // C1: legacy path has no parity evidence; default to not_configured.
    parity_evidence_state: "not_configured",
    live_trust_complete: null,
    // DEADMAN-GUI-01: legacy path has no deadman state; default to unknown.
    deadman_status: "unknown",
    deadman_last_heartbeat_utc: null,
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

// Error codes that indicate a truthful "pre-run / no snapshot" response (HTTP 200 + no_snapshot
// or equivalent) — NOT a missing/unreachable route.
const NO_SNAPSHOT_ERRORS = new Set([
  "no_broker_snapshot",       // portfolio/positions, portfolio/orders/open, portfolio/fills
  "no_denial_truth",          // risk/denials when execution loop not running
  "no_reconcile_detail_truth", // reconcile/mismatches when no snapshot
]);

export function deriveDataSourceDetail(args: {
  probeResults: EndpointFetchResult<unknown>[];
  usedMockSections: string[];
  daemonReachable: boolean;
}): DataSourceDetail {
  const realEndpoints = args.probeResults.filter((r) => r.ok).map((r) => r.endpoint);
  const missingEndpoints = args.probeResults.filter((r) => !r.ok).map((r) => r.endpoint);
  // Categorize failed probes: no_snapshot (pre-run, route is alive and responding truthfully)
  // vs no_active_run (HTTP 503 = execution loop not started) vs genuinely unavailable.
  const noSnapshotEndpoints = args.probeResults
    .filter((r) => !r.ok && r.error != null && NO_SNAPSHOT_ERRORS.has(r.error))
    .map((r) => r.endpoint);
  const noActiveRunEndpoints = args.probeResults
    .filter((r) => !r.ok && r.error === "HTTP 503")
    .map((r) => r.endpoint);

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
    noSnapshotEndpoints,
    noActiveRunEndpoints,
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

// ---------------------------------------------------------------------------
// OBS-SESSION-DISCORD-01: Autonomous readiness partial shape (session-window diagnostics).
// Only the session-window fields consumed by the GUI are typed here.
// truth_state === "active" for paper+alpaca; "not_applicable" otherwise.
// ---------------------------------------------------------------------------

export interface AutonomousReadinessPartial {
  truth_state: string;
  session_in_window: boolean;
  session_window_state: string;
  now_utc: string;
  session_start_utc: string | null;
  session_stop_utc: string | null;
  session_window_source: string;
  // STRATEGY-DECISION-OBSERVABILITY-01: bar dispatch and decision diagnostics.
  arm_state?: string;
  nyse_market_session?: string;
  bar_ticker_gate?: string;
  bar_tick_dispatch_count?: number | null;
  last_bar_signal_qty?: number | null;
  bar_context_source?: string;
  bar_context_bars_loaded?: number | null;
  blockers?: string[];
  overall_ready?: boolean;
  strategy_decision_diagnostics?: {
    strategy_id: string;
    symbol: string;
    timeframe: string;
    lookback_bars: number;
    threshold_bps: number;
    latest_bar_ts: number | null;
    latest_close_micros: number | null;
    lookback_bar_ts: number | null;
    lookback_close_micros: number | null;
    move_bps: number | null;
    abs_move_bps: number | null;
    gap_to_threshold_bps: number | null;
    raw_direction: number;
    decision: string;
    reason: string;
  } | null;
}

// ---------------------------------------------------------------------------
// GUI-OPS-01/02/03: Canonical daemon response wrapper types + mappers
// ---------------------------------------------------------------------------

// Active alerts response wrapper (CC-06).
// truth_state is always "active" — computed from live in-memory daemon state.
// Rows are ActiveAlertRow objects, not OperatorAlert directly.
export interface ActiveAlertsWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  alert_count: number;
  rows: Array<{
    alert_id: string;
    severity: string;
    class: string;
    summary: string;
    detail: string | null;
    source: string;
  }>;
}

// Events feed response wrapper (CC-06).
// truth_state: "active" (DB present) | "backend_unavailable" (no DB pool).
export interface EventsFeedWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  rows: Array<{
    event_id: string;
    ts_utc: string;
    kind: string;
    detail: string;
    run_id: string | null;
    // OPS-11: present (non-null string) for operator_action and signal_admission rows;
    // null/absent for runtime_transition and autonomous_session rows.
    audit_event_id?: string | null;
  }>;
}

// Per-order execution timeline response wrapper (A5A).
// truth_state: "active" | "no_fills_yet" | "no_order" | "no_db"
export interface DaemonOrderTimelineResponse {
  canonical_route: string;
  truth_state: string;
  backend: string;
  order_id: string;
  broker_order_id: string | null;
  symbol: string | null;
  requested_qty: number | null;
  filled_qty: number | null;
  current_status: string | null;
  current_stage: string | null;
  last_event_at: string | null;
  rows: Array<{
    event_id: string;
    ts_utc: string;
    stage: string;
    source: string;
    detail: string | null;
    fill_qty: number | null;
    fill_price_micros: number | null;
    slippage_bps: number | null;
    provenance_ref: string | null;
  }>;
}

// Execution outbox response wrapper (OPS-08 / EXEC-06).
// truth_state: "active" | "no_active_run" | "no_db"
export interface ExecutionOutboxWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  run_id: string | null;
  rows: Array<{
    idempotency_key: string;
    run_id: string;
    status: string;
    lifecycle_stage: string;
    symbol: string | null;
    side: string | null;
    qty: number | null;
    order_type: string | null;
    strategy_id: string | null;
    signal_source: string | null;
    created_at_utc: string;
    claimed_at_utc: string | null;
    dispatching_at_utc: string | null;
    sent_at_utc: string | null;
  }>;
}

// Fill quality telemetry response wrapper (TV-EXEC-01).
// truth_state: "active" | "no_active_run" | "no_db"
export interface FillQualityWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  rows: Array<{
    telemetry_id: string;
    run_id: string;
    internal_order_id: string;
    broker_order_id: string | null;
    symbol: string;
    side: string;
    ordered_qty: number;
    fill_qty: number;
    fill_price_micros: number;
    reference_price_micros: number | null;
    slippage_bps: number | null;
    fill_kind: string;
    fill_received_at_utc: string;
    submit_to_fill_ms: number | null;
  }>;
}

// Paper journal response wrapper (JOUR-01).
// Each lane has its own independent truth_state.
export interface PaperJournalWrapper {
  canonical_route: string;
  run_id: string | null;
  fills_lane: {
    truth_state: string;
    backend: string;
    rows: FillQualityWrapper["rows"];
  };
  admissions_lane: {
    truth_state: string;
    backend: string;
    rows: Array<{
      event_id: string;
      ts_utc: string;
      signal_id: string;
      strategy_id: string;
      symbol: string;
      side: string;
      qty: number;
      run_id: string;
    }>;
  };
}

// ---------------------------------------------------------------------------
// GUI-OPS-03: Active alerts → OperatorAlert mapper
// Exported for testability.
// ---------------------------------------------------------------------------

function deriveAlertDomain(faultClass: string): OperatorAlert["domain"] {
  const prefix = faultClass.split(".")[0] ?? "";
  switch (prefix) {
    case "risk": return "risk";
    case "reconcile": return "reconcile";
    case "execution": return "execution";
    case "integrity": return "integrity";
    case "portfolio": return "portfolio";
    case "strategy": return "strategy";
    case "oms": return "oms";
    case "audit": return "audit";
    case "metrics": return "metrics";
    default: return "system";
  }
}

export function mapActiveAlertsResponse(wrapper: ActiveAlertsWrapper): OperatorAlert[] {
  return (wrapper.rows ?? []).map((row) => ({
    id: row.alert_id,
    severity: row.severity as Severity,
    title: row.summary,
    message: row.detail ?? row.summary,
    domain: deriveAlertDomain(row.class),
  }));
}

// ---------------------------------------------------------------------------
// GUI-OPS-01: Events feed → FeedEvent mapper
// Exported for testability.
// ---------------------------------------------------------------------------

export function mapEventsFeedResponse(wrapper: EventsFeedWrapper): FeedEvent[] {
  return (wrapper.rows ?? []).map((row) => ({
    id: row.event_id,
    at: row.ts_utc,
    severity: "info" as const,
    source: row.kind,
    text: row.detail,
  }));
}

// ---------------------------------------------------------------------------
// GUI-OPS-02: Execution outbox wrapper → ExecutionOutboxSurface
// Extracted from api.ts IIFE so the mapping + truth canonicalization can be
// tested in isolation without React or the full model assembly.
// ---------------------------------------------------------------------------

const OUTBOX_VALID_STATES: ExecutionOutboxSurface["truth_state"][] = ["active", "no_active_run", "no_db"];

export function mapExecutionOutboxWrapper(wrapper: ExecutionOutboxWrapper | null | undefined): ExecutionOutboxSurface {
  if (wrapper == null) return { truth_state: "unavailable", run_id: null, rows: [] };
  const ts = OUTBOX_VALID_STATES.includes(wrapper.truth_state as ExecutionOutboxSurface["truth_state"])
    ? (wrapper.truth_state as ExecutionOutboxSurface["truth_state"])
    : "unavailable";
  return { truth_state: ts, run_id: wrapper.run_id ?? null, rows: wrapper.rows ?? [] };
}

// Returns a non-null notice string for every non-"active" state.
// Exported so screens can render honest lane notices and tests can prove fail-closed rendering.
export function executionOutboxNotice(surface: ExecutionOutboxSurface): string | null {
  switch (surface.truth_state) {
    case "active": return null;
    case "no_active_run": return "No active run — outbox history is unavailable until execution starts.";
    case "no_db": return "Outbox unavailable: no database pool configured. Do not treat empty rows as authoritative.";
    case "unavailable": return "Outbox endpoint unavailable. Do not treat empty rows as authoritative.";
  }
}

// ---------------------------------------------------------------------------
// GUI-OPS-02: Fill quality telemetry wrapper → FillQualitySurface
// ---------------------------------------------------------------------------

const FQ_VALID_STATES: FillQualitySurface["truth_state"][] = ["active", "no_active_run", "no_db"];

export function mapFillQualityWrapper(wrapper: FillQualityWrapper | null | undefined): FillQualitySurface {
  if (wrapper == null) return { truth_state: "unavailable", rows: [] };
  const ts = FQ_VALID_STATES.includes(wrapper.truth_state as FillQualitySurface["truth_state"])
    ? (wrapper.truth_state as FillQualitySurface["truth_state"])
    : "unavailable";
  return { truth_state: ts, rows: wrapper.rows ?? [] };
}

export function fillQualityNotice(surface: FillQualitySurface): string | null {
  switch (surface.truth_state) {
    case "active": return null;
    case "no_active_run": return "No active run — fill quality telemetry unavailable until execution starts.";
    case "no_db": return "Fill quality unavailable: no database pool configured.";
    case "unavailable": return "Fill quality endpoint unavailable.";
  }
}

// ---------------------------------------------------------------------------
// A5A: per-order execution timeline notice
// Returns a non-null notice string for every non-"active" state that carries
// unavailable or ambiguous truth. "no_fills_yet" and "no_order" are meaningful
// loaded states; "no_db" is an explicit unavailable-truth condition.
// ---------------------------------------------------------------------------

export function orderTimelineNotice(surface: OrderTimelineSurface): string | null {
  switch (surface.truth_state) {
    case "active": return null;
    case "no_fills_yet": return null;
    case "no_order": return null;
    case "no_db": return "Timeline unavailable: no database pool configured. Do not treat empty rows as authoritative.";
  }
}

// ---------------------------------------------------------------------------
// GUI-OPS-01: Paper journal wrapper → PaperJournalSurface
// Dual-lane: fills_lane and admissions_lane each carry an independent truth_state.
// ---------------------------------------------------------------------------

const PJ_VALID_STATES: PaperJournalSurface["fills_truth_state"][] = ["active", "no_active_run", "no_db"];

export function mapPaperJournalWrapper(wrapper: PaperJournalWrapper | null | undefined): PaperJournalSurface {
  if (wrapper == null) {
    return { run_id: null, fills_truth_state: "unavailable", fills: [], admissions_truth_state: "unavailable", admissions: [] };
  }
  const fts = PJ_VALID_STATES.includes(wrapper.fills_lane.truth_state as PaperJournalSurface["fills_truth_state"])
    ? (wrapper.fills_lane.truth_state as PaperJournalSurface["fills_truth_state"])
    : "unavailable";
  const ats = PJ_VALID_STATES.includes(wrapper.admissions_lane.truth_state as PaperJournalSurface["admissions_truth_state"])
    ? (wrapper.admissions_lane.truth_state as PaperJournalSurface["admissions_truth_state"])
    : "unavailable";
  return {
    run_id: wrapper.run_id ?? null,
    fills_truth_state: fts,
    fills: wrapper.fills_lane.rows ?? [],
    admissions_truth_state: ats,
    admissions: wrapper.admissions_lane.rows ?? [],
  };
}

// ---------------------------------------------------------------------------
// GUI-PAPER-STATUS-VISIBILITY-01: Autonomous paper status wrapper → surface
//
// GET /api/v1/autonomous/paper-status (PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01).
// Backend truth_state: "active" (paper+alpaca, all fields authoritative) |
// "not_applicable" (deployment is not paper+alpaca — every field still present
// but carries sentinel "not_applicable" values). Read-only visibility surface;
// never used to place, modify, or flatten orders.
// ---------------------------------------------------------------------------

export interface AutonomousPaperStatusWrapper {
  canonical_route: string;
  truth_state: string;
  mode: string;
  live_routing_enabled: boolean;
  runtime_status: string;
  arm_state: string;
  kill_switch_active: boolean;
  deadman_status: string;
  ws_continuity: string;
  reconcile_status: string;
  mismatch_count: number;
  open_order_count: number;
  position_count: number;
  current_symbol: string | null;
  current_position_qty: number | null;
  target_qty: number | null;
  computed_delta_qty: number | null;
  no_order_reason: string | null;
  last_strategy_decision: string | null;
  flatten_available: boolean;
  flatten_blockers: string[];
  watchlist_outcome: string;
  watchlist_approved: boolean;
  readiness_classification: string;
  blockers: string[];
  next_operator_action: string;
  autonomous_session_state: string;
  now_utc: string;
}

const AUTONOMOUS_PAPER_STATUS_VALID_TRUTH_STATES = new Set(["active", "not_applicable"]);

const UNAVAILABLE_AUTONOMOUS_PAPER_STATUS: AutonomousPaperStatusSurface = {
  truth_state: "unavailable",
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
};

// Read-only mapper: preserves the daemon's truth_state verbatim ("active" |
// "not_applicable"). Any other shape (probe failure, structurally invalid
// body, unrecognized truth_state) maps to the explicit "unavailable" sentinel
// — empty/zero fields must never be displayed as if they were authoritative.
export function mapAutonomousPaperStatusWrapper(
  wrapper: AutonomousPaperStatusWrapper | null | undefined,
): AutonomousPaperStatusSurface {
  if (wrapper == null || typeof wrapper.truth_state !== "string") {
    return UNAVAILABLE_AUTONOMOUS_PAPER_STATUS;
  }
  if (!AUTONOMOUS_PAPER_STATUS_VALID_TRUTH_STATES.has(wrapper.truth_state)) {
    return UNAVAILABLE_AUTONOMOUS_PAPER_STATUS;
  }
  return {
    truth_state: wrapper.truth_state as AutonomousPaperStatusSurface["truth_state"],
    mode: wrapper.mode,
    live_routing_enabled: wrapper.live_routing_enabled,
    runtime_status: wrapper.runtime_status,
    arm_state: wrapper.arm_state,
    kill_switch_active: wrapper.kill_switch_active,
    deadman_status: wrapper.deadman_status,
    ws_continuity: wrapper.ws_continuity,
    reconcile_status: wrapper.reconcile_status,
    mismatch_count: wrapper.mismatch_count,
    open_order_count: wrapper.open_order_count,
    position_count: wrapper.position_count,
    current_symbol: wrapper.current_symbol ?? null,
    current_position_qty: wrapper.current_position_qty ?? null,
    target_qty: wrapper.target_qty ?? null,
    computed_delta_qty: wrapper.computed_delta_qty ?? null,
    no_order_reason: wrapper.no_order_reason ?? null,
    last_strategy_decision: wrapper.last_strategy_decision ?? null,
    flatten_available: wrapper.flatten_available,
    flatten_blockers: wrapper.flatten_blockers ?? [],
    watchlist_outcome: wrapper.watchlist_outcome,
    watchlist_approved: wrapper.watchlist_approved,
    readiness_classification: wrapper.readiness_classification,
    blockers: wrapper.blockers ?? [],
    next_operator_action: wrapper.next_operator_action,
    autonomous_session_state: wrapper.autonomous_session_state,
    now_utc: wrapper.now_utc,
  };
}

// ---------------------------------------------------------------------------
// GUI-PAPER-STATUS-VISIBILITY-01: Watchlist status wrapper → surface
//
// GET /api/v1/watchlist/status (PAPER-HANDOFF-READONLY-01). The daemon route
// always returns HTTP 200 with an explicit `status` outcome — there is no
// backend truth_state field. The GUI adds its own truth_state purely to
// distinguish "we reached the endpoint and have its answer" (active) from
// "we could not reach/parse the endpoint" (unavailable). approved_for_live
// is a hard daemon invariant (always false) and is passed through verbatim.
// ---------------------------------------------------------------------------

export interface WatchlistStatusWrapper {
  configured_path: string | null;
  status: string;
  approved_for_autonomous_paper: boolean;
  approved_for_live: boolean;
  symbols: string[];
  top_symbol: string | null;
  strategy_assignments: unknown;
  max_symbols_to_trade: number | null;
  max_concurrent_positions: number | null;
  failure_reasons: string[];
  checked_at_utc: string;
}

const UNAVAILABLE_WATCHLIST_STATUS: WatchlistStatusSurface = {
  truth_state: "unavailable",
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
};

function strategyAssignmentCount(value: unknown): number | null {
  if (value == null || typeof value !== "object" || Array.isArray(value)) return null;
  return Object.keys(value as Record<string, unknown>).length;
}

// Read-only mapper. A failed/missing probe maps to the explicit "unavailable"
// surface — an empty symbol list must never be displayed as if the watchlist
// were authoritatively loaded-and-empty.
export function mapWatchlistStatusWrapper(
  wrapper: WatchlistStatusWrapper | null | undefined,
): WatchlistStatusSurface {
  if (wrapper == null || typeof wrapper.status !== "string") {
    return UNAVAILABLE_WATCHLIST_STATUS;
  }
  return {
    truth_state: "active",
    configured_path: wrapper.configured_path ?? null,
    status: wrapper.status,
    approved_for_autonomous_paper: wrapper.approved_for_autonomous_paper,
    approved_for_live: wrapper.approved_for_live,
    symbols: wrapper.symbols ?? [],
    top_symbol: wrapper.top_symbol ?? null,
    strategy_assignment_count: strategyAssignmentCount(wrapper.strategy_assignments),
    max_symbols_to_trade: wrapper.max_symbols_to_trade ?? null,
    max_concurrent_positions: wrapper.max_concurrent_positions ?? null,
    failure_reasons: wrapper.failure_reasons ?? [],
    checked_at_utc: wrapper.checked_at_utc ?? null,
  };
}

// ---------------------------------------------------------------------------
// GUI-PAPER-STATUS-VISIBILITY-01: Watchlist admission-check wrapper → surface
//
// GET /api/v1/watchlist/admission-check?symbol=&strategy_id=
// (PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01). Pure dry-run advisory check — the
// GUI must never invent a symbol/strategy to probe with. The surface carries
// an explicit `state`:
//   "not_checked"  — no safe (symbol, strategy_id) pair exists in GUI truth.
//   "checked"      — the daemon answered for the exact pair the GUI checked.
//   "unavailable"  — a safe pair existed but the probe failed.
// ---------------------------------------------------------------------------

export interface WatchlistAdmissionCheckWrapper {
  allowed: boolean;
  reason: string;
  status: string;
  approved_for_autonomous_paper: boolean;
  approved_for_live: boolean;
  symbol: string;
  strategy_id: string;
  top_symbol: string | null;
  strategy_assignments: unknown;
  note: string;
  checked_at_utc: string;
}

export function notCheckedAdmissionCheck(reason: string): AdmissionCheckSurface {
  return {
    state: "not_checked",
    reason_unchecked: reason,
    symbol: null,
    strategy_id: null,
    allowed: null,
    reason_code: null,
    status: null,
    approved_for_autonomous_paper: null,
    approved_for_live: null,
    note: null,
    checked_at_utc: null,
  };
}

export function unavailableAdmissionCheck(symbol: string, strategyId: string): AdmissionCheckSurface {
  return {
    state: "unavailable",
    reason_unchecked: null,
    symbol,
    strategy_id: strategyId,
    allowed: null,
    reason_code: null,
    status: null,
    approved_for_autonomous_paper: null,
    approved_for_live: null,
    note: null,
    checked_at_utc: null,
  };
}

// Read-only mapper. Only called when the GUI has already confirmed it is
// looking at a response for the exact (symbol, strategy_id) pair it checked —
// see the checkedSymbol/checkedStrategyId guard in fetchOperatorModel.
export function mapWatchlistAdmissionCheckWrapper(
  wrapper: WatchlistAdmissionCheckWrapper,
): AdmissionCheckSurface {
  return {
    state: "checked",
    reason_unchecked: null,
    symbol: wrapper.symbol,
    strategy_id: wrapper.strategy_id,
    allowed: wrapper.allowed,
    reason_code: wrapper.reason,
    status: wrapper.status,
    approved_for_autonomous_paper: wrapper.approved_for_autonomous_paper,
    approved_for_live: wrapper.approved_for_live,
    note: wrapper.note,
    checked_at_utc: wrapper.checked_at_utc ?? null,
  };
}

// ---------------------------------------------------------------------------
// A3/A4 daemon wrapper types
// ---------------------------------------------------------------------------

// System topology wrapper (A3).
// truth_state is always "active" — derived from daemon in-memory state.
export interface SystemTopologyWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  updated_at: string;
  services: unknown[];
}

// Incidents wrapper (A3).
// truth_state is always "not_wired" — no incident manager implemented.
export interface IncidentsWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  note: string;
  rows: unknown[];
}

// Replace/cancel chains wrapper (A4).
// truth_state is always "not_wired" — no chain lineage tracked.
export interface ReplaceCancelChainsWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  note: string;
  chains: unknown[];
}

// Alert triage wrapper (A4).
// truth_state is always "alerts_no_triage" — source is real, lifecycle is not.
export interface AlertTriageWrapper {
  canonical_route: string;
  truth_state: string;
  backend: string;
  triage_note: string;
  rows: AlertTriageWrapperRow[];
}

export interface AlertTriageWrapperRow {
  alert_id: string;
  severity: string;
  status: string;
  title: string;
  domain: string;
  linked_incident_id: string | null;
  linked_order_id: string | null;
  linked_strategy_id: string | null;
  created_at: string;
  assigned_to: string | null;
}

// Used by PortfolioScreen to render honest per-lane notices.
// Exported here so tests can prove fail-closed behaviour without importing the .tsx screen.
export function paperJournalLaneNotice(truthState: PaperJournalTruthState): string | null {
  switch (truthState) {
    case "active": return null;
    case "no_active_run": return "No active run — data unavailable until execution starts.";
    case "no_db": return "Unavailable: no database pool configured. Do not treat empty rows as authoritative.";
    case "unavailable": return "Endpoint unavailable.";
  }
}

// ---------------------------------------------------------------------------
// GUI-09: Mode-change guidance normalizer (CC-03)
// ---------------------------------------------------------------------------

/**
 * Validate and narrow an unknown response from GET /api/v1/ops/mode-change-guidance
 * to ModeChangeGuidanceResponse.
 *
 * Returns null for null, non-object, or structurally incomplete responses so
 * callers can render an honest unavailable notice instead of partial data.
 *
 * Required fields (all must be present):
 *   canonical_route, current_mode, operator_next_steps, transition_verdicts,
 *   preconditions, restart_workflow.
 *
 * Exported for test isolation — pure function, no side effects.
 */
export function normalizeModeChangeGuidance(raw: unknown): ModeChangeGuidanceResponse | null {
  if (raw == null || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (
    typeof r["canonical_route"] !== "string" ||
    typeof r["current_mode"] !== "string" ||
    !Array.isArray(r["operator_next_steps"]) ||
    !Array.isArray(r["transition_verdicts"]) ||
    !Array.isArray(r["preconditions"]) ||
    r["restart_workflow"] == null ||
    typeof r["restart_workflow"] !== "object"
  ) {
    return null;
  }
  // restart_workflow must carry an explicit truth_state string.
  // A present object without truth_state is structurally incomplete — fail closed.
  const rw = r["restart_workflow"] as Record<string, unknown>;
  if (typeof rw["truth_state"] !== "string") {
    return null;
  }
  // If pending_intent is non-null, all fields consumed by OpsScreen must be
  // structurally valid strings. A malformed non-null pending_intent must fail
  // closed — it cannot be allowed to pass normalization and render undefined
  // values into the UI.
  const pi = rw["pending_intent"];
  if (pi != null) {
    if (typeof pi !== "object") return null;
    const p = pi as Record<string, unknown>;
    if (
      typeof p["intent_id"] !== "string" ||
      typeof p["from_mode"] !== "string" ||
      typeof p["to_mode"] !== "string" ||
      typeof p["transition_verdict"] !== "string" ||
      typeof p["initiated_by"] !== "string" ||
      typeof p["initiated_at_utc"] !== "string" ||
      typeof p["note"] !== "string"
    ) {
      return null;
    }
  }
  return raw as ModeChangeGuidanceResponse;
}
