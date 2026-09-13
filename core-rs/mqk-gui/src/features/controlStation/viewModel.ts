// core-rs/mqk-gui/src/features/controlStation/viewModel.ts
//
// GUI-CS-01A: Control Station truth/view model. Pure selectors over the
// existing SystemModel — no new API surface, no duplicate source-authority
// framework. Reuses panelTruthRenderState (per-panel truth gating) and
// deriveLatestHaltSummary (halt cause derivation) exactly as the existing
// Dashboard/Incidents screens do.
//
// Every section carries its own truth/tone signal rather than collapsing to
// one whole-page block: "unknown"/"disconnected"/"degraded" truth must never
// render as healthy, and a section with no authoritative surface renders
// explicit not_wired/unknown state rather than a fabricated zero.

import type {
  AutonomousDailyFinalizationStatus,
  AutonomousDailyOutcomeClass,
  AutonomousPaperStatusTruthState,
  EnvironmentMode,
  HealthState,
  RuntimeStatus,
  Severity,
  SystemModel,
  SystemStatus,
} from "../system/types";
import { panelTruthRenderState, type TruthRenderState } from "../system/truthRendering";
import { deriveLatestHaltSummary, type HaltSummaryStatus } from "../system/haltSummary";

export type CsTone = "good" | "warn" | "bad" | "unknown";

const TONE_RANK: Record<CsTone, number> = { good: 0, warn: 1, unknown: 2, bad: 3 };

function worstTone(tones: CsTone[]): CsTone {
  return tones.reduce<CsTone>((worst, next) => (TONE_RANK[next] > TONE_RANK[worst] ? next : worst), "good");
}

// A HealthState of "unknown" or "disconnected" is a truth gap, not a clean
// bill of health — it must rank strictly above "good" so it can never be
// silently rendered as healthy. Exported so the presentation layer maps the
// same handful of raw HealthState fields to tone without a second copy of
// this switch.
//
// Takes `string` rather than `HealthState`: these fields are raw JSON off
// the wire, and the daemon has shipped values outside the declared union
// (e.g. STATUS-TRUTH-01's broker_status="stale"). A switch keyed on the
// narrow union type would let such a value silently match no case and fall
// through as neither good nor bad — the explicit default below fails closed
// instead.
export function healthTone(state: string): CsTone {
  switch (state) {
    case "ok":
      return "good";
    case "warning":
      return "warn";
    case "critical":
      return "bad";
    case "disconnected":
    case "unknown":
      return "unknown";
    default:
      return "unknown";
  }
}

// AP-04B: market_data_health's real daemon vocabulary is
// "not_configured" | "signal_ingestion_ready" (StrategyMarketDataSource::
// as_health_str in mqk-daemon/src/state/types.rs) — not the generic
// HealthState union. Both are ordinary, non-fault states on the current
// path; anything else falls back to the generic fail-closed classifier
// rather than a bespoke default that could silently mean "good".
function marketDataHealthTone(state: string): CsTone {
  switch (state) {
    case "not_configured":
    case "signal_ingestion_ready":
      return "good";
    default:
      return healthTone(state);
  }
}

// AP-05: WS continuity truth is only meaningful for an external (Alpaca)
// broker snapshot — a synthetic/paper broker reporting "not_applicable" is
// ordinary operation, not a fault (mirrors GlobalStatusBar's DESKTOP-12
// not_applicable guard). For an external broker, only "live" is proven
// continuity; "cold_start_unproven" and "gap_detected" must never render as
// good, and an external broker reporting "not_applicable" (or any other
// unrecognized value) contradicts broker_snapshot_source=external and fails
// closed rather than being treated as proven.
function wsContinuityTone(
  brokerSnapshotSource: SystemStatus["broker_snapshot_source"],
  continuity: SystemStatus["alpaca_ws_continuity"],
): CsTone {
  if (brokerSnapshotSource !== "external") return "good";
  switch (continuity) {
    case "live":
      return "good";
    case "cold_start_unproven":
      return "warn";
    case "gap_detected":
      return "bad";
    case "not_applicable":
    default:
      return "unknown";
  }
}

// Deadman heartbeat truth (mqk-daemon/src/state/deadman.rs): "healthy" only
// while a run is actively ticking; "expired" is a fired watchdog and never
// good. "inactive" is legitimate for an idle/stopped runtime — do not
// manufacture a warning merely because the watchdog is off duty — but is a
// truth gap while runtime_status is "running" (a running runtime should have
// a healthy watchdog, not an inactive one). Any other value (e.g.
// "unavailable", "unknown") fails closed.
function deadmanStatusTone(runtimeStatus: RuntimeStatus, deadmanStatus: string): CsTone {
  switch (deadmanStatus) {
    case "healthy":
      return "good";
    case "expired":
      return "bad";
    case "inactive":
      return runtimeStatus === "running" ? "unknown" : "good";
    default:
      return "unknown";
  }
}

// ---------------------------------------------------------------------------
// SYSTEM
// ---------------------------------------------------------------------------

