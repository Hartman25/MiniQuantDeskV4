//! Request and response types for all mqk-daemon HTTP endpoints.
//!
//! These types are `Serialize + Deserialize` so they can be JSON-encoded
//! by Axum and decoded by tests.  No business logic lives here.

use mqk_runtime::observability::ExecutionSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// /v1/health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
}

// ---------------------------------------------------------------------------
// Gate refusal (403) — Patch L1
// ---------------------------------------------------------------------------

/// Response body when a daemon route is refused due to a gate check failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRefusedResponse {
    pub error: String,
    /// Which gate failed: "integrity_armed" | "risk_allowed" | "reconcile_clean"
    pub gate: String,
}

// ---------------------------------------------------------------------------
// /v1/integrity/arm  /v1/integrity/disarm
// ---------------------------------------------------------------------------

/// Response for integrity arm / disarm endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityResponse {
    /// true = armed (execution allowed), false = disarmed (execution blocked).
    pub armed: bool,
    /// Active run ID at the moment of the call (if any).
    pub active_run_id: Option<Uuid>,
    /// Current run-lifecycle state ("idle" | "running" | "halted").
    pub state: String,
}

// ---------------------------------------------------------------------------
// Authoritative operator control actions — DMON-06
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionAuditFields {
    /// Whether this action produced a durable DB write that the daemon can prove.
    pub durable_db_write: bool,
    /// Human-readable write target(s) for the durable state update.
    pub durable_targets: Vec<String>,
    /// Optional audit/event id if emitted by current architecture.
    pub audit_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionResponse {
    /// Explicit action contract identifier (e.g., "control.arm").
    pub requested_action: String,
    /// Whether the daemon accepted this action request.
    pub accepted: bool,
    /// Disposition summary (e.g., "applied", "rejected", "not_authoritative").
    pub disposition: String,
    /// Resulting arming state where known by current architecture.
    pub resulting_integrity_state: Option<String>,
    /// Resulting desired armed state where known by current architecture.
    pub resulting_desired_armed: Option<bool>,
    /// Blockers that caused rejection.
    pub blockers: Vec<String>,
    /// Non-blocking warnings for operator visibility.
    pub warnings: Vec<String>,
    /// Daemon environment/profile scope if known.
    pub environment: Option<String>,
    /// Action scope (local/cluster/etc.) where known.
    pub scope: Option<String>,
    /// Auditability metadata that this daemon can currently prove.
    pub audit: OperatorActionAuditFields,
    /// Durable restart intent snapshot.  Present only when `action_key` is
    /// "request-mode-change" and the transition is `admissible_with_restart`
    /// (disposition = "pending_restart").  Null in all other cases.
    pub pending_restart_intent: Option<PendingRestartIntentSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionAuditRow {
    pub audit_event_id: String,
    pub ts_utc: String,
    pub requested_action: String,
    pub disposition: String,
    pub run_id: Option<String>,
    pub runtime_transition: Option<String>,
    pub provenance_ref: String,
}

/// Response wrapper for `/api/v1/audit/operator-actions`.
///
/// `truth_state`:
/// - `"active"` — durable operator-action history was queried from Postgres;
///   `backend` names the exact source table and `rows` is authoritative.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionsAuditResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<OperatorActionAuditRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditArtifactRow {
    pub artifact_id: String,
    pub artifact_type: String,
    pub run_id: String,
    pub created_at_utc: String,
    pub provenance_ref: String,
}

/// Response wrapper for `/api/v1/audit/artifacts`.
///
/// `truth_state`:
/// - `"active"` — durable artifact history was queried from Postgres; `rows`
///   is authoritative and `backend` names the exact source table.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditArtifactsResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<AuditArtifactRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineRow {
    pub ts_utc: String,
    pub kind: String,
    pub run_id: Option<String>,
    pub detail: String,
    pub provenance_ref: String,
    /// OPTR-03: For `kind = "operator_action"` rows, the raw UUID string from
    /// `audit_events.event_id`.  Allows direct stable correlation to
    /// `/api/v1/audit/operator-actions` rows without parsing `provenance_ref`.
    /// `None` for `kind = "runtime_transition"` rows (sourced from `runs`).
    pub audit_event_id: Option<String>,
}

/// Response wrapper for `/api/v1/ops/operator-timeline`.
///
/// `truth_state`:
/// - `"active"` — durable operator timeline history was queried from Postgres;
///   `rows` is authoritative and `backend` names the exact source table set.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<OperatorTimelineRow>,
}

// ---------------------------------------------------------------------------
// Trading read APIs — DAEMON-1
// ---------------------------------------------------------------------------

use mqk_schemas::{BrokerAccount, BrokerFill, BrokerOrder, BrokerPosition, BrokerSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingAccountResponse {
    /// Explicit snapshot truth state for operator-honest read semantics.
    ///
    /// - `no_snapshot` = no broker snapshot is loaded.
    /// - `stale_snapshot` = reconcile has flagged snapshot freshness as stale.
    /// - `current_snapshot` = daemon has a currently-usable broker snapshot.
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub account: Option<BrokerAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingPositionsResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub positions: Option<Vec<BrokerPosition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingOrdersResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub orders: Option<Vec<BrokerOrder>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingFillsResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub fills: Option<Vec<BrokerFill>>,
}

/// Full raw snapshot (if available). This is intentionally read-only.
/// A later patch will wire snapshot ingestion from the broker/reconciler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingSnapshotResponse {
    pub snapshot: Option<BrokerSnapshot>,
}