export interface ControlStationSystemSection {
  daemonOnline: boolean;
  environment: EnvironmentMode;
  dbStatus: HealthState;
  brokerStatus: HealthState;
  marketDataHealth: HealthState;
  marketDataTone: CsTone;
  reconcileStatus: HealthState;
  integrityStatus: HealthState;
  wsContinuity: SystemStatus["alpaca_ws_continuity"];
  wsTone: CsTone;
  killSwitchActive: boolean;
  integrityHaltActive: boolean;
  riskHaltActive: boolean;
  deadmanStatus: string;
  deadmanTone: CsTone;
  hasWarning: boolean;
  hasCritical: boolean;
  tone: CsTone;
}

function buildSystemSection(model: SystemModel): ControlStationSystemSection {
  const { status } = model;
  // daemon_reachable is only meaningful once a model has actually connected;
  // an unconnected fallback model must never read as "daemon online".
  const daemonOnline = model.connected && status.daemon_reachable;

  const marketDataTone = marketDataHealthTone(status.market_data_health);
  const wsTone = wsContinuityTone(status.broker_snapshot_source, status.alpaca_ws_continuity);
  const deadmanTone = deadmanStatusTone(status.runtime_status, status.deadman_status);

  // GUI-CS-01D: has_warning/has_critical are the daemon's own aggregate
  // truth and may carry warning/critical information not duplicated by any
  // of the individual fields above (e.g. a risk-truth read failure) — they
  // must contribute to the headline tone in their own right, not only
  // through the fields the GUI happens to enumerate.
  let tone = worstTone([
    healthTone(status.db_status),
    healthTone(status.broker_status),
    marketDataTone,
    healthTone(status.reconcile_status),
    healthTone(status.integrity_status),
    wsTone,
    deadmanTone,
    status.has_critical ? "bad" : "good",
    status.has_warning ? "warn" : "good",
  ]);
  if (!daemonOnline) tone = worstTone([tone, "unknown"]);
  if (status.kill_switch_active || status.integrity_halt_active || status.risk_halt_active) tone = "bad";

  return {
    daemonOnline,
    environment: status.environment,
    dbStatus: status.db_status,
    brokerStatus: status.broker_status,
    marketDataHealth: status.market_data_health,
    marketDataTone,
    reconcileStatus: status.reconcile_status,
    integrityStatus: status.integrity_status,
    wsContinuity: status.alpaca_ws_continuity,
    wsTone,
    killSwitchActive: status.kill_switch_active,
    integrityHaltActive: status.integrity_halt_active,
    riskHaltActive: status.risk_halt_active,
    deadmanStatus: status.deadman_status,
    deadmanTone,
    hasWarning: status.has_warning,
    hasCritical: status.has_critical,
    tone,
  };
}

// ---------------------------------------------------------------------------
// TRADING DOMAIN
// ---------------------------------------------------------------------------

export interface ControlStationTradingDomainSection {
  environment: EnvironmentMode;
  daemonMode: string;
  adapterId: string;
  runtimeStatus: RuntimeStatus;
  strategyArmed: boolean;
  executionArmed: boolean;
  liveRoutingEnabled: boolean;
  marketSession: string;
  tradingWindow: string;
  sessionTruth: TruthRenderState | null;
  tone: CsTone;
}

function buildTradingDomainSection(model: SystemModel): ControlStationTradingDomainSection {
  const { status, sessionState } = model;

  let tone: CsTone = "good";
  if (!model.connected) tone = "unknown";
  else if (status.live_routing_enabled) tone = "bad";
  else if (status.runtime_status === "halted") tone = "bad";
  else if (status.runtime_status === "degraded") tone = "warn";

  return {
    environment: status.environment,
    daemonMode: status.daemon_mode,
    adapterId: status.adapter_id,
    // runtime_status is reported independently of daemon reachability — a
    // reachable daemon with an idle runtime must remain "idle", never
    // reinterpreted as "running" or folded into the daemon-online signal.
    runtimeStatus: status.runtime_status,
    strategyArmed: status.strategy_armed,
    executionArmed: status.execution_armed,
    liveRoutingEnabled: status.live_routing_enabled,
    marketSession: sessionState.market_session,
    tradingWindow: sessionState.system_trading_window,
    sessionTruth: panelTruthRenderState(model, "session"),
    tone,
  };
}

// ---------------------------------------------------------------------------
// PORTFOLIO
// ---------------------------------------------------------------------------

export interface ControlStationPortfolioSection {
  portfolioTruth: TruthRenderState | null;
  executionTruth: TruthRenderState | null;
  /** Broker-snapshot truth (portfolio panel authority) — never gated on execution/session state. */
  brokerPositionCount: number;
  brokerOpenOrderCount: number;
  /** Active-session execution truth (execution panel authority) — a distinct source from the broker snapshot above. */
  activeSessionOrderCount: number;
  pendingSessionOrderCount: number;
  stuckSessionOrderCount: number;
}