// ---------------------------------------------------------------------------
// /api/v1 summary spine — GUI alignment patch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatusResponse {
    pub environment: Option<String>,
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub deployment_blocker: Option<String>,
    pub runtime_status: String,
    pub broker_status: String,
    /// AP-04: How broker_snapshot truth is sourced.
    /// `"synthetic"` = paper (local OMS); `"external"` = Alpaca (AP-03 REST fetch).
    /// Independent of market_data_health / strategy feed policy.
    pub broker_snapshot_source: String,
    /// AP-05: Alpaca websocket continuity truth.
    /// `"not_applicable"` for Paper; `"cold_start_unproven"`, `"live"`, or
    /// `"gap_detected"` for Alpaca.  Only `"live"` indicates proven continuity;
    /// all other values are fail-closed.
    pub alpaca_ws_continuity: String,
    pub db_status: String,
    pub market_data_health: String,
    pub reconcile_status: String,
    pub integrity_status: String,
    pub audit_writer_status: String,
    pub last_heartbeat: Option<String>,
    pub deadman_status: String,
    pub loop_latency_ms: Option<u64>,
    pub active_account_id: Option<String>,
    pub config_profile: Option<String>,
    pub has_warning: bool,
    pub has_critical: bool,
    pub strategy_armed: bool,
    pub execution_armed: bool,
    pub live_routing_enabled: Option<bool>,
    pub kill_switch_active: bool,
    pub risk_halt_active: bool,
    pub integrity_halt_active: bool,
    pub daemon_reachable: bool,
    pub fault_signals: Vec<FaultSignal>,
    /// PT-AUTO-03: Autonomous signal intake count for this execution run.
    ///
    /// `None` when `ExternalSignalIngestion` is not configured for this deployment
    /// (i.e., not paper+alpaca).  Field is not applicable and carries no meaning.
    ///
    /// `Some(n)` for paper+alpaca: the number of distinct new outbox enqueues
    /// (Gate 7 Ok(true)) accepted so far this run.  Resets to 0 at each run start.
    /// When `autonomous_signal_limit_hit` is `Some(true)`, this value equals
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN` (100) and Gate 1d is blocking all further
    /// signals until the next run start.
    pub autonomous_signal_count: Option<u32>,
    /// PT-AUTO-03: Whether the autonomous day signal intake limit has been reached.
    ///
    /// `None` when `ExternalSignalIngestion` is not configured (not applicable).
    ///
    /// `Some(true)` means Gate 1d is currently refusing all incoming signals with
    /// `409/day_limit_reached`.  No further signals will be accepted until the next
    /// `run/start` resets the counter.
    ///
    /// `Some(false)` means Gate 1d is not tripping; signal intake is still open
    /// (subject to all other gates).
    pub autonomous_signal_limit_hit: Option<bool>,
    /// B8: Canonical asset-class scope for this execution path.
    ///
    /// Always `"equity_only"` on the current canonical path.  Only US equities
    /// (stocks and ETFs) are supported.  Options, futures, crypto, and FX are
    /// not wired into the execution, portfolio, risk, or broker adapter paths.
    ///
    /// Operators and strategy authors must not assume support for any other
    /// asset class.  Signal admission will explicitly reject signals carrying
    /// `asset_class` values other than `"equity"` or absent (equity implied).
    pub asset_class_scope: String,

    // -----------------------------------------------------------------------
    // C1: Live-trust truth surface
    // -----------------------------------------------------------------------
    /// C1: Parity evidence state for the configured artifact.
    ///
    /// Derived by evaluating `parity_evidence.json` via the same evaluator used
    /// by the `/api/v1/system/parity-evidence` route.  Surfaced here so
    /// operators can observe live-trust state on the primary status surface
    /// without navigating to a secondary endpoint.
    ///
    /// Values:
    /// - `"not_configured"` — `MQK_ARTIFACT_PATH` absent or empty; gate not applicable.
    /// - `"absent"` — artifact path set but `parity_evidence.json` not found.
    ///   Absent evidence ≠ parity proven.
    /// - `"invalid"` — `parity_evidence.json` found but structurally invalid.
    /// - `"incomplete"` — evidence present but `live_trust_complete=false`.
    ///   All current builds produce this value.  Live-capital is not trusted.
    /// - `"complete"` — `live_trust_complete=true`.  Not reachable in current
    ///   builds.  Explicit ceiling — no operator action can advance this
    ///   without a proof patch that replaces the TV-03 pipeline claim.
    /// - `"unavailable"` — the evaluator itself could not run (panic-safe wrapper).
    pub parity_evidence_state: String,

    /// C1: Whether the live-trust chain is complete enough for live-capital execution.
    ///
    /// Non-null only when `parity_evidence_state == "incomplete"` or `"complete"`.
    /// Always `false` in current builds — the TV-03 parity pipeline explicitly
    /// writes `live_trust_complete=false` and the daemon never fabricates a
    /// positive trust claim.
    ///
    /// `null` when parity evidence is absent, invalid, not configured, or the
    /// evaluator is unavailable.  Null is not a positive trust claim.
    pub live_trust_complete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultSignal {
    pub class: String,
    pub severity: String,
    pub summary: String,
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// DATA-FRESHNESS-READINESS-GATE-01: Market-data freshness status
// ---------------------------------------------------------------------------

/// Market-data freshness status for the configured strategy symbol/timeframe.
///
/// Surfaced on `autonomous/readiness` and `system/preflight` so operators can
/// verify that sufficient fresh bars exist before paper trading starts.
///
/// `freshness_state` values:
/// - `"ok"` — enough completed bars exist and latest bar is within the staleness threshold.
/// - `"stale"` — enough rows but latest bar is older than the staleness threshold.
/// - `"missing"` — 0 completed bars in `md_bars` for this symbol/timeframe.
/// - `"insufficient"` — completed rows exist but fewer than `min_required_rows`.
/// - `"unavailable"` — DB not reachable; freshness cannot be verified (not a blocker).
/// - `"not_applicable"` — symbol or timeframe not configured; gate not applicable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFreshnessStatus {
    pub symbol: String,
    pub timeframe: String,
    pub completed_rows: u64,
    pub min_required_rows: u64,
    /// RFC 3339 timestamp of the latest completed bar, or `null`.
    pub latest_complete_bar_ts: Option<String>,
    /// RFC 3339 timestamp of the latest completed bar, or `null`.
    ///
    /// Kept alongside the original singular field so operator diagnostics use
    /// the explicit `latest_completed_bar_ts` name.
    pub latest_completed_bar_ts: Option<String>,
    /// RFC 3339 UTC timestamp of when this freshness status was evaluated.
    pub now_utc: String,
    /// Age of the latest completed bar in seconds, or `null` when no bar exists.
    pub age_seconds: Option<i64>,
    /// Effective max allowed age for this symbol/timeframe in seconds.
    pub max_allowed_age_seconds: i64,
    pub freshness_state: String,
    /// Machine-readable reason code, e.g. `intraday_bar_stale`.
    pub reason_code: String,
    pub reason: String,
}

impl MarketDataFreshnessStatus {
    /// True when freshness is `"ok"` — all checks pass, start is not blocked.
    pub fn is_ok(&self) -> bool {
        self.freshness_state == "ok"
    }

    /// True when this status should block startup.
    ///
    /// `"missing"`, `"insufficient"`, and `"stale"` are blockers.
    /// `"unavailable"` and `"not_applicable"` are not blockers (no DB evidence).
    pub fn is_start_blocker(&self) -> bool {
        matches!(
            self.freshness_state.as_str(),
            "missing" | "insufficient" | "stale"
        )
    }
}

/// Aggregate multi-symbol market-data readiness report
/// (PREMARKET-DATA-READINESS-GATE-01).
///
/// Extends [`MarketDataFreshnessStatus`] (single symbol/timeframe) to the
/// full set of symbols the current deployment requires — the approved
/// `watchlist-v2` artifact's symbols when one is configured and approved,
/// otherwise the legacy single `MQK_STRATEGY_SYMBOL`. `start_execution_runtime`
/// fails closed when any required symbol is missing, insufficient, or stale;
/// this report is also surfaced read-only on `system/preflight` and
/// `autonomous/readiness` for operator visibility.
///
/// `aggregate_status` values:
/// - `"ok"` — every required symbol passed its freshness check.
/// - `"not_applicable"` — no symbol is configured; gate not applicable.
/// - `"unavailable"` — no symbol is blocking, but at least one symbol's
///   freshness could not be verified (DB unreachable); not a start blocker.
/// - `"missing"` / `"insufficient"` / `"stale"` — exactly one required
///   symbol is blocking, with this state; see `blockers` for which symbol.
/// - `"mixed_blocked"` — more than one required symbol is blocking
///   simultaneously; see `blockers` and `per_symbol` for the exact symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSymbolFreshnessReport {
    pub aggregate_status: String,
    /// True unless `aggregate_status` is one of the blocking states
    /// (`"missing"`, `"insufficient"`, `"stale"`, `"mixed_blocked"`).
    pub start_allowed: bool,
    /// Normalized (trimmed, uppercased, deduped) symbols actually checked.
    /// Empty when `aggregate_status == "not_applicable"`.
    pub required_symbols: Vec<String>,
    /// One row per required symbol/timeframe, in the order checked.
    pub per_symbol: Vec<MarketDataFreshnessStatus>,
    /// Operator-readable reasons for every blocking symbol (the `reason` of
    /// each `per_symbol` entry where `is_start_blocker()` is true). Empty
    /// when `start_allowed` is true.
    pub blockers: Vec<String>,
}

/// One resolved required symbol/timeframe pair with explicit provenance
/// (WATCHLIST-INGEST-PLAN-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPlanSymbolTimeframe {
    pub symbol: String,
    pub timeframe: String,
    /// Same value as the response's top-level `symbol_source` — repeated
    /// per-entry so a future per-symbol timeframe/source split does not
    /// require a breaking schema change.
    pub source: String,
}

/// Static description of how this ingest plan relates to existing
/// market-data surfaces (WATCHLIST-INGEST-PLAN-01). Both fields are
/// currently always `true`: every required symbol is checked by the
/// `market_data_readiness` premarket gate, and `md_bars` is the canonical
/// store that gate reads from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPlanCoverageExpectation {
    pub uses_market_data_readiness: bool,
    pub uses_md_bars: bool,
}

/// Response for `GET /api/v1/market-data/ingest-plan` (WATCHLIST-INGEST-PLAN-01).
///
/// Answers, in one place: which symbols/timeframe does the bot require for
/// trading readiness, where did that list come from, and which source
/// should premarket ingest and the GUI coverage panel expect. Reuses
/// [`crate::market_data_freshness::required_symbols_with_source_from_env`] —
/// the exact resolver `PREMARKET-DATA-READINESS-GATE-01` already uses — so
/// this surface and the readiness gate can never disagree.
///
/// # `truth_state` values
/// - `"active"` — at least one required symbol resolved from a usable source.
/// - `"not_configured"` — no timeframe and/or no symbol source is configured;
///   mirrors the readiness gate's `"not_applicable"`.
/// - `"degraded"` — a watchlist path is configured but is not the active
///   source (e.g. `Missing`/`Invalid`/`LoadedNotApproved`/v1); the plan still
///   resolved symbols via fallback, but the operator's intended source is
///   not actually in effect. See `warnings` for why.
///
/// # Safety invariants
/// - Read-only. No DB, no provider/broker calls, no network access.
/// - Does not touch live/paper execution state. No arm_state required.
/// - Never uses the full instrument registry as the required-symbol source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPlanResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// `"watchlist_v2"` | `"env_strategy_symbol"` | `"none"`.
    pub symbol_source: String,
    /// Normalized (trimmed, uppercased, deduped) required symbols, in
    /// resolution order. Empty when `symbol_source == "none"`.
    pub required_symbols: Vec<String>,
    /// Shared timeframe applied to every required symbol, or `None` when
    /// `MQK_STRATEGY_MD_TIMEFRAME` is not configured.
    pub timeframe: Option<String>,
    /// One row per required symbol, each carrying its own timeframe/source.
    pub required_symbol_timeframes: Vec<IngestPlanSymbolTimeframe>,
    pub coverage_expectation: IngestPlanCoverageExpectation,
    /// Operator-readable explanations, e.g. a configured-but-unusable
    /// watchlist artifact, or a missing shared timeframe. Empty when the
    /// resolution is unambiguous.
    pub warnings: Vec<String>,
    pub checked_at_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeErrorResponse {
    pub error: String,
    pub fault_class: String,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightStatusResponse {
    pub daemon_reachable: bool,
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub db_reachable: Option<bool>,
    pub broker_config_present: Option<bool>,
    pub market_data_config_present: Option<bool>,
    pub audit_writer_ready: Option<bool>,
    pub runtime_idle: Option<bool>,
    pub strategy_disarmed: bool,
    pub execution_disarmed: bool,
    pub live_routing_disabled: bool,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    // AUTON-TRUTH-02: Autonomous-paper readiness fields.
    //
    // Populated only for the canonical Paper+Alpaca deployment.
    // All fields are `None` / empty for other deployments — they carry no
    // meaning and must not be interpreted as pass/fail on non-paper+alpaca.
    /// True only for Paper+Alpaca. Determines whether the fields below apply.
    pub autonomous_readiness_applicable: bool,
    /// WS continuity proven: `Some(true)` only when `alpaca_ws_continuity == "live"`.
    /// `None` when not paper+alpaca.
    pub ws_continuity_ready: Option<bool>,
    /// Reconcile not dirty/stale: `Some(true)` when reconcile is neither "dirty" nor "stale".
    /// `None` when not paper+alpaca.
    pub reconcile_ready: Option<bool>,
    /// Autonomous arm state: `"armed"` | `"arm_pending"` | `"halted"` | `"not_applicable"`.
    ///
    /// - `"armed"` — in-memory integrity is armed; start can proceed.
    /// - `"arm_pending"` — disarmed in memory but not halted; the session
    ///   controller will call `try_autonomous_arm` (DB-ARMED → advances to armed).
    /// - `"halted"` — operator halt asserted; requires manual operator arm.
    /// - `"not_applicable"` — not paper+alpaca.
    pub autonomous_arm_state: String,
    /// Exact autonomous-paper blockers derived from the same gate order as
    /// `start_execution_runtime`.  Empty when not paper+alpaca or when all
    /// checks pass.  These are operator-actionable reasons why the next
    /// autonomous start attempt will refuse.
    pub autonomous_blockers: Vec<String>,
    /// Whether the current wall-clock time is inside the autonomous session window.
    /// `Some(true)` = in window, `Some(false)` = outside window.
    /// `None` when not paper+alpaca.
    pub session_in_window: Option<bool>,
    // C2: Live-trust truth on the preflight surface.
    //
    // Mirrors the same two fields added to `SystemStatusResponse` by C1.
    // Preflight is the primary pre-start operator checklist; an operator who
    // consults preflight without also reading `/api/v1/system/status` must
    // still see the explicit live-trust ceiling so that
    // `deployment_start_allowed=true` on a live-shadow or live-capital
    // deployment cannot be mistaken for live-trust being established.
    //
    // Values are derived from the same `evaluate_parity_evidence_guarded()`
    // call used by C1 and the dedicated parity-evidence route.
    //
    /// Machine-readable parity evidence state.
    ///
    /// `"not_configured"` | `"absent"` | `"invalid"` |
    /// `"incomplete"` | `"complete"` | `"unavailable"`
    ///
    /// Always present (structural field).  `"not_configured"` is the honest
    /// ceiling when no artifact path is set; `"incomplete"` means evidence
    /// exists but `live_trust_complete=false` in this build.
    pub parity_evidence_state: String,
    /// Explicit live-trust boolean derived from parity evidence.
    ///
    /// `Some(false)` when evidence is present but incomplete (current builds).
    /// `None` for every non-Present outcome — null is never a positive trust
    /// claim on this surface.
    pub live_trust_complete: Option<bool>,
    // DATA-FRESHNESS-READINESS-GATE-01: market-data freshness.
    //
    // Populated only for Paper+Alpaca deployments where both MQK_STRATEGY_SYMBOL
    // and MQK_STRATEGY_MD_TIMEFRAME are configured.  `null` for other deployments
    // or when env vars are absent.
    /// Market-data freshness status for the configured strategy symbol/timeframe.
    ///
    /// `null` when not applicable (non-paper+alpaca or env vars absent).
    pub market_data_freshness: Option<MarketDataFreshnessStatus>,
    // PREMARKET-DATA-READINESS-GATE-01: multi-symbol premarket readiness.
    /// Aggregate market-data readiness across every required symbol (the
    /// approved watchlist-v2 artifact's symbols, or the legacy single
    /// `MQK_STRATEGY_SYMBOL`). Same gate `start_execution_runtime` enforces.
    ///
    /// `null` when not applicable (non-paper+alpaca).
    pub market_data_readiness: Option<MultiSymbolFreshnessReport>,
}

// ---------------------------------------------------------------------------
// STRATEGY-DECISION-OBSERVABILITY-01: strategy diagnostic snapshot
// ---------------------------------------------------------------------------

/// Read-only diagnostic snapshot exposing the exact intraday scalper signal
/// decision from the most recent bar dispatch.
///
/// Operators can check `move_bps`, `threshold_bps`, and `gap_to_threshold_bps`
/// when `last_bar_signal_qty == 0` to understand why no signal fired.
///
/// All price values are in micros (1 USD = 1_000_000).  `None` fields indicate
/// the value was not computable (e.g. insufficient bars).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDecisionDiagnostics {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    /// Number of lookback bars required for a valid signal.
    pub lookback_bars: u64,
    /// Minimum absolute displacement in basis points required for a signal.
    pub threshold_bps: i64,
    /// Unix-second timestamp of the latest bar used in the dispatch.
    pub latest_bar_ts: Option<i64>,
    /// Close price of the latest bar in micros (1 USD = 1_000_000).
    pub latest_close_micros: Option<i64>,
    /// Unix-second timestamp of the lookback anchor bar.
    pub lookback_bar_ts: Option<i64>,
    /// Close price of the lookback anchor bar in micros.
    pub lookback_close_micros: Option<i64>,
    /// Signed displacement in basis points: `(latest_close - lookback_close) * 10_000 / lookback_close`.
    pub move_bps: Option<i64>,
    /// Absolute value of `move_bps`.
    pub abs_move_bps: Option<i64>,
    /// `threshold_bps - abs_move_bps`.  Positive = still below threshold.
    /// Zero or negative = threshold met or exceeded.
    pub gap_to_threshold_bps: Option<i64>,
    /// Raw strategy direction: `+1` (bullish), `0` (neutral), `-1` (bearish).
    pub raw_direction: i64,
    /// One of: `"signal_long"` | `"flat_due_to_negative_direction"` |
    /// `"flat_below_threshold"` | `"insufficient_bars"`.
    pub decision: String,
    /// Human-readable reason for the decision.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// AUTON-TRUTH-01: GET /api/v1/autonomous/readiness
// ---------------------------------------------------------------------------

/// Autonomous-paper readiness truth surface.
///
/// Surfaces the real gate state that governs whether the session controller
/// can start an execution run on the canonical Paper+Alpaca path.  All field
/// values are derived directly from live daemon state; nothing is synthesised.
///
/// `truth_state`:
/// - `"active"` — deployment is Paper+Alpaca; all fields are authoritative.
/// - `"not_applicable"` — deployment is not Paper+Alpaca; autonomous readiness
///   does not apply.  All boolean fields are `false`; `blockers` contains
///   a single explanatory entry.
///
/// `overall_ready` is the conjunction of all individual readiness flags.  Only
/// `true` when every gate that `start_execution_runtime` enforces would pass
/// right now.  `false` does NOT mean the system is broken — it means at least
/// one gate would refuse start in its current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousPaperReadinessResponse {
    pub canonical_route: String,
    /// `"active"` for paper+alpaca; `"not_applicable"` otherwise.
    pub truth_state: String,
    /// True when deployment is Paper+Alpaca (the canonical autonomous path).
    pub canonical_path: bool,
    /// Alpaca WS continuity: `"live"` | `"cold_start_unproven"` | `"gap_detected"` | `"not_applicable"`.
    pub ws_continuity: String,
    /// True only when `ws_continuity == "live"` (BRK-00R-04 gate).
    pub ws_continuity_ready: bool,
    /// Reconcile status: `"ok"` | `"dirty"` | `"stale"` | `"unknown"`.
    pub reconcile_status: String,
    /// True when reconcile is not `"dirty"` or `"stale"` (BRK-09R gate).
    pub reconcile_ready: bool,
    /// Autonomous supervisory state from `AppState::autonomous_session_truth()`.
    /// `"clear"` | `"start_refused"` | `"recovery_retrying"` | `"recovery_succeeded"`
    /// | `"recovery_failed"` | `"ws_gap_partial_recovery"` | `"run_ended_unexpectedly"`
    /// | `"stop_failed"` | `"stopped_at_boundary"` | `"not_applicable"`.
    /// `"ws_gap_partial_recovery"` (BRK-GAP-01): WS gap detected; fill recovery
    /// available via REST; non-fill lifecycle events permanently unrecoverable;
    /// operator reconcile/repair required.
    pub autonomous_session_state: String,
    /// Human-readable detail from the current autonomous supervisory truth, if any.
    pub autonomous_session_detail: Option<String>,
    /// Integrity arm state as known in-memory.
    /// `"armed"` | `"arm_pending"` | `"halted"` | `"not_applicable"`.
    pub arm_state: String,
    /// True when in-memory integrity is armed (`arm_state == "armed"`).
    pub arm_ready: bool,
    /// True when `ExternalSignalIngestion` is configured (always true for paper+alpaca).
    pub signal_ingestion_configured: bool,
    /// True when the current wall-clock time is inside the configured autonomous session
    /// window (NYSE regular session hours or the fixed UTC window from env vars).
    /// False when outside the window — the session controller will not attempt a start.
    pub session_in_window: bool,
    /// Human-readable session-window state: `"in_window"` | `"outside_window"`.
    pub session_window_state: String,
    /// True when no locally-owned execution run is active (`locally_owned_run_id()` returns
    /// `None`).  False means a run is already active; start would return 409 Conflict.
    pub runtime_start_allowed: bool,
    /// Exact reasons why an autonomous start would be refused right now, in gate order.
    /// Empty when all checks pass.
    pub blockers: Vec<String>,
    /// True only when every readiness gate would pass: ws_continuity_ready &&
    /// reconcile_ready && arm_ready && signal_ingestion_configured &&
    /// session_in_window && runtime_start_allowed.
    pub overall_ready: bool,
    /// AUTON-HIST-01: True when at least one autonomous session event could not
    /// be persisted (no DB configured or DB write failure).  Sticky — never
    /// cleared in-session.  When true, `/api/v1/events/feed` autonomous-session
    /// history is incomplete or absent.  Operator must restart with a working DB.
    pub autonomous_history_degraded: bool,
    /// AUTON-NO-TRADE-01: Current NYSE market-session classification as seen by
    /// the autonomous bar ticker (Gate 2).
    ///
    /// One of `"regular"` | `"premarket"` | `"after_hours"` | `"closed"`.
    /// `"regular"` means the bar ticker will deposit on its next interval tick.
    /// Any other value means Gate 2 is blocking all bar deposits.
    /// `"not_applicable"` for non-paper+alpaca deployments.
    pub nyse_market_session: String,
    /// AUTON-NO-TRADE-01: Bar ticker Gate 2 state derived from `nyse_market_session`.
    ///
    /// `"open"` when `nyse_market_session == "regular"` — bar deposits allowed.
    /// `"closed_outside_session"` when the NYSE market is not in regular session
    /// — bar deposits are blocked regardless of arm/WS/run state.
    /// `"not_applicable"` for non-paper+alpaca deployments.
    pub bar_ticker_gate: String,
    /// AUTON-NO-TRADE-02: Count of bar ticks dispatched to the native strategy
    /// this session.
    ///
    /// `null` when `ExternalSignalIngestion` is not configured (not applicable).
    /// Zero at session start.  Non-zero proves the strategy is being invoked.
    pub bar_tick_dispatch_count: Option<u64>,
    /// AUTON-NO-TRADE-02: Sum of target quantities returned by the native strategy
    /// on the last bar dispatch.
    ///
    /// `null` when no bar has been dispatched this session.
    /// Zero means the strategy returned hold/flat on the last tick — either
    /// the lookback window is insufficiently populated or the bar is not complete
    /// (`is_complete == false` because no price reference was available).
    pub last_bar_signal_qty: Option<i64>,
    /// AUTON-SIGNAL-CONTEXT-01: Source of the bar context used in the most recent dispatch.
    ///
    /// `"db_loaded"` — context was built from completed bars fetched from `md_bars`.
    /// `"stub_no_price"` — fallback single-bar stub with `is_complete=false` was used.
    /// `"no_dispatch_yet"` — no bar has been dispatched this session.
    /// `"not_applicable"` — not a paper+alpaca deployment.
    pub bar_context_source: String,
    /// AUTON-SIGNAL-CONTEXT-01: Number of completed DB bars used in the last dispatch.
    ///
    /// `null` when `bar_context_source != "db_loaded"`.
    /// Non-null and ≥ 0 when DB bars were loaded; this is the actual window size.
    pub bar_context_bars_loaded: Option<u64>,

    // --- OBS-SESSION-DISCORD-01: Session-window diagnostics ---
    /// RFC 3339 timestamp of when this response was generated (UTC).
    pub now_utc: String,
    /// Effective session start time as `"HH:MM UTC"` when a fixed env-window is
    /// active; `null` when the NYSE regular-session seam is used (the boundary
    /// depends on the market calendar, not a fixed time).
    pub session_start_utc: Option<String>,
    /// Effective session stop time as `"HH:MM UTC"` when a fixed env-window is
    /// active; `null` for the NYSE regular-session seam.
    pub session_stop_utc: Option<String>,
    /// `"env"` when `MQK_SESSION_START_HH_MM` and `MQK_SESSION_STOP_HH_MM` are
    /// both set and valid; `"default"` when absent or unparseable (falls back to
    /// NYSE regular-session seam).
    pub session_window_source: String,
    /// Always `"UTC"` — all session times in this response are UTC.
    pub session_window_basis: String,
    /// Raw value of `MQK_SESSION_START_HH_MM` as read from the environment, or
    /// `null` if the variable is absent or empty.
    pub session_start_env_raw: Option<String>,
    /// Raw value of `MQK_SESSION_STOP_HH_MM` as read from the environment, or
    /// `null` if the variable is absent or empty.
    pub session_stop_env_raw: Option<String>,
    // DATA-FRESHNESS-READINESS-GATE-01: market-data freshness.
    /// Market-data freshness status for the configured strategy symbol/timeframe.
    ///
    /// `null` when not applicable (non-paper+alpaca or env vars absent).
    pub market_data_freshness: Option<MarketDataFreshnessStatus>,
    // PREMARKET-DATA-READINESS-GATE-01: multi-symbol premarket readiness.
    /// Aggregate market-data readiness across every required symbol (the
    /// approved watchlist-v2 artifact's symbols, or the legacy single
    /// `MQK_STRATEGY_SYMBOL`). Factored into `overall_ready` and `blockers`
    /// — the same gate `start_execution_runtime` enforces.
    ///
    /// `null` when not applicable (non-paper+alpaca).
    pub market_data_readiness: Option<MultiSymbolFreshnessReport>,
    // STRATEGY-DECISION-OBSERVABILITY-01: signal decision diagnostics.
    /// Read-only diagnostic snapshot from the most recent native strategy bar dispatch.
    ///
    /// `null` when no bar has been dispatched this session or the deployment is
    /// not paper+alpaca.  Non-null exposes the exact decision path: move_bps,
    /// threshold_bps, gap_to_threshold_bps, and decision reason.
    pub strategy_decision_diagnostics: Option<StrategyDecisionDiagnostics>,
}

// ---------------------------------------------------------------------------
// PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01: GET /api/v1/autonomous/paper-status
// ---------------------------------------------------------------------------

/// Comprehensive autonomous paper trading status summary.
///
/// Single surface for operator and Claude to inspect all relevant gate state
/// in one request.  Read-only, no DB mutations, no broker calls, no orders.
///
/// `truth_state`:
/// - `"active"` — deployment is Paper+Alpaca; all fields are authoritative.
/// - `"not_applicable"` — deployment is not Paper+Alpaca.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousPaperStatusResponse {
    pub canonical_route: String,
    /// `"active"` for paper+alpaca; `"not_applicable"` otherwise.
    pub truth_state: String,
    /// Deployment mode: `"paper"` | `"live_shadow"` | `"live_capital"`.
    pub mode: String,
    /// Always `false` for paper mode.  Surfaced explicitly so operator can
    /// confirm live routing is not active before starting a paper session.
    pub live_routing_enabled: bool,
    /// Runtime state: `"idle"` | `"running"` | `"halted"` | `"unknown"`.
    pub runtime_status: String,
    /// Arm state: `"armed"` | `"arm_pending"` | `"halted"` | `"disarmed_db"`.
    pub arm_state: String,
    /// `true` when the kill-switch is active (integrity halted).
    pub kill_switch_active: bool,
    /// Deadman heartbeat status: `"healthy"` | `"expired"` | `"inactive"` | `"unavailable"`.
    pub deadman_status: String,
    /// Alpaca WS continuity: `"live"` | `"cold_start_unproven"` | `"gap_detected"`.
    pub ws_continuity: String,
    /// Reconcile status: `"ok"` | `"dirty"` | `"stale"` | `"unknown"`.
    pub reconcile_status: String,
    /// Total mismatch count (positions + orders + fills + unmatched broker events).
    pub mismatch_count: usize,
    /// Live OMS order count from in-memory execution snapshot.
    /// `0` when no execution snapshot is loaded.
    pub open_order_count: usize,
    /// Portfolio position count from in-memory execution snapshot.
    /// `0` when no execution snapshot is loaded.
    pub position_count: usize,
    /// Configured strategy symbol (`MQK_STRATEGY_SYMBOL`).
    /// `null` when the env var is absent or empty.
    pub current_symbol: Option<String>,
    /// Net quantity held for `current_symbol` from the in-memory portfolio snapshot.
    /// `null` when no snapshot is loaded or the symbol has no position.
    pub current_position_qty: Option<i64>,
    /// Target quantity from the most recent strategy bar dispatch.
    /// `null` when no bar has been dispatched this session.
    pub target_qty: Option<i64>,
    /// `target_qty - current_position_qty`.  `null` when either is absent.
    pub computed_delta_qty: Option<i64>,
    /// Why the last bar dispatch did not produce an order.
    /// `null` — not persisted in AppState; available in daemon logs only.
    pub no_order_reason: Option<String>,
    /// Decision from the most recent strategy diagnostic:
    /// `"signal_long"` | `"flat_due_to_negative_direction"` |
    /// `"flat_below_threshold"` | `"insufficient_bars"` | `null`.
    pub last_strategy_decision: Option<String>,
    /// `true` when the flatten-paper-positions route gates would all pass.
    pub flatten_available: bool,
    /// Reasons flatten is blocked, in gate order.  Empty when `flatten_available`.
    pub flatten_blockers: Vec<String>,
    /// `true` — evidence capture routes exist (EVIDENCE-CAPTURE-TRADE-FLOW-01).
    pub evidence_ready: bool,
    /// `true` — GUI trade lifecycle visibility exists (GUI-TRADE-LIFECYCLE-VISIBILITY-01).
    pub gui_visibility_ready: bool,
    /// `true` when `DISCORD_WEBHOOK_URL` is configured, so the Discord
    /// lifecycle alerts implemented under DISCORD-TRADE-LIFECYCLE-REAL-01 will
    /// actually be delivered (best-effort). `false` when unconfigured — the
    /// alert code paths exist but every `notify_*` call is a silent no-op, so
    /// Discord visibility is not operationally ready (DISCORD-LIFECYCLE-OBSERVABILITY-COMPLETION-01).
    pub discord_visibility_ready: bool,
    /// Watchlist intake outcome for the configured artifact:
    /// `"not_configured"` | `"missing"` | `"invalid"` | `"loaded_not_approved"` | `"loaded_approved"`.
    pub watchlist_outcome: String,
    /// `true` when the watchlist artifact is approved for autonomous paper trading.
    pub watchlist_approved: bool,
    /// Readiness classification:
    /// - `"ready_for_market_smoke"` — all start gates pass now.
    /// - `"market_proof_pending"` — code-complete; only outside session window or run active.
    /// - `"blocked"` — one or more hard blockers prevent autonomous start.
    pub readiness_classification: String,
    /// Active start blockers in gate order.  Empty when ready_for_market_smoke.
    pub blockers: Vec<String>,
    /// Recommended operator action given current state.
    pub next_operator_action: String,
    /// Autonomous supervisory state from the session controller:
    /// `"clear"` | `"start_refused"` | `"recovery_retrying"` | `"recovery_succeeded"`
    /// | `"recovery_failed"` | `"ws_gap_partial_recovery"` | `"run_ended_unexpectedly"`
    /// | `"stop_failed"` | `"stopped_at_boundary"` | `"not_applicable"`.
    pub autonomous_session_state: String,
    /// RFC 3339 timestamp when this response was generated (UTC).
    pub now_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummaryResponse {
    pub has_snapshot: bool,
    pub active_orders: usize,
    pub pending_orders: usize,
    pub dispatching_orders: usize,
    pub reject_count_today: usize,
    pub cancel_replace_count_today: Option<usize>,
    pub avg_ack_latency_ms: Option<u64>,
    pub stuck_orders: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSummaryResponse {
    pub has_snapshot: bool,
    /// PORT-05: Machine-readable truth state for operator supervision.
    ///
    /// - `"no_snapshot"` — no broker snapshot is loaded; all financial fields are
    ///   `null`.  Empty portfolio must NOT be inferred from this state.
    /// - `"active"` — a broker snapshot is present; fields derive from it.
    ///
    /// **`session_boundary = "in_memory_only"`** — the broker snapshot is held
    /// in-memory and reset on every daemon restart.  After a restart this surface
    /// returns `"no_snapshot"` until a fresh snapshot is loaded.
    pub truth_state: String,
    pub account_equity: Option<f64>,
    pub cash: Option<f64>,
    pub long_market_value: Option<f64>,
    pub short_market_value: Option<f64>,
    pub daily_pnl: Option<f64>,
    pub buying_power: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSummaryResponse {
    pub has_snapshot: bool,
    pub gross_exposure: Option<f64>,
    pub net_exposure: Option<f64>,
    pub concentration_pct: Option<f64>,
    pub daily_pnl: Option<f64>,
    pub drawdown_pct: Option<f64>,
    pub loss_limit_utilization_pct: Option<f64>,
    pub kill_switch_active: bool,
    pub active_breaches: usize,
    /// Sticky `RiskState.halted` flag from the live risk gate
    /// (RISK-ENGINE-HALTED-VISIBILITY-01). `None` when the gate cannot
    /// report sticky-halt state (e.g. no execution snapshot yet, or a
    /// fail-closed gate). Distinct from `kill_switch_active`, which derives
    /// from the transient `sys_risk_block_state.blocked` flag reset each
    /// orchestrator tick.
    pub risk_engine_halted: Option<bool>,
    /// Reserved for a future reason code explaining why the risk engine is
    /// sticky-halted. Always `None` today — `RiskState` does not currently
    /// track a reason alongside `halted`.
    pub risk_engine_halt_reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileSummaryResponse {
    /// RECON-06: Machine-readable truth state disambiguating reconcile lifecycle.
    ///
    /// - `"never_run"` — the reconcile loop has not completed a tick since daemon
    ///   start.  `status = "unknown"` in this state.  Not the same as an error.
    /// - `"active"` — reconcile has completed at least one tick; `status` is
    ///   authoritative (`"ok"` or a mismatch count summary).
    /// - `"stale"` — the last reconcile result is too old to be considered
    ///   authoritative; operator must trigger a fresh snapshot.
    pub truth_state: String,
    pub status: String,
    pub last_run_at: Option<String>,
    pub snapshot_watermark_ms: Option<i64>,
    pub mismatched_positions: usize,
    pub mismatched_orders: usize,
    pub mismatched_fills: usize,
    pub unmatched_broker_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileMismatchRow {
    pub id: String,
    pub domain: String,
    pub symbol: String,
    pub internal_value: String,
    pub broker_value: String,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileMismatchesResponse {
    pub truth_state: String,
    pub snapshot_at_utc: Option<String>,
    pub rows: Vec<ReconcileMismatchRow>,
    /// RECON-06: Operator review guidance when mismatches are present.
    ///
    /// `None` when `rows` is empty (no review needed) or when truth_state is
    /// not `"active"` (not authoritative).
    ///
    /// `Some(guidance)` when `rows` is non-empty and truth_state is `"active"`:
    /// the guidance string explicitly names the required operator actions.
    pub review_workflow: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateResponse {
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub deployment_blocker: Option<String>,
    pub operator_auth_mode: String,
    pub strategy_allowed: bool,
    pub execution_allowed: bool,
    pub system_trading_window: String,
    /// Classified market session type: `"regular"` | `"premarket"` | `"after_hours"` | `"closed"`.
    /// For paper/backtest (always-on policy): always `"regular"`.
    pub market_session: String,
    /// Operational exchange calendar state: `"open"` | `"closed"` | `"holiday"`.
    /// For paper/backtest (always-on policy): always `"open"`.
    pub exchange_calendar_state: String,
    /// Stable identifier for the calendar spec driving this session response.
    /// `"always_on"` (paper/backtest) or `"nyse_weekdays"` (live/shadow).
    pub calendar_spec_id: String,
    /// Operator-facing notes describing the authority basis of session truth.
    pub notes: Vec<String>,
    /// C4: Current parity-evidence state on this surface.
    ///
    /// Same values as C1/C2/C3: `"not_configured"` | `"absent"` | `"invalid"` |
    /// `"incomplete"` | `"complete"` | `"unavailable"`.
    ///
    /// `"not_configured"` means `MQK_ARTIFACT_PATH` is unset — not a positive
    /// trust claim.  `"incomplete"` means evidence is present but
    /// `live_trust_complete=false` in the current build.
    pub parity_evidence_state: String,
    /// C4: Whether live-trust is complete in the current build.
    ///
    /// `null` when parity evidence is not present (not_configured / absent /
    /// invalid / unavailable).  `false` in all current builds where the TV-03
    /// parity pipeline has not completed a shadow evaluation cycle.
    ///
    /// `null` is never a positive trust claim.
    pub live_trust_complete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFingerprintResponse {
    /// `"no_db"` = DB pool absent; `config_hash` is sourced from in-memory daemon config only —
    /// no durable comparison baseline is available.  `runtime_generation_id` and
    /// `last_restart_at` will be `null`.
    ///
    /// `"no_run"` = DB pool present but no durable run found; `config_hash` is sourced from
    /// in-memory daemon config — same caveat as `"no_db"`.
    ///
    /// `"active"` = DB pool present and a durable run was found; `config_hash` is sourced
    /// from that run's durable record and is authoritative.
    pub truth_state: String,
    pub config_hash: String,
    pub adapter_id: String,
    pub risk_policy_version: Option<String>,
    pub strategy_bundle_version: Option<String>,
    pub build_version: String,
    pub environment_profile: String,
    pub runtime_generation_id: Option<String>,
    pub last_restart_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffRow {
    pub diff_id: String,
    pub changed_at: String,
    pub changed_domain: String,
    pub before_version: String,
    pub after_version: String,
    pub summary: String,
}

/// Response wrapper for `/api/v1/system/config-diffs`.
///
/// `truth_state`:
/// - `"not_wired"` — the daemon does not have an authoritative comparison
///   baseline available; `backend` is `"not_wired"` and empty `rows` **must
///   not** be treated as authoritative zero.
/// - `"active"` — the daemon compared current runtime-selection truth against
///   the latest durable daemon run in `postgres.runs`; `backend` names the
///   exact authoritative source and `rows` is authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffsResponse {
    /// Stable route identity for downstream callers and tests.
    pub canonical_route: String,
    /// `"not_wired"` = no authoritative comparison baseline is available.
    pub truth_state: String,
    /// `"not_wired"` until the daemon can compare against durable run truth.
    pub backend: String,
    /// Empty when `truth_state == "not_wired"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<ConfigDiffRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryRow {
    /// Sourced: canonical strategy identity from `sys_strategy_registry`.
    pub strategy_id: String,
    /// Sourced: human-readable display name from `sys_strategy_registry.display_name`.
    /// CC-01C: surfaced from durable registry truth.
    pub display_name: String,
    /// Sourced: durable `enabled` flag from `sys_strategy_registry`.
    /// `true` = registered + active; `false` = registered + inactive (known but disabled).
    pub enabled: bool,
    /// Sourced: operator-assigned category from `sys_strategy_registry.kind`.
    /// Empty string when unclassified.  CC-01C: surfaced from durable registry truth.
    pub kind: String,
    /// Sourced: RFC3339 timestamp when this strategy was first registered.
    /// From `sys_strategy_registry.registered_at_utc`.  CC-01C: durable provenance.
    pub registered_at: String,
    /// Sourced: optional operator note from `sys_strategy_registry.note`.
    /// Empty string when none was recorded.  CC-01C: surfaced from durable registry truth.
    pub note: String,
    /// Sourced: reflects the current daemon integrity arm state at response time.
    pub armed: bool,
    /// B2B: Cross-referenced admission state from fleet config × DB registry.
    ///
    /// - `"runnable"` — in configured fleet + registry present + `enabled=true`.
    ///   This strategy can be activated at runtime start.
    /// - `"blocked_disabled"` — in configured fleet + registry present but
    ///   `enabled=false`.  B2A gate will refuse start until re-enabled.
    /// - `"blocked_not_registered"` — in configured fleet but no registry row
    ///   exists.  Daemon was configured for this strategy but it was never
    ///   registered in `sys_strategy_registry`.
    /// - `"not_configured"` — in registry but NOT in the daemon's configured
    ///   fleet (`MQK_STRATEGY_IDS`).  This daemon will not activate it.
    /// - `"no_fleet_configured"` — in registry; `MQK_STRATEGY_IDS` is not set
    ///   so no strategy is admitted for runtime execution by this daemon.
    pub admission_state: String,
    /// `null` — no strategy health monitor is wired; honest null, not synthetic "ok".
    pub health_status: Option<String>,
    /// `null` — universe membership is not tracked by the daemon; honest null.
    pub universe_size: Option<usize>,
    /// `null` — per-strategy outbox query not wired; honest null, not synthetic zero.
    pub pending_intents: Option<usize>,
    /// `null` — strategy-level position attribution not wired; honest null.
    pub open_positions: Option<usize>,
    /// `null` — no strategy-level portfolio accounting wired; honest null, not synthetic zero.
    pub today_pnl: Option<f64>,
    /// `null` — no strategy-level drawdown tracking wired; honest null, not synthetic zero.
    pub drawdown_pct: Option<f64>,
    /// `null` — no regime detector wired; honest null, not synthetic string.
    pub regime: Option<String>,
    /// B3: Wired for the single active fleet strategy; `null` for all others.
    ///
    /// Values when wired:
    /// - `"open"` — within the per-run autonomous signal limit.
    /// - `"day_limit_reached"` — per-run autonomous signal limit exceeded;
    ///   no further signals accepted until next run start.
    ///
    /// `null` when this strategy is not the daemon's single-strategy fleet
    /// target, or when the fleet is not configured.
    pub throttle_state: Option<String>,
    /// B3: RFC3339 timestamp of the last `deposit_strategy_bar_input` call.
    ///
    /// Wired for the single active fleet strategy; `null` otherwise.
    /// `null` also when no bar input has been deposited in this daemon
    /// process lifetime (no signal has been accepted and dispatched yet).
    pub last_decision_time: Option<String>,
    /// AUTON-NO-TRADE-01: Sum of target quantities from the last bar dispatch.
    ///
    /// Wired for the single active fleet strategy; `null` otherwise.
    /// `null` when no bar has been dispatched this session.
    /// Zero means strategy returned no trade signal last tick (hold/flat).
    /// Non-zero means targets were produced (though admission gates may still
    /// filter them before the outbox).
    pub last_bar_signal_qty: Option<i64>,
    /// AUTON-NO-TRADE-01: Total bar ticks dispatched to this strategy this session.
    ///
    /// Wired for the single active fleet strategy; `null` otherwise.
    /// Zero at session start; increments each time `tick_strategy_dispatch`
    /// fires a bar result.  Non-zero proves the strategy is being invoked.
    pub bar_tick_dispatch_count: Option<u64>,
}