function buildPortfolioSection(model: SystemModel): ControlStationPortfolioSection {
  return {
    portfolioTruth: panelTruthRenderState(model, "portfolio"),
    executionTruth: panelTruthRenderState(model, "execution"),
    brokerPositionCount: model.positions.length,
    brokerOpenOrderCount: model.openOrders.length,
    activeSessionOrderCount: model.executionSummary.active_orders,
    pendingSessionOrderCount: model.executionSummary.pending_orders,
    stuckSessionOrderCount: model.executionSummary.stuck_orders,
  };
}

// ---------------------------------------------------------------------------
// AUTONOMY
// ---------------------------------------------------------------------------

export interface ControlStationAutonomySection {
  applicable: boolean;
  truthState: AutonomousPaperStatusTruthState;
  armState: string;
  readinessClassification: string;
  nextOperatorAction: string | null;
  blockers: string[];
  dailyOperationTransportState: "available" | "endpoint_unavailable";
  dailyOperationTruthState: string | null;
  dailyOperationFinalizationStatus: AutonomousDailyFinalizationStatus | null;
  dailyOperationOutcomeClass: AutonomousDailyOutcomeClass | null;
  dailyOperationMarketDate: string | null;
}

function buildAutonomySection(model: SystemModel): ControlStationAutonomySection {
  const aps = model.autonomousPaperStatus;
  const dailyOp = model.autonomousDailyOperation;
  return {
    applicable: model.preflight.autonomous_readiness_applicable === true,
    truthState: aps.truth_state,
    armState: aps.arm_state,
    readinessClassification: aps.readiness_classification,
    nextOperatorAction: aps.next_operator_action,
    blockers: aps.blockers,
    dailyOperationTransportState: dailyOp.transport_state,
    dailyOperationTruthState: dailyOp.truth_state,
    dailyOperationFinalizationStatus: dailyOp.operation?.finalization_status ?? null,
    dailyOperationOutcomeClass: dailyOp.operation?.outcome_class ?? null,
    dailyOperationMarketDate: dailyOp.operation?.market_date ?? null,
  };
}

// ---------------------------------------------------------------------------
// M1 VALIDATION
//
// There is no authoritative daemon surface today that counts countable soak
// sessions toward the M1 10-session/5-clean gate. autonomousDailyOperations
// history rows carry per-day finalization/outcome/evidence state but no
// accepted "counts toward M1" semantics — deriving one here would fabricate
// a program-level judgment the daemon does not make. Render explicit
// not_wired rather than inventing or hardcoding a count.
// ---------------------------------------------------------------------------

export type M1ValidationState = "not_wired";

export interface ControlStationM1ValidationSection {
  state: M1ValidationState;
  reason: string;
}

function buildM1ValidationSection(): ControlStationM1ValidationSection {
  return {
    state: "not_wired",
    reason:
      "No authoritative countable-soak-session surface is exposed by the daemon API. " +
      "autonomousDailyOperations history has no accepted M1 session/clean-session semantics — not derived here.",
  };
}

// ---------------------------------------------------------------------------
// INCIDENTS
// ---------------------------------------------------------------------------

export interface ControlStationIncidentsSection {
  incidentsTruth: TruthRenderState | null;
  openIncidentCount: number;
  totalIncidentCount: number;
  haltStatus: HaltSummaryStatus;
  haltSeverity: Severity;
  haltReason: string | null;
  killSwitchActive: boolean | null;
  liveRoutingEnabled: boolean | null;
}

const OPEN_INCIDENT_STATUSES = new Set(["open", "investigating"]);

function buildIncidentsSection(model: SystemModel): ControlStationIncidentsSection {
  const halt = deriveLatestHaltSummary(model);
  return {
    incidentsTruth: panelTruthRenderState(model, "incidents"),
    openIncidentCount: model.incidents.filter((incident) => OPEN_INCIDENT_STATUSES.has(incident.status)).length,
    totalIncidentCount: model.incidents.length,
    haltStatus: halt.status,
    haltSeverity: halt.severity,
    haltReason: halt.reason,
    killSwitchActive: halt.kill_switch_active,
    liveRoutingEnabled: halt.live_routing_enabled,
  };
}

// ---------------------------------------------------------------------------
// COMPOSITE
// ---------------------------------------------------------------------------

export interface ControlStationViewModel {
  connected: boolean;
  lastUpdatedAt: string | null;
  system: ControlStationSystemSection;
  tradingDomain: ControlStationTradingDomainSection;
  portfolio: ControlStationPortfolioSection;
  autonomy: ControlStationAutonomySection;
  m1Validation: ControlStationM1ValidationSection;
  incidents: ControlStationIncidentsSection;
}

export function buildControlStationViewModel(model: SystemModel): ControlStationViewModel {
  return {
    connected: model.connected,
    lastUpdatedAt: model.lastUpdatedAt,
    system: buildSystemSection(model),
    tradingDomain: buildTradingDomainSection(model),
    portfolio: buildPortfolioSection(model),
    autonomy: buildAutonomySection(model),
    m1Validation: buildM1ValidationSection(),
    incidents: buildIncidentsSection(model),
  };
}