/// Response wrapper for `/api/v1/strategy/summary`.
///
/// `truth_state` (CC-01B):
/// - `"no_db"` — DB unavailable; `rows` is empty and **must not** be treated as
///   authoritative.  Fail-closed: callers must not infer "no active strategies"
///   from this state.
/// - `"registry"` — reading from `postgres.sys_strategy_registry`; `rows` are
///   authoritative.  Empty `rows` means no strategies have been registered
///   (authoritative empty ≠ unavailable).  Each row carries the durable
///   `enabled` flag and B2B `admission_state`.
///
/// `runtime_execution_mode` (B2B):
/// - `"single_strategy"` — fleet configured with exactly one strategy ID.
/// - `"fleet_not_configured"` — `MQK_STRATEGY_IDS` not set or empty; daemon
///   operates in Dormant bootstrap mode.
/// - `"fleet"` — fleet configured with two or more strategies (informational;
///   runtime execution remains single-strategy at this revision).
/// - `"unknown"` — DB unavailable; fleet truth may be partially derivable from
///   env but runtime truth is not confirmable without DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryResponse {
    pub canonical_route: String,
    pub backend: String,
    /// `"no_db"` = DB unavailable; rows empty and not authoritative (fail closed).
    /// `"registry"` = reading from postgres.sys_strategy_registry; rows authoritative.
    pub truth_state: String,
    /// B2B: Execution mode label for the configured fleet.
    /// `"single_strategy"` | `"fleet_not_configured"` | `"fleet"` | `"unknown"`.
    pub runtime_execution_mode: String,
    /// B2B: Number of strategies in the daemon's configured fleet (`MQK_STRATEGY_IDS`).
    /// `null` when `truth_state == "no_db"` (cannot confirm).
    /// `0` when fleet is configured but empty.
    pub configured_fleet_size: Option<usize>,
    /// Empty when `truth_state == "no_db"`.  Authoritative when `truth_state == "registry"`.
    /// Includes synthetic rows for fleet entries with no registry record
    /// (`admission_state == "blocked_not_registered"`).
    pub rows: Vec<StrategySummaryRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySuppressionRow {
    pub suppression_id: String,
    pub strategy_id: String,
    pub state: String,
    pub trigger_domain: String,
    pub trigger_reason: String,
    pub started_at: String,
    pub cleared_at: Option<String>,
    pub note: String,
}

/// Response wrapper for `/api/v1/strategy/suppressions`.
///
/// `truth_state`:
/// - `"no_db"` — no DB pool configured; source unavailable; rows is empty and
///   **must not** be treated as authoritative zero.
/// - `"active"` — DB present; rows are authoritative.  Empty `rows` means
///   no suppressions exist.  Non-empty rows are real durable records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySuppressionsResponse {
    pub canonical_route: String,
    pub backend: String,
    /// `"no_db"` = DB unavailable; rows empty and not authoritative.
    /// `"active"` = DB present; rows authoritative (empty = no suppressions).
    pub truth_state: String,
    /// Empty when `truth_state == "no_db"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<StrategySuppressionRow>,
}

/// Response wrapper for `GET /api/v1/strategy/multi-symbol-dispatch-summary`.
///
/// Read-only daemon runtime surface backed by in-memory
/// [`crate::state::PerSymbolTargetState`] rows. Empty `per_symbol` with
/// `truth_state = "no_snapshot"` means no target-state snapshot has been
/// recorded yet; callers must not treat it as a successful zero-position state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MultiSymbolDispatchSummaryResponse {
    pub canonical_route: String,
    pub backend: String,
    pub truth_state: String,
    pub runtime_execution_mode: String,
    pub configured_symbol_count: usize,
    pub per_symbol: Vec<PerSymbolDispatchRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerSymbolDispatchRow {
    pub symbol: String,
    pub strategy_id: String,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta: i64,
    pub no_order_reason: String,
    pub last_decision_id: Option<String>,
    pub last_decision_disposition: Option<String>,
    pub day_order_count: u32,
    pub day_order_limit: Option<u32>,
    pub bar_staleness_secs: Option<i64>,
}

// ---------------------------------------------------------------------------
// /api/v1/strategy/dry-run/status (MULTI-STRATEGY-DRY-RUN-STATUS-01)
// ---------------------------------------------------------------------------

/// One dry-run secondary-strategy diagnostic, mapped field-for-field from
/// [`crate::state::DryRunStrategyDiagnostic`].
///
/// `submitted` is always `false` — dry-run strategies never reach the outbox
/// or broker; see `state/dry_run_strategy.rs` for the structural proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DryRunStrategyDiagnosticRow {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta_qty: i64,
    /// One of: `"already_at_target"`, `"b5_short_sale_guard"`,
    /// `"order_would_be_submitted"`, `"evaluation_unavailable"`.
    pub decision: String,
    pub reason: String,
    /// Stable label for the classified order intent (e.g. `"ShortOpen"`,
    /// `"LongOpen"`, `"SellToClose"`, `"NoOp"`), or `"unavailable"`.
    pub would_classify_as: String,
    /// `true` when the B5 short-sale guard in `bar_result_to_decisions` would
    /// drop this intent before it reaches the outbox.
    pub would_b5_block: bool,
    /// `true` when the default fail-closed short-entry policy would also
    /// block this intent. Always `false` for non-short-open intents.
    pub would_policy_block: bool,
    /// Machine-readable short-entry policy block reason (e.g.
    /// `"short_entries_disabled"`). `None` when `would_policy_block` is `false`.
    pub policy_reason_code: Option<String>,
    /// Always `false`. Dry-run strategies never submit orders.
    pub submitted: bool,
    /// RFC3339 timestamp shared by every row in the same snapshot — the wall
    /// clock time the daemon last replaced the dry-run diagnostic snapshot,
    /// not a per-strategy evaluation time (all strategies in one snapshot are
    /// evaluated within the same execution-loop tick).
    pub evaluated_at_utc: String,
}

/// Response wrapper for `GET /api/v1/strategy/dry-run/status`.
///
/// Read-only daemon runtime surface backed by the in-memory latest dry-run
/// diagnostic snapshot (`AppState::dry_run_diagnostics`). This route never
/// touches the broker, the outbox, or any submission path — it only reads a
/// snapshot that `state/dry_run_strategy.rs` and `state/loop_runner.rs`
/// already prove cannot contain a submitted order.
///
/// `truth_state == "not_configured"`: `MQK_DRY_RUN_STRATEGY_IDS` is unset or
/// blank — `dry_run_strategy_diagnostics` is always empty (default-off;
/// existing single-strategy behavior unchanged).
///
/// `truth_state == "active"`: one or more dry-run strategy ids are
/// configured. `dry_run_strategy_diagnostics` is empty until the first
/// execution-loop tick evaluates them, then holds the latest snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DryRunStrategyStatusResponse {
    pub canonical_route: String,
    pub backend: String,
    pub truth_state: String,
    /// Strategy ids from `MQK_DRY_RUN_STRATEGY_IDS`, in configured order.
    /// Empty when dry-run is not configured.
    pub configured_dry_run_strategy_ids: Vec<String>,
    pub dry_run_strategy_diagnostics: Vec<DryRunStrategyDiagnosticRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/runtime-leadership
// ---------------------------------------------------------------------------

/// One durable checkpoint event in the runtime lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeadershipCheckpointRow {
    pub checkpoint_id: String,
    /// "restart" | "leader_acquired" | "leader_lost" | "recovery_complete" | "snapshot_refresh"
    pub checkpoint_type: String,
    pub timestamp: String,
    pub generation_id: String,
    pub leader_node: String,
    /// "ok" | "warning" | "critical"
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeadershipResponse {
    /// "local" for a single-node daemon; cluster node identifier otherwise.
    pub leader_node: String,
    /// "held" = running and owns the lease; "contested" = unknown state;
    /// "lost" = idle or halted.
    pub leader_lease_state: String,
    /// Unique identifier for the current runtime generation when authoritative
    /// runtime state exists. `null` when no active run or durable latest-run
    /// record is available; the daemon must not fabricate a placeholder ID.
    pub generation_id: Option<String>,
    /// Count of run starts in the last 24 h, sourced from the `runs` table
    /// (`started_at_utc > now() - interval '24 hours'`).
    /// `null` when no DB pool is configured; a real authoritative count otherwise.
    pub restart_count_24h: Option<u32>,
    /// UTC timestamp of the most recent run start, if known.
    pub last_restart_at: Option<String>,
    /// "complete" = reconcile confirmed clean post-restart;
    /// "in_progress" = reconcile not yet finished;
    /// "degraded" = reconcile found mismatches or is stale.
    pub post_restart_recovery_state: String,
    /// Reconcile timestamp or "none" when reconcile has not yet run.
    pub recovery_checkpoint: String,
    /// Ordered lifecycle checkpoint events (empty when DB unavailable).
    pub checkpoints: Vec<RuntimeLeadershipCheckpointRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/metadata — asset capability matrix (ASSET-CAPABILITY-MATRIX-01)
// ---------------------------------------------------------------------------

/// Per-asset-class capability record.  All fields are static (compile-time
/// truth); none are derived from runtime state or broker connectivity.
///
/// `live_ready` is `false` for every asset class, including US equities, until
/// a dedicated live-readiness review patch explicitly promotes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetCapabilityEntry {
    /// Asset class identifier (snake_case, e.g. "us_equities").
    pub asset_class: String,
    /// `true` only if this asset class is wired for execution.  All non-equity
    /// classes are `false` by default and must not be enabled without a named
    /// promotion gate patch.
    pub enabled: bool,
    /// `true` only if paper execution for this asset class has been proven.
    pub paper_ready: bool,
    /// `false` for every class until a live-readiness review explicitly sets it.
    pub live_ready: bool,
    /// Broker adapter identifier, or `"none"` when no adapter is wired.
    pub broker_adapter: String,
    /// Human-readable status note for operator surfaces.
    pub notes: String,
}

/// Static asset capability matrix returned in `/api/v1/system/metadata`.
///
/// This is read-only metadata; it is never used for order routing or dispatch.
/// The matrix is built from compile-time constants — it never reads env vars,
/// DB state, or runtime flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetCapabilityMatrix {
    /// Schema version for forward-compatibility parsing.
    pub schema_version: String,
    /// Source label confirming this matrix is statically defined.
    pub static_source: String,
    /// `false` globally — live capital requires a separate named review patch.
    pub live_capital_ready: bool,
    /// Asset classes where `enabled == true` in this build.
    pub default_enabled_asset_classes: Vec<String>,
    /// Asset classes where `enabled == false` in this build.
    pub disabled_asset_classes: Vec<String>,
    /// Full per-class capability records.
    pub entries: Vec<AssetCapabilityEntry>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/metadata
// ---------------------------------------------------------------------------

/// Canonical system metadata surface.  All fields are derived from durable
/// daemon state at request time; no placeholders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetadataResponse {
    /// Daemon binary version from the build manifest.
    pub build_version: String,
    /// API version in use (currently "v1").
    pub api_version: String,
    /// Active broker adapter identifier (e.g. "paper", "alpaca").
    pub broker_adapter: String,
    /// Overall daemon endpoint health: "ok" if armed, "warning" otherwise.
    pub endpoint_status: String,
    /// Deployment mode label (paper/live/backtest).
    pub daemon_mode: String,
    /// Adapter ID — mirrors broker_adapter for GUI convenience.
    pub adapter_id: String,
    /// Static asset capability matrix (ASSET-CAPABILITY-MATRIX-01).
    /// Read-only metadata; not used for routing or dispatch.
    pub asset_capability_matrix: AssetCapabilityMatrix,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/action  — canonical operator action dispatcher
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/ops/action.
/// `action_key` is the canonical GUI action identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsActionRequest {
    /// Canonical action key: "arm-execution", "arm-strategy", "disarm-execution",
    /// "disarm-strategy", "start-system", "stop-system", "kill-switch",
    /// "request-mode-change" (persists restart intent when admissible),
    /// "cancel-mode-transition" (cancels a pending restart intent),
    /// "change-system-mode" (returns 409 — guidance only, preserved for compat),
    /// "flatten-paper-positions" (paper-only operator flatten via OMS/outbox path).
    pub action_key: String,
    /// Optional reason string for audit trail. Not required by the dispatcher.
    pub reason: Option<String>,
    /// Required for "request-mode-change": target deployment mode label.
    /// One of: "paper", "live-shadow", "live-capital", "backtest".
    pub target_mode: Option<String>,
    /// Optional symbol filter for "flatten-paper-positions".
    /// When present, only that symbol is flattened.
    /// When absent, all non-flat positions are flattened.
    pub symbol: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/repair/outbox-ambiguous — OPS-REPAIR-01
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/ops/repair/outbox-ambiguous.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxRepairRequest {
    /// Idempotency key of the AMBIGUOUS outbox row to release.
    pub idempotency_key: String,
}

/// Response for POST /api/v1/ops/repair/outbox-ambiguous.
#[derive(Debug, Clone, Serialize)]
pub struct OutboxRepairResponse {
    /// `true` if the row was released; `false` if refused.
    pub accepted: bool,
    /// `"released"` or `"refused"`.
    pub decision: String,
    pub idempotency_key: String,
    /// Human-readable summary of the evidence or refusal reason.
    pub evidence: String,
    /// Refusal gate name if `accepted == false`.
    pub gate: Option<String>,
    /// Durable audit event UUID written for this action (if DB and run_id available).
    pub audit_event_id: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/repair/halted-run-fill-plan — BROKER-FILL-REPLAY-REPAIR-01
// ---------------------------------------------------------------------------

/// Classification of one stale broker-order-map entry for a HALTED run.
///
/// `"unapplied_inbox_fill"` — unapplied fill row exists in `oms_inbox`.
/// `"cursor_only_fill_evidence"` — no inbox row but the broker cursor confirms
///   a fill event matching `broker_order_id` was received.
/// `"no_fill_evidence"` — broker_order_map entry present but no fill evidence in
///   either `oms_inbox` or the broker cursor.
/// `"ambiguous"` — cannot classify safely; operator investigation required.
///
/// Only `"unapplied_inbox_fill"` and `"cursor_only_fill_evidence"` indicate a
/// fill that should be reflected in portfolio state.
#[derive(Debug, Clone, Serialize)]
pub struct HaltedRunFillEntry {
    /// Internal order ID (= `oms_outbox.idempotency_key`).
    pub internal_order_id: String,
    /// Exchange-assigned broker order ID.
    pub broker_order_id: String,
    /// Run that owns this order.
    pub run_id: String,
    /// `oms_outbox.status` at time of query.
    pub outbox_status: String,
    /// When the run was halted (ISO-8601).
    pub halted_at_utc: Option<String>,
    /// Number of unapplied inbox rows (`applied_at_utc IS NULL`) for this run.
    pub unapplied_inbox_count: usize,
    /// Unapplied inbox row event kinds present (e.g. `["fill"]`, `["ack", "fill"]`).
    pub unapplied_inbox_event_kinds: Vec<String>,
    /// `true` when the broker event cursor `last_message_id` contains the
    /// `broker_order_id`, confirming the broker processed this order's fill.
    pub cursor_fill_evidence: bool,
    /// The `last_message_id` from the broker cursor at query time, if present.
    pub cursor_last_message_id: Option<String>,
    /// Classification of this entry's repair state.
    ///
    /// One of: `"unapplied_inbox_fill"`, `"cursor_only_fill_evidence"`,
    /// `"no_fill_evidence"`, `"ambiguous"`.
    pub classification: String,
    /// Operator action description prescribed by the planner.
    pub prescribed_action: String,
    /// `true` if a safe mutation path is known (i.e. `BROKER-FILL-REPLAY-APPLY-01`
    /// would be safe to execute for this entry).  Currently always `false` —
    /// mutation is deferred to `BROKER-FILL-REPLAY-APPLY-01`.
    pub mutation_safe: bool,
}

/// Response for GET /api/v1/ops/repair/halted-run-fill-plan.
///
/// Dry-run planner: identifies stale broker-order-map entries for HALTED runs
/// and classifies whether a fill event was received but not applied.
///
/// No DB state is modified by this route.
///
/// `truth_state` values:
/// - `"active"` — plan computed successfully.
/// - `"no_db"` — daemon has no DB; plan cannot be computed.
/// - `"backend_unavailable"` — DB query failed.
#[derive(Debug, Clone, Serialize)]
pub struct HaltedRunFillPlanResponse {
    pub truth_state: String,
    /// All stale broker_order_map entries for HALTED runs found at query time.
    /// Empty when no stale entries exist.
    pub entries: Vec<HaltedRunFillEntry>,
    /// Human-readable summary for operator UI.
    pub summary: String,
    /// `true` when at least one entry has `classification` of
    /// `"unapplied_inbox_fill"` or `"cursor_only_fill_evidence"`.
    pub repair_required: bool,
    /// Follow-up patch required for mutation: `"BROKER-FILL-REPLAY-APPLY-01"`.
    /// `None` when `repair_required == false`.
    pub follow_up_patch: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/repair/halted-run-fill-apply — BROKER-FILL-REPLAY-APPLY-01
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/ops/repair/halted-run-fill-apply.
#[derive(Debug, Clone, Deserialize)]
pub struct HaltedRunFillApplyRequest {
    pub run_id: String,
    pub internal_order_id: String,
    pub broker_order_id: String,
    /// When `true` (default), no state is mutated; the response describes
    /// what would happen.  When `false`, `confirmation` is required.
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Must equal `"APPLY_HALTED_FILL_REPAIR"` when `dry_run = false`.
    pub confirmation: Option<String>,
}

/// Response for POST /api/v1/ops/repair/halted-run-fill-apply.
#[derive(Debug, Clone, Serialize)]
pub struct HaltedRunFillApplyResponse {
    /// `"active"`, `"no_db"`, or `"backend_unavailable"`.
    pub truth_state: String,
    /// `"applied"`, `"dry_run_ok"`, `"already_repaired"`, `"refused"`, or `"noop"`.
    pub decision: String,
    pub dry_run: bool,
    pub run_id: String,
    pub internal_order_id: String,
    pub broker_order_id: String,
    /// Classification derived from the planner logic.
    pub classification: String,
    /// Human-readable explanation of the decision.
    pub evidence: String,
    /// Gate name when `decision = "refused"`.
    pub gate: Option<String>,
    /// Durable audit event UUID written for this action.
    pub audit_event_id: Option<String>,
    /// Follow-up patch required if further recovery is needed.
    pub follow_up_patch: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/repair/halted-run-fill-rest-recovery — BROKER-FILL-REST-RECOVERY-APPLY-01
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/ops/repair/halted-run-fill-rest-recovery.
#[derive(Debug, Clone, Deserialize)]
pub struct HaltedRunFillRestRecoveryRequest {
    pub run_id: String,
    pub internal_order_id: String,
    pub broker_order_id: String,
    /// When `true` (default), no state is mutated; the response describes the
    /// recovered fill for operator review.  When `false`, `confirmation` is
    /// required and the recovered fill is inserted into the inbox and stamped applied.
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Must equal `"APPLY_REST_FILL_RECOVERY"` when `dry_run = false`.
    pub confirmation: Option<String>,
}

/// Authoritative fill details recovered from Alpaca REST account activities.
#[derive(Debug, Clone, Serialize)]
pub struct RestRecoveredFill {
    /// Alpaca-assigned activity ID; used as authoritative fill identity.
    pub broker_activity_id: String,
    pub symbol: String,
    pub side: String,
    /// Fill quantity as returned by Alpaca (decimal string).
    pub qty_str: String,
    /// Fill price as returned by Alpaca (decimal string).
    pub price_str: String,
    /// Transaction timestamp from Alpaca (ISO 8601).
    pub timestamp: String,
    /// Always `"alpaca_rest_activity"` — identifies the evidence source.
    pub source: String,
    /// `false` when `dry_run=true` (plan-only evidence); `true` after a
    /// successful confirmed apply.
    pub mutation_safe: bool,
}

/// Response for POST /api/v1/ops/repair/halted-run-fill-rest-recovery.
///
/// `truth_state` values: `"active"`, `"no_db"`, `"backend_unavailable"`.
/// `decision` values:
/// - `"rest_recovered_fill_evidence"` — dry_run=true, fill evidence found, no mutation.
/// - `"applied"` — fill inbox row inserted and stamped applied.
/// - `"already_repaired"` — idempotent: row was already applied from a prior call.
/// - `"refused"` — any failure gate fired.
#[derive(Debug, Clone, Serialize)]
pub struct HaltedRunFillRestRecoveryResponse {
    pub truth_state: String,
    pub decision: String,
    /// Mirrors the `dry_run` field from the request.
    pub dry_run: bool,
    /// `true` only when `decision = "applied"`.  `false` for plan-only and refusals.
    pub mutated: bool,
    pub run_id: String,
    pub internal_order_id: String,
    pub broker_order_id: String,
    /// Classification derived from planner logic at query time.
    pub classification: String,
    /// Human-readable explanation of the decision.
    pub evidence: String,
    pub gate: Option<String>,
    pub audit_event_id: Option<String>,
    /// Authoritative fill details from Alpaca REST.
    pub rest_fill: Option<RestRecoveredFill>,
    /// Stable `broker_message_id` of the inbox row.  Set on `"applied"` and
    /// `"already_repaired"`; `None` otherwise.
    pub inbox_broker_message_id: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/repair/halted-run-portfolio-snapshot — PORTFOLIO-SNAPSHOT-DURABILITY-01
// ---------------------------------------------------------------------------

/// Per-symbol position summary derived from applied inbox fills.
#[derive(Debug, Clone, Serialize)]
pub struct PortfolioPositionSummary {
    pub symbol: String,
    /// Signed quantity: positive = long, negative = short.  Flat symbols are omitted.
    pub qty_signed: i64,
    /// Number of open FIFO lots.
    pub lot_count: usize,
}

fn default_dry_run_true() -> bool {
    true
}

/// Request body for POST /api/v1/ops/repair/halted-run-portfolio-snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct HaltedRunPortfolioSnapshotRequest {
    pub run_id: String,
    /// If true (default), compute and return the portfolio summary without writing any snapshot.
    #[serde(default = "default_dry_run_true")]
    pub dry_run: bool,
    /// Required when `dry_run = false`: must equal `"WRITE_PORTFOLIO_SNAPSHOT"`.
    pub confirmation: Option<String>,
}

/// Response for POST /api/v1/ops/repair/halted-run-portfolio-snapshot.
///
/// `truth_state` values: `"active"`, `"no_db"`, `"backend_unavailable"`.
///
/// `decision` values:
/// - `"dry_run_ok"` — positions computed; no snapshot written.
/// - `"snapshot_written"` — snapshot written to audit store.
/// - `"already_current"` — snapshot for this applied-fill dataset already exists.
/// - `"refused"` — a gate fired; see `gate` and `evidence`.
#[derive(Debug, Clone, Serialize)]
pub struct HaltedRunPortfolioSnapshotResponse {
    pub truth_state: String,
    pub decision: String,
    pub dry_run: bool,
    pub run_id: String,
    /// Number of applied fill/partial_fill inbox rows used for portfolio reconstruction.
    pub applied_fill_count: usize,
    /// Derived open positions (flat symbols omitted).
    pub positions: Vec<PortfolioPositionSummary>,
    /// Derived cash balance in micros, starting from `initial_cash_micros`.
    pub cash_micros: i64,
    /// Accumulated realized PnL from all applied fills, in micros.
    pub realized_pnl_micros: i64,
    /// Initial cash seed used for reconstruction (0 unless run config specifies otherwise).
    pub initial_cash_micros: i64,
    /// Whether a durable snapshot artifact was written to the audit store on this call.
    pub snapshot_written: bool,
    /// Audit event UUID of the written (or existing) snapshot.
    pub audit_event_id: Option<String>,
    /// Always `"applied_inbox_rows"` when positions are derived.
    pub source: String,
    pub evidence: String,
    pub gate: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/catalog — canonical Action Catalog
// ---------------------------------------------------------------------------

/// One entry in the canonical operator Action Catalog.
///
/// The catalog lists every action the daemon's `/api/v1/ops/action` dispatcher
/// can actually execute right now.  `enabled` reflects current runtime state;
/// `disabled_reason` explains why the action is unavailable when `enabled` is false.
///
/// `change-system-mode` is intentionally absent — it returns 409 from ops_action
/// (mode transitions require a controlled daemon restart).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCatalogEntry {
    /// Canonical action identifier, e.g. "arm-execution".
    pub action_key: String,
    /// Human-readable label for operator UI.
    pub label: String,
    /// Severity level: 0 = informational, 1 = normal, 2 = elevated, 3 = emergency.
    pub level: u8,
    /// Human-readable description of what this action does.
    pub description: String,
    /// Whether this action requires an operator reason string.
    pub requires_reason: bool,
    /// Confirmation prompt text shown before the action executes.
    pub confirm_text: String,
    /// Whether this action is currently executable given system state.
    pub enabled: bool,
    /// Why the action is disabled; None when enabled is true.
    pub disabled_reason: Option<String>,
}

/// Response body for GET /api/v1/ops/catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCatalogResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// All actions the daemon currently supports.  State-aware availability
    /// (enabled/disabled_reason) is computed from the live daemon state at
    /// request time.
    pub actions: Vec<ActionCatalogEntry>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/mode-change-guidance — controlled mode-transition workflow
// ---------------------------------------------------------------------------

/// Runtime state relevant to mode-transition safety decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangeRestartTruth {
    /// Run ID owned by this daemon instance at the time of the request, if any.
    pub local_owned_run_id: Option<Uuid>,
    /// Most recent durable active run ID from the DB, if any.
    pub durable_active_run_id: Option<Uuid>,
    /// True when a durable active run exists but is not owned by this instance.
    pub durable_active_without_local_ownership: bool,
}

// ---------------------------------------------------------------------------
// CC-03C: Mounted controlled restart workflow truth
// ---------------------------------------------------------------------------

/// A single durable pending restart intent surfaced at the control-plane.
///
/// Sourced exclusively from `sys_restart_intent` (CC-03B).  Fields are
/// intentionally the minimal operator-visible subset: full lifecycle fields
/// (completed_at_utc) are not surfaced here because the mounted surface only
/// shows the **pending** workflow state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRestartIntentSnapshot {
    /// UUID of the durable intent record.
    pub intent_id: String,
    /// Current deployment mode at the time the intent was created.
    pub from_mode: String,
    /// Intended target deployment mode.
    pub to_mode: String,
    /// CC-03A canonical transition verdict string stored in the DB.
    /// One of: `"same_mode"`, `"admissible_with_restart"`, `"refused"`, `"fail_closed"`.
    pub transition_verdict: String,
    /// Who initiated this intent: `"operator"`, `"system"`, or `"recovery"`.
    pub initiated_by: String,
    /// RFC3339 UTC timestamp when the intent was initiated.
    pub initiated_at_utc: String,
    /// Optional operator note or provenance reference.  Empty string if none.
    pub note: String,
}

/// CC-03C: Mounted restart workflow truth for the operator control surface.
///
/// Sourced from `sys_restart_intent` (CC-03B).  Always present in
/// `ModeChangeGuidanceResponse`; truth state determines authority.
///
/// `truth_state` values:
/// - `"active"` — DB was reachable, a pending restart intent was found;
///   `pending_intent` is the authoritative durable record.
/// - `"no_pending"` — DB was reachable, no pending intent exists; honest
///   absence.  Must NOT be treated as "restart is safe to skip".
/// - `"backend_unavailable"` — no DB pool is configured; restart workflow
///   truth cannot be determined; fail-closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartWorkflowTruth {
    /// Authority state for this restart workflow surface.
    pub truth_state: String,
    /// Pending restart intent, present only when `truth_state == "active"`.
    pub pending_intent: Option<PendingRestartIntentSnapshot>,
}

/// CC-03A: Per-target canonical mode-transition verdict.
///
/// One entry per possible target [`crate::state::DeploymentMode`], derived
/// exclusively from [`crate::mode_transition::evaluate_mode_transition`].
/// Callers must treat this as read-only truth — not as a configuration surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeTransitionEntry {
    /// Target mode label (e.g. `"live-shadow"`).
    pub target_mode: String,
    /// Canonical verdict: one of `"same_mode"`, `"admissible_with_restart"`,
    /// `"refused"`, `"fail_closed"`.
    pub verdict: String,
    /// Human-readable explanation of the verdict.
    pub reason: String,
    /// Ordered operator preconditions.  Non-empty only when
    /// `verdict == "admissible_with_restart"`.
    pub preconditions: Vec<String>,
}

/// Response for GET /api/v1/ops/mode-change-guidance and for the
/// `change-system-mode` arm of POST /api/v1/ops/action (409 CONFLICT).
///
/// Mode transitions are **never** authoritative via API — there is no hot
/// switching.  This response provides the operator with an explicit,
/// authoritative workflow for executing a controlled restart-driven mode
/// change without guesswork.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangeGuidanceResponse {
    /// Self-identifying route: "/api/v1/ops/mode-change-guidance".
    pub canonical_route: String,
    /// Current deployment mode label (e.g. "paper", "live", "backtest").
    pub current_mode: String,
    /// Always false — mode transitions require a controlled daemon restart.
    pub transition_permitted: bool,
    /// Authoritative reason why hot switching is refused.
    pub transition_refused_reason: String,
    /// Conditions that must be satisfied before the daemon can be safely restarted.
    pub preconditions: Vec<String>,
    /// Ordered explicit steps the operator must follow for a safe mode transition.
    pub operator_next_steps: Vec<String>,
    /// Restart truth from the daemon's run registry.  None when no DB connection.
    pub restart_truth: Option<ModeChangeRestartTruth>,
    /// CC-03A: Canonical transition verdicts for every possible target mode,
    /// derived from [`crate::mode_transition::evaluate_mode_transition`].
    ///
    /// This field makes the mode-transition state machine observable at the
    /// API surface and ensures `build_mode_change_guidance` derives its
    /// transition semantics from the canonical seam rather than ad hoc logic.
    pub transition_verdicts: Vec<ModeTransitionEntry>,
    /// CC-03C: Durable restart workflow truth — the mounted, operator-visible
    /// controlled restart workflow state sourced from `sys_restart_intent`.
    ///
    /// Always present.  `truth_state` determines authority:
    /// `"active"` = pending intent found; `"no_pending"` = honest absence;
    /// `"backend_unavailable"` = no DB, fail-closed.
    pub restart_workflow: RestartWorkflowTruth,
    // C3: Current parity evidence state on the mode-change-guidance surface.
    //
    // Before C3 an operator consulting mode-change-guidance to plan a transition
    // to LiveShadow or LiveCapital could see that those transitions require
    // parity evidence (in transition_verdicts preconditions) without seeing the
    // *current* state of that evidence on this deployment.  They would have to
    // consult a second surface (/api/v1/system/status or
    // /api/v1/system/parity-evidence) to learn whether evidence is absent,
    // present but incomplete, or complete.  An operator who only consulted this
    // surface could mistake "admissible_with_restart + precondition to provide
    // evidence" for "evidence is missing and I need to produce it" when in
    // reality evidence already exists and live_trust_complete=false is the
    // current-build ceiling — a structural gap, not a missing file.
    //
    // C3 surfaces both fields directly on this response so operators planning
    // a mode transition see the full constraint picture in one place.
    //
    // Values are derived from the same `evaluate_parity_evidence_guarded()`
    // call used by C1 (status), C2 (preflight), and the dedicated
    // parity-evidence route.  The four surfaces stay in sync.
    /// Machine-readable parity evidence state.
    ///
    /// `"not_configured"` | `"absent"` | `"invalid"` |
    /// `"incomplete"` | `"complete"` | `"unavailable"`
    ///
    /// Always present (structural field).  Allows operators to see the current
    /// state of the evidence precondition listed in `transition_verdicts`
    /// without consulting a separate surface.
    pub parity_evidence_state: String,
    /// Explicit live-trust ceiling derived from parity evidence.
    ///
    /// `Some(false)` when evidence is present but incomplete (current builds).
    /// `None` for every non-Present outcome — null is never a positive trust
    /// claim on this surface.
    ///
    /// An operator who sees `live-shadow: admissible_with_restart` alongside
    /// `live_trust_complete: false` knows the ceiling is a structural proof gap
    /// in the current build, not a missing artifact.
    pub live_trust_complete: Option<bool>,
}

// ---------------------------------------------------------------------------
// /api/v1/strategy/signal — PT-DAY-01: strategy-driven paper execution
// ---------------------------------------------------------------------------

/// Strategy signal submission request.
///
/// The caller (research-py or operator tooling) is responsible for computing
/// the signal from real market data.  The daemon validates the signal against
/// the current execution context and enqueues it for broker-backed dispatch.
///
/// `signal_id` is the caller-supplied idempotency key.  UUIDv5 derived from
/// (strategy_id, signal_ts, symbol, side, qty) is recommended to guarantee
/// deterministic deduplication across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignalRequest {
    /// Caller-supplied idempotency key; unique per signal intent.
    pub signal_id: String,
    /// Authoritative strategy identifier for attribution and suppression checks.
    pub strategy_id: String,
    pub symbol: String,
    /// Order direction: "buy" or "sell".
    pub side: String,
    /// Positive integer quantity (number or string representation).
    pub qty: serde_json::Value,
    /// Order type: "market" (default) or "limit".
    pub order_type: Option<String>,
    /// Time-in-force: "day" (default), "gtc", "ioc", "fok", "opg", "cls".
    pub time_in_force: Option<String>,
    /// Limit price in integer micros (required for limit orders; absent for market).
    pub limit_price: Option<serde_json::Value>,
    /// B8: Explicit asset class for this signal.
    ///
    /// Optional.  When absent, `"equity"` is implied (backward compatible).
    /// Only `"equity"` is accepted.  Supplying any other value (e.g. `"option"`,
    /// `"future"`, `"crypto"`, `"fx"`) will cause Gate 0 to reject the signal
    /// with an explicit `"unsupported_asset_class"` blocker.
    ///
    /// This field exists to make the asset-class boundary machine-checkable.
    /// Strategy authors can include it for explicit documentation of intent;
    /// the daemon validates it to ensure unsupported asset classes cannot
    /// accidentally reach the outbox.
    #[serde(default)]
    pub asset_class: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignalResponse {
    pub accepted: bool,
    /// Disposition: "enqueued" | "duplicate" | "rejected" | "unavailable" | "suppressed"
    ///              | "budget_denied" | "sizing_denied" | "exposure_denied"
    ///              | "exhaustion_denied" | "continuity_gap" | "outside_session"
    ///              | "day_limit_reached".
    pub disposition: String,
    pub signal_id: String,
    pub strategy_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
    /// RTS-07: `true` when this submission placed a *new* execution intent in the
    /// outbox (Gate 7 `Ok(true)`).
    ///
    /// When `true`: an outbox row was written and carries
    /// `signal_source = "external_signal_ingestion"` as a provenance mark.
    /// The orchestrator's next Phase 1 tick will claim and dispatch it to the
    /// broker.  This is the only path that produces a pending execution intent.
    ///
    /// When `false`: no new outbox row was placed.  This covers gate failures,
    /// duplicate submissions (`disposition = "duplicate"`), and validation errors.
    /// The prior runtime state is unchanged.
    ///
    /// `#[serde(default)]` preserves backward compatibility: clients that do not
    /// send or receive this field deserialise it as `false`.
    #[serde(default)]
    pub intent_placed: bool,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/orders — canonical OMS order surface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderSubmitRequest {
    pub client_request_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: serde_json::Value,
    pub order_type: Option<String>,
    pub time_in_force: Option<String>,
    pub limit_price: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderSubmitResponse {
    pub accepted: bool,
    pub disposition: String,
    pub client_request_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderCancelRequest {
    pub cancel_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderCancelResponse {
    pub accepted: bool,
    pub disposition: String,
    pub order_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
}

/// One live order row sourced from the in-memory OMS runtime snapshot.
///
/// Fields that are not present in the OMS snapshot are emitted as `null`:
/// - `strategy_id`: `null` — no strategy attribution at the OMS layer.
/// - `side`: `null` — per-order side is not tracked in the OMS snapshot.
/// - `order_type`: `null` — order type is not captured in OMS state.
/// - `age_ms`: `null` — per-order creation time is not in the OMS snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOrderRow {
    /// Internal (client) order identifier assigned by this daemon.
    pub internal_order_id: String,
    /// Broker-assigned order ID; `None` until the submit is confirmed.
    pub broker_order_id: Option<String>,
    pub symbol: String,
    /// `null` — OMS runtime has no strategy attribution per order.
    pub strategy_id: Option<String>,
    /// `null` — per-order side is not tracked in the OMS snapshot.
    pub side: Option<String>,
    /// `null` — order type is not captured at OMS snapshot level.
    pub order_type: Option<String>,
    pub requested_qty: i64,
    pub filled_qty: i64,
    /// Canonical OMS state: `"Open"` | `"PartiallyFilled"` | `"Filled"` |
    /// `"CancelPending"` | `"Cancelled"` | `"ReplacePending"` | `"Rejected"`
    pub current_status: String,
    /// Display-friendly lifecycle stage derived from `current_status`.
    pub current_stage: String,
    /// `null` — per-order creation timestamps are not in the OMS snapshot.
    pub age_ms: Option<u64>,
    pub has_warning: bool,
    /// `true` when `current_status == "Rejected"`.
    pub has_critical: bool,
    /// RFC 3339 timestamp of the execution snapshot that produced this row.
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// /api/v1/portfolio/positions  /api/v1/portfolio/orders/open  /api/v1/portfolio/fills
// Canonical broker-snapshot portfolio surfaces (Cluster 2)
// MT-07B: extracted to api_types/portfolio_snapshot.rs
// ---------------------------------------------------------------------------

#[path = "api_types/portfolio_snapshot.rs"]
mod portfolio_snapshot;
pub use portfolio_snapshot::*;

// ---------------------------------------------------------------------------
// /api/v1/risk/denials — canonical risk denial truth surface (Cluster 3)
// ---------------------------------------------------------------------------

/// One structured denial row from the risk gate.
///
/// Fields map 1:1 to the GUI `RiskDenialRow` type so the operator sees exact
/// denial evidence without transformation.
///
/// `strategy_id` is `None` / `null` at all times: the risk gate operates on
/// the order itself and has no access to which strategy generated it.  The
/// field is optional in the type contract so that it is honest (`null` in
/// JSON) rather than a placeholder empty string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialRow {
    pub id: String,
    pub at: String,
    /// Always `null` — strategy attribution is not available on the risk gate
    /// path.  The gate sees the order, not the originating strategy.
    pub strategy_id: Option<String>,
    pub symbol: String,
    /// The risk rule that was violated, e.g. `"PositionLimitExceeded"`.
    pub rule: String,
    /// Human-readable denial message derived from `rule` + `evidence`.
    pub message: String,
    /// `"warning"` | `"critical"`.  Critical when the denial class is
    /// terminal (e.g. `RiskEngineUnavailable`, `CapitalLimitExceeded`).
    pub severity: String,
}

/// Response wrapper for `GET /api/v1/risk/denials`.
///
/// `truth_state` explicitly distinguishes three semantically different
/// response postures:
///
/// - `"active"` — execution loop is running AND a DB pool is available.
///   `denials` contains ONLY rows that are durably stored in
///   `sys_risk_denial_events`.  Restart-safe.  An empty `denials` array
///   means the risk gate has genuinely never denied any order in this
///   deployment (not just the current session).
///
/// - `"active_session_only"` — execution loop is running but NO DB pool is
///   available.  `denials` is populated from the in-memory ring buffer only.
///   NOT restart-safe: rows will be lost on daemon restart.  Returned only
///   in DB-less test environments; production deployments always have a pool.
///
/// - `"durable_history"` — execution loop is not currently running but the
///   DB has historical denial rows from a prior session.  `denials` is
///   durably sourced; restart-safe.  The GUI passes this through as
///   `ok: true` and renders the historical rows.
///
/// - `"no_snapshot"` — no durable rows exist and the loop is not running.
///   `denials` is always empty and **must not** be treated as authoritative
///   zero.  GUI IIFE emits `ok: false` → risk panel blocks.
///
/// The GUI IIFE blocks only on `"no_snapshot"` and `"not_wired"`.
/// `"active"`, `"active_session_only"`, and `"durable_history"` all pass
/// through as `ok: true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialsResponse {
    /// `"active"` = loop running + DB pool → durable rows only.
    /// `"active_session_only"` = loop running + no DB pool → ring buffer only.
    /// `"durable_history"` = loop not running, DB has historical rows.
    /// `"no_snapshot"` = no DB rows and loop not running.
    pub truth_state: String,
    /// UTC timestamp of the execution snapshot (present when loop is running).
    pub snapshot_at_utc: Option<String>,
    /// Denial rows.  Restart-safe when `truth_state` is `"active"` or
    /// `"durable_history"`.  Ephemeral when `"active_session_only"`.
    /// Always empty when `"no_snapshot"`.
    pub denials: Vec<RiskDenialRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/diagnostics/snapshot (B4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSnapshotResponse {
    pub snapshot: Option<ExecutionSnapshot>,
}

// ---------------------------------------------------------------------------
// /api/v1/metrics/dashboards (CC-05)
// ---------------------------------------------------------------------------

/// Metrics dashboard composed from existing truthful summary surfaces.
///
/// Gives an operator one endpoint for current performance and health KPIs
/// without hitting four separate summary routes.  All panels use explicit
/// truth_state semantics — None is never a fabricated zero.
///
/// Fields that are not derivable from current sources (daily_pnl, drawdown_pct,
/// loss_limit_utilization_pct) are always None.  This is intentional and honest:
/// the underlying summary routes also return None for these fields because the
/// data source does not exist yet.
///
/// # Panel truth states
///
/// - `portfolio_snapshot_state` / `risk_snapshot_state`: `"no_snapshot"` when
///   `broker_snapshot` is absent; `"active"` when present.
/// - `execution_snapshot_state`: `"no_snapshot"` when execution loop has not
///   started; `"active"` when execution loop is running with a snapshot.
/// - `reconcile_status`: always present (`"unknown"` before first reconcile tick).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsDashboardResponse {
    /// Canonical route for this surface.
    pub canonical_route: String,

    // --- Portfolio panel (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub portfolio_snapshot_state: String,
    pub account_equity: Option<f64>,
    pub long_market_value: Option<f64>,
    pub short_market_value: Option<f64>,
    pub cash: Option<f64>,
    /// Not derivable from broker snapshot — always None in current sources.
    pub daily_pnl: Option<f64>,
    pub buying_power: Option<f64>,

    // --- Risk panel (from broker_snapshot positions + runtime state) ---
    /// `"no_snapshot"` | `"active"`
    pub risk_snapshot_state: String,
    pub gross_exposure: Option<f64>,
    pub net_exposure: Option<f64>,
    pub concentration_pct: Option<f64>,
    /// Not derivable from broker snapshot — always None in current sources.
    pub drawdown_pct: Option<f64>,
    /// Not derivable without a loss-limit config — always None in current sources.
    pub loss_limit_utilization_pct: Option<f64>,
    pub kill_switch_active: bool,
    pub active_breaches: usize,

    // --- Execution panel (from execution_snapshot / OMS) ---
    /// `"no_snapshot"` | `"active"`
    pub execution_snapshot_state: String,
    pub active_order_count: usize,
    pub pending_order_count: usize,
    pub dispatching_order_count: usize,
    pub reject_count_today: usize,

    // --- Reconcile panel ---
    /// `"ok"` | `"unknown"` | `"dirty"` | `"stale"` | `"unavailable"`
    pub reconcile_status: String,
    pub reconcile_last_run_at: Option<String>,
    /// Sum of all mismatch counts across positions, orders, fills, and broker events.
    pub reconcile_total_mismatches: usize,

    // --- Event risk panel (B7) ---
    /// B7: Whether corporate-actions / earnings screening is active on the
    /// current execution path.
    ///
    /// `"not_wired"` — the daemon has no connection to a corporate-actions or
    /// earnings calendar feed.  No pre-event position flattening, no ex-dividend
    /// price-adjustment ingestion, and no earnings blackout gate are present on
    /// the paper+alpaca canonical path.  The backtest engine has an explicit
    /// `CorporateActionPolicy` (fail-closed for forbidden periods), but that
    /// policy does not extend to live or paper execution.
    ///
    /// This field exists so the risk panel can never be mistaken for an
    /// environment with active corporate-actions protection.
    pub corp_actions_screening: String,
}

// ---------------------------------------------------------------------------
// /api/v1/oms/overview (CC-04)
// ---------------------------------------------------------------------------

/// Single canonical OMS overview composed from mounted truth surfaces.
///
/// Gives an operator one endpoint to check current trading state without
/// piecing together scattered surfaces.  All lanes use explicit truth_state
/// semantics — absence of a snapshot is never silently treated as "zero".
///
/// # Lane semantics
///
/// - `runtime_*`: derived from StatusSnapshot — always present.
/// - `account_snapshot_state` / `portfolio_snapshot_state`: `"no_snapshot"`
///   when broker_snapshot is absent, `"active"` when present.
/// - `execution_has_snapshot`: false when execution loop has never started.
/// - `reconcile_*`: always present (defaults to `"unknown"` when unrun).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmsOverviewResponse {
    /// Canonical route for this surface.
    pub canonical_route: String,

    // --- Runtime lane ---
    /// `"idle"` | `"running"` | `"halted"` | `"unknown"`
    pub runtime_status: String,
    pub integrity_armed: bool,
    pub kill_switch_active: bool,
    pub daemon_mode: String,
    /// Count of active fault signals. Full detail at `GET /api/v1/system/status`.
    pub fault_signal_count: usize,

    // --- Account lane (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub account_snapshot_state: String,
    /// Account equity as parsed f64. None when snapshot absent or parse fails.
    pub account_equity: Option<f64>,
    /// Account cash as parsed f64. None when snapshot absent or parse fails.
    pub account_cash: Option<f64>,

    // --- Portfolio lane (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub portfolio_snapshot_state: String,
    /// UTC timestamp of broker snapshot capture. None when no snapshot.
    pub portfolio_snapshot_at_utc: Option<String>,
    pub position_count: usize,
    pub open_order_count: usize,
    pub fill_count: usize,

    // --- Execution lane (from execution_snapshot / OMS) ---
    /// false when execution loop has not started or no active run.
    pub execution_has_snapshot: bool,
    pub execution_active_orders: usize,
    pub execution_pending_orders: usize,

    // --- Reconcile lane ---
    /// `"ok"` | `"unknown"` | `"dirty"` | `"stale"` | `"unavailable"`
    pub reconcile_status: String,
    pub reconcile_last_run_at: Option<String>,
    /// Sum of mismatched_positions + mismatched_orders + mismatched_fills +
    /// unmatched_broker_events.
    pub reconcile_total_mismatches: usize,

    // --- Protection lane (B4) ---
    /// B4: Whether protective stop / bracket order wiring is supported on the
    /// current execution path.
    ///
    /// `"not_supported"` — the canonical paper+alpaca path does not submit stop
    /// or bracket orders to the broker.  Submit validation explicitly rejects
    /// `order_type = "stop"`.  This field exists so operators cannot mistake an
    /// empty order book for a protected execution environment.
    pub stop_order_wiring: String,

    // --- Event risk lane (B7) ---
    /// B7: Whether corporate-actions / earnings screening is active on the
    /// current execution path.
    ///
    /// `"not_wired"` — no corp-actions or earnings calendar feed is connected.
    /// No pre-event position flattening, ex-dividend price adjustment, or
    /// earnings blackout gate exists on the paper+alpaca canonical path.
    /// The backtest engine has an explicit `CorporateActionPolicy` but it does
    /// not extend to live or paper execution.  This field exists so operators
    /// cannot mistake a running system for one with active event-risk screening.
    pub corp_actions_screening: String,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/protection-status (B4)
// ---------------------------------------------------------------------------

/// Honest protection-status contract for the canonical paper+alpaca execution path.
///
/// B4 closure: stop and bracket order wiring is NOT supported in the current
/// system.  This type surfaces that gap explicitly so operator tooling and
/// runbooks cannot mistake the current system for a protected execution
/// environment.
///
/// # Truth semantics
///
/// - `truth_state = "not_wired"` — no broker-backed stop or bracket orders.
/// - `stop_order_wiring = "not_supported"` — submit validator explicitly rejects
///   `order_type = "stop"`.
/// - `bracket_order_wiring = "not_supported"` — no OCO / OTOCO bracket types
///   are passed to the Alpaca broker adapter.
///
/// When full broker-backed protective exits are implemented (B5+), these fields
/// will transition to `"wired"` and `"broker_backed"` respectively.  Until then
/// this response is the canonical source of truth for protection capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectionStatusResponse {
    pub canonical_route: String,
    /// `"not_wired"` — broker-backed stop / bracket orders are not implemented.
    pub truth_state: String,
    /// `"not_supported"` — stop `order_type` is explicitly rejected by the submit
    /// validator.  No stop orders can reach the broker on the current path.
    pub stop_order_wiring: String,
    /// `"not_supported"` — no OCO / OTOCO bracket order types are wired to the
    /// Alpaca broker adapter.  No child legs are submitted alongside parent orders.
    pub bracket_order_wiring: String,
    /// Honest note for operator tooling and runbooks.
    pub note: String,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/event-risk-status (EVENT-RISK-01)
// ---------------------------------------------------------------------------

/// Honest event-risk screening status for the canonical paper+alpaca execution path.
///
/// EVENT-RISK-01 closure: no earnings calendar feed is connected, no pre-event
/// position flattening gate exists, and no earnings blackout is enforced at
/// signal admission.  This type surfaces that gap explicitly — following the
/// same pattern as B4's `ProtectionStatusResponse` — so operator tooling and
/// runbooks cannot mistake the current system for an event-risk-aware execution
/// environment.
///
/// # Truth semantics
///
/// - `truth_state = "not_wired"` — no admission gate is configured; nothing is enforcing.
/// - `truth_state = "partial"` — Gate 1h (`MQK_EVENT_RISK_BLACKOUT_PATH`) is configured
///   and enforcing symbol-level blackout periods; `earnings_calendar_feed` and
///   `pre_event_flattening` remain absent.
/// - `earnings_calendar_feed = "not_connected"` — no earnings calendar data
///   source is connected.  No upcoming-earnings awareness exists.
/// - `pre_event_flattening = "not_wired"` — no pre-event position flattening
///   gate exists.  Positions are not automatically exited before earnings.
/// - `signal_admission_gate = "configured"` — Gate 1h is enforcing (env var set).
/// - `signal_admission_gate = "not_configured"` — Gate 1h exists in code but is
///   not enforcing (`MQK_EVENT_RISK_BLACKOUT_PATH` absent).
///
/// The backtest engine has an explicit `CorporateActionPolicy::ForbidPeriods`
/// that halts simulation on declared exclusion periods.  Gate 1h brings equivalent
/// enforcement to the live/paper signal path for operator-declared periods only —
/// not from a live calendar feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRiskStatusResponse {
    pub canonical_route: String,
    /// `"not_wired"` — no gate configured; `"partial"` — Gate 1h configured (others absent).
    pub truth_state: String,
    /// `"not_connected"` — no earnings calendar feed is connected.
    pub earnings_calendar_feed: String,
    /// `"not_wired"` — no pre-event position flattening gate exists.
    pub pre_event_flattening: String,
    /// `"configured"` / `"not_configured"` — whether `MQK_EVENT_RISK_BLACKOUT_PATH` is set (Gate 1h).
    pub signal_admission_gate: String,
    /// Honest note for operator tooling and runbooks.
    pub note: String,
}

// ---------------------------------------------------------------------------
// /api/v1/alerts/active (CC-06)
// ---------------------------------------------------------------------------

/// One active alert row sourced from current daemon fault signals.
///
/// An active alert is a current fault signal computed from live daemon state.
/// There is no persistent alert table, no alert lifecycle, and no ack state:
/// alerts exist while their underlying condition is present and disappear when
/// the condition is resolved.  `alert_id` is the fault signal class (a stable
/// slug), not a UUIDv4 — there is no durable alert registry.
///
/// Source: `build_fault_signals(StatusSnapshot, ReconcileStatusSnapshot, risk_truth: Option<bool>)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlertRow {
    /// Stable slug derived from the fault signal class.
    /// Identical to `class` — no persistent lifecycle ID exists.
    pub alert_id: String,
    /// `"warning"` | `"critical"`
    pub severity: String,
    /// Structured fault class (e.g., `"runtime.halt.operator_or_safety"`).
    pub class: String,
    /// Human-readable description of the current condition.
    pub summary: String,
    /// Optional detail string when the fault signal carries extra context.
    pub detail: Option<String>,
    /// Where this alert was computed from.
    /// Always `"daemon.runtime_state"` — in-memory computation, not DB-backed.
    pub source: String,
}

/// Response wrapper for `GET /api/v1/alerts/active`.
///
/// `truth_state`:
/// - `"active"` — always returned; the source is current in-memory daemon
///   state and is always available.  `rows` may be empty (no current alerts)
///   or populated with real fault-signal-backed alert rows.
///   Empty `rows` means the daemon has no current active fault conditions.
///   This is an authoritative "healthy" state, not an absence of source.
///
/// No ack/triage lifecycle exists.  Alerts do not persist beyond the lifetime
/// of their underlying condition.  Do not rely on `alert_id` across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlertsResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Always `"active"` — computed from live in-memory state at request time.
    pub truth_state: String,
    /// `"daemon.runtime_state"` — not DB-backed; computed from StatusSnapshot
    /// and ReconcileStatusSnapshot at request time.
    pub backend: String,
    /// Count of currently active alerts.  Equals `rows.len()`.
    pub alert_count: usize,
    /// Active alert rows.  Empty means no current fault conditions.
    pub rows: Vec<ActiveAlertRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/events/feed (CC-06)
// ---------------------------------------------------------------------------

/// One recent event row from the operator/runtime event feed.
///
/// Events are sourced from three durable DB tables:
/// - `runs` — runtime lifecycle transitions (CREATED, ARMED, RUNNING,
///   STOPPED, HALTED).
/// - `audit_events` (topic=`'operator'`) — operator action events written
///   by `write_operator_audit_event` / `write_control_operator_audit_event`.
/// - `audit_events` (topic=`'signal_ingestion'`) — signal admission events
///   written by the strategy-signal route at Gate 7 `Ok(true)`.
/// - `sys_autonomous_session_events` — autonomous supervisor history events
///   written by `set_autonomous_session_truth` (AUTON-PAPER-02).
///
/// `event_id` equals `provenance_ref` and encodes the exact DB source row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFeedRow {
    /// Provenance reference for this event.
    /// Format: `"runs:{run_id}:{column}"` for runtime transitions,
    /// `"audit_events:{event_id}"` for operator and signal-admission actions,
    /// `"sys_autonomous_session_events:{id}"` for autonomous supervisor events.
    pub event_id: String,
    /// RFC 3339 timestamp.
    pub ts_utc: String,
    /// `"runtime_transition"` | `"operator_action"` | `"signal_admission"` | `"autonomous_session"`
    pub kind: String,
    /// Detail string (e.g., `"HALTED"`, `"control.arm"`).
    pub detail: String,
    /// Run ID associated with this event, if any.
    pub run_id: Option<String>,
    /// Stable provenance reference (equals `event_id`).
    pub provenance_ref: String,
    /// OPS-11: For `kind = "operator_action"` and `kind = "signal_admission"` rows,
    /// the raw UUID string from `audit_events.event_id`.  Allows direct stable correlation
    /// to `/api/v1/audit/operator-actions` rows without parsing `event_id`.
    /// `None` for `kind = "runtime_transition"` and `kind = "autonomous_session"` rows
    /// (sourced from `runs` and `sys_autonomous_session_events` respectively).
    pub audit_event_id: Option<String>,
}

/// Response wrapper for `GET /api/v1/events/feed`.
///
/// `truth_state`:
/// - `"active"` — DB pool is present; `rows` contains the most recent events
///   from `runs`, `audit_events`, and `sys_autonomous_session_events`;
///   authoritative.  Empty `rows` means no durable events exist yet.
/// - `"backend_unavailable"` — no DB pool configured; `rows` is always empty
///   and **must not** be treated as authoritative empty history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsFeedResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// `"active"` = DB present, rows are authoritative recent events.
    /// `"backend_unavailable"` = no DB pool, rows empty, not authoritative.
    pub truth_state: String,
    /// `"postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events"` when active;
    /// `"unavailable"` when no DB pool.
    pub backend: String,
    /// Recent events sorted newest-first.  At most 50 rows.
    pub rows: Vec<EventFeedRow>,
}

// ---------------------------------------------------------------------------
// TV-EXEC-01: Fill-quality telemetry response types
// ---------------------------------------------------------------------------

/// One row in the fill-quality telemetry response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillQualityTelemetryRow {
    pub telemetry_id: Uuid,
    pub run_id: Uuid,
    pub internal_order_id: String,
    pub broker_order_id: Option<String>,
    pub broker_fill_id: Option<String>,
    pub broker_message_id: String,
    pub symbol: String,
    /// `"buy"` or `"sell"`
    pub side: String,
    pub ordered_qty: i64,
    pub fill_qty: i64,
    pub fill_price_micros: i64,
    /// `None` for market orders.
    pub reference_price_micros: Option<i64>,
    /// `None` when reference_price_micros is absent.
    pub slippage_bps: Option<i64>,
    pub submit_ts_utc: Option<String>,
    pub fill_received_at_utc: String,
    pub submit_to_fill_ms: Option<i64>,
    /// `"partial_fill"` or `"final_fill"`
    pub fill_kind: String,
    pub provenance_ref: String,
    pub created_at_utc: String,
}

/// Response wrapper for `GET /api/v1/execution/fill-quality`.
///
/// `truth_state`:
/// - `"active"` — DB pool and active run present; `rows` is authoritative.
///   Empty `rows` means no fills have been recorded for this run.
/// - `"no_active_run"` — daemon has a DB but no active run; `rows` is empty.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillQualityTelemetryResponse {
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` when active; `"unavailable"` otherwise.
    pub backend: String,
    /// Most recent fills for the active run, newest-fill first. At most 100 rows.
    pub rows: Vec<FillQualityTelemetryRow>,
}

// ---------------------------------------------------------------------------
// TV-01B: Runtime artifact intake contract
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/system/artifact-intake`.
///
/// Surfaces the runtime artifact intake truth for the operator: whether a
/// promoted artifact has been configured, whether it is structurally valid,
/// and its identity if accepted.
///
/// `truth_state` values:
/// - `"not_configured"` — `MQK_ARTIFACT_PATH` is not set or empty; operator
///   has not provided an artifact.  Must NOT be treated as "no artifact needed".
/// - `"invalid"` — path is set but the file is unreadable, not valid JSON,
///   has wrong `schema_version`, or is missing required fields.  Fail-closed.
/// - `"accepted"` — the `promoted_manifest.json` is present and structurally
///   valid.  This is intake acceptance only — it does not imply deployability
///   or that any economic gate has been passed.
/// - `"unavailable"` — the intake evaluator itself could not run (e.g.,
///   unexpected evaluator failure).  Fail-closed: intake status is unknown.
///
/// This is the minimum honest runtime artifact intake contract (TV-01B).
/// TV-01C will thread `artifact_id` into run-start provenance.
/// TV-01D will prove the full promoted artifact → runtime consumption chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactIntakeResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// Intake outcome: `"not_configured"` | `"invalid"` | `"accepted"` | `"unavailable"`.
    pub truth_state: String,
    /// Content-addressed artifact identity.  Non-null only when
    /// `truth_state == "accepted"`.
    pub artifact_id: Option<String>,
    /// Artifact type string (e.g. `"signal_pack"`).  Non-null only when
    /// `truth_state == "accepted"`.
    pub artifact_type: Option<String>,
    /// Promotion stage (e.g. `"paper"`).  Non-null only when
    /// `truth_state == "accepted"`.
    pub stage: Option<String>,
    /// Producing system identifier.  Non-null only when
    /// `truth_state == "accepted"`.
    pub produced_by: Option<String>,
    /// Human-readable reason for `"invalid"` or `"unavailable"` outcomes.
    /// Null for `"not_configured"` and `"accepted"`.  Callers must check
    /// `truth_state` to distinguish the two failure modes.
    pub invalid_reason: Option<String>,
    /// Path that was evaluated.  Null when `truth_state == "not_configured"`.
    pub evaluated_path: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/parity-evidence — TV-03A / TV-03B
// ---------------------------------------------------------------------------

/// TV-03A/TV-03B: Parity evidence manifest truth surface.
///
/// Reads `parity_evidence.json` (schema `parity-v1`) from the artifact
/// directory configured via `MQK_ARTIFACT_PATH` and returns the honest
/// parity-evidence state.  Written by the Python TV-03 pipeline.
///
/// `truth_state` values:
/// - `"not_configured"` — no artifact path configured; parity evidence gate
///   not applicable.
/// - `"absent"` — artifact path set but `parity_evidence.json` not found in
///   the artifact directory.  Absent evidence ≠ parity proven.
/// - `"invalid"` — `parity_evidence.json` found but structurally invalid.
/// - `"present"` — `parity_evidence.json` is valid and readable.
///   `live_trust_complete` is surfaced honestly (always `false` in current
///   builds).
/// - `"unavailable"` — evaluator itself could not run.
///
/// The operator surface guarantees:
/// - Absent, invalid, and unavailable are never conflated with "present".
/// - `live_trust_complete=false` is surfaced explicitly, not hidden.
/// - `evidence_available=false` is surfaced explicitly (no shadow run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityEvidenceResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// `"not_configured"` | `"absent"` | `"invalid"` | `"present"` | `"unavailable"`.
    pub truth_state: String,
    /// Canonical artifact identity from the evidence file.
    /// Non-null only when `truth_state == "present"`.
    pub artifact_id: Option<String>,
    /// Whether the full parity proof chain is complete enough for live capital.
    /// Always `false` in current builds.  Non-null only when `truth_state == "present"`.
    pub live_trust_complete: Option<bool>,
    /// Whether a shadow evaluation run was actually performed.
    /// Non-null only when `truth_state == "present"`.
    pub evidence_available: Option<bool>,
    /// Human-readable description of what shadow evidence exists or is missing.
    /// Non-null only when `truth_state == "present"`.
    pub evidence_note: Option<String>,
    /// ISO-8601 UTC string when this parity evidence was produced.
    /// Non-null only when `truth_state == "present"`.
    pub produced_at_utc: Option<String>,
    /// Human-readable reason for invalid or unavailable states.
    /// Non-null only when `truth_state` is `"invalid"` or `"unavailable"`.
    pub invalid_reason: Option<String>,
    /// Artifact directory path that was evaluated.
    /// Non-null when `truth_state != "not_configured"`.
    pub evaluated_path: Option<String>,
}

// ---------------------------------------------------------------------------
// /v1/system/run-artifact — TV-01C
// ---------------------------------------------------------------------------

/// TV-01C: Artifact provenance accepted at the most recent `start_execution_runtime`.
///
/// `truth_state` values:
/// - `"active"` — an artifact was accepted at run start and the run is active;
///   all identity fields are populated.
/// - `"no_run"` — no run is active (daemon is idle/halted); artifact provenance
///   is not surfaced.  All identity fields are null.  Fail-closed.
///
/// This is distinct from `ArtifactIntakeResponse` (`/api/v1/system/artifact-intake`),
/// which evaluates the currently configured file on demand.  This route surfaces
/// what was actually accepted and consumed when the run started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifactProvenanceResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// `"active"` | `"no_run"`.
    pub truth_state: String,
    /// Content-addressed artifact identity.  Non-null only when `truth_state == "active"`.
    pub artifact_id: Option<String>,
    /// Artifact type string.  Non-null only when `truth_state == "active"`.
    pub artifact_type: Option<String>,
    /// Promotion stage.  Non-null only when `truth_state == "active"`.
    pub stage: Option<String>,
    /// Producing system identifier.  Non-null only when `truth_state == "active"`.
    pub produced_by: Option<String>,
}

// ---------------------------------------------------------------------------
// JOUR-01: Paper trading journal and evidence surface
// ---------------------------------------------------------------------------

/// One durable signal-admission record from the journal.
///
/// Sourced from `audit_events` (topic=`'signal_ingestion'`, event_type=`'signal.admitted'`).
/// Written by the strategy signal route at Gate 7 `Ok(true)`.
///
/// Fields are extracted from the `payload` JSON column.  Parsing failure
/// for any field skips that row rather than emitting fabricated values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalAdmissionRow {
    /// Audit event UUID (stable identifier for this admission record).
    pub event_id: String,
    /// RFC 3339 UTC timestamp when this signal was admitted.
    pub ts_utc: String,
    /// Caller-supplied signal idempotency key.
    pub signal_id: String,
    /// Originating strategy identifier.
    pub strategy_id: String,
    pub symbol: String,
    /// `"buy"` or `"sell"`
    pub side: String,
    /// Ordered quantity.
    pub qty: i64,
    /// Run ID this admission belongs to.
    pub run_id: String,
    /// Stable DB provenance reference: `"audit_events:{event_id}"`.
    pub provenance_ref: String,
}

/// Fill evidence lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run; `rows` is authoritative fill history.
///   Empty `rows` = no fills yet recorded for this run.
/// - `"no_active_run"` — DB present but no active run; rows empty; not authoritative.
/// - `"no_db"` — no DB pool; rows empty; not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalFillsLane {
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<FillQualityTelemetryRow>,
}

/// Signal-admission history lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run; `rows` is the durable admitted-signal log.
///   Empty `rows` = no signals admitted yet.
/// - `"no_active_run"` — DB present but no active run; rows empty; not authoritative.
/// - `"no_db"` — no DB pool; rows empty; not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalAdmissionsLane {
    pub truth_state: String,
    /// `"postgres.audit_events[topic=signal_ingestion]"` when active.
    /// `"unavailable"` otherwise.
    pub backend: String,
    pub rows: Vec<PaperJournalAdmissionRow>,
}

/// Response for `GET /api/v1/paper/journal`.
///
/// Unified paper-trading evidence surface for operator review.  Separates
/// fill evidence (what executed) from signal-admission history (what was
/// submitted and accepted into the outbox).
///
/// Both lanes carry independent `truth_state` values.  An operator can
/// answer:
/// - What fills were produced by this run? → `fills_lane`
/// - What signals were admitted for dispatch? → `admissions_lane`
///
/// Neither lane fabricates history.  If a lane is unavailable its `rows`
/// are empty and `truth_state` says so explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Active run ID when both lanes are `"active"`.  `None` otherwise.
    pub run_id: Option<String>,
    /// Fill evidence sourced from `postgres.fill_quality_telemetry`.
    pub fills_lane: PaperJournalFillsLane,
    /// Signal-admission history sourced from `postgres.audit_events`.
    pub admissions_lane: PaperJournalAdmissionsLane,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/outbox — OPS-08 / EXEC-06: paper execution timeline
// ---------------------------------------------------------------------------

/// One row from the durable execution outbox for a run.
///
/// Fields extracted from `order_json` are `None` when the key is absent —
/// never fabricated.  `lifecycle_stage` is a display-friendly derivation
/// of `status` for operator readability.
///
/// Source: `postgres.oms_outbox` for the active run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutboxRow {
    /// Idempotency key assigned at signal intake or manual submit.
    pub idempotency_key: String,
    /// Run ID this outbox row belongs to.
    pub run_id: String,
    /// Durable status: `"PENDING"` | `"CLAIMED"` | `"DISPATCHING"` |
    /// `"SENT"` | `"ACKED"` | `"FAILED"` | `"AMBIGUOUS"`
    pub status: String,
    /// Display-friendly lifecycle stage derived from `status`.
    /// `"queued"` | `"claimed"` | `"submitting"` | `"sent_to_broker"` |
    /// `"acknowledged"` | `"failed"` | `"ambiguous"` | `"unknown"`
    pub lifecycle_stage: String,
    /// Symbol from `order_json["symbol"]`. `None` if absent.
    pub symbol: Option<String>,
    /// `"buy"` or `"sell"` from `order_json["side"]`. `None` if absent.
    pub side: Option<String>,
    /// Ordered qty from `order_json["qty"]`. `None` if absent.
    pub qty: Option<i64>,
    /// `"market"` or `"limit"` from `order_json["order_type"]`. `None` if absent.
    pub order_type: Option<String>,
    /// Originating strategy from `order_json["strategy_id"]`. `None` if absent
    /// (e.g., manual operator submit has no strategy attribution).
    pub strategy_id: Option<String>,
    /// Provenance mark from `order_json["signal_source"]`.
    /// `"external_signal_ingestion"` for strategy-driven intents; `None` for
    /// manual operator submits.
    pub signal_source: Option<String>,
    /// UTC timestamp when this intent was enqueued (durable).
    pub created_at_utc: String,
    /// UTC timestamp when the orchestrator claimed this row for dispatch.
    /// `None` if not yet claimed.
    pub claimed_at_utc: Option<String>,
    /// UTC timestamp when dispatch to broker began.
    /// `None` if not yet dispatching.
    pub dispatching_at_utc: Option<String>,
    /// UTC timestamp when the broker confirmed receipt.
    /// `None` if not yet sent.
    pub sent_at_utc: Option<String>,
}

/// Response wrapper for `GET /api/v1/execution/outbox`.
///
/// Surfaces the authoritative durable execution intent timeline for the
/// active run.  Operators can use this to understand what was submitted,
/// what is in-flight, what succeeded, and what failed — without relying
/// on ephemeral in-memory state.
///
/// `truth_state`:
/// - `"active"` — DB pool and active run present; `rows` is the authoritative
///   durable outbox for this run, ordered newest-first (at most 200 rows).
///   Empty `rows` means no execution intents have been enqueued yet in this run.
/// - `"no_active_run"` — DB pool present but no active run; `rows` is empty
///   and must NOT be treated as authoritative zero history.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutboxResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.oms_outbox"` when active; `"unavailable"` otherwise.
    pub backend: String,
    /// Active run ID when `truth_state == "active"`. `None` otherwise.
    pub run_id: Option<String>,
    /// At most 200 rows, newest-first.  Authoritative only when `truth_state == "active"`.
    pub rows: Vec<ExecutionOutboxRow>,
}

// ---------------------------------------------------------------------------
// Per-order execution-analysis surfaces (Cluster A5A–A5E)
// MT-07F: extracted to api_types/order_analysis.rs
// ---------------------------------------------------------------------------

#[path = "api_types/order_analysis.rs"]
mod order_analysis;
pub use order_analysis::*;

// ---------------------------------------------------------------------------
// GET /api/v1/execution/transport (Batch A2)
// ---------------------------------------------------------------------------

/// One outbox or inbox transport lane summary row.
///
/// Shape matches the GUI `TransportQueueRow` interface so it can be consumed
/// without mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportQueueRow {
    /// "outbox" | "inbox"
    pub queue_id: String,
    /// "outbox" | "inbox"
    pub direction: String,
    /// "idle" | "active" | "retrying" | "pending" | "applied"
    pub status: String,
    pub depth: usize,
    /// Age of oldest item in this lane, in milliseconds.
    pub oldest_age_ms: u64,
    pub retry_count: usize,
    pub duplicate_events: usize,
    pub orphaned_claims: usize,
    pub lag_ms: Option<u64>,
    pub last_activity_at: Option<String>,
    pub notes: String,
}

/// Response for `GET /api/v1/execution/transport`.
///
/// Shape matches the GUI `TransportSummary` interface (extra fields are
/// ignored by the GUI JSON consumer).
///
/// `truth_state`:
/// - `"active"` — an execution snapshot is present; all counts are authoritative.
/// - `"no_snapshot"` — no execution snapshot (run not started or daemon freshly
///   booted); all counts are zero and must NOT be read as authoritative-zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTransportResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// Total non-ACKED outbox rows in the current snapshot.
    pub outbox_depth: usize,
    /// Total recent inbox event rows in the current snapshot.
    pub inbox_depth: usize,
    /// Age of the oldest CLAIMED outbox row in milliseconds; 0 if none.
    pub max_claim_age_ms: u64,
    /// Count of FAILED + AMBIGUOUS outbox rows (proxy for dispatch retries).
    pub dispatch_retries: usize,
    /// Count of CLAIMED rows stale > 30 s (proxy for orphaned claims).
    pub orphaned_claims: usize,
    /// Always 0 — duplicate detection is not derivable from the in-memory snapshot.
    pub duplicate_inbox_events: usize,
    /// Per-lane queue summaries: [outbox, inbox] when snapshot is present, [] otherwise.
    pub queues: Vec<TransportQueueRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/quality (Batch A2)
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/market-data/quality`.
///
/// Shape matches the GUI `MarketDataQualitySummary` interface (extra fields
/// `canonical_route`, `truth_state`, `market_data_source`, `ws_continuity`
/// are ignored by the GUI JSON consumer).
///
/// `truth_state` is always `"active"` — this route derives from daemon in-memory
/// state which is always available.  Use `overall_health` to distinguish
/// configured vs not-configured states.
///
/// `overall_health`:
/// - `"ok"` — ExternalSignalIngestion + WS Live (stream confirmed healthy).
/// - `"warning"` — ExternalSignalIngestion + WS ColdStartUnproven or NotApplicable
///   (configured but continuity not yet proven).
/// - `"critical"` — ExternalSignalIngestion + WS GapDetected (active data gap).
/// - `"not_configured"` — no market-data source is wired
///   (`StrategyMarketDataSource::NotConfigured`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataQualityResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// "ok" | "warning" | "critical" | "not_configured" — maps to GUI `HealthState`.
    pub overall_health: String,
    /// Always 0 — freshness SLA is not tracked in current implementation.
    pub freshness_sla_ms: u64,
    /// Always 0 — stale symbol count is not tracked.
    pub stale_symbol_count: usize,
    /// Always 0 — missing bar count is not tracked.
    pub missing_bar_count: usize,
    /// Always 0 — venue disagreement is not tracked.
    pub venue_disagreement_count: usize,
    /// Always 0 — strategy blocks are not tracked here.
    pub strategy_blocks: usize,
    /// Always empty — no per-venue breakdown is available from in-memory state.
    pub venues: Vec<JsonValue>,
    /// Always empty — no per-issue tracking is available from in-memory state.
    pub issues: Vec<JsonValue>,
    /// "not_configured" | "signal_ingestion_ready" — raw source label.
    pub market_data_source: String,
    /// "not_applicable" | "cold_start_unproven" | "live" | "gap_detected".
    pub ws_continuity: String,
}

// ---------------------------------------------------------------------------
// /api/v1/system/topology (A3)
// ---------------------------------------------------------------------------

/// One service node in the local daemon topology.
///
/// Represents only what the daemon can prove from its own in-memory state.
/// No cluster/distributed topology is claimed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemTopologyServiceRow {
    /// Stable identifier, e.g. "daemon.runtime", "postgres", "broker.adapter".
    pub service_key: String,
    /// Human-readable label.
    pub label: String,
    /// Logical layer: "runtime" | "execution" | "data" | "broker" | "strategy".
    pub layer: String,
    /// "ok" | "warning" | "critical" | "unknown" | "not_configured" | "not_started".
    pub health: String,
    /// One-sentence role description.
    pub role: String,
    /// Keys of services this node directly depends on.
    pub dependency_keys: Vec<String>,
    /// What breaks if this service fails.
    pub failure_impact: String,
    /// RFC3339 UTC of last observed liveness signal; None if not tracked.
    pub last_heartbeat: Option<String>,
    /// Round-trip latency in ms; None if not measured.
    pub latency_ms: Option<u64>,
    /// Free-form notes (version, continuity state, pool status, etc.).
    pub notes: String,
}

/// Response for GET /api/v1/system/topology (A3).
///
/// `truth_state = "active"` always: derived entirely from daemon in-memory
/// state which is always present.  All fields represent local single-process
/// truth only — no cluster or distributed topology is claimed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemTopologyResponse {
    pub canonical_route: String,
    /// Always "active" — derived from in-memory state.
    pub truth_state: String,
    pub backend: String,
    /// RFC3339 UTC timestamp of this response.
    pub updated_at: String,
    pub services: Vec<SystemTopologyServiceRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/incidents (OPS-01)
// ---------------------------------------------------------------------------

/// A single incident row as surfaced by GET /api/v1/incidents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentRow {
    pub incident_id: String,
    pub opened_at_utc: String,
    pub title: String,
    /// "info" | "warning" | "critical"
    pub severity: String,
    /// "open" | "resolved"
    pub status: String,
    /// Alert class slug that prompted this incident; None if standalone.
    pub linked_alert_id: Option<String>,
    pub opened_by: String,
}

/// Response for GET /api/v1/incidents (OPS-01).
///
/// `truth_state`:
/// - `"active"` — DB pool present; rows are authoritative.
/// - `"no_db"` — no DB pool; rows are always empty (not absence of incidents).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentsResponse {
    pub canonical_route: String,
    /// "active" | "no_db"
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<IncidentRow>,
}

/// Request body for POST /api/v1/incidents (OPS-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIncidentRequest {
    pub title: String,
    /// "info" | "warning" | "critical"
    pub severity: String,
    /// Alert class slug from `/api/v1/alerts/active` that prompted this incident.
    pub linked_alert_id: Option<String>,
    /// Operator identifier; defaults to "operator" if absent.
    pub opened_by: Option<String>,
}

/// Response for POST /api/v1/incidents (OPS-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIncidentResponse {
    pub canonical_route: String,
    pub incident_id: String,
    pub opened_at_utc: String,
    pub title: String,
    pub severity: String,
    pub status: String,
    pub linked_alert_id: Option<String>,
    pub opened_by: String,
}

/// Response for POST /api/v1/incidents/:id/resolve (ALERTS-OPS-01A).
///
/// Returns the post-update incident row.  `status` is always `"resolved"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveIncidentResponse {
    pub canonical_route: String,
    pub incident_id: String,
    pub opened_at_utc: String,
    pub title: String,
    pub severity: String,
    /// Always `"resolved"` on success.
    pub status: String,
    pub linked_alert_id: Option<String>,
    pub opened_by: String,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/replace-cancel-chains (EXEC-02)
// ---------------------------------------------------------------------------

/// One cancel or replace lifecycle event within a replace/cancel chain.
///
/// `operation` is one of:
/// - `"cancel_ack"` — broker acknowledged the cancel; order is terminal.
/// - `"replace_ack"` — broker acknowledged the replace; `new_total_qty` is set.
/// - `"cancel_reject"` — broker rejected the cancel request.
/// - `"replace_reject"` — broker rejected the replace request.
///
/// Source: `postgres.oms_order_lifecycle_events` via EXEC-02 Phase 3b hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLifecycleEventApiRow {
    /// Equals `broker_message_id` — stable deduplication identity.
    pub event_id: String,
    pub internal_order_id: String,
    /// `"cancel_ack"` | `"replace_ack"` | `"cancel_reject"` | `"replace_reject"`
    pub operation: String,
    /// Broker-assigned order ID; `null` for paper adapters.
    pub broker_order_id: Option<String>,
    /// Post-replace total qty (`replace_ack` only); `null` for all others.
    pub new_total_qty: Option<i64>,
    /// RFC 3339 timestamp when the event was recorded by the orchestrator.
    pub recorded_at_utc: String,
}

/// Response for GET /api/v1/execution/replace-cancel-chains (EXEC-02).
///
/// `truth_state` values:
/// - `"no_db"` — DB pool unavailable; chain data cannot be read.
/// - `"no_active_run"` — DB present but no active run ID is known.
/// - `"active"` — DB-backed; `chains` contains lifecycle events for the
///   active run (empty array = no cancel/replace events yet in this run).
///
/// Source: `postgres.oms_order_lifecycle_events` written by
/// `ExecutionOrchestrator::tick()` Phase 3b (EXEC-02).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceCancelChainsResponse {
    pub canonical_route: String,
    /// `"no_db"` | `"no_active_run"` | `"active"`
    pub truth_state: String,
    pub backend: String,
    /// Operator guidance note.
    pub note: String,
    /// Lifecycle events for the active run, oldest-first.
    /// Empty array = no cancel/replace operations recorded yet.
    pub chains: Vec<OrderLifecycleEventApiRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/alerts/triage (A4)
// ---------------------------------------------------------------------------

/// One alert row on the triage surface.
///
/// Sourced from the same in-memory fault-signal computation as
/// `/api/v1/alerts/active`.  `status` reflects DB-backed ack state when DB
/// is present; always `"unacked"` when no DB pool is available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertTriageAlertRow {
    pub alert_id: String,
    /// "info" | "warning" | "critical".
    pub severity: String,
    /// "acked" | "unacked".  "acked" requires DB presence; "unacked" when no DB.
    pub status: String,
    pub title: String,
    pub domain: String,
    /// None — no incident linkage exists.
    pub linked_incident_id: Option<String>,
    /// "open" | "resolved" when a linked incident exists; None when no incident is linked.
    pub linked_incident_status: Option<String>,
    /// None — alert-to-order linkage is not tracked at this surface.
    pub linked_order_id: Option<String>,
    /// None — alert-to-strategy linkage is not tracked at this surface.
    pub linked_strategy_id: Option<String>,
    /// RFC3339 UTC of when this alert was acked (acked rows); None for unacked
    /// rows (in-memory fault signals have no durable creation timestamp).
    pub created_at: Option<String>,
    /// None — assignment is not implemented.
    pub assigned_to: Option<String>,
}

/// Response for GET /api/v1/alerts/triage (A4).
///
/// `truth_state`:
/// - `"active"` — alert source is real; ack state is DB-backed from
///   `sys_alert_acks`.  `status` is authoritative ("acked" | "unacked").
/// - `"no_db"` — alert source is real (in-memory); ack state unavailable
///   (no DB pool).  All rows have `status = "unacked"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertTriageResponse {
    pub canonical_route: String,
    /// "active" (DB-backed ack state) | "no_db" (ack state unavailable).
    pub truth_state: String,
    pub backend: String,
    /// Operator notice about triage lifecycle scope.
    pub triage_note: String,
    pub rows: Vec<AlertTriageAlertRow>,
}

/// Request body for POST /api/v1/alerts/triage/ack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertAckRequest {
    pub alert_id: String,
    /// Optional operator identifier for audit trail. Defaults to "operator".
    pub acked_by: Option<String>,
}

// ---------------------------------------------------------------------------
// FLOW-01 / FLOW-03: Execution flow read model
// ---------------------------------------------------------------------------

/// One row in the execution flow read model assembled from existing durable
/// tables: `oms_outbox`, `oms_order_lifecycle_events`, `fill_quality_telemetry`.
///
/// `row_id` is stable and deterministic across re-queries of the same event.
/// `source_table` identifies which DB table the row was derived from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionFlowApiRow {
    /// Stable deterministic row identifier. Never a UUIDv4.
    /// Format varies by source: `"outbox:enqueued:{key}"`, `"lifecycle:{id}"`,
    /// `"fill:{uuid}"`.
    pub row_id: String,
    /// RFC 3339 timestamp from the authoritative source column.
    pub ts_utc: String,
    /// Stage label. Examples: `"outbox_enqueued"`, `"outbox_claimed"`,
    /// `"outbox_dispatching"`, `"broker_sent"`, `"broker_cancel_ack"`,
    /// `"broker_partial_fill"`, `"broker_final_fill"`.
    pub stage: String,
    /// `"info"` | `"warn"` | `"error"`.
    pub severity: String,
    /// The durable run this event belongs to.
    pub run_id: String,
    /// Internal order identifier. For outbox rows this equals `idempotency_key`.
    /// `None` when not derivable from the source row.
    pub internal_order_id: Option<String>,
    /// Broker-assigned order identifier. `None` until the broker acknowledges.
    pub broker_order_id: Option<String>,
    /// Ticker symbol. `None` for outbox lifecycle events where the symbol
    /// is not stored in a dedicated column.
    pub symbol: Option<String>,
    /// Short operator-readable description of this event.
    pub message: String,
    /// Which durable table this row was assembled from.
    pub source_table: String,
}

/// Response wrapper for `GET /api/v1/execution/flow`.
///
/// `truth_state`:
/// - `"active"` — DB available, run context resolved; `rows` is authoritative.
///   Empty `rows` means no flow events match the query — not absence of source.
/// - `"no_active_run"` — DB available but no active run exists and no explicit
///   `run_id` was provided. `rows` is empty and **not authoritative**.
/// - `"no_db"` — no DB pool configured. `rows` is empty and not authoritative.
///
/// The operator must not interpret `"no_active_run"` or `"no_db"` as
/// authoritative empty history.
///
/// Filters: `run_id` (UUID, optional), `order_id` (string, optional),
/// `limit` (1–200, default 100).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionFlowResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// `"active"` | `"no_active_run"` | `"no_db"`.
    pub truth_state: String,
    /// Durable sources joined. `"unavailable"` when truth_state is not `"active"`.
    pub backend: String,
    /// The run_id used for the query. `None` when truth_state is not `"active"`.
    pub run_id: Option<String>,
    /// Flow event rows, sorted oldest-first. At most `limit` rows (default 100,
    /// max 200). Empty when truth_state is not `"active"` or when no events match.
    pub rows: Vec<ExecutionFlowApiRow>,
}

/// Response for POST /api/v1/alerts/triage/ack.
///
/// **Advisory semantics — not authoritative fault resolution.**
///
/// The ack records that an operator has acknowledged the alert class.
/// It does **not** suppress the fault from `/api/v1/alerts/active` and does
/// **not** resolve the underlying condition.  The alert remains active on the
/// live surface until the fault condition itself clears, regardless of ack state.
///
/// `ack_scope` is always `"annotation_only"` to make this contract explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertAckResponse {
    pub canonical_route: String,
    pub alert_id: String,
    pub acked_at_utc: String,
    pub acked_by: String,
    /// Always `"annotation_only"`.
    ///
    /// The ack is a durable operator annotation in `sys_alert_acks`.
    /// It does not suppress or resolve the fault — the alert continues to
    /// appear in `/api/v1/alerts/active` until the underlying condition clears.
    pub ack_scope: String,
}

// ---------------------------------------------------------------------------
// BACKTEST-DAEMON-JOBS-01: Backtest job API types
// ---------------------------------------------------------------------------

/// POST /api/v1/backtests/jobs — submit a CSV- or md_bars-sourced backtest job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestJobRequest {
    /// BACKTEST-DB-BARS-SOURCE-01: bars source — `"csv"` (default) or
    /// `"md_bars"`. Omitted/unrecognized-but-blank defaults to `"csv"` for
    /// backward compatibility with pre-existing CSV-only requests; any other
    /// unrecognized value is refused explicitly (never silently treated as csv).
    #[serde(default = "default_backtest_job_source")]
    pub source: String,
    /// Absolute path to the CSV bars file. Required when `source="csv"`.
    /// Ignored (and may be omitted) when `source="md_bars"`.
    #[serde(default)]
    pub bars_path: String,
    /// Strategy name (e.g. "swing_momentum"). Must be a registered built-in.
    pub strategy: String,
    /// Ticker symbol (e.g. "TEST", "SPY").
    pub symbol: String,
    /// Bar timeframe in seconds (must be > 0). Drives the backtest engine
    /// config for both sources.
    pub timeframe_secs: i64,
    /// Starting cash in micros (must be > 0).
    pub initial_cash_micros: i64,
    /// Optional output directory root (artifacts written to `<out_dir>/<run_id>/`).
    /// Defaults to `exports/backtests` relative to daemon working directory.
    pub out_dir: Option<String>,
    /// Enable integrity checks. Defaults to false.
    pub integrity_enabled: Option<bool>,
    /// Integrity stale threshold in ticks (seconds for time-indexed bar feeds).
    ///
    /// When `integrity_enabled` is true and a bar arrives more than this many seconds
    /// after the previous bar, integrity disarms and sets `execution_blocked=true`.
    ///
    /// If omitted, the daemon applies a timeframe-aware default:
    /// - `timeframe_secs >= 86400` (daily): **172800** (2 days — covers normal daily gaps)
    /// - otherwise: **120** (mirrors `conservative_defaults`)
    ///
    /// The conservative_defaults value of 120 causes immediate `execution_blocked=true`
    /// on the first daily bar gap (86400 s >> 120 s). Always set this explicitly
    /// for daily data, or rely on the timeframe-aware default.
    pub integrity_stale_threshold_ticks: Option<u64>,
    /// Run in shadow mode (strategy signals observed but not executed). Defaults to false.
    pub shadow: Option<bool>,
    /// BACKTEST-DB-BARS-SOURCE-01: `md_bars` timeframe string for the DB query
    /// (e.g. `"1D"`, `"5m"`) — independent of `timeframe_secs`, which only
    /// drives the engine config. Required when `source="md_bars"`; ignored
    /// when `source="csv"`.
    #[serde(default)]
    pub timeframe: Option<String>,
    /// BACKTEST-DB-BARS-SOURCE-01: inclusive range start, RFC3339
    /// (e.g. `"2026-06-01T00:00:00Z"`). Required when `source="md_bars"`.
    #[serde(default)]
    pub start: Option<String>,
    /// BACKTEST-DB-BARS-SOURCE-01: inclusive range end, RFC3339. Required when
    /// `source="md_bars"`. Must be `>= start`.
    #[serde(default)]
    pub end: Option<String>,
}

fn default_backtest_job_source() -> String {
    "csv".to_string()
}

/// Response to POST /api/v1/backtests/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestJobAcceptedResponse {
    pub accepted: bool,
    pub job_id: Uuid,
    /// "queued" immediately after acceptance.
    pub status: String,
    /// Populated only if job already completed synchronously (not expected).
    pub artifact_dir: Option<String>,
    /// Populated if request was refused before queuing.
    pub error: Option<String>,
}

/// Single job summary row in GET /api/v1/backtests/jobs list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestJobSummary {
    pub job_id: Uuid,
    pub status: String,
    pub strategy: String,
    pub symbol: String,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    pub artifact_dir: Option<String>,
    pub error: Option<String>,
}

/// Response to GET /api/v1/backtests/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestJobsListResponse {
    pub truth_state: String,
    pub jobs: Vec<BacktestJobSummary>,
}

/// Response to GET /api/v1/backtests/jobs/:job_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestJobStatusResponse {
    pub truth_state: String,
    pub job_id: Uuid,
    pub status: String,
    pub strategy: String,
    pub symbol: String,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    pub artifact_dir: Option<String>,
    pub manifest_path: Option<String>,
    pub metrics_path: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// DATA-INGEST-DAEMON-JOBS-01: Market-data ingest job API
// ---------------------------------------------------------------------------

fn default_dry_run() -> bool {
    true
}

fn default_asset_class_equity() -> String {
    "equity".to_string()
}

/// Request body for POST /api/v1/ingest/jobs.
///
/// For CSV jobs: set source="csv", csv_path, timeframe.
/// For provider dry-run jobs: set source="twelvedata", mode="sync_provider",
///   symbols_source="registry", dry_run=true (default), allow_provider_api_calls=false (default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobRequest {
    /// Source type: "csv" | "twelvedata".
    pub source: String,
    /// Job mode. Required for provider jobs: "sync_provider".
    /// Omit or null for CSV jobs.
    pub mode: Option<String>,
    /// For source="csv": path to the CSV file (required, must not be empty).
    pub csv_path: Option<String>,
    /// Timeframe string: "1D" | "1m" | "5m".
    pub timeframe: String,
    /// Source label stored in the quality report (defaults to source if omitted).
    pub source_label: Option<String>,
    /// Output directory root for quality report artifacts.
    /// Defaults to "exports/md_ingest" relative to daemon working directory.
    pub out_dir: Option<String>,
    // -----------------------------------------------------------------------
    // Provider-job fields (DATA-INGEST-DAEMON-PROVIDER-JOBS-01 / DATA-PROVIDER-FOUNDATION-01)
    // -----------------------------------------------------------------------
    /// Symbol source for provider jobs: "registry" uses config/instruments/equities.json.
    /// Required when source="twelvedata" and mode="sync_provider".
    pub symbols_source: Option<String>,
    /// Override the instrument registry path.
    /// When omitted, uses MQK_INSTRUMENT_REGISTRY_PATH or "config/instruments/equities.json".
    pub registry_path: Option<String>,
    /// Override the provider registry path.
    /// When omitted, uses MQK_PROVIDER_REGISTRY_PATH or "config/providers/providers.json".
    pub provider_registry_path: Option<String>,
    /// Asset class to ingest. Default: "equity".
    /// Accepted: "equity" | "etf" | "crypto" | "futures" | "options" | "forex".
    #[serde(default = "default_asset_class_equity")]
    pub asset_class: String,
    /// Inclusive start date YYYY-MM-DD (optional; used for scoped provider sync).
    pub start: Option<String>,
    /// Inclusive end date YYYY-MM-DD (optional; used for scoped provider sync).
    pub end: Option<String>,
    /// Dry-run mode: resolve symbols and validate, but do NOT call provider,
    /// do NOT write DB, do NOT write CSV. Default: true (safe).
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Permit real provider API calls. Default: false (safe).
    /// Must be explicitly true to allow live provider ingestion.
    #[serde(default)]
    pub allow_provider_api_calls: bool,
    /// Optional guardrail: max API credits per minute.
    pub api_credits_per_minute: Option<i64>,
    /// Optional guardrail: max API credits per day.
    pub api_credits_per_day: Option<i64>,
}

/// Response to POST /api/v1/ingest/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobAcceptedResponse {
    pub accepted: bool,
    pub job_id: Uuid,
    /// "queued" immediately after acceptance; "refused" on validation failure.
    pub status: String,
    pub source: String,
    /// Populated if request was refused before queuing.
    pub error: Option<String>,
    // Provider job fields (null for CSV jobs or on refusal):
    /// Whether this is a dry-run job (no provider calls, no writes).
    pub dry_run: Option<bool>,
    /// Whether provider API calls are permitted.
    pub provider_api_calls_allowed: Option<bool>,
    /// Number of planned symbols (null until background task resolves them).
    pub symbols_count: Option<usize>,
    /// Number of provider API calls made (always 0 at acceptance time).
    pub api_calls_made: Option<i64>,
}

/// Single job summary row in GET /api/v1/ingest/jobs list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobSummary {
    pub job_id: Uuid,
    pub status: String,
    pub source: String,
    /// Job mode: null for CSV, "sync_provider" for provider jobs.
    pub mode: Option<String>,
    pub timeframe: String,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    pub rows_read: Option<i64>,
    pub rows_inserted: Option<i64>,
    pub rows_rejected: Option<i64>,
    /// Filesystem path to the written data_quality.json artifact (completed jobs).
    pub quality_report_path: Option<String>,
    pub error: Option<String>,
    // Provider job fields:
    pub dry_run: bool,
    pub symbols_count: Option<usize>,
    pub api_calls_made: i64,
    pub symbols_completed: Option<usize>,
    pub symbols_failed: Option<usize>,
}

/// Response to GET /api/v1/ingest/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobsListResponse {
    pub truth_state: String,
    pub jobs: Vec<IngestJobSummary>,
}

// ---------------------------------------------------------------------------
// DATA-PROVIDER-LATEST-BAR-POLL-01: latest closed-bar feed poll-once
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/market-data/feed/poll-once`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedPollOnceRequest {
    pub provider_id: String,
    pub symbols: Vec<String>,
    pub timeframe: String,
    /// Dry-run mode: validate cadence/symbols/provider registry only.
    /// Makes zero provider calls and zero DB writes. Default: true.
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Permit real provider API calls. Default: false.
    /// Must be explicitly true when `dry_run=false`.
    #[serde(default)]
    pub allow_provider_api_calls: bool,
    /// Deterministic UTC reference time for tests/operator replay.
    /// Accepts RFC3339 UTC timestamps.
    pub now_utc: Option<String>,
    /// Override provider registry path; omitted uses AppState default.
    pub provider_registry_path: Option<String>,
}

/// Per-symbol result for one latest closed-bar poll.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedPollSymbolResult {
    pub symbol: String,
    pub status: String,
    pub expected_latest_closed_bar_ts: i64,
    pub returned_bar_ts: Option<i64>,
    pub rows_inserted: u64,
    pub rows_updated: u64,
    pub rows_skipped: u64,
    pub error: Option<String>,
}

/// Response body for `POST /api/v1/market-data/feed/poll-once`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedPollOnceResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub provider_id: String,
    pub timeframe: String,
    pub dry_run: bool,
    pub provider_api_calls_allowed: bool,
    pub symbols_count: usize,
    pub poll_time_utc: String,
    pub latest_expected_closed_bar_ts: i64,
    pub next_poll_ts: i64,
    pub inserted_count: u64,
    pub updated_count: u64,
    pub skipped_count: u64,
    pub error_count: u64,
    pub api_calls_made: u64,
    pub symbols: Vec<MarketDataFeedPollSymbolResult>,
    pub error: Option<String>,
}

/// Response body for `GET /api/v1/market-data/feed/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub limitation: String,
    pub last_poll: Option<MarketDataFeedPollOnceResponse>,
}

/// Request body for `POST /api/v1/market-data/feed/scheduler/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedSchedulerStartRequest {
    pub provider_id: String,
    pub symbols: Vec<String>,
    pub timeframe: String,
    /// Permit real provider API calls. Default: false.
    /// Must be explicitly true when `dry_run=false`.
    #[serde(default)]
    pub allow_provider_api_calls: bool,
    /// Dry-run mode: validate cadence/symbols/provider registry only.
    /// Makes zero provider calls and zero DB writes. Default: true.
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Run one poll immediately after start. Default: false.
    #[serde(default)]
    pub poll_immediately: bool,
    /// Deterministic UTC reference time for tests/operator replay.
    /// Accepts RFC3339 UTC timestamps.
    pub now_utc: Option<String>,
    /// Override provider registry path; omitted uses AppState default.
    pub provider_registry_path: Option<String>,
}

/// Response body for latest-bar scheduler start/stop/status routes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataFeedSchedulerStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub limitation: String,
    pub running: bool,
    pub provider_id: Option<String>,
    pub timeframe: Option<String>,
    pub symbols: Vec<String>,
    pub last_poll_utc: Option<String>,
    pub next_poll_utc: Option<String>,
    pub latest_expected_closed_bar_utc: Option<String>,
    pub last_result: Option<MarketDataFeedPollOnceResponse>,
    pub last_error: Option<String>,
    pub started_at_utc: Option<String>,
    pub stopped_at_utc: Option<String>,
    pub poll_count: u64,
    pub inserted_count: u64,
    pub unchanged_or_skipped_count: u64,
    pub error_count: u64,
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-RESULTS-01: md_bars coverage query response
// ---------------------------------------------------------------------------

/// One (symbol, timeframe) group row in the coverage response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdBarsCoverageRow {
    pub symbol: String,
    pub timeframe: String,
    /// Total bar count for this group.
    pub bars: i64,
    /// Earliest `end_ts` (Unix seconds) in this group.
    pub min_end_ts: i64,
    /// Latest `end_ts` (Unix seconds) in this group.
    pub max_end_ts: i64,
    /// RFC3339 timestamp of the most-recent ingest for this group. Null when not tracked.
    pub latest_ingested_at: Option<String>,
}

/// Response for `GET /api/v1/market-data/coverage`.
///
/// `truth_state` values:
/// - `"active"`        — DB responded; one or more groups returned.
/// - `"empty"`         — DB responded; no rows match the filter.
/// - `"db_unavailable"` — daemon has no DB pool configured.
/// - `"unavailable"`   — DB pool present but query failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdBarsCoverageResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// Echoes the `?timeframe=` query param, or `null` when not supplied (all timeframes).
    pub timeframe: Option<String>,
    pub rows: Vec<MdBarsCoverageRow>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01: Intraday refresh evidence status
// ---------------------------------------------------------------------------

/// Per-symbol row from the latest intraday refresh evidence file.
///
/// Fields map directly from the `intraday-refresh-v1` evidence JSON.
/// Fields absent in the evidence file (e.g., provider fields in check_only mode)
/// are `None` — never substituted with defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntradayRefreshSymbolStatus {
    pub symbol: String,
    /// "PASS" | "FAIL", or `null` when the evidence is from check_only mode.
    pub gate: Option<String>,
    /// Total completed bars in md_bars for this symbol/timeframe.
    pub completed_count: Option<i64>,
    /// ISO timestamp of the latest completed bar end_ts (max_ts_iso in evidence).
    pub latest_completed_bar_ts: Option<String>,
    /// Age of the latest bar in minutes (-1 if unavailable in evidence).
    pub staleness_min: Option<i64>,
    /// Provider source string used for the refresh, e.g. "twelvedata" or "alpaca".
    pub provider_source: Option<String>,
    /// Whether the provider key was configured when the refresh ran.
    pub provider_configured: Option<bool>,
    /// Whether a provider sync was attempted.
    pub provider_attempted: Option<bool>,
    /// Whether the provider sync succeeded (exit code 0).
    pub provider_success: Option<bool>,
    /// Rows inserted into md_bars during the refresh.
    pub rows_inserted: Option<i64>,
    /// Rows updated in md_bars during the refresh.
    pub rows_updated: Option<i64>,
    /// Rows dropped because the bar was flagged incomplete.
    pub rows_filtered_incomplete: Option<i64>,
    /// Rows dropped because the bar was still in-progress (current bar).
    pub rows_filtered_in_progress: Option<i64>,
    /// Fail reasons for this symbol, empty on PASS.
    pub fail_reasons: Vec<String>,
}

/// Response for `GET /api/v1/market-data/intraday-refresh/status`.
///
/// Read-only. Reads the latest `intraday_refresh_*.json` evidence file
/// written by `Refresh-IntradayMarketData.ps1`. No provider calls, no DB writes,
/// no broker interaction.
///
/// `truth_state` values:
/// - `"active"`               — latest evidence file parsed successfully.
/// - `"no_evidence"`          — no evidence file found in the evidence directory.
/// - `"parse_error"`          — evidence file found but JSON is malformed or has
///   an unsupported schema_version.
/// - `"backend_unavailable"`  — evidence directory could not be read.
///
/// `stale_or_missing_evidence` is `true` when:
/// - `truth_state != "active"`, or
/// - `produced_at_utc` is more than 24 h in the past.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntradayRefreshStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// Filesystem path of the evidence file that was read. `null` when no file found.
    pub evidence_path: Option<String>,
    /// `true` when evidence is absent, unreadable, or older than 24 h.
    pub stale_or_missing_evidence: bool,
    pub schema_version: Option<String>,
    pub produced_at_utc: Option<String>,
    /// "check_only" | "once" | "interval".
    pub mode: Option<String>,
    /// Provider source from evidence file: "twelvedata" | "alpaca".
    pub source: Option<String>,
    pub timeframe: Option<String>,
    /// Whether all symbols passed the fail-closed gates on the most recent run.
    pub all_passed: Option<bool>,
    /// Human-readable pass/fail reason from the evidence file.
    pub reason: Option<String>,
    pub symbols: Vec<IntradayRefreshSymbolStatus>,
    /// Error description when `truth_state` is `"parse_error"` or `"backend_unavailable"`.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-SYNC-ALL-01: Tracked-equities registry preview
// ---------------------------------------------------------------------------

/// One enabled equity entry in GET /api/v1/ingest/tracked-equities.
///
/// Contains only the fields needed for operator display — not the full TrackedInstrument.
/// No provider API calls. No DB writes. Pure registry read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedEquitySummary {
    pub symbol: String,
    pub instrument_id: String,
    pub provider: String,
    pub venue: String,
    pub timeframes: Vec<String>,
}

/// Response for GET /api/v1/ingest/tracked-equities.
///
/// `truth_state` values:
/// - `"active"`              — registry loaded; symbols returned.
/// - `"registry_unavailable"` — file not found or not readable.
/// - `"registry_invalid"`    — file found but failed JSON parsing or validation.
///
/// Safety invariants:
/// - No broker adapter called. No provider API calls. No DB writes.
/// - Does not touch live/paper execution state.
/// - Read-only access to config/instruments/equities.json (or MQK_INSTRUMENT_REGISTRY_PATH).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedEquitiesResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// Absolute or relative path to the registry file that was read.
    pub registry_path: String,
    /// Total count of enabled equity instruments.
    pub count: usize,
    /// All enabled equity symbols in deterministic alphabetical order.
    pub symbols: Vec<TrackedEquitySummary>,
    /// First symbol alphabetically (None when count=0).
    pub first_symbol: Option<String>,
    /// Last symbol alphabetically (None when count=0).
    pub last_symbol: Option<String>,
    /// Populated on registry_unavailable or registry_invalid.
    pub error: Option<String>,
}

/// Response to GET /api/v1/ingest/jobs/:job_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobStatusResponse {
    pub truth_state: String,
    pub job_id: Uuid,
    pub status: String,
    pub source: String,
    /// Job mode: null for CSV, "sync_provider" for provider jobs.
    pub mode: Option<String>,
    pub timeframe: String,
    /// CSV path for source="csv" jobs.
    pub csv_path: Option<String>,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    pub rows_read: Option<i64>,
    pub rows_inserted: Option<i64>,
    pub rows_rejected: Option<i64>,
    /// Filesystem path to the written data_quality.json artifact (completed jobs).
    pub quality_report_path: Option<String>,
    pub error: Option<String>,
    // Provider job fields:
    pub dry_run: bool,
    pub provider_api_calls_allowed: bool,
    pub api_calls_made: i64,
    pub symbols_source: Option<String>,
    pub registry_path_used: Option<String>,
    pub symbols_count: Option<usize>,
    pub planned_first_symbol: Option<String>,
    pub planned_last_symbol: Option<String>,
    // Provider registry fields (DATA-PROVIDER-FOUNDATION-01):
    /// Asset class requested for this job.
    pub asset_class: String,
    /// Whether the provider is enabled in the provider registry (None if registry unavailable).
    pub provider_enabled: Option<bool>,
    /// Provider verification status from the provider registry (None if registry unavailable).
    pub provider_verification_status: Option<String>,
    /// Number of symbols for which provider fetch succeeded (real jobs only; null for dry-run).
    pub symbols_completed: Option<usize>,
    /// Number of symbols for which provider fetch failed (real jobs only; null for dry-run).
    pub symbols_failed: Option<usize>,
}

// ---------------------------------------------------------------------------
// BRK-GAP-REST-RECOVERY-01 — WS gap fill recovery API types
// ---------------------------------------------------------------------------

/// POST /api/v1/ops/repair/ws-gap-fill-recovery — request body.
#[derive(Debug, Deserialize)]
pub struct WsGapFillRecoveryRequest {
    /// Run UUID to recover fills for.
    pub run_id: String,
    /// If `true`, plan only (classify what would be recovered without mutating inbox).
    /// Default: `true`.  Set to `false` to actually insert the inbox rows.
    #[serde(default = "default_ws_gap_dry_run")]
    pub dry_run: bool,
}

fn default_ws_gap_dry_run() -> bool {
    true
}

/// One fill successfully recovered or already present from the WS gap window.
#[derive(Debug, Serialize, Clone)]
pub struct WsGapRecoveredFill {
    /// Alpaca activity ID (the authoritative broker-side activity identifier).
    pub broker_activity_id: String,
    /// Alpaca broker order UUID matched in `broker_order_map`.
    pub broker_order_id: String,
    /// OMS internal order ID that owns this broker order.
    pub internal_order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty_str: String,
    pub price_str: String,
    /// `"fill"` or `"partial_fill"`.
    pub event_kind: String,
    /// Stable `broker_message_id` used as the inbox idempotency key.
    pub inbox_broker_message_id: String,
    /// `true` when the row was already present (idempotent recovery).
    pub already_present: bool,
}

/// POST /api/v1/ops/repair/ws-gap-fill-recovery — response body.
#[derive(Debug, Serialize)]
pub struct WsGapFillRecoveryResponse {
    pub truth_state: String,
    pub run_id: String,
    /// `rest_activity_after` cursor value used as the lower bound for fetching.
    /// `None` when no prior cursor was available (recovery covered all activities).
    pub rest_activity_after: Option<String>,
    /// Whether a `WsGapFillFetcher` was configured on this daemon.
    pub fetcher_available: bool,
    /// Total activities returned by the fetcher.
    pub activities_fetched: usize,
    /// Number of fills successfully inserted (or dry-run planned).
    pub recovered_count: usize,
    /// Number of fills that were already present in the inbox (idempotent).
    pub already_present_count: usize,
    /// Number of REST activities where `order_id` was not in the run's broker_order_map.
    pub unknown_order_count: usize,
    /// Number of REST activities refused due to malformed/missing fields.
    pub malformed_count: usize,
    /// Whether this response reflects a dry-run (no mutation).
    pub dry_run: bool,
    /// Filled when the operation was refused before any fetch or mutation.
    pub gate: Option<String>,
    pub evidence: String,
    /// Fills recovered (or planned in dry-run mode).
    pub recovered_fills: Vec<WsGapRecoveredFill>,
    /// Whether the persisted `rest_activity_after` cursor was advanced after recovery.
    /// Always `false` for dry_run, REST error, no eligible activity, or unsafe batch.
    pub cursor_advanced: bool,
    /// New `rest_activity_after` value after advancement; `None` when `cursor_advanced` is `false`.
    pub new_rest_activity_after: Option<String>,
}

// ---------------------------------------------------------------------------
// BROKER-POSITION-BASELINE-ADOPTION-01
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/ops/repair/adopt-broker-position-baseline`.
///
/// The operator must supply the literal confirmation string
/// `"ADOPT_BROKER_POSITION_BASELINE"` to prevent accidental adoption.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AdoptBrokerPositionBaselineRequest {
    /// Must equal `"ADOPT_BROKER_POSITION_BASELINE"` (case-sensitive).
    pub confirmation: String,
}

/// Response from `POST /api/v1/ops/repair/adopt-broker-position-baseline`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AdoptBrokerPositionBaselineResponse {
    pub truth_state: String,
    /// Whether the adoption was accepted and written.
    pub accepted: bool,
    /// Human-readable outcome description for the operator.
    pub decision: String,
    /// Number of positions recorded in the adopted baseline snapshot.
    pub baseline_position_count: usize,
    /// Number of open orders recorded in the adopted baseline snapshot.
    pub baseline_order_count: usize,
    /// ISO-8601 timestamp of the broker snapshot that was adopted.
    pub snapshot_captured_at: Option<String>,
    /// Audit event ID written alongside the adoption (UUIDv5, deterministic).
    pub audit_event_id: Option<String>,
    /// Filled when the route refused the request before any mutation.
    pub gate: Option<String>,
    /// True when idle reconcile comparison was run and its result persisted.
    /// False for refused responses (adoption did not complete).
    #[serde(default)]
    pub reconcile_refreshed: bool,
    /// Reconcile status after idle refresh: `"ok"`, `"dirty"`, or `""` (not run).
    #[serde(default)]
    pub reconcile_status_after: String,
    /// Position mismatch count from idle reconcile (0 if not run or clean).
    #[serde(default)]
    pub reconcile_mismatched_positions: usize,
    /// Order mismatch count from idle reconcile (0 if not run or clean).
    #[serde(default)]
    pub reconcile_mismatched_orders: usize,
    /// Fill mismatch count from idle reconcile (0 if not run or clean).
    #[serde(default)]
    pub reconcile_mismatched_fills: usize,
}

// ---------------------------------------------------------------------------
// /api/v1/watchlist/status — PAPER-HANDOFF-READONLY-01
// ---------------------------------------------------------------------------

/// Read-only watchlist artifact status response.
///
/// Returned by `GET /api/v1/watchlist/status`.  Exposes the outcome of loading
/// the `watchlist-v1` artifact configured at `MQK_PAPER_WATCHLIST_PATH`.
///
/// # Invariants
/// - `approved_for_live` is ALWAYS `false`.  There is no outcome in which live
///   trading is authorized from the scanner watchlist path.
/// - `approved_for_autonomous_paper` is `true` only when `status == "loaded_approved"`.
/// - This endpoint is read-only: no broker calls, no DB mutations, no orders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistStatusResponse {
    /// Path configured in `MQK_PAPER_WATCHLIST_PATH`.  `null` when not configured.
    pub configured_path: Option<String>,
    /// One of: `"not_configured"` | `"missing"` | `"invalid"` |
    ///         `"loaded_not_approved"` | `"loaded_approved"`.
    pub status: String,
    /// `"watchlist-v1"` or `"watchlist-v2"` from the loaded artifact.
    /// `null` when no artifact was loaded (not_configured, missing, or invalid).
    pub schema_version: Option<String>,
    /// Whether the scanner-level promotion approved autonomous paper trading.
    /// Always `false` except when `status == "loaded_approved"`.
    pub approved_for_autonomous_paper: bool,
    /// Always `false` — hard live-lock invariant.
    pub approved_for_live: bool,
    /// Approved symbols list.  Empty unless `status == "loaded_approved"`.
    pub symbols: Vec<String>,
    /// Top-ranked symbol (`symbols[0]`) if present.  `null` otherwise.
    pub top_symbol: Option<String>,
    /// Strategy assignment map: symbol → strategy_id.
    /// Empty unless `status == "loaded_approved"`.
    pub strategy_assignments: serde_json::Value,
    /// `max_symbols_to_trade` from artifact.  `null` when not loaded.
    pub max_symbols_to_trade: Option<u64>,
    /// `max_concurrent_positions` from artifact.  `null` when not loaded.
    pub max_concurrent_positions: Option<u64>,
    /// Validation failure reasons.  Non-empty only when `status == "invalid"`.
    pub failure_reasons: Vec<String>,
    /// UTC timestamp when the status check was performed.
    pub checked_at_utc: String,
}

// ---------------------------------------------------------------------------
// /api/v1/watchlist/admission-check — PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01
// ---------------------------------------------------------------------------

/// Dry-run watchlist signal admission check response.
///
/// Returned by `GET /api/v1/watchlist/admission-check?symbol=<sym>&strategy_id=<id>`.
///
/// # Invariants
/// - `approved_for_live` is ALWAYS `false`.  No outcome authorizes live trading.
/// - `note` is ALWAYS `"dry_run_only_not_enforced"`.  This check is advisory only.
/// - This endpoint is read-only: no broker calls, no DB mutations, no orders,
///   no outbox writes, no inbox writes.
/// - The real `POST /api/v1/strategy/signal` route is NOT modified by this patch.
///   Watchlist admission is not enforced in the live signal path (PAPER-HANDOFF-ENFORCE-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistAdmissionCheckResponse {
    /// Whether the (symbol, strategy_id) pair would be admitted under the current watchlist.
    pub allowed: bool,
    /// Reason string.  One of:
    ///   `"watchlist_not_configured"` | `"watchlist_missing"` | `"watchlist_invalid"` |
    ///   `"watchlist_not_approved"` | `"symbol_not_approved"` |
    ///   `"strategy_not_assigned"` | `"allowed"`.
    pub reason: String,
    /// Watchlist intake status.  One of: `"not_configured"` | `"missing"` | `"invalid"` |
    ///   `"loaded_not_approved"` | `"loaded_approved"`.
    pub status: String,
    /// Whether the watchlist artifact approved autonomous paper trading.
    pub approved_for_autonomous_paper: bool,
    /// Always `false` — hard live-lock invariant.
    pub approved_for_live: bool,
    /// Symbol that was checked.
    pub symbol: String,
    /// Strategy ID that was checked.
    pub strategy_id: String,
    /// Top-ranked symbol in the watchlist artifact (if loaded).
    pub top_symbol: Option<String>,
    /// Strategy assignment map: symbol → strategy_id (if artifact loaded).
    pub strategy_assignments: serde_json::Value,
    /// Always `"dry_run_only_not_enforced"`.
    /// This check does not enforce admission in the live signal path.
    /// PAPER-HANDOFF-ENFORCE-01 will wire watchlist admission into the real signal gate.
    pub note: String,
    /// UTC timestamp when the check was performed.
    pub checked_at_utc: String,
}
