//! Request and response types for all mqk-daemon HTTP endpoints.
//!
//! These types are `Serialize + Deserialize` so they can be JSON-encoded
//! by Axum and decoded by tests.  No business logic lives here.

use std::collections::BTreeMap;

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
    /// PAPER-DAILY-PNL-CAPTURE-01B: captured baseline provenance.  Present
    /// only when `action_key == "capture-account-equity-baseline"` and
    /// `accepted == true`.  Null in every other case.
    pub captured_baseline: Option<CapturedAccountEquityBaselineSnapshot>,
}

/// PAPER-DAILY-PNL-CAPTURE-01B: provenance snapshot of a durably-written
/// `sys_account_equity_baseline` row, returned by the
/// `"capture-account-equity-baseline"` `ops/action` on success.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedAccountEquityBaselineSnapshot {
    /// Trading date the baseline row was captured for, "YYYY-MM-DD".
    pub trading_date: String,
    pub equity: f64,
    pub cash: f64,
    pub currency: String,
    /// RFC3339 UTC timestamp when this capture call ran.
    pub captured_at_utc: String,
    /// Fixed provenance tag: `"operator:capture-account-equity-baseline"`.
    pub captured_by: String,
    /// Real `BrokerSnapshotTruthSource` label at the time of capture.
    pub broker_snapshot_source: String,
    /// Deterministic `Uuid::new_v5` audit event ID (see route doc comment
    /// for the exact seed format).
    pub audit_event_id: String,
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

    /// ASSET-CORE-05C: compact shadow summary comparing production session
    /// truth against the ASSET-CORE-05B per-instrument session-profile seam.
    /// Observability only — never a readiness gate or blocker. See
    /// `/api/v1/system/instrument-sessions/parity` for the full breakdown.
    pub instrument_session_shadow: InstrumentSessionShadowSummary,

    /// ASSET-CORE-05D: compact runtime session-source cutover scaffold
    /// summary. Default-off (`session_source_mode == "legacy"`).
    /// Observability only — never a readiness gate or blocker, and
    /// `production_cutover_enabled`/`runtime_uses_session_v2`/
    /// `trading_uses_session_v2` are always `false`.
    pub runtime_session_source: RuntimeSessionSourceSummaryResponse,
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

    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: the same canonical strict
    // daily-data readiness report `GET /api/v1/market-data/readiness`
    // returns, projected here so the ingest plan cannot disagree with the
    // dedicated route on symbols, timeframes, configured strategy IDs,
    // expected provider identity, or blocking reason codes. A zero-admissible-
    // instrument dry-run sync report (§C.11) remains blocked remediation
    // truth here via `daily_data_readiness.assignments[].blockers`, never
    // data-ready.
    pub daily_data_readiness: DailyDataReadinessResponse,
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

    /// ASSET-CORE-05C: compact shadow summary comparing production session
    /// truth against the ASSET-CORE-05B per-instrument session-profile seam.
    /// Observability only — never a readiness gate or blocker, and never
    /// added to `blockers`/`warnings`. See
    /// `/api/v1/system/instrument-sessions/parity` for the full breakdown.
    pub instrument_session_shadow: InstrumentSessionShadowSummary,

    /// ASSET-CORE-05D: compact runtime session-source cutover scaffold
    /// summary. Default-off (`session_source_mode == "legacy"`).
    /// Observability only — never added to `blockers`/`warnings` and never
    /// alters `deployment_start_allowed`.
    pub runtime_session_source: RuntimeSessionSourceSummaryResponse,

    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: the same canonical strict
    // daily-data readiness report `GET /api/v1/market-data/readiness`
    // returns, projected here so preflight cannot disagree with the
    // dedicated route on symbols, timeframes, configured strategy IDs,
    // effective runtime binding, expected provider identity, start_allowed,
    // or blocking reason codes. `applicability == "not_applicable"` for
    // non-Paper+ExternalSignalIngestion deployments (not a readiness gate on
    // this surface — see `daily_data_readiness.start_allowed` for that).
    pub daily_data_readiness: DailyDataReadinessResponse,

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
    // PROJECTION: additive compact daily-operation outcome summary. Fails
    // soft independently of every other field on this response.
    pub daily_operation: AutonomousDailyOperationSummary,
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

    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: the same canonical strict
    // daily-data readiness report `GET /api/v1/market-data/readiness`
    // returns, projected here so autonomous readiness cannot disagree with
    // the dedicated route. Factored into `blockers`/`overall_ready` above
    // (a blocked assignment already contributes to `blockers`); this field
    // exposes the full per-assignment identity/verdict for operator review.
    pub daily_data_readiness: DailyDataReadinessResponse,

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
    // PROJECTION: additive compact daily-operation outcome summary. Fails
    // soft independently of every other field on this response.
    pub daily_operation: AutonomousDailyOperationSummary,

    // DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: narrow additive
    // process-local dynamic-selection lifecycle truth. Not the final Bundle 7
    // status route/GUI panel — a preview projection only, factored into
    // neither `blockers` nor `overall_ready`.
    pub dynamic_selection: DynamicSelectionReadinessProjection,
}

/// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: narrow, additive,
/// process-local projection of dynamic-selection lifecycle truth.
///
/// `configured_mode`/`effective_mode`/`live_lock_applied` are a fresh
/// preview evaluation (same mode resolver `start_execution_runtime` uses),
/// current-config truth as of this request — not necessarily the mode the
/// active run (if any) was actually started under. `disposition` and every
/// field below it reflect the actual committed
/// `AppState::dynamic_selection_runtime_snapshot()` for the currently owned
/// run, and are `None`/`false`/`0` when no run is active or no commitment
/// has happened yet. Never fabricates presence — a `null` `disposition` here
/// means "no committed truth exists right now", not "Off".
#[derive(Debug, Clone, Serialize, Deserialize)]
///
/// ATOMICITY-SINGLE-SNAPSHOT-REPAIR requirement 6: every field on this
/// struct except the `preview_*` trio is *committed run-scoped truth* —
/// when `disposition` is `Some`, every other non-`preview_*` field on this
/// value was read from that exact same committed `DynamicSelectionRuntimeState`
/// snapshot, never mixed with a fresh env-preview read. When `disposition`
/// is `None` (no run has committed a disposition), every non-`preview_*`
/// field is in its neutral/absent state (`None`/`false`/`0`) — never
/// silently populated from the current env config. The `preview_*` fields
/// are the only place a fresh, current-env-config evaluation appears, and
/// are always present regardless of whether a run is active — never proof
/// of an active run's binding.
pub struct DynamicSelectionReadinessProjection {
    /// `"off"` | `"shadow"` | `"paper_enforced"` — the committed run's
    /// resolved mode. `null` when `disposition` is `null`.
    pub configured_mode: Option<String>,
    /// `"off"` | `"shadow"` | `"paper_enforced"` — the committed run's mode
    /// after the deployment-mode live lock. `null` when `disposition` is `null`.
    pub effective_mode: Option<String>,
    /// `true` when the live lock forced the committed run's `effective_mode`
    /// down to `"off"`. `false` (neutral default) when `disposition` is `null`.
    pub live_lock_applied: bool,
    /// `"off"` | `"shadow_allowed"` | `"shadow_invalid"` | `"paper_enforced_allowed"`.
    /// `null` when no run is active or nothing has been committed yet.
    /// `"paper_enforced_refused"` is never observed here — a refused start
    /// commits no `AppState` selection state.
    pub disposition: Option<String>,
    pub plan_present: bool,
    pub host_pool_present: bool,
    pub selected_pair_count: u64,
    /// The run this committed truth belongs to. `null` when `disposition` is `null`.
    pub owning_run_id: Option<Uuid>,
    /// Always `false` in this patch.
    pub approved_for_live: bool,
    /// Fresh preview evaluation of the *current* env config — `"off"` |
    /// `"shadow"` | `"paper_enforced"`. Never proof of an active run's
    /// binding; distinctly named so it can never be mistaken for the
    /// committed `configured_mode` above.
    pub preview_configured_mode: String,
    /// Fresh preview evaluation of the *current* env config, after the
    /// deployment-mode live lock.
    pub preview_effective_mode: String,
    /// `true` when the live lock would force the *current* env config's
    /// effective mode down to `"off"`.
    pub preview_live_lock_applied: bool,
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
    /// `true` when the `paper` Discord channel specifically (see the
    /// `notify` module's channel routing) is configured — the channel Paper
    /// trade/run lifecycle notifications (DISCORD-TRADE-LIFECYCLE-REAL-01)
    /// actually route to for this deployment. Checking any other channel
    /// (e.g. `c2`/`live`/`alerts` alone) would be untruthful here: a
    /// deployment with only those configured still has Paper lifecycle
    /// visibility not ready, because every Paper `notify_*` call is a
    /// silent no-op until the `paper` channel itself is configured
    /// (DISCORD-LIFECYCLE-OBSERVABILITY-COMPLETION-01, DISCORD-CHANNEL-
    /// ROUTING-01).
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
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
    // PROJECTION: additive compact daily-operation outcome summary. Always
    // present; fails soft independently of every other field on this
    // response (a daily-operation DB failure here never changes this
    // route's own truth_state/HTTP status).
    pub daily_operation: AutonomousDailyOperationSummary,
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
// PROJECTION: GET /api/v1/autonomous/daily-operation[s] and the additive
// summary block shared by readiness/paper-status/preflight.
// ---------------------------------------------------------------------------

/// One projected `sys_autonomous_daily_operations` row for the read-only
/// daily-operation API. Strictly a read model over already-durable columns
/// plus already-validated full-run-lineage activity counts — no evidence is
/// computed here, and no classifier/finalizer is ever invoked to produce it.
///
/// `outcome_class`/`outcome_reason_code`/`finalized_at_utc` are `null` for
/// every nonterminal `state` — never a fabricated default while pending.
/// `strategy_evaluation_count`/`order_activity_count`/`fill_count` are
/// `null` only when the full run lineage could not be established (never a
/// false zero); an authoritative empty lineage with no `run_id` yet bound
/// produces real zeroes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationApiRow {
    pub operation_id: String,
    pub market_date: String,
    pub deployment_mode: String,
    pub adapter_id: String,

    pub state: String,
    pub state_reason_code: Option<String>,
    /// `"finalized"` | `"awaiting_finalization"` | `"blocked_insufficient_evidence"` | `"not_yet_eligible"`.
    pub finalization_status: String,

    /// `"no_trade"` | `"with_activity"` | `"completed"`; `null` while nonterminal.
    pub outcome_class: Option<String>,
    /// The durable `outcome` column verbatim; `null` while nonterminal.
    pub outcome_reason_code: Option<String>,
    /// RFC3339; `null` while nonterminal.
    pub finalized_at_utc: Option<String>,

    pub run_id: Option<String>,
    pub bars_observed: i64,
    pub bars_dispatched: i64,
    pub last_completed_bar_ts: Option<i64>,
    pub last_dispatched_bar_ts: Option<i64>,

    /// Distinct durable `strategy_signal_evaluations` rows across the full
    /// validated run lineage. `null` only when the lineage itself could not
    /// be established or read — never a false zero.
    pub strategy_evaluation_count: Option<i64>,
    /// Every durable `oms_outbox` row across the full run lineage, plus
    /// `oms_inbox` rows with `event_kind IN ('ack','cancel_ack','replace_ack','reject')`.
    /// Fills are never double-counted here. `null` only when unavailable.
    pub order_activity_count: Option<i64>,
    /// `oms_inbox` rows across the full run lineage with
    /// `event_kind IN ('fill','partial_fill')`. `null` only when unavailable.
    pub fill_count: Option<i64>,

    /// `"complete"` | `"pending"` | `"degraded"` | `"unavailable"`.
    pub evidence_state: String,
    /// Bounded closed reason codes currently applying, if any. Never a raw
    /// error, SQL fragment, path, or credential.
    pub evidence_blockers: Vec<String>,

    pub created_at_utc: String,
    pub updated_at_utc: String,
}

/// Response for `GET /api/v1/autonomous/daily-operation[?market_date=]`.
///
/// `truth_state`:
/// - `"active"` — the operation row and every required read-model field
///   were queried successfully; `operation` is authoritative.
/// - `"not_found"` — the DB was reachable and no operation row exists yet
///   for the requested slot; a legitimate empty state, not unavailability.
/// - `"backend_unavailable"` — `AppState` has no configured DB pool.
/// - `"query_failed"` — a DB pool exists but a required read failed.
/// - `"invalid_request"` — the `market_date` query parameter was malformed;
///   only this variant accompanies HTTP 400 rather than HTTP 200.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub operation: Option<AutonomousDailyOperationApiRow>,
    pub message: Option<String>,
}

/// Response for `GET /api/v1/autonomous/daily-operations?limit=`.
///
/// `truth_state` uses the same vocabulary as [`AutonomousDailyOperationResponse`]
/// (no `"invalid_request"` here — `limit` is always clamped, never rejected).
/// An authoritative empty `rows` list is `"active"`, never `"backend_unavailable"`
/// or `"query_failed"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationsResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// The caller-supplied `limit` verbatim (defaulted to `20` when absent),
    /// before clamping — surfaced so an operator can see when their request
    /// was adjusted.
    pub requested_limit: i64,
    /// `requested_limit` clamped to `[1, 100]` — the limit actually used.
    pub effective_limit: i64,
    pub rows: Vec<AutonomousDailyOperationApiRow>,
    pub message: Option<String>,
}

/// Request body for `POST /api/v1/autonomous/daily-operation/retry`
/// (AUTONOMOUS-DAILY-OPERATOR-RETRY-01). `operation_id` is required so the
/// operator always targets an exact durable operation — this route never
/// retries "whatever operation happens to exist". `expected_market_date`
/// (`"YYYY-MM-DD"`), when supplied, is an additional safety assertion: a
/// mismatch against the target operation's own `market_date` refuses the
/// request without mutating anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationRetryRequest {
    pub operation_id: Uuid,
    pub expected_market_date: Option<String>,
}

/// Response for `POST /api/v1/autonomous/daily-operation/retry`
/// (AUTONOMOUS-DAILY-OPERATOR-RETRY-01).
///
/// `truth_state`:
/// - `"recovered"` — the operation durably left `manual_intervention_required`
///   for `preparing_data` this call.
/// - `"already_recovered"` — this call's own CAS write raced into an
///   already-applied outcome (the DB proved the exact same transition was
///   already durably applied). No mutation.
/// - `"not_manual"` — the operation is not currently `manual_intervention_required`.
///   A repeat call after a prior successful recovery lands here too (the
///   operation is by then `preparing_data`) — an operation's current state
///   alone cannot prove *why* it left the manual state, so this is the
///   honest response rather than an assumed `"already_recovered"`.
/// - `"still_blocked"` — the canonical read-only readiness/identity
///   re-evaluation this call performed still reports a blocker. No mutation.
/// - `"not_recoverable"` — the operation does not belong to the narrow
///   pristine-pre-start recoverable class (runtime activity present, an
///   unsafe/administrative blocker reason, or a stale identity). No mutation.
/// - `"not_authorized"` — refused for a non-paper `deployment_mode`.
/// - `"session_closed"` — the operation's authorized session window has
///   already closed. No mutation.
/// - `"conflict"` — the operation's durable state changed between this
///   call's read and its CAS write. No mutation.
/// - `"not_found"` — no operation exists for the supplied `operation_id`.
/// - `"backend_unavailable"` — the DB was unreachable, or a required read/
///   write failed. No claimed mutation.
///
/// `runtime_started` / `arm_modified` / `halt_changed` / `reconcile_changed`
/// are always `false` and `orders_submitted` is always `0` on every branch —
/// this route never performs any of those actions, on any path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationRetryResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub operation_id: String,
    pub previous_state: Option<String>,
    pub new_state: Option<String>,
    pub previous_reason_code: Option<String>,
    pub runtime_started: bool,
    pub arm_modified: bool,
    pub halt_changed: bool,
    pub reconcile_changed: bool,
    pub orders_submitted: i64,
    pub message: Option<String>,
}

/// Compact additive daily-operation outcome summary, embedded unchanged into
/// `AutonomousPaperReadinessResponse`, `AutonomousPaperStatusResponse`, and
/// `PreflightStatusResponse`. Fails soft independently of its parent route:
/// a daily-operation DB failure here is reported only via this block's own
/// `truth_state` and never changes the parent response's other fields, HTTP
/// status, or gate results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousDailyOperationSummary {
    /// `"active"` | `"not_found"` | `"backend_unavailable"` | `"query_failed"`.
    pub truth_state: String,
    pub operation_id: Option<String>,
    pub market_date: Option<String>,
    pub state: Option<String>,
    pub finalization_status: Option<String>,
    pub outcome_class: Option<String>,
    pub outcome_reason_code: Option<String>,
    pub finalized_at_utc: Option<String>,
    pub evidence_state: Option<String>,
    pub evidence_blockers: Vec<String>,
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
    /// PAPER-DAILY-PNL-BASELINE-01-COMBINED: `current_account_equity -
    /// daily_pnl_baseline_equity` when `daily_pnl_truth_state == "active"`.
    /// `null` in every other truth state — never fabricated or
    /// approximated from marks alone.
    pub daily_pnl: Option<f64>,
    /// Reason `daily_pnl` is unavailable, populated whenever
    /// `daily_pnl_truth_state != "active"`.
    pub daily_pnl_unavailable_reason: Option<String>,
    /// Machine-readable truth state for `daily_pnl`, mirroring the
    /// `pnl_truth_state` pattern already proven for unrealized P&L:
    ///
    /// - `"active"` — a previous-session-close baseline row exists for the
    ///   required prior trading day; `daily_pnl` is computed and populated.
    /// - `"baseline_unavailable"` — no baseline row exists for the required
    ///   prior trading day, and no older baseline row was found either
    ///   (e.g. first day this capture mechanism has ever run).
    /// - `"stale_baseline"` — a baseline row exists, but for a trading day
    ///   further in the past than the immediately preceding trading day
    ///   (e.g. a multi-day daemon outage skipped a capture); reported
    ///   rather than silently used.
    /// - `"no_snapshot"` — no broker snapshot; mirrors `truth_state`.
    /// - `"db_unavailable"` — no DB pool configured; baseline lookup was
    ///   never attempted.
    pub daily_pnl_truth_state: String,
    /// `YYYY-MM-DD` trading date of the baseline row actually used (the
    /// required prior trading day if `"active"`, or the stale row's date if
    /// `"stale_baseline"`). `None` otherwise.
    pub daily_pnl_baseline_trading_date: Option<String>,
    /// The baseline row's captured equity value, in dollars. `None` unless
    /// a baseline row was found (`"active"` or `"stale_baseline"`).
    pub daily_pnl_baseline_equity: Option<f64>,
    /// The baseline row's `captured_by` provenance string. `None` unless a
    /// baseline row was found.
    pub daily_pnl_baseline_source: Option<String>,
    /// The baseline row's `captured_at_utc`, RFC3339. `None` unless a
    /// baseline row was found.
    pub daily_pnl_baseline_captured_at_utc: Option<String>,
    /// PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01: sum of each position's
    /// `(mark_price - avg_price) * qty` (see
    /// `/api/v1/portfolio/positions`), only when every position's own P&L is
    /// computable. `null` otherwise — see `pnl_truth_state`.
    pub unrealized_pnl: Option<f64>,
    /// `"active"` — every position's P&L is computable; `unrealized_pnl` is populated.
    /// `"no_snapshot"` — no broker snapshot; mirrors `truth_state`.
    /// `"mark_unavailable"` — at least one non-flat position has no completed `md_bars` row.
    /// `"db_unavailable"` — no DB pool configured.
    pub pnl_truth_state: String,
    /// Human-readable reason, present whenever `pnl_truth_state` is not `"active"`.
    pub pnl_unavailable_reason: Option<String>,
    pub buying_power: Option<f64>,
}

// ---------------------------------------------------------------------------
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01E: read-only durable portfolio truth.
//
// Distinct from PortfolioSummaryResponse/PortfolioPositionsResponse above,
// which are broker-snapshot-derived and in-memory-only (reset on every
// daemon restart). These surfaces read the durable tables B4-B/B4-C/B4-D
// added and survive a restart. `null` always means unavailable/unproven --
// never zero; a true zero is always the literal numeric `0`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDurableSummaryResponse {
    /// Overall gate for this response: `"active"` when a durable snapshot
    /// was found, otherwise mirrors `snapshot_truth_state`.
    pub truth_state: String,

    pub snapshot_truth_state: String,
    pub snapshot_id: Option<String>,
    pub captured_at_utc: Option<String>,
    pub source: Option<String>,
    pub deployment_mode: Option<String>,
    pub account_equity: Option<f64>,
    pub cash: Option<f64>,
    pub currency: Option<String>,
    pub run_id: Option<String>,
    pub operation_id: Option<String>,

    /// `"active"` | `"fill_history_incomplete"` | `"accounting_epoch_unavailable"`
    /// | `"not_found"` | `"db_unavailable"`.
    pub accounting_truth_state: String,
    /// `"complete"` | `"incomplete"`, present only when
    /// `accounting_truth_state != "not_found"`.
    pub accounting_epoch: Option<String>,
    pub accounting_epoch_reason: Option<String>,
    pub last_applied_inbox_id: Option<i64>,
    /// The durable broker snapshot whose positions were replayed to derive
    /// `accounting_epoch`. `None` when the accounting row predates this
    /// provenance column, or when no accounting row exists yet — a `None`
    /// here means completeness cannot be traced to a specific snapshot.
    pub accounting_source_snapshot_id: Option<String>,

    pub realized_pnl: Option<f64>,
    pub realized_pnl_truth_state: String,
    pub realized_pnl_unavailable_reason: Option<String>,
    pub fees: Option<f64>,
    /// Cumulative cash movement produced by this run's durably-replayed
    /// fills (not the absolute account cash balance -- that is `cash`
    /// above, from the durable snapshot).
    pub cumulative_cash_movement: Option<f64>,

    pub unrealized_pnl: Option<f64>,
    pub unrealized_pnl_truth_state: String,
    pub unrealized_pnl_unavailable_reason: Option<String>,

    pub daily_pnl: Option<f64>,
    pub daily_pnl_truth_state: String,
    pub daily_pnl_unavailable_reason: Option<String>,

    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDurablePositionRow {
    pub symbol: String,
    pub qty_signed: i64,
    pub avg_entry_price: f64,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDurablePositionsResponse {
    /// `"active"` | `"snapshot_unavailable"` | `"snapshot_stale"` |
    /// `"db_unavailable"` | `"query_failed"`.
    pub truth_state: String,
    pub snapshot_id: Option<String>,
    pub captured_at_utc: Option<String>,
    pub run_id: Option<String>,
    pub positions: Vec<PortfolioDurablePositionRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDurableSnapshotRow {
    pub snapshot_id: String,
    pub captured_at_utc: String,
    pub deployment_mode: String,
    pub source: String,
    pub equity: f64,
    pub cash: f64,
    pub currency: String,
    pub truth_state: String,
    pub run_id: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDurableSnapshotsResponse {
    /// `"active"` (zero or more rows found) | `"db_unavailable"` | `"query_failed"`.
    pub truth_state: String,
    pub snapshots: Vec<PortfolioDurableSnapshotRow>,
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
    /// `"active"` (durable risk-block state was queried, confirmed present
    /// or confirmed absent) | `"no_db"` (no DB configured) | `"query_failed"`
    /// (DB configured but the risk-block-state read errored). When not
    /// `"active"`, `kill_switch_active` is fail-closed `true` rather than a
    /// possibly-false confirmed-clear reading (OPERATOR-RISK-UNKNOWN-TRUTH-01).
    pub truth_state: String,
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

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/live-weights (PORTFOLIO-LIVE-WEIGHTS-01)
// ---------------------------------------------------------------------------

/// Per-symbol row for `GET /api/v1/portfolio/live-weights`.
///
/// Mirrors `mqk_portfolio::PositionWeightRow`. Money/notional fields are
/// clamped from `i128` to `i64` for JSON transport — portfolio values at
/// i64-micros scale (+/- ~9.2 trillion dollars) cannot realistically reach
/// that clamp boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioLiveWeightRow {
    pub symbol: String,
    pub signed_qty: i64,
    pub mark_price_micros: Option<i64>,
    /// Epoch seconds of the bar/source the mark was taken from.
    pub mark_ts_utc: Option<i64>,
    /// Provenance string, e.g. `"md_bars:1D:close"`. `None` iff `missing_mark`.
    pub mark_source: Option<String>,
    pub market_value_micros: Option<i64>,
    pub absolute_notional_micros: Option<i64>,
    /// Signed weight in basis points of NAV. `None` unless `truth_state ==
    /// "active"`.
    pub weight_bps: Option<i64>,
    pub missing_mark: bool,
}

/// Response for `GET /api/v1/portfolio/live-weights`.
///
/// Truthful, read-only live position valuation seam
/// (`PORTFOLIO-LIVE-WEIGHTS-01`). Marks are sourced exclusively from the
/// latest *completed* `md_bars` row at `timeframe` for each non-flat
/// position — never from the broker, a live quote, an entry price, or a
/// last order price. Does not enforce any risk limit; this is a valuation
/// seam only.
///
/// `truth_state`:
/// - `"no_snapshot"` — no execution snapshot exists yet this session (the
///   runtime has not produced a portfolio snapshot). All financial fields
///   are `null` and `positions` is empty.
/// - `"db_unavailable"` — a snapshot exists with at least one non-flat
///   position, but no DB pool is configured, so marks cannot be looked up
///   at all.
/// - `"missing_marks"` — DB is available but at least one non-flat position
///   has no completed bar at `timeframe`. NAV/weights are not computed for
///   *any* position; see `missing_mark_symbols`.
/// - `"nav_unavailable"` — every non-flat position has a confirmed mark, but
///   NAV (cash + sum of market values) is `<= 0`; weights are not computed.
/// - `"active"` — NAV and weights are fully computed. This also covers a
///   flat / no-position portfolio, where NAV == cash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioLiveWeightsResponse {
    pub truth_state: String,
    /// Timeframe used for the md_bars mark lookup (echoed; defaults to `"1D"`).
    pub timeframe: String,
    pub cash_micros: i64,
    pub nav_micros: Option<i64>,
    pub gross_exposure_micros: Option<i64>,
    pub positions: Vec<PortfolioLiveWeightRow>,
    pub missing_mark_symbols: Vec<String>,
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
    /// Active session-profile model for this response.
    ///
    /// Current execution remains equity-only. Non-equity profile names are
    /// surfaced only in `supported_session_profiles` as read-only scaffolds.
    pub session_profile: String,
    /// Authority for the active session-profile answer.
    ///
    /// `"configured_override"` covers existing always-on daemon modes; `"fallback"`
    /// covers the current NYSE weekday heuristic. No live/paper trading decision
    /// is changed by this diagnostic field.
    pub session_authority: String,
    /// Open/closed answer for the active profile when this route can derive one.
    pub session_profile_is_open: Option<bool>,
    /// Machine-readable reason for the active profile/authority status.
    pub session_profile_reason_code: String,
    /// Operator-facing explanation of the active profile/authority status.
    pub session_profile_message: String,
    /// Session profiles the current model can name.
    ///
    /// Only `equity_us_regular` is wired into current behavior. Crypto/futures/FX
    /// entries are model-only scaffolds and do not enable trading.
    pub supported_session_profiles: Vec<String>,
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
// /api/v1/system/asset-risk-policy — ASSET-CORE-03B
// ---------------------------------------------------------------------------

/// Read-only per-asset-class policy record, mirrored from
/// `mqk_execution::asset_risk_policy::AssetRiskPolicy`. This surface exists
/// purely to give operators visibility into the existing static policy
/// model — it does not enforce anything itself, is not consulted by any
/// order/risk/routing path, and does not require DB, broker, or provider
/// access to compute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRiskPolicyEntry {
    /// Asset class identifier (e.g. "equity", "etf_as_equity", "crypto").
    pub asset_class: String,
    /// One of "enabled", "disabled", "research_only", "unsupported".
    pub state: String,
    pub paper_trading_enabled: bool,
    pub live_trading_enabled: bool,
    pub requires_margin_model: bool,
    pub requires_contract_multiplier: bool,
    pub requires_session_profile: bool,
    pub requires_currency_conversion: bool,
    pub reason_code: String,
    pub message: String,
}

/// Read-only asset-risk-policy status surface (`ASSET-CORE-03B`).
///
/// This reports the existing `mqk_execution::asset_risk_policy` model
/// as-is. It does not change, weaken, or bypass Gate 0 (signal admission,
/// `routes/strategy.rs`) or the broker-submit routing guard
/// (`mqk_execution::gateway::BrokerGateway::submit_with_context`), which
/// remain the only active enforcement boundaries. `production_enforcement_enabled`
/// and `non_equity_routing_enabled` mirror the static constants
/// `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED` /
/// `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED` from that model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRiskPolicyStatusResponse {
    pub schema_version: String,
    /// Mirrors `mqk_execution::asset_risk_policy::ASSET_RISK_POLICY_SOURCE`.
    pub policy_source: String,
    /// Mirrors `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED`. Always `false`
    /// until a separate, scope-reviewed production-wiring patch changes it.
    pub production_enforcement_enabled: bool,
    /// Mirrors `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED`. Always `false` until
    /// a separate, scope-reviewed production-wiring patch changes it.
    pub non_equity_routing_enabled: bool,
    /// Per-class policy records, one per `mqk_execution::asset_risk_policy::default_asset_risk_policies()` entry.
    pub entries: Vec<AssetRiskPolicyEntry>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/instrument-registry-v2/status — ASSET-CORE-01C
// ---------------------------------------------------------------------------

/// Read-only instrument-registry-v2 status surface (ASSET-CORE-01C).
///
/// Answers exactly: can the currently configured v1 registry
/// (`AppState::instrument_registry_path`) be loaded, converted to
/// `mqk_md::instrument_registry_v2::InstrumentRegistryV2`, and validated —
/// and what does the converted shape look like? It does **not** answer "can
/// we trade crypto/futures/options/forex now" — the answer to that remains
/// no regardless of this route's output. `production_cutover_enabled` and
/// `trading_uses_v2` are always `false`: no daemon/runtime/ingest/backtest/
/// GUI/risk/broker path reads `InstrumentRegistryV2` for any decision. The
/// only file any production path reads is `config/instruments/equities.json`
/// (v1) at `registry_path`.
///
/// `truth_state`:
/// - `"active"` — v1 loaded, converted to v2, and v2 validated cleanly.
/// - `"v1_load_failed"` — registry file exists but failed to parse/load as v1.
/// - `"v2_validation_failed"` — v1 loaded and converted, but the converted v2
///   registry failed `validate_registry_v2`. `validation_errors` carries the
///   first violation message (the validator fails closed on the first error
///   found; it is not an exhaustive list).
/// - `"unavailable"` — no file exists at `registry_path`.
///
/// `"v2_conversion_failed"` is reserved for forward compatibility but is
/// unreachable today: `convert_v1_registry_to_v2` is a pure, infallible
/// function (no `Result`, no panics on well-typed `TrackedInstrument` input),
/// so conversion never fails once v1 has loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentRegistryV2StatusResponse {
    pub truth_state: String,
    /// Path to the v1 registry file that was read (`AppState::instrument_registry_path`).
    pub registry_path: String,
    /// `InstrumentRegistryV2::schema_version` once v1 has loaded and converted; `None` otherwise.
    pub schema_version: Option<u32>,
    pub v1_count: usize,
    pub v2_count: usize,
    pub validation_passed: bool,
    /// First validation violation, if any. Empty when `validation_passed` is `true`.
    pub validation_errors: Vec<String>,
    /// Instrument count by `asset_class` (e.g. `"equity"`). Counts all converted
    /// entries, not just enabled ones.
    pub asset_class_counts: BTreeMap<String, usize>,
    /// Instrument count by `instrument_kind`. Only instruments carrying a
    /// non-`None` `instrument_kind` (e.g. `"etf"`) are represented here —
    /// untagged plain equities do not appear, mirroring
    /// `instrument_registry::sector_map`'s omission convention.
    pub instrument_kind_counts: BTreeMap<String, usize>,
    /// Instrument count by contract shape: `"equity"`, `"etf"`, `"future"`,
    /// `"option"`, `"crypto_pair"`, `"forex_pair"`, or `"none"` when `contract`
    /// is absent.
    pub contract_kind_counts: BTreeMap<String, usize>,
    pub enabled_count: usize,
    /// Count of instruments tagged `instrument_kind = "etf"`.
    pub etf_count: usize,
    pub non_equity_count: usize,
    pub enabled_non_equity_count: usize,
    pub paper_trading_enabled_count: usize,
    pub live_trading_enabled_count: usize,
    /// Always `false`. No production v2 registry cutover path exists.
    pub production_cutover_enabled: bool,
    /// Always `false`. No daemon/runtime/ingest/backtest/GUI/risk/broker path
    /// reads `InstrumentRegistryV2` for any decision.
    pub trading_uses_v2: bool,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/instrument-sessions/status — ASSET-CORE-05B
// ---------------------------------------------------------------------------

/// Per-profile summary for the read-only instrument session diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionProfileSummary {
    pub profile_id: String,
    pub asset_class: String,
    pub instrument_kind: String,
    pub production_backed: bool,
    pub model_only: bool,
    pub instrument_count: usize,
}

/// Per-instrument session-profile diagnostic row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionStatusRow {
    pub symbol: String,
    pub instrument_id: String,
    pub asset_class: String,
    pub instrument_kind: Option<String>,
    pub session_profile_id: String,
    pub profile_truth_state: String,
    pub session_state: String,
    pub reason_code: String,
    pub production_backed: bool,
    pub model_only: bool,
    pub trading_uses_this: bool,
}

/// Read-only per-instrument session-profile status.
///
/// This is diagnostic only. It makes the v1->v2 registry conversion truth and
/// ASSET-CORE-05A session profile seam visible per instrument, but never feeds
/// trading, routing, risk, OMS, portfolio, broker, or runtime enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionStatusResponse {
    pub truth_state: String,
    pub as_of_utc: String,
    pub registry_path: String,
    pub registry_v2_valid: bool,
    pub v1_count: usize,
    pub v2_count: usize,
    pub instrument_count: usize,
    /// Always `false`. No production v2 session/profile cutover exists.
    pub production_cutover_enabled: bool,
    /// Always `false`. No trading path consumes this profile assignment.
    pub trading_uses_session_v2: bool,
    /// Always `false`. Runtime session gates are not switched to this surface.
    pub runtime_uses_session_v2: bool,
    /// `true` only if a converted v2 non-equity row is enabled.
    pub non_equity_enabled: bool,
    pub profiles: Vec<InstrumentSessionProfileSummary>,
    pub instruments: Vec<InstrumentSessionStatusRow>,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/instrument-sessions/parity — ASSET-CORE-05C
// ---------------------------------------------------------------------------
//
// Read-only shadow comparison: production session truth (the equity-only
// `MarketSessionState`/`MarketCalendarProvider` path that actually gates
// trading today) versus the ASSET-CORE-05A/05B per-instrument session-profile
// seam. This is observability only — see module docs in
// `state/market_calendar.rs` and the route handler in `routes/system.rs` for
// the full honesty contract. Nothing here is consumed by any trading,
// runtime, or routing decision.

/// Per-instrument parity row — ASSET-CORE-05C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionParityRow {
    pub symbol: String,
    pub asset_class: String,
    pub instrument_kind: Option<String>,
    pub session_profile_id: String,
    /// Production session-state string (`MarketSessionState::as_str()`), or
    /// `"not_applicable"` when no production calendar exists for this asset
    /// class (i.e. anything other than equity/ETF).
    pub production_session_state: String,
    /// ASSET-CORE-05B per-instrument profile session-state string.
    pub profile_session_state: String,
    /// `"matched"` | `"mismatched"` | `"unknown"` | `"model_only"` |
    /// `"unsupported_model_only"` | `"registry_missing"` | `"registry_invalid"`
    /// | `"conversion_failed"` | `"timestamp_invalid"`.
    pub parity_state: String,
    pub production_backed: bool,
    pub model_only: bool,
    /// Always `false`. No trading/runtime path consumes this row.
    pub trading_uses_this: bool,
    pub reason_code: String,
}

/// Read-only shadow parity status — ASSET-CORE-05C.
///
/// Compares current production session truth against the ASSET-CORE-05B
/// per-instrument session-profile seam for every (filtered) registry
/// instrument at a supplied timestamp. Diagnostic only: never feeds trading,
/// routing, risk, OMS, portfolio, broker, or runtime enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionParityResponse {
    pub truth_state: String,
    pub as_of_utc: String,
    pub registry_v2_valid: bool,
    /// Always `false`. No production v2 session/profile cutover exists.
    pub production_cutover_enabled: bool,
    /// Always `false`. Runtime session gates are not switched to this surface.
    pub runtime_uses_session_v2: bool,
    /// Always `false`. No trading path consumes this profile assignment.
    pub trading_uses_session_v2: bool,
    /// Always `true`. This route is shadow/observability only.
    pub shadow_only: bool,
    /// `true` only when every checked equity row's parity_state is `"matched"`.
    pub all_equity_profiles_match_production: bool,
    pub checked_count: usize,
    pub matched_count: usize,
    pub mismatched_count: usize,
    pub unknown_count: usize,
    pub model_only_count: usize,
    pub rows: Vec<InstrumentSessionParityRow>,
    pub errors: Vec<String>,
}

/// Compact shadow-parity summary embedded on `/api/v1/system/status` and
/// `/api/v1/system/preflight` — ASSET-CORE-05C.
///
/// Intentionally does NOT carry per-instrument rows; see the dedicated
/// `route` field for the full breakdown. Never used as a readiness gate or
/// blocker on either surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSessionShadowSummary {
    /// `"active"` when computed successfully, `"unavailable"` when the
    /// registry could not be loaded/converted/validated (registry missing,
    /// invalid, or conversion failed). Never blocks the parent surface.
    pub truth_state: String,
    /// Always `true`. This summary is shadow/observability only.
    pub shadow_only: bool,
    pub production_cutover_enabled: bool,
    pub runtime_uses_session_v2: bool,
    pub trading_uses_session_v2: bool,
    pub checked_count: usize,
    pub matched_count: usize,
    pub mismatched_count: usize,
    pub unknown_count: usize,
    pub model_only_count: usize,
    pub all_equity_profiles_match_production: bool,
    pub route: String,
}

// ---------------------------------------------------------------------------
// ASSET-CORE-05D: runtime session-source cutover scaffold (default-off)
// ASSET-CORE-05E: active runtime cutover hook (default-off)
// ---------------------------------------------------------------------------
//
// Compact, additive operator-visibility surface embedded on
// `/api/v1/system/status` and `/api/v1/system/preflight`, mirroring the
// existing `instrument_session_shadow` (ASSET-CORE-05C) field pattern.
//
// Honesty contract:
// - `production_cutover_enabled`, `runtime_uses_session_v2`, and
//   `trading_uses_session_v2` are always `false` in `"legacy"` and
//   `"v2_equity_shadow"` modes. No trading, risk, OMS, or broker path reads
//   this seam for any decision in any mode.
// - Default (no `MQK_RUNTIME_SESSION_SOURCE` set): `session_source_mode ==
//   "legacy"`, `candidate_v2_session_state`/`candidate_v2_parity_state` are
//   both `null`, and no v2 registry is loaded at all.
// - `"v2_equity_shadow"` mode only ever reports a *candidate* evaluation; it
//   never silently activates non-equity rows or an unproven/mismatched
//   registry, and it never drives `session_controller.rs` — see
//   `fallback_reason` and `activation_refusal_reason`.
// - `"v2_equity_active"` mode (ASSET-CORE-05E) may drive
//   `session_controller.rs::AutonomousSessionSchedule::is_in_session`, but
//   only when the same candidate evaluation proves
//   `candidate_would_activate: true` — see `active_source_used`. Refusal
//   fails closed; it is surfaced via `fallback_reason`, never silent.

/// Compact runtime session-source summary — ASSET-CORE-05D / ASSET-CORE-05E.
///
/// See `mqk_daemon::state::runtime_session_source` for the full evaluation
/// seam this summary is built from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSessionSourceSummaryResponse {
    /// `"legacy"` | `"v2_equity_shadow"` | `"v2_equity_active"`. Always
    /// `"legacy"` by default.
    pub session_source_mode: String,
    /// `true` only when `session_source_mode == "v2_equity_active"` is
    /// explicitly configured (regardless of whether it was accepted) —
    /// see `active_source_used` for "currently in effect". Always `false`
    /// for `"legacy"`/`"v2_equity_shadow"`.
    pub production_cutover_enabled: bool,
    /// `true` only when `active_source_used` is `true`. `session_controller.rs`
    /// does not read this seam unless `"v2_equity_active"` is explicitly
    /// configured AND the candidate evaluation proves safe.
    pub runtime_uses_session_v2: bool,
    /// Mirrors `runtime_uses_session_v2`: the only session gate on the
    /// autonomous paper-trading path is `AutonomousSessionSchedule::is_in_session`,
    /// so "runtime" and "trading" readiness share the same v2-usage truth. No
    /// separate trading/risk/OMS path reads this seam.
    pub trading_uses_session_v2: bool,
    /// Real legacy production session state at evaluation time (the actual
    /// state the daemon uses today), e.g. `"regular_open"`.
    pub legacy_session_state: String,
    /// Candidate v2 session state. `null` unless `session_source_mode` is
    /// `"v2_equity_shadow"` or `"v2_equity_active"`.
    pub candidate_v2_session_state: Option<String>,
    /// `"matched"` | `"mismatched"` | `"no_instruments_checked"` | `null`.
    /// `null` unless `session_source_mode` is `"v2_equity_shadow"` or
    /// `"v2_equity_active"`.
    pub candidate_v2_parity_state: Option<String>,
    /// `true` when the candidate evaluation proved safe to activate, `false`
    /// when it was evaluated but refused, `null` when no evaluation occurred
    /// at all (`"legacy"` mode, or the registry could not be loaded at all —
    /// see `fallback_reason`).
    pub candidate_would_activate: Option<bool>,
    /// `true` only when the v2 equity source is actually driving the
    /// in-window decision for this evaluation. The headline ASSET-CORE-05E
    /// operator-visible signal; equivalent to `runtime_uses_session_v2`.
    pub active_source_used: bool,
    /// Operator-facing reason the v2 candidate/active evaluation could not
    /// activate (registry missing/invalid, non-equity row enabled, or parity
    /// mismatch). `null` in `"legacy"` mode or when the evaluation activates
    /// cleanly.
    pub fallback_reason: Option<String>,
    /// Operator-facing reason an explicitly-configured but unrecognized
    /// `MQK_RUNTIME_SESSION_SOURCE` value was refused (mode fell back to
    /// `"legacy"`). `null` unless the env var was set to an unrecognized
    /// value.
    pub activation_refusal_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/instrument-registry-v2-source/status — ASSET-CORE-01D
// ---------------------------------------------------------------------------

/// Per-instrument enablement counts nested under
/// [`InstrumentRegistryV2SourceStatusResponse`]. Observational only — these
/// counts are never read to gate, block, or route any order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentRegistryV2EnabledCounts {
    pub enabled: usize,
    pub paper_trading_enabled: usize,
    pub live_trading_enabled: usize,
}

/// Read-only operator-visibility status for the separate v2 registry
/// **source** configured via `MQK_INSTRUMENT_REGISTRY_V2_PATH`
/// (ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED).
///
/// This is a distinct surface from
/// [`InstrumentRegistryV2StatusResponse`] (`/api/v1/system/instrument-registry-v2/status`,
/// ASSET-CORE-01C), which converts the *v1* registry (`equities.json`) to v2
/// shape for diagnostics. This response instead answers: is a *separate* v2
/// source configured, and if so, what does it contain? The only production
/// reader of `AppState::instrument_registry_v2_path` is the read-only
/// `GET /api/v1/backtests/economics-suggestion` route
/// (INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED) — `used_for_trading`,
/// `enabled_for_live_trading`, and `enabled_for_paper_trading` are always
/// `false`, independent of any per-instrument flag in the configured file.
///
/// `truth_state`:
/// - `"not_configured"` — `MQK_INSTRUMENT_REGISTRY_V2_PATH` is unset.
/// - `"configured_valid"` — configured, loads, and validates cleanly.
/// - `"registry_unavailable"` — configured but the file is missing/unreadable.
/// - `"validation_failed"` — configured, loads, but fails `validate_registry_v2`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentRegistryV2SourceStatusResponse {
    pub truth_state: String,
    /// `true` iff `MQK_INSTRUMENT_REGISTRY_V2_PATH` is set, regardless of
    /// whether the configured path is healthy.
    pub configured: bool,
    /// The configured path. `None` only when `configured` is `false`.
    pub path: Option<String>,
    /// Env var this source is read from. Always `"MQK_INSTRUMENT_REGISTRY_V2_PATH"`.
    pub source: String,
    /// `InstrumentRegistryV2::schema_version` once loaded; `None` otherwise.
    pub schema_version: Option<u32>,
    /// Always `"backtest_economics_suggestions_only"`.
    pub purpose: String,
    /// Always `false`. No daemon/runtime/ingest/risk/OMS/broker path reads
    /// this source for any decision.
    pub used_for_trading: bool,
    /// Always `false`, independent of `enabled_counts.live_trading_enabled`.
    pub enabled_for_live_trading: bool,
    /// Always `false`, independent of `enabled_counts.paper_trading_enabled`.
    pub enabled_for_paper_trading: bool,
    pub total_instruments: usize,
    /// Instrument count by `asset_class`. Empty unless `truth_state` is
    /// `"configured_valid"`.
    pub asset_class_counts: BTreeMap<String, usize>,
    pub enabled_counts: InstrumentRegistryV2EnabledCounts,
    pub non_equity_present: bool,
    /// `true` when every non-equity instrument is disabled. Vacuously `true`
    /// when `non_equity_present` is `false` (including on failure paths,
    /// where there is nothing enabled to report).
    pub non_equity_all_disabled: bool,
    pub has_economics_metadata: bool,
    /// First N symbols in registry order. Empty unless `truth_state` is
    /// `"configured_valid"`.
    pub sample_symbols: Vec<String>,
    /// Populated only when `truth_state` is `"registry_unavailable"` or
    /// `"validation_failed"`.
    pub validation_errors: Vec<String>,
    /// One-line human-readable summary for operator surfaces.
    pub message: String,
}

// ---------------------------------------------------------------------------
// /api/v1/system/instrument-economics/status — ASSET-CORE-04B
// ---------------------------------------------------------------------------
//
// Read-only operator-visibility surface for the ASSET-CORE-04B registry-v2
// -> `mqk_portfolio::InstrumentEconomics` bridge
// (`mqk_daemon::state::instrument_economics_bridge`). Loads the same
// configured v1 registry as ASSET-CORE-01C/05B, converts it to v2 in memory,
// validates it, and bridges every instrument. Diagnostic only: no DB,
// provider, or broker call; no writes; no trading/runtime/risk/order-path
// consumer of this bridge exists anywhere in the workspace.

/// Per-instrument economics-bridge diagnostic row — ASSET-CORE-04B.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentEconomicsStatusRow {
    pub symbol: String,
    pub instrument_id: String,
    pub asset_class: String,
    pub instrument_kind: Option<String>,
    pub enabled: bool,
    pub paper_trading_enabled: bool,
    pub live_trading_enabled: bool,
    /// `"active"` | `"missing_currency"` | `"currency_conversion_unsupported"`
    /// | `"missing_contract"` | `"missing_multiplier"` | `"invalid_multiplier"`
    /// | `"unsupported_instrument"`.
    pub truth_state: String,
    pub reason_code: String,
    /// `true` for every non-`"equity"` row. See
    /// `mqk_daemon::state::instrument_economics_bridge` module docs.
    pub model_only: bool,
    /// Always `false`. This bridge never enables, implies, or decides
    /// trading permission for any instrument.
    pub trading_enabled_by_bridge: bool,
    /// `Some` only when `truth_state == "active"`.
    pub quote_currency: Option<String>,
    pub contract_multiplier_micros: Option<i64>,
    pub quantity_scale: Option<i64>,
    pub tick_size_micros: Option<i64>,
}

/// Read-only registry-v2 -> instrument-economics bridge status — ASSET-CORE-04B.
///
/// Honesty contract: `bridge_model_only`, and every `*_uses_instrument_economics`
/// flag, are always as documented on the field regardless of `truth_state` or
/// per-row outcomes — this route can never observe a state where any of them
/// would be `true`, because no such caller exists in the workspace.
///
/// `truth_state`:
/// - `"active"` — v1 loaded, converted to v2, validated, and bridged.
/// - `"unavailable"` — no file exists at `registry_path`.
/// - `"v1_load_failed"` — registry file exists but failed to parse/load as v1.
/// - `"v2_validation_failed"` — converted v2 registry failed `validate_registry_v2`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentEconomicsStatusResponse {
    pub truth_state: String,
    pub registry_path: String,
    pub registry_v2_valid: bool,
    /// Always `true`. The bridge as a whole is model/diagnostic-only.
    pub bridge_model_only: bool,
    /// Always `false`. No trading path reads this bridge.
    pub trading_uses_instrument_economics: bool,
    /// Always `false`. No runtime path reads this bridge.
    pub runtime_uses_instrument_economics: bool,
    /// Always `false`. No risk path reads this bridge.
    pub risk_uses_instrument_economics: bool,
    /// Always `false`. No order path reads this bridge.
    pub order_path_uses_instrument_economics: bool,
    /// Total instruments in the converted v2 registry. Unaffected by the
    /// `symbol`/`limit` query params -- always the full-registry total, even
    /// when `rows` below is filtered/truncated.
    pub instrument_count: usize,
    pub bridged_count: usize,
    pub failed_count: usize,
    pub model_only_count: usize,
    /// Count of rows that are both `enabled` and non-`"equity"`. Always `0`
    /// for any registry that passed `validate_registry_v2` without the
    /// test-only `allow_enabled_non_equity_for_testing` escape hatch.
    pub non_equity_enabled_count: usize,
    /// Per-instrument rows, filtered by `symbol` and truncated by `limit`
    /// when supplied. May be a strict subset of `instrument_count`.
    pub rows: Vec<InstrumentEconomicsStatusRow>,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/economics/status — ASSET-CORE-04D / ASSET-CORE-04F
// ---------------------------------------------------------------------------
//
// Read-only diagnostic route composing the ASSET-CORE-04B registry-v2 ->
// instrument-economics bridge with the ASSET-CORE-04A/04C economics model
// against the *live* in-memory execution snapshot's positions/cash and the
// latest completed `md_bars` mark per non-flat symbol -- the same
// position/cash and mark sources `/api/v1/portfolio/live-weights`
// (`PORTFOLIO-LIVE-WEIGHTS-01`) already uses. This is an observability seam,
// not a cutover: nothing here feeds any trading, runtime, risk, order, or
// broker path, and `account_currency` is a hardcoded `"USD"` constant
// (`PortfolioSnapshot` carries no account-currency field today).
//
// ASSET-CORE-04F added an explicit, default-off `?registry_source=v2` lane:
// the route's registry input defaults to exactly the pre-existing v1-load ->
// `convert_v1_registry_to_v2` behavior (`"legacy"`), but a caller may instead
// request a server-side-configured registry-v2 file
// (`AppState::portfolio_economics_registry_v2_path`, sourced only from
// `MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH`) so a real (non-equity-only) v2
// document can be valued through this route. There is no client-supplied
// filesystem path and no fallback from a requested-but-unavailable v2 source
// back to legacy.

/// Per-position diagnostic row — ASSET-CORE-04D.
///
/// `asset_class`/`quote_currency` are empty strings when the position's
/// symbol could not be resolved to a registry-v2 instrument at all — the
/// route never fabricates equity/USD defaults for an unresolved symbol;
/// `truth_state`/`reason_code` explain why in that case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioEconomicsStatusPositionRow {
    pub symbol: String,
    pub instrument_id: String,
    pub asset_class: String,
    pub quote_currency: String,
    /// Signed whole-unit quantity as held in the execution snapshot
    /// (positive = long, negative = short, 0 = flat). Not micros-scaled.
    pub signed_qty: i64,
    pub mark_price_micros: Option<i64>,
    pub mark_ts_utc: Option<i64>,
    /// Provenance string, e.g. `"md_bars:1D:close"`. `None` when no mark was
    /// looked up (flat position) or none was found.
    pub mark_source: Option<String>,
    pub notional_micros: Option<i64>,
    pub absolute_notional_micros: Option<i64>,
    /// Absolute (always non-negative) weight in basis points of NAV. `Some`
    /// only when the overall snapshot truth_state is `"active"`.
    pub weight_bps: Option<i64>,
    /// `true` when the resolved registry instrument's asset class is not
    /// `"equity"`. `false` (never fabricated `true`) when the symbol could
    /// not be resolved at all — absence of evidence is not evidence of
    /// non-equity status.
    pub model_only: bool,
    /// `"active"` | `"position_value_unavailable"` | `"currency_conversion_unsupported"`.
    pub truth_state: String,
    pub reason_code: String,
}

/// One row of [`PortfolioEconomicsStatusResponse::asset_class_exposures`] or
/// `::currency_exposures`. Direct passthrough of the ASSET-CORE-04C exposure
/// breakdown, `i128` clamped to `i64` for transport (matches
/// `PortfolioLiveWeightsResponse`'s existing convention).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioEconomicsStatusExposureRow {
    pub key: String,
    pub signed_notional_micros: i64,
    pub absolute_notional_micros: i64,
    pub weight_bps: Option<i64>,
}

/// Response for `GET /api/v1/portfolio/economics/status` — ASSET-CORE-04D.
///
/// Composes the ASSET-CORE-04B registry-v2 bridge with the ASSET-CORE-04A/04C
/// economics model against the live execution snapshot. Read-only,
/// diagnostic-only: the honesty flags below are always as documented
/// regardless of `truth_state` — no caller anywhere in the workspace reads
/// this route's output for any trading, runtime, risk, or order decision.
///
/// `truth_state`:
/// - `"no_snapshot"` — no in-memory execution snapshot exists yet.
/// - `"registry_unavailable"` — no file exists at `registry_path`.
/// - `"registry_invalid"` — the registry file exists but failed to load as
///   v1, or failed v2 conversion/validation.
/// - `"no_positions"` — snapshot present, zero positions, NAV == cash.
/// - `"active"` — every position is either valued or flat; NAV/gross/net/
///   weights are populated.
/// - `"position_value_unavailable"` — at least one position could not be
///   valued for a reason other than a missing mark or currency mismatch
///   (e.g. its symbol is absent from the registry, or its registry row
///   failed to bridge to economics).
/// - `"missing_marks"` — DB is present but every unvalued position is
///   missing a completed `md_bars` row at `timeframe`.
/// - `"db_unavailable"` — no DB pool is configured at all, so no mark could
///   be looked up for any non-flat position (distinct from `"missing_marks"`,
///   mirroring `PortfolioLiveWeightsResponse`'s existing precedent).
/// - `"currency_conversion_unsupported"` — a resolved position's quote
///   currency differs from `account_currency`.
/// - `"nav_unavailable"` — every position is valued/flat but `nav_micros <= 0`.
/// - `"aggregation_unavailable"` — summing NAV/exposure overflowed `i128`.
/// - `"invalid_registry_source"` — ASSET-CORE-04F: the `registry_source`
///   query param was supplied but is neither `"legacy"` nor `"v2"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioEconomicsStatusResponse {
    pub truth_state: String,
    pub reason_code: String,
    pub message: String,

    /// Always `true`. This route is model/diagnostic-only.
    pub model_only: bool,
    /// Always `false`. No trading path reads this route.
    pub trading_uses_portfolio_economics: bool,
    /// Always `false`. No runtime path reads this route.
    pub runtime_uses_portfolio_economics: bool,
    /// Always `false`. No risk path reads this route.
    pub risk_uses_portfolio_economics: bool,
    /// Always `false`. No order path reads this route.
    pub order_path_uses_portfolio_economics: bool,

    /// Timeframe used for the md_bars mark lookup (echoed; defaults to `"1D"`).
    pub timeframe: String,
    /// Normalized (uppercased) `symbol` query param, if supplied.
    pub symbol_filter: Option<String>,
    pub registry_path: String,
    pub registry_v2_valid: bool,

    /// ASSET-CORE-04F: normalized `registry_source` query param this response
    /// answers for — `"legacy"` (default/omitted/explicit) or `"v2"`, or the
    /// verbatim lowercased value the caller sent when it is neither (paired
    /// with `truth_state == "invalid_registry_source"`).
    pub registry_source_requested: String,
    /// ASSET-CORE-04F: which registry lane actually produced this response —
    /// `"legacy"` | `"v2"` | `"none"`. `"none"` whenever the requested lane
    /// was never loaded at all (unknown `registry_source`, v2 requested but
    /// not configured/missing/invalid) — this route never silently falls
    /// back from a requested `"v2"` to `"legacy"`.
    pub registry_source_used: String,
    /// ASSET-CORE-04F: `true` iff `MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH`
    /// is set server-side, independent of what `registry_source` this
    /// specific call requested.
    pub registry_v2_configured: bool,
    /// ASSET-CORE-04F: echo of the configured v2 path (`None` when
    /// unconfigured), independent of `registry_source_used`. Never a
    /// client-supplied path — this route never accepts an arbitrary
    /// filesystem path from a query parameter.
    pub registry_v2_path: Option<String>,
    /// ASSET-CORE-04F: `Some(reason_code)` whenever `truth_state` is
    /// specifically a registry load/validate/selection failure
    /// (`"registry_unavailable"`, `"registry_invalid"`, or
    /// `"invalid_registry_source"`), `None` otherwise — lets an operator
    /// detect a broken registry-source configuration without string-matching
    /// the general-purpose `reason_code` vocabulary.
    pub registry_error_code: Option<String>,

    pub has_execution_snapshot: bool,
    /// Total positions in the execution snapshot. Unaffected by `symbol`/
    /// `limit` — always the full-portfolio total, even when `positions`
    /// below is filtered/truncated.
    pub position_count: usize,
    pub valued_position_count: usize,
    pub failed_position_count: usize,
    /// Count of positions whose only blocker is a missing completed mark.
    pub missing_mark_count: usize,
    /// Count of held positions whose resolved registry instrument is not
    /// `"equity"`. Always counts only *resolved* positions — a position
    /// whose symbol is absent from the registry contributes to neither this
    /// nor an equity count, since its asset class is genuinely unknown.
    pub non_equity_position_count: usize,
    /// Whole-registry count of rows that are both `enabled` and non-equity
    /// (ASSET-CORE-04B `InstrumentEconomicsBridgeSummary::non_equity_enabled_count`,
    /// unaffected by which symbols are currently held). Always `0` for any
    /// registry that passed `validate_registry_v2` without the test-only
    /// escape hatch.
    pub non_equity_enabled_count: usize,

    /// Hardcoded `"USD"`. `PortfolioSnapshot` carries no account-currency
    /// field today; this mirrors the implicit USD assumption every other
    /// live portfolio/accounting path in this crate already makes. Never
    /// used to perform FX conversion.
    pub account_currency: String,
    pub cash_micros: Option<i64>,
    pub nav_micros: Option<i64>,
    pub gross_exposure_micros: Option<i64>,
    pub net_exposure_micros: Option<i64>,

    /// Per-position rows, filtered by `symbol` and truncated by `limit` when
    /// supplied. May be a strict subset of `position_count`.
    pub positions: Vec<PortfolioEconomicsStatusPositionRow>,
    pub asset_class_exposures: Vec<PortfolioEconomicsStatusExposureRow>,
    pub currency_exposures: Vec<PortfolioEconomicsStatusExposureRow>,
    /// Human-readable, one entry per position that could not be valued.
    pub blockers: Vec<String>,
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
    /// Required for "capture-account-equity-baseline": target trading date
    /// in "YYYY-MM-DD" form. Must be a real NYSE trading day per
    /// `NyseWeekdaysProvider` (PAPER-DAILY-PNL-CAPTURE-01B).
    pub trading_date: Option<String>,
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
    /// STRATEGY-PROMOTION-REGISTRY-01D: canonical strategy timeframe in
    /// seconds — part of the exact `(strategy_id, symbol, timeframe_secs)`
    /// identity the paper-promotion gate checks.
    ///
    /// Required once the promotion gate applies (i.e. always, in this
    /// patch): a missing or non-positive value fails closed with
    /// `"promotion_timeframe_unknown"` — this route never silently assumes
    /// a default timeframe, since guessing wrong could approve trading
    /// under the wrong strategy configuration's promotion record.
    #[serde(default)]
    pub timeframe_secs: Option<i64>,
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
// /api/v1/broker/assets/:symbol/shortable-preflight
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortablePreflightResponse {
    pub canonical_route: String,
    pub symbol: String,
    pub asset_class: Option<String>,
    pub tradable: Option<bool>,
    pub shortable: Option<bool>,
    pub marginable: Option<bool>,
    pub easy_to_borrow: Option<bool>,
    /// "active" | "not_configured" | "broker_unavailable" |
    /// "symbol_not_found" | "query_failed" | "unsupported_adapter".
    pub truth_state: String,
    pub source: Option<String>,
    pub message: String,
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
// AUTON-NO-SIGNAL-OBS-01: strategy signal-evaluation journal response types
// ---------------------------------------------------------------------------

/// One row in the strategy signal-evaluation journal response.
///
/// A no-signal row (`signal_generated = false`) is informational, not an
/// error — it never implies an `oms_outbox`/order/fill row exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvaluationRow {
    pub evaluation_id: Uuid,
    pub ts_utc: String,
    /// `None` when no run was active at evaluation time.
    pub run_id: Option<Uuid>,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    /// `"db_loaded"`, `"no_bars_available"`, or `"stale_bars"`.
    pub bar_context_source: String,
    pub bars_loaded: i64,
    /// `None` only when no completed bars exist at all.
    pub latest_bar_ts_utc: Option<String>,
    pub signal_generated: bool,
    /// Signed sum of strategy target quantities. `None` when `on_bar` never ran.
    pub signal_qty: Option<i64>,
    /// `"buy"` / `"sell"`; `None` when `signal_qty` is `None` or zero.
    pub signal_side: Option<String>,
    pub reason_code: String,
    pub reason: String,
    /// `"pre_dispatch_gate"` or `"strategy_evaluated"`.
    pub decision_stage: String,
    pub source: String,
}

/// Response wrapper for `GET /api/v1/execution/signal-evaluations`.
///
/// Deliberately not scoped to the active run (unlike `execution_fill_quality`):
/// the operator must be able to inspect a no-signal evaluation recorded
/// before a daemon restart, even when no run is currently active.
///
/// `truth_state`:
/// - `"active"` — DB pool present and at least one row exists; `rows` is authoritative.
/// - `"no_rows"` — DB pool present but no evaluation has been recorded yet.
/// - `"db_unavailable"` — no DB pool configured; `rows` is empty and not authoritative.
/// - `"query_failed"` — DB pool present but the query itself failed; `rows` is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvaluationsResponse {
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.strategy_signal_evaluations"` when DB-backed; `"unavailable"` otherwise.
    pub backend: String,
    /// Most recent evaluations across all runs/symbols, newest first. At most 100 rows.
    pub rows: Vec<SignalEvaluationRow>,
}

// ---------------------------------------------------------------------------
// AUTON-NO-TRADE-OFFHOURS-01C: autonomous no-trade diagnostic response types
// ---------------------------------------------------------------------------

/// One row in the autonomous no-trade diagnostic journal response.
///
/// A snapshot of one `GET /api/v1/autonomous/readiness` verdict. Recording a
/// row never implies an order was attempted — `paper_order_attempted` and
/// `live_order_attempted` are always `false` for every row this patch writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoTradeDiagnosticRow {
    pub diagnostic_id: Uuid,
    pub observed_at_utc: String,
    /// `None` when no active run existed at observation time — the common
    /// off-hours case, never a fabricated default.
    pub run_id: Option<Uuid>,
    pub mode: String,
    /// `"in_window"` or `"outside_window"`.
    pub session_window_state: String,
    pub runtime_start_allowed: bool,
    pub arm_state: String,
    pub overall_ready: bool,
    pub reason_code: String,
    pub reason: String,
    pub stage: String,
    /// Always `false` for every row this patch writes.
    pub paper_order_attempted: bool,
    /// Always `false` for every row this patch writes.
    pub live_order_attempted: bool,
    pub source: String,
}

/// Response wrapper for `GET /api/v1/autonomous/no-trade-diagnostics`.
///
/// Deliberately not scoped to the active run (unlike `execution_fill_quality`):
/// the operator must be able to inspect an off-hours no-trade explanation
/// recorded before a daemon restart, even when no run is currently active.
///
/// `truth_state`:
/// - `"active"` — DB pool present and at least one row exists; `rows` is authoritative.
/// - `"no_rows"` — DB pool present but no diagnostic has been recorded yet.
/// - `"db_unavailable"` — no DB pool configured; `rows` is empty and not authoritative.
/// - `"query_failed"` — DB pool present but the query itself failed; `rows` is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoTradeDiagnosticsResponse {
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.autonomous_no_trade_diagnostics"` when DB-backed; `"unavailable"` otherwise.
    pub backend: String,
    /// Most recent diagnostics across all runs, newest first. At most 100 rows.
    pub rows: Vec<NoTradeDiagnosticRow>,
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

/// One fill row in the paper journal, with durable strategy lineage attached.
///
/// WAVE05-PAPER-JOURNAL-STRATEGY-LINEAGE-01: `strategy_id` and
/// `strategy_semantic_fingerprint` are recovered from the EXACT originating
/// outbox row via `fill_quality_telemetry.internal_order_id ==
/// oms_outbox.idempotency_key` (unique index `uq_outbox_idempotency`) —
/// never inferred by symbol, timestamp proximity, current strategy
/// assignment, or current registry/promotion state. See
/// `strategy_attribution_state` for why a field is `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalFillRow {
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
    /// Durable `order_json.strategy_id` from the originating outbox row.
    /// `None` for manual/non-strategy orders and when
    /// `strategy_attribution_state == "lineage_missing"`.
    pub strategy_id: Option<String>,
    /// Durable `order_json.strategy_semantic_fingerprint` from the
    /// originating decision. `None` for manual orders, for legacy strategy
    /// orders persisted before fingerprint capture, and when
    /// `strategy_attribution_state == "lineage_missing"`. Never re-derived
    /// from current registry/promotion state.
    pub strategy_semantic_fingerprint: Option<String>,
    /// - `"attributed"` — originating outbox row found, run-coherent, and
    ///   carries a well-formed `strategy_id`.
    /// - `"unattributed_manual"` — originating outbox row found, run-coherent,
    ///   but carries no `strategy_id` and no strategy signal provenance;
    ///   genuine absence, not corruption.
    /// - `"lineage_missing"` — `internal_order_id` does not resolve to any
    ///   `oms_outbox` row. A contradiction between fill and outbox truth;
    ///   distinct from `"unattributed_manual"` so it is never mistaken for
    ///   a genuine non-strategy order.
    /// - `"lineage_invalid"` — the originating outbox row was found but its
    ///   lineage is not trustworthy (cross-run mismatch, or malformed/
    ///   contradictory durable attribution fields). See
    ///   `strategy_attribution_reason` for the specific cause. Never
    ///   collapsed into `"unattributed_manual"` or `"attributed"`.
    pub strategy_attribution_state: String,
    /// Bounded machine-readable reason code, set only when
    /// `strategy_attribution_state == "lineage_invalid"`:
    /// - `"run_mismatch"` — the originating outbox row belongs to a
    ///   different run than the fill.
    /// - `"strategy_id_malformed"` — `strategy_id` is present but is JSON
    ///   `null`, a non-string type, or a blank string.
    /// - `"strategy_id_missing_for_strategy_source"` — `signal_source`
    ///   indicates a strategy-originated order but `strategy_id` is absent.
    /// - `"fingerprint_without_strategy_id"` — `strategy_semantic_fingerprint`
    ///   is present and well-formed but `strategy_id` is absent.
    /// - `"fingerprint_malformed"` — `strategy_semantic_fingerprint` is
    ///   present but is JSON `null`, a non-string type, or a blank string.
    ///
    /// `None` for every other `strategy_attribution_state`. Never carries
    /// unbounded raw JSON/error text.
    pub strategy_attribution_reason: Option<String>,
}

/// Fill evidence lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run; `rows` is authoritative fill history.
///   Empty `rows` = no fills yet recorded for this run. A row's own
///   `strategy_attribution_state` may still be `"lineage_missing"` or
///   `"lineage_invalid"` — those are authoritative row-level truths, not
///   lane-level failures.
/// - `"no_active_run"` — DB present but no active run; rows empty; not authoritative.
/// - `"no_db"` — no DB pool; rows empty; not authoritative.
/// - `"query_failed"` — DB + active run present but the fills query or a
///   per-row strategy-lineage lookup errored (a genuine DB/query failure,
///   not a lineage-integrity finding); rows empty; not authoritative. A
///   per-row lineage DB error degrades the whole lane rather than surfacing
///   a partially-attributed row set as if it were authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalFillsLane {
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<PaperJournalFillRow>,
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

/// One deterministic FIFO closed-trade fragment: an opposite-side canonical
/// effective fill fully or partially closed an open FIFO lot.
///
/// WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01: derived from the SAME
/// canonical effective-fill replay that feeds `mqk_portfolio`'s FIFO
/// accounting (never a second raw `oms_inbox` replay, never raw
/// `BrokerEvent.delta_qty`). `gross_realized_pnl_micros` is GROSS trading
/// P&L — the same semantic as `mqk_portfolio::PortfolioState::
/// realized_pnl_micros` — fees are never netted into it here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalClosedTradeRow {
    pub run_id: Uuid,
    pub symbol: String,
    /// Direction of the LOT THAT WAS CLOSED: `"long"` (closed by a sell) or
    /// `"short"` (closed by a buy) — not the closing fill's own side.
    pub direction: String,
    pub qty: i64,
    pub entry_price_micros: i64,
    pub exit_price_micros: i64,
    pub gross_realized_pnl_micros: i64,
    /// `oms_inbox.inbox_id` / `internal_order_id` of the fill that opened
    /// this lot.
    pub open_inbox_id: i64,
    pub open_internal_order_id: String,
    /// `oms_inbox.inbox_id` / `internal_order_id` of the fill that closed
    /// this lot.
    pub close_inbox_id: i64,
    pub close_internal_order_id: String,
    /// Durable opening-side `strategy_id`. `None` for manual orders or when
    /// lineage is missing/invalid — never fabricated. A LEGACY strategy
    /// order (persisted before fingerprint capture) still carries a real,
    /// non-`None` `strategy_id` here; only `open_strategy_semantic_fingerprint`
    /// is `None` for that case. See `attribution_state`.
    pub open_strategy_id: Option<String>,
    /// `None` for manual orders, for LEGACY strategy orders persisted before
    /// fingerprint capture, or when lineage is missing/invalid — never
    /// re-derived from current registry/promotion state.
    pub open_strategy_semantic_fingerprint: Option<String>,
    /// Durable closing-side `strategy_id`. Same absence rules as
    /// `open_strategy_id` (legacy orders still carry a real `strategy_id`
    /// here).
    pub close_strategy_id: Option<String>,
    /// Same absence rules as `open_strategy_semantic_fingerprint`.
    pub close_strategy_semantic_fingerprint: Option<String>,
    /// - `"attributed"` — open and close share the same `strategy_id` AND
    ///   the same `strategy_semantic_fingerprint`.
    /// - `"cross_strategy"` — open and close resolve to different
    ///   `strategy_id`s. Gross closure P&L is still shown; never assigned to
    ///   either strategy's analytics.
    /// - `"semantic_identity_changed"` — same `strategy_id` on both sides
    ///   but a different `strategy_semantic_fingerprint`; not the same
    ///   semantic strategy.
    /// - `"manual_or_mixed"` — at least one side is a genuine manual/
    ///   non-strategy order.
    /// - `"lineage_incomplete"` — at least one side is a legacy strategy
    ///   fill missing its fingerprint; gross math is visible but exact
    ///   semantic attribution cannot be proven.
    /// - `"lineage_invalid"` — at least one side's originating outbox row
    ///   lineage is malformed/contradictory.
    /// - `"lineage_missing"` — at least one side's originating outbox row
    ///   does not exist.
    pub attribution_state: String,
}

/// Closed-trade attribution lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run + accounting epoch `"complete"` +
///   `sum_gross_realized_pnl_micros` proven equal to both the canonical
///   replay's `realized_pnl_micros` and (when present) the durable
///   `sys_paper_portfolio_accounting_state.realized_pnl_micros`; `rows` is
///   authoritative.
/// - `"incomplete"` — DB + active run present, projection math is
///   internally proven consistent, but the run's durable accounting epoch
///   is `"incomplete"` (see `accounting_epoch_reason`) or no durable
///   accounting-state row exists yet. `rows` still reflects observed fills
///   but must not be read as complete strategy P&L for the account —
///   inherited/adopted positions this run's fill history cannot explain may
///   be missing their opening lot.
/// - `"parity_failed"` — FAIL CLOSED: the projection's summed gross P&L
///   contradicted canonical account replay or durable accounting truth.
///   `rows` is empty; never returns apparently-authoritative strategy
///   metrics on a proven contradiction.
/// - `"query_failed"` — a DB query or lineage lookup errored; `rows` empty.
/// - `"no_active_run"` — DB present but no active run; `rows` empty.
/// - `"no_db"` — no DB pool; `rows` empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalClosedTradesLane {
    pub truth_state: String,
    pub backend: String,
    /// Durable `sys_paper_portfolio_accounting_state.accounting_epoch`
    /// (`"complete"` | `"incomplete"`) when known. `None` when no durable
    /// accounting-state row exists for this run yet, or the lane is not
    /// `"active"`/`"incomplete"`.
    pub accounting_epoch: Option<String>,
    pub accounting_epoch_reason: Option<String>,
    /// Sum of `gross_realized_pnl_micros` across `rows`. `Some` only when
    /// `truth_state` is `"active"` or `"incomplete"`.
    pub sum_gross_realized_pnl_micros: Option<i64>,
    /// WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01-REPAIR-01: the exact
    /// shared `classify_portfolio_provenance` verdict for this run's
    /// snapshot/accounting relationship (the same closed vocabulary
    /// `routes/durable_portfolio.rs` and `routes/paper_lifecycle.rs`
    /// expose) — `"active"`, `"fill_history_incomplete"`,
    /// `"accounting_epoch_unavailable"`, `"accounting_snapshot_mismatch"`,
    /// `"not_found"`, `"query_failed"`, `"unsupported_source"`, or
    /// `"invalid_snapshot"`. Never collapsed into a generic `"incomplete"`
    /// without this field naming the exact defect. `None` only when the
    /// projection itself never got far enough to classify it (top-level
    /// projection-vs-canonical-replay parity failure, run lookup failure, or
    /// projection build failure).
    pub accounting_provenance_state: Option<String>,
    /// The canonical `recover_oms_and_portfolio_traced` replay watermark
    /// (`max(inbox_id)` across all applied rows) this projection was built
    /// from.
    pub canonical_last_applied_inbox_id: Option<i64>,
    /// Durable `sys_paper_portfolio_accounting_state.last_applied_inbox_id`
    /// for this run, when an accounting row exists.
    pub accounting_last_applied_inbox_id: Option<i64>,
    /// `"same_watermark"` | `"accounting_watermark_mismatch"`. `Some` only
    /// when `accounting_provenance_state == "active"` (the only case where a
    /// same-watermark comparison is meaningful) — `None` otherwise. A
    /// mismatch here means the durable accounting row's replay watermark is
    /// stale relative to the canonical projection even though its
    /// `source_snapshot_id` and `accounting_epoch` otherwise look current;
    /// `truth_state` must never be `"active"` when this is
    /// `"accounting_watermark_mismatch"`.
    pub accounting_watermark_state: Option<String>,
    pub rows: Vec<PaperJournalClosedTradeRow>,
}

/// Response for `GET /api/v1/paper/journal`.
///
/// Unified paper-trading evidence surface for operator review.  Separates
/// fill evidence (what executed) from signal-admission history (what was
/// submitted and accepted into the outbox) and attributed closed-trade
/// history (what FIFO lots closed, and which exact strategy identity they
/// attribute to).
///
/// Every lane carries an independent `truth_state` value.  An operator can
/// answer:
/// - What fills were produced by this run? → `fills_lane`
/// - What signals were admitted for dispatch? → `admissions_lane`
/// - What FIFO trades closed, and were they attributable to one exact
///   strategy semantic identity? → `closed_trades_lane`
///
/// No lane fabricates history.  If a lane is unavailable its `rows`
/// are empty and `truth_state` says so explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Active run ID when lanes are `"active"`.  `None` otherwise.
    pub run_id: Option<String>,
    /// Fill evidence sourced from `postgres.fill_quality_telemetry`.
    pub fills_lane: PaperJournalFillsLane,
    /// Signal-admission history sourced from `postgres.audit_events`.
    pub admissions_lane: PaperJournalAdmissionsLane,
    /// Attributed FIFO closed-trade history — see
    /// [`PaperJournalClosedTradesLane`].
    pub closed_trades_lane: PaperJournalClosedTradesLane,
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/performance — WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01
// ---------------------------------------------------------------------------

/// One attribution-state coverage bucket: a deterministic fragment count and
/// gross realized P&L total for every P2 `attribution_state`, proving no
/// economic P&L silently disappears even when it cannot be attributed to one
/// exact semantic strategy. `sum(gross_realized_pnl_micros)` across every
/// bucket in a response always equals the upstream closed-trade authority's
/// total gross realized P&L for the resolved run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPerformanceCoverageBucket {
    /// One of P2's frozen `ClosureAttribution` states: `"attributed"`,
    /// `"cross_strategy"`, `"semantic_identity_changed"`, `"manual_or_mixed"`,
    /// `"lineage_incomplete"`, `"lineage_invalid"`, or `"lineage_missing"`.
    pub attribution_state: String,
    pub fragment_count: i64,
    pub gross_realized_pnl_micros: i64,
}

/// One exact semantic-strategy performance row, keyed by
/// `(strategy_id, strategy_semantic_fingerprint)`. Built ONLY from P2
/// `"attributed"` closure fragments grouped into `AttributedCloseEvent`s
/// (fragments sharing the same closing economic fill collapse into one
/// event). All P&L fields are GROSS trading P&L before fees — see the
/// response's `pnl_basis`/`fee_allocation_state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPerformanceRow {
    pub strategy_id: String,
    pub strategy_semantic_fingerprint: String,
    /// Raw P2 FIFO closure fragment count (before close-event grouping).
    pub attributed_fragment_count: i64,
    /// Count of distinct closing economic fills (`close_inbox_id` +
    /// `close_internal_order_id`) — the unit every other metric here is
    /// computed over, NOT the raw fragment count.
    pub attributed_close_event_count: i64,
    pub attributed_closed_qty: i64,
    pub gross_realized_pnl_micros: i64,
    /// Sum of positive close-event gross P&L.
    pub gross_profit_micros: i64,
    /// Absolute sum of negative close-event gross P&L (always `>= 0`).
    pub gross_loss_abs_micros: i64,
    pub winning_close_event_count: i64,
    pub losing_close_event_count: i64,
    pub flat_close_event_count: i64,
    /// `winning / (winning + losing)`. Flat events are excluded from the
    /// denominator. `None` when that denominator is `0`.
    pub hit_rate: Option<f64>,
    /// Total gross P&L divided by close-event count. `None` when there are
    /// zero close events.
    pub gross_expectancy_micros_per_close_event: Option<f64>,
    /// `None` when there are zero winning close events.
    pub average_win_micros: Option<f64>,
    /// `None` when there are zero losing close events.
    pub average_loss_abs_micros: Option<f64>,
    /// `gross_profit_micros / gross_loss_abs_micros`. `None` when
    /// `gross_loss_abs_micros == 0` — NEVER infinity, NaN, or a fabricated
    /// sentinel value.
    pub profit_factor: Option<f64>,
    /// Maximum drawdown of the REALIZED closed-P&L cumulative curve across
    /// this strategy's ordered attributed close events (peak-to-trough,
    /// always `>= 0`). This is NOT account-equity drawdown, mark-to-market
    /// drawdown, intratrade drawdown, or MAE.
    pub max_realized_pnl_drawdown_micros: i64,
    /// WAVE05-STRATEGY-DECAY-AND-REGIME-MONITOR-01 (P4): conservative
    /// forward Paper performance-decay monitoring over this strategy's exact
    /// attributed close-event series. Observational only -- never demotes,
    /// suppresses, or otherwise changes trading behavior.
    pub decay_monitor: StrategyDecayMonitor,
    /// P4: current market-regime CONTEXT for this strategy's most recent
    /// exact durable (symbol, timeframe) -- research-only observational
    /// context, never an execution or risk-gate authority.
    pub regime_context: StrategyRegimeContext,
    /// WAVE05-STRATEGY-RISK-VISIBILITY-01 (P5): deterministic, VISIBILITY-
    /// ONLY strategy-level risk surface built from P3/P4 read models plus
    /// the existing durable strategy-suppression read seam. Never mutates
    /// suppression, promotion, accounting, or trading state.
    pub risk_visibility: StrategyRiskVisibility,
}

/// P5.2-P5.5: read-only strategy risk visibility. No mutation fields or
/// buttons anywhere in this type -- this route never calls
/// `insert_strategy_suppression`/`clear_strategy_suppression`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRiskVisibility {
    /// Closed vocabulary, in precedence order:
    /// - `"unavailable"` -- upstream P3 performance authority is not active
    ///   (structurally unreachable today since a row only exists when the
    ///   response's own `truth_state == "active"`; included for completeness
    ///   of the precedence rule).
    /// - `"suppressed"` -- an active durable strategy suppression exists for
    ///   this `strategy_id` (suppression is keyed by `strategy_id`, NOT
    ///   fingerprint -- see `active_strategy_suppression`'s doc).
    /// - `"insufficient_data"` -- P4 `decay_state == "insufficient_data"`.
    /// - `"watch"` -- P4 `decay_state == "decay_observed"`.
    /// - `"normal"` -- otherwise.
    pub risk_visibility_state: String,
    /// At least: `"active_strategy_suppression"`,
    /// `"gross_expectancy_sign_flip_negative"` (P4 `decay_observed`),
    /// `"semantic_identity_change_excluded_pnl"`, `"cross_strategy_closure_pnl"`,
    /// `"incomplete_lineage_pnl"`, `"manual_mixed_closure_pnl"` (the last
    /// four are response-wide attribution-coverage facts, not scoped to this
    /// one row -- attributing a cross-strategy or manual closure to a single
    /// exact strategy row would be arbitrary), and
    /// `"observational_high_volatility_context"` (informational ONLY -- this
    /// flag alone never changes `risk_visibility_state`).
    pub risk_flags: Vec<String>,
    /// `true` when a durable `sys_strategy_suppressions` row is currently
    /// `active` for this exact `strategy_id`. Suppression is keyed by
    /// `strategy_id`, NOT `(strategy_id, strategy_semantic_fingerprint)` --
    /// an active suppression for strategy A applies operationally to EVERY
    /// semantic version (fingerprint) of A under the current admission gate,
    /// and this field is `true` on every one of that strategy_id's rows,
    /// never fingerprint-specific.
    pub active_strategy_suppression: bool,
    pub active_suppression_id: Option<String>,
    pub active_suppression_trigger_domain: Option<String>,
    pub active_suppression_trigger_reason: Option<String>,
    /// Closed vocabulary, text/visibility only -- never invokes a mutation:
    /// `"insufficient_evidence"` (unavailable/insufficient_data) |
    /// `"already_suppressed"` (suppressed) | `"review"` (watch) |
    /// `"none"` (normal).
    pub recommended_operator_action: String,
}

/// P4.3: aggregate metrics over one decay-monitor window (baseline or
/// recent) of attributed close events. Same GROSS-before-fees P&L basis as
/// the parent row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDecayWindowMetrics {
    pub event_count: i64,
    pub gross_realized_pnl_micros: i64,
    /// `None` only when `event_count == 0` (never occurs for a populated
    /// baseline/recent window, since both are exactly 10/5 events).
    pub gross_expectancy_micros_per_close_event: Option<f64>,
    /// Flat events excluded from the denominator; `None` when winning+losing == 0.
    pub hit_rate: Option<f64>,
    pub gross_profit_micros: i64,
    pub gross_loss_abs_micros: i64,
    pub max_realized_pnl_drawdown_micros: i64,
}

/// P4.2/P4.4: deterministic, conservative decay monitor over a strategy's
/// most recent 15 attributed close events (baseline = the 10 immediately
/// preceding the most recent 5; recent = the newest 5). Detects only a
/// strong gross-expectancy sign reversal -- `decay_observed` is a
/// deterministic monitoring flag, NOT proof that the strategy's true alpha
/// has disappeared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDecayMonitor {
    /// `"insufficient_data"` (fewer than 15 attributed close events) |
    /// `"decay_observed"` (baseline expectancy > 0, recent expectancy < 0) |
    /// `"improvement_observed"` (baseline <= 0, recent > 0) |
    /// `"no_expectancy_sign_flip"` (same-sign / no reversal).
    pub decay_state: String,
    /// `None` only when `decay_state == "insufficient_data"`.
    pub baseline: Option<StrategyDecayWindowMetrics>,
    /// `None` only when `decay_state == "insufficient_data"`.
    pub recent: Option<StrategyDecayWindowMetrics>,
    /// `recent.gross_expectancy... - baseline.gross_expectancy...`. `None`
    /// when either window is unavailable.
    pub expectancy_delta_micros: Option<f64>,
    /// `recent.hit_rate - baseline.hit_rate`. `None` when either side is
    /// unavailable (including when either window's own hit_rate is `None`).
    pub hit_rate_delta: Option<f64>,
}

/// P4.5-P4.7: current market-regime CONTEXT for a strategy's most recent
/// exact durable (symbol, timeframe_secs), resolved ONLY from the exact
/// originating order (`fetch_order_symbol_timeframe_context`) -- never from
/// current config, current registry state, or a symbol-latest lookup.
/// `regime_authority` is always `"research_only_observational"`: this MUST
/// NEVER gate execution, risk, promotion, or suppression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRegimeContext {
    /// `"active_observational"` | `"insufficient_data"` (too few completed
    /// bars for the detector) | `"context_unavailable"` (most recent
    /// attributed close event's exact symbol/timeframe cannot be proven) |
    /// `"context_ambiguous"` (multiple distinct exact timeframes exist for
    /// the same strategy+symbol among its attributed close events) |
    /// `"query_failed"` (a bar/order query errored).
    pub regime_truth_state: String,
    /// Always `"research_only_observational"` -- never execution or
    /// risk-gate authority. See `REGIME_CAN_AFFECT_EXECUTION`/
    /// `REGIME_CAN_AFFECT_RISK_GATE` in the Wave05 spec (always `NO`).
    pub regime_authority: String,
    /// The exact durable symbol this context resolved to. `None` when
    /// `regime_truth_state` is `"context_unavailable"` or `"context_ambiguous"`.
    pub symbol: Option<String>,
    pub timeframe_secs: Option<i64>,
    /// One of `mqk_backtest::regime::MarketRegimeKind`'s codes (e.g.
    /// `"bull_trend"`, `"high_volatility"`, `"insufficient_data"`). `None`
    /// unless a detection was actually run.
    pub regime_kind: Option<String>,
    pub confidence: Option<f64>,
    pub reason_codes: Vec<String>,
    pub input_bar_count: Option<i64>,
    pub valid_bar_count: Option<i64>,
}

/// Response for `GET /api/v1/strategy/performance`.
///
/// `truth_state`:
/// - `"active"` — the upstream closed-trade authority (shared with the Paper
///   Journal `closed_trades_lane`) is fully proven current; `rows` and
///   `attribution_coverage` are authoritative. Zero rows is a valid
///   authoritative zero, distinguishable from every unavailable state below.
/// - `"incomplete"` | `"parity_failed"` | `"query_failed"` — the upstream
///   closed-trade authority is not fully active; `rows` and
///   `attribution_coverage` are always empty (never a fabricated zero-valued
///   performance row) — see `accounting_provenance_state` for the exact
///   upstream reason.
/// - `"not_found"` — no run resolved (explicit `run_id` not found, or no
///   durable PAPER run exists yet for this engine).
/// - `"unsupported_source"` — the resolved run is not PAPER mode.
/// - `"db_unavailable"` — no DB pool configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPerformanceResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub run_id: Option<String>,
    /// The exact shared `classify_portfolio_provenance` verdict for the
    /// upstream closed-trade authority, when it got far enough to classify
    /// one. `None` when run resolution itself failed/was unsupported.
    pub accounting_provenance_state: Option<String>,
    /// Always `"gross_realized_before_fees"` — every P&L field in this
    /// response is gross trading P&L; fees are never netted in.
    pub pnl_basis: String,
    /// Always `"not_allocated_to_strategy_close_events"` — fees are not
    /// currently allocated deterministically across FIFO closure fragments,
    /// so no `net_pnl`/`net_expectancy`/`after_cost_expectancy` field exists
    /// anywhere in this response.
    pub fee_allocation_state: String,
    pub rows: Vec<StrategyPerformanceRow>,
    /// Deterministic coverage across every P2 attribution state — proves no
    /// economic P&L silently disappears. Always empty when `truth_state`
    /// is not `"active"`.
    pub attribution_coverage: Vec<StrategyPerformanceCoverageBucket>,
    /// `sum(attribution_coverage[].gross_realized_pnl_micros)`. `Some` only
    /// when `truth_state == "active"`.
    pub total_gross_realized_pnl_micros: Option<i64>,
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

// ---------------------------------------------------------------------------
// PAPER-ORDER-LIFECYCLE-VIS-01C: GET /api/v1/execution/paper-lifecycle
// ---------------------------------------------------------------------------

/// One `runs` row, as surfaced by the paper-lifecycle route.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleRunRow {
    pub run_id: String,
    pub engine_id: String,
    pub mode: String,
    pub status: String,
    pub started_at_utc: String,
    pub armed_at_utc: Option<String>,
    pub running_at_utc: Option<String>,
    pub stopped_at_utc: Option<String>,
    pub halted_at_utc: Option<String>,
}

/// One `strategy_signal_evaluations` row, as surfaced by the paper-lifecycle route.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleSignalEvaluationRow {
    pub evaluation_id: String,
    pub ts_utc: String,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    pub signal_generated: bool,
    pub signal_qty: Option<i64>,
    pub signal_side: Option<String>,
    pub reason_code: String,
    pub reason: String,
    pub decision_stage: String,
}

/// One `autonomous_no_trade_diagnostics` row, as surfaced by the paper-lifecycle route.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleNoTradeDiagnosticRow {
    pub diagnostic_id: String,
    pub observed_at_utc: String,
    pub mode: String,
    pub session_window_state: String,
    pub arm_state: String,
    pub overall_ready: bool,
    pub reason_code: String,
    pub reason: String,
    pub stage: String,
    pub paper_order_attempted: bool,
    pub live_order_attempted: bool,
}

/// One `oms_outbox` row, as surfaced by the paper-lifecycle route.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleOutboxRow {
    pub idempotency_key: String,
    pub status: String,
    pub symbol: Option<String>,
    pub side: Option<String>,
    pub qty: Option<i64>,
    pub created_at_utc: String,
    pub claimed_at_utc: Option<String>,
    pub dispatching_at_utc: Option<String>,
    pub sent_at_utc: Option<String>,
}

/// One `oms_inbox` row, as surfaced by the paper-lifecycle route.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleInboxRow {
    pub inbox_id: i64,
    pub broker_message_id: String,
    pub internal_order_id: Option<String>,
    pub broker_order_id: Option<String>,
    /// `"ack"` | `"fill"` | `"partial_fill"` | `"cancel_ack"` | `"cancel_reject"` |
    /// `"replace_ack"` | `"replace_reject"` | `"reject"`.
    pub event_kind: String,
    pub received_at_utc: String,
    /// `None` until the fill/event has been applied to the portfolio.
    pub applied_at_utc: Option<String>,
}

/// Deterministic classification of the assembled lifecycle rows for one run.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleSummary {
    /// See `PaperLifecycleResponse::truth_state` doc for the full vocabulary.
    pub overall_lifecycle_state: String,
    pub signal_count: usize,
    pub generated_signal_count: usize,
    pub no_trade_diagnostic_count: usize,
    pub outbox_count: usize,
    pub inbox_count: usize,
    pub paper_order_attempted: bool,
    pub live_order_attempted: bool,
    pub broker_ack_seen: bool,
    pub fill_seen: bool,
    pub order_failed_or_rejected: bool,
    /// `true` only when every non-portfolio/non-P&L stage below is
    /// durably resolved for this run. Portfolio/P&L visibility is never
    /// counted toward this flag — see `portfolio_truth_state` doc.
    pub full_lifecycle_visible: bool,
}

/// `GET /api/v1/execution/paper-lifecycle` response.
///
/// Read-only, DB-backed reconstruction of one paper run's lifecycle chain:
/// run -> strategy signal evaluation -> no-trade diagnostics (if any) ->
/// oms_outbox order intent/submission -> oms_inbox broker ack/fill ->
/// portfolio/accounting visibility status -> P&L visibility readiness.
///
/// Restart-surviving: every field is sourced from a durable DB row via an
/// explicit `run_id` or the durably-resolved latest PAPER run
/// (`mqk_db::fetch_latest_run_for_engine`), never from in-memory
/// active-run state. Never calls a broker/provider. Never writes.
#[derive(Debug, Clone, Serialize)]
pub struct PaperLifecycleResponse {
    pub canonical_route: String,
    /// Route-level resolution truth_state: `"db_unavailable"` |
    /// `"invalid_request"` | `"not_found"` | `"no_rows"` | `"active"`.
    pub truth_state: String,
    /// `"resolved"` | `"not_found"` | `"unavailable"`.
    pub run_truth_state: String,
    /// `"present"` | `"empty"` | `"unavailable"`.
    pub signal_truth_state: String,
    /// `"present"` | `"empty"` | `"unavailable"`.
    pub no_trade_truth_state: String,
    /// `"present"` | `"empty"` | `"unavailable"`.
    pub outbox_truth_state: String,
    /// `"present"` | `"empty"` | `"unavailable"`.
    pub inbox_truth_state: String,
    /// `"active"` (a durable snapshot exists for exactly this run) |
    /// `"snapshot_unavailable"` (no durable snapshot for this run) |
    /// `"unsupported_source"` (resolved run is not PAPER-mode) |
    /// `"query_failed"` (the durable snapshot query itself failed). Reads
    /// the run-scoped durable snapshot table (B4-B/B4-C, run-scoped by the
    /// B4 closure repair) — never `AppState.broker_snapshot` (in-memory,
    /// lost on restart) and never another run's snapshot.
    pub portfolio_truth_state: String,
    /// `"active"` | `"fill_history_incomplete"` | `"not_found"` |
    /// `"unsupported_source"` | `"query_failed"` — same run-scoped durable
    /// accounting-state read, same failure vocabulary as
    /// `portfolio_truth_state`.
    pub pnl_truth_state: String,
    pub run_id: Option<String>,
    pub run: Option<PaperLifecycleRunRow>,
    pub signal_evaluations: Vec<PaperLifecycleSignalEvaluationRow>,
    pub no_trade_diagnostics: Vec<PaperLifecycleNoTradeDiagnosticRow>,
    pub outbox_orders: Vec<PaperLifecycleOutboxRow>,
    pub inbox_events: Vec<PaperLifecycleInboxRow>,
    /// `None` only when `truth_state != "active"`.
    pub lifecycle_summary: Option<PaperLifecycleSummary>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
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
    /// Optional backtest-only instrument economics override.
    ///
    /// Omitted requests preserve default equity economics exactly. When present,
    /// omitted `contract_multiplier` defaults to 1 and margins remain
    /// metadata-only.
    #[serde(default)]
    pub economics: Option<BacktestEconomicsRequest>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestEconomicsRequest {
    pub contract_multiplier: Option<i64>,
    pub initial_margin_micros: Option<i64>,
    pub maintenance_margin_micros: Option<i64>,
}

/// GET /api/v1/backtests/economics-suggestion?symbol=<SYMBOL>
///
/// Read-only operator hint for backtest economics. This is not a trading
/// permission surface and does not make InstrumentRegistryV2 a live/runtime
/// input.
///
/// INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED: `asset_class`, `enabled`,
/// `paper_trading_enabled`, and `live_trading_enabled` are populated whenever
/// a matching instrument is found (v1-converted or from a configured separate
/// v2 source), and `None` otherwise. They exist so an operator viewing a
/// suggestion for a disabled/non-equity instrument (e.g. a future or crypto
/// pair carried only in a configured v2 source) cannot mistake the presence
/// of economics metadata for trading permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestEconomicsSuggestionResponse {
    pub truth_state: String,
    pub symbol: String,
    pub source: String,
    pub contract_multiplier: Option<i64>,
    pub initial_margin_micros: Option<i64>,
    pub maintenance_margin_micros: Option<i64>,
    pub reason: Option<String>,
    pub asset_class: Option<String>,
    pub enabled: Option<bool>,
    pub paper_trading_enabled: Option<bool>,
    pub live_trading_enabled: Option<bool>,
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
// STRATEGY-SCANNER-JOBS-GUI-01B: Strategy scanner job API
//
// Research/review only. Does not write to oms_outbox, oms_inbox, broker
// maps, or any order/execution table. Does not require arm_state. Does not
// start/stop the trading runtime. No provider/broker/network call. Jobs
// are in-memory only (process-lifetime); no DB persistence.
// ---------------------------------------------------------------------------

fn default_scan_top() -> usize {
    20
}

/// POST /api/v1/strategy-scans/jobs — submit a bounded local-data strategy
/// scan job. Runs the same scanner core as `mqk backtest scan-strategies`
/// (`mqk_backtest::execute_strategy_scan` / `write_scan_artifacts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanJobSubmitRequest {
    /// Defaults to `config/instruments/equities.json` when omitted.
    pub registry_path: Option<String>,
    /// Defaults to `exports/md_backup` when omitted.
    pub bars_root: Option<String>,
    /// Scanner timeframe label (e.g. `"1D"`, `"5m"`). Required, must not be blank.
    pub timeframe: String,
    /// Single strategy_id to scan (e.g. `"swing_momentum"`). Required, must not be blank.
    pub strategy: String,
    /// Number of top-ranked candidates to include in the summary. Bounded
    /// `1..=100`. Defaults to 20 when omitted.
    #[serde(default = "default_scan_top")]
    pub top: usize,
    /// Optional cap on the number of registry symbols scanned. Bounded
    /// `1..=200` when supplied.
    pub limit_symbols: Option<usize>,
    /// Defaults to `exports/strategy_scans` when omitted.
    pub out_dir: Option<String>,
}

/// Response to POST /api/v1/strategy-scans/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanJobAcceptedResponse {
    pub accepted: bool,
    pub job_id: Uuid,
    /// "queued" immediately after acceptance, "refused" on validation failure.
    pub status: String,
    /// Populated only if the job already completed synchronously (not expected).
    pub artifact_dir: Option<String>,
    /// Populated if the request was refused before queuing.
    pub error: Option<String>,
}

/// Single job summary row in GET /api/v1/strategy-scans/jobs list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanJobSummary {
    pub job_id: Uuid,
    pub status: String,
    pub timeframe: String,
    pub strategy: String,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    pub artifact_dir: Option<String>,
    pub ranked_count: Option<usize>,
    pub skipped_count: Option<usize>,
    pub error: Option<String>,
}

/// Response to GET /api/v1/strategy-scans/jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanJobsListResponse {
    pub truth_state: String,
    pub jobs: Vec<StrategyScanJobSummary>,
}

/// Response to GET /api/v1/strategy-scans/jobs/:job_id.
///
/// `summary` reuses `mqk_backtest::ScanSummary` verbatim (ranked/skipped
/// counts, top-ranked candidates, top skip reasons) — the daemon does not
/// re-derive or re-summarize scanner output. `warnings` always carries the
/// fixed research-only disclosure text once a job reaches `completed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanJobStatusResponse {
    pub truth_state: String,
    pub job_id: Uuid,
    pub status: String,
    pub submitted_at_utc: String,
    pub completed_at_utc: Option<String>,
    pub request: StrategyScanJobSubmitRequest,
    pub artifact_dir: Option<String>,
    pub summary: Option<mqk_backtest::ScanSummary>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-JOBS-GUI-01C: Strategy scanner artifact readback API
// ---------------------------------------------------------------------------

/// GET /api/v1/strategy-scans/artifact?artifact_dir=<path>
///
/// Read-only. Reads only `manifest.json` / `summary.json` / `candidates.json`
/// inside a directory that must resolve inside the configured scan artifact
/// root (default `exports/strategy_scans`). Never reads an arbitrary file
/// path. `truth_state`:
/// - `active` — all three files read and parsed successfully.
/// - `missing_artifact` — the directory (or one of the three files) does not exist.
/// - `invalid_artifact` — a file exists but failed to parse as JSON / did not
///   match the expected schema.
/// - `path_rejected` — `artifact_dir` did not resolve inside the configured
///   scan artifact root.
/// - `read_failed` — an unexpected filesystem error occurred while reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanArtifactResponse {
    pub truth_state: String,
    pub artifact_dir: String,
    pub manifest: Option<mqk_backtest::ScanManifest>,
    pub summary: Option<mqk_backtest::ScanSummary>,
    /// Candidate rows, capped at 200 (see `MAX_ARTIFACT_CANDIDATE_ROWS` in
    /// `routes/strategy_scans.rs`). `None` when the artifact could not be read.
    pub candidates: Option<Vec<mqk_backtest::StrategyScanCandidate>>,
    pub top_candidates: Vec<mqk_backtest::StrategyScanCandidate>,
    pub skip_reasons: Vec<mqk_backtest::ScanSkipReasonCount>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01D: Strategy scan review-artifact readback API
//
// Research/review only. Read-only. Does not write to oms_outbox, oms_inbox,
// broker maps, or any order/execution/admission table. No provider/broker/
// network call.
// ---------------------------------------------------------------------------

/// GET /api/v1/strategy-scans/review-artifact?review_dir=<path>
///
/// Read-only. Reads only `manifest.json` / `summary.json` /
/// `review_decisions.json` inside a directory that must resolve inside the
/// configured review artifact root (default `exports/strategy_reviews`).
/// Never reads an arbitrary file path. `truth_state`:
/// - `active` — all three files read and parsed successfully.
/// - `missing_artifact` — the directory (or one of the three files) does not exist.
/// - `invalid_artifact` — a file exists but failed to parse as JSON / did not
///   match the expected schema.
/// - `path_rejected` — `review_dir` did not resolve inside the configured
///   review artifact root.
/// - `read_failed` — an unexpected filesystem error occurred while reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyScanReviewArtifactResponse {
    pub truth_state: String,
    pub review_dir: String,
    pub manifest: Option<mqk_backtest::ReviewManifest>,
    pub summary: Option<mqk_backtest::ReviewSummary>,
    /// Decision rows, capped at 200 (see `MAX_REVIEW_ARTIFACT_DECISION_ROWS`
    /// in `routes/strategy_scans.rs`). `None` when the artifact could not be
    /// read.
    pub decisions: Option<Vec<mqk_backtest::StrategyScanReviewDecision>>,
    pub top_paper_candidates: Vec<mqk_backtest::StrategyScanReviewDecision>,
    pub top_watchlist_candidates: Vec<mqk_backtest::StrategyScanReviewDecision>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
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
    /// Override instrument registry path; omitted uses AppState default
    /// (DAILY-DATA-READINESS-01B-PROVIDER-CONTRACT-INTEGRATION-01 §B2.4).
    pub instrument_registry_path: Option<String>,
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
    /// Rows read from provider output before DB upsert.
    pub rows_read: Option<i64>,
    /// Rows accepted by provider/DB ingest validation.
    pub rows_ok: Option<i64>,
    /// Rows inserted into md_bars during the refresh.
    pub rows_inserted: Option<i64>,
    /// Rows updated in md_bars during the refresh.
    pub rows_updated: Option<i64>,
    /// Rows dropped because the bar was flagged incomplete.
    pub rows_filtered_incomplete: Option<i64>,
    /// Rows dropped because the bar was still in-progress (current bar).
    pub rows_filtered_in_progress: Option<i64>,
    /// Age of the latest completed bar in seconds, from refresh evidence or route derivation.
    /// This is the evidence **snapshot** value as of `produced_at_utc` --
    /// preserved unchanged for backward compatibility. Proof-window fields
    /// below derive from `effective_latest_completed_bar_age_secs`, not this
    /// field. INTRADAY-PROVIDER-CLOCK-SKEW-01F.
    pub latest_completed_bar_age_secs: Option<i64>,
    /// Maximum allowed completed-bar age in seconds for this symbol/timeframe.
    pub max_allowed_age_secs: Option<i64>,
    /// Seconds elapsed between the evidence file's `produced_at_utc` and the
    /// instant this route evaluated the request. `None` when `produced_at_utc`
    /// is missing or unparseable. Clamped at 0 (never negative).
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01F.
    pub evidence_elapsed_secs: Option<i64>,
    /// `latest_completed_bar_age_secs + evidence_elapsed_secs` -- the bar's
    /// age as of *now*, matching the live-age semantics the dispatch-tick
    /// freshness gate uses. This is the value `freshness_headroom_secs`,
    /// `staleness_overage_secs`, `near_expiry`, and `proof_window_risk` are
    /// derived from. `None` when either input is unavailable.
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01F.
    pub effective_latest_completed_bar_age_secs: Option<i64>,
    /// Machine-readable post-refresh freshness state.
    pub freshness_truth_state: Option<String>,
    /// Machine-readable post-refresh verdict reason.
    pub reason_code: Option<String>,
    /// Conservative route-level symbol verdict.
    pub passed: bool,
    /// Fail reasons for this symbol, empty on PASS.
    pub fail_reasons: Vec<String>,
    /// `max_allowed_age_secs - effective_latest_completed_bar_age_secs` when
    /// still within cap (`None` when already past cap or either input field
    /// is missing). Derived from *effective* (elapsed-time-adjusted) age,
    /// not the raw evidence snapshot age. INTRADAY-PROVIDER-CLOCK-SKEW-01B,
    /// 01F.
    pub freshness_headroom_secs: Option<i64>,
    /// `effective_latest_completed_bar_age_secs - max_allowed_age_secs` when
    /// already past cap (`None` when still within cap or either input field
    /// is missing). Derived from *effective* (elapsed-time-adjusted) age,
    /// not the raw evidence snapshot age. INTRADAY-PROVIDER-CLOCK-SKEW-01B,
    /// 01F.
    pub staleness_overage_secs: Option<i64>,
    /// `true` when the symbol is currently within cap but has 120s or less of
    /// headroom remaining -- likely to fail on the very next dispatch tick
    /// even though this evidence read reports it as passing.
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B.
    pub near_expiry: bool,
    /// Conservative risk classification for starting a proof window right now:
    /// `"low"` (ample headroom), `"medium"`, `"high"` (already stale, or fresh
    /// but near expiry), or `"unknown"` (age/cap fields missing from evidence).
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B.
    pub proof_window_risk: String,
    /// Human-readable operator guidance, present only when risk is elevated
    /// (`near_expiry`, already stale, or evidence fields missing).
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B.
    pub operator_action: Option<String>,
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
    /// `true` only when `all_passed == Some(true)` and every symbol's
    /// `proof_window_risk` (effective-age-derived, INTRADAY-PROVIDER-CLOCK-SKEW-01F)
    /// is `"low"` or `"medium"` -- i.e. safe to start a proof window right
    /// now without an imminent freshness-gate failure, accounting for time
    /// already elapsed since the evidence was produced. `None` when
    /// `truth_state != "active"` or there are no symbols to evaluate.
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B, 01F.
    pub proof_window_ready: Option<bool>,
    /// Worst-case `proof_window_risk` across all symbols, ranked
    /// high, unknown, medium, low. `None` when there are no symbols.
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B.
    pub proof_window_risk: Option<String>,
    /// First non-empty per-symbol `operator_action`, if any symbol has one.
    /// INTRADAY-PROVIDER-CLOCK-SKEW-01B.
    pub operator_action: Option<String>,
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED:
// read-only latest-mark evidence status
// ---------------------------------------------------------------------------

/// One latest-mark row surfaced by `GET /api/v1/market-data/latest-marks/status`.
///
/// Fields map directly from a `LatestMark` value serialized into the
/// `coinlore-latest-mark-v1` evidence contract. All fields are optional --
/// a malformed or partial evidence file must never be filled in with a
/// default value here; `None` means the field was absent or not a string/
/// number of the expected type. This type intentionally has no `open`/
/// `high`/`low`/`is_complete`/`end_ts` field: it represents a ticker-style
/// latest mark, never a completed OHLCV bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestMarkStatusRow {
    pub canonical_symbol: String,
    pub provider_id: Option<String>,
    pub provider_symbol: Option<String>,
    pub provider_coin_id: Option<String>,
    pub price_usd: Option<String>,
    pub volume24_usd: Option<String>,
    pub as_of_client_request_ts: Option<i64>,
    pub provider_ts: Option<i64>,
    pub truth_state: Option<String>,
    pub kind: Option<String>,
}

/// Response for `GET /api/v1/market-data/latest-marks/status`.
///
/// Read-only. Reads the latest `coinlore_latest_mark_*.json` evidence file
/// written by `mqk md coinlore-latest-mark --output-dir`. No DB connection,
/// no provider/network call, no CLI execution, no daemon runtime start, no
/// trading state mutation.
///
/// `truth_state` values:
/// - `"active"`             — latest evidence file parsed successfully and is fresh.
/// - `"stale"`               — evidence parsed successfully but is older than
///   `max_evidence_age_secs`, or carries no `produced_at_utc`.
/// - `"no_evidence"`        — no evidence file found in the evidence directory.
/// - `"parse_error"`        — evidence file found but JSON is malformed or has
///   an unsupported `schema_version`.
/// - `"unsafe_evidence"`    — evidence claims `db_write`/`md_bars_write`/
///   `completed_bar_claim=true`, or a mark carries a bar-like field
///   (`open`/`high`/`low`/`close`/`is_complete`/`end_ts`). Never surfaced as
///   `active` regardless of freshness -- this is a fail-closed safety state,
///   not a freshness state.
/// - `"backend_unavailable"` — evidence directory or file could not be read.
///
/// This route reuses the same evidence directory as
/// `GET /api/v1/market-data/intraday-refresh/status`
/// (`st.md_refresh_evidence_dir`, default `exports/market_data`), filtered to
/// the distinct `coinlore_latest_mark_*.json` filename prefix so the two
/// evidence streams never collide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestMarkStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub provider: Option<String>,
    pub produced_at_utc: Option<String>,
    /// Filesystem path of the evidence file that was read. `null` when no file found.
    pub evidence_path: Option<String>,
    /// `true` when evidence is absent, unreadable, malformed, unsafe, or
    /// older than `max_evidence_age_secs`.
    pub stale_or_missing_evidence: bool,
    /// Maximum evidence age (seconds) before `truth_state` becomes `"stale"`.
    /// Default 86400 (24h); overridable via `MQK_LATEST_MARK_EVIDENCE_MAX_AGE_SECS`.
    pub max_evidence_age_secs: i64,
    pub network_call_made: Option<bool>,
    pub db_write: Option<bool>,
    pub md_bars_write: Option<bool>,
    pub completed_bar_claim: Option<bool>,
    pub provider_enabled: Option<bool>,
    pub symbols_requested: Vec<String>,
    pub marks: Vec<LatestMarkStatusRow>,
    pub all_passed: Option<bool>,
    pub reason_code: Option<String>,
    pub fail_reasons: Vec<String>,
    /// Error description when `truth_state` is `"parse_error"`,
    /// `"unsafe_evidence"`, or `"backend_unavailable"`.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01: read-only Kraken
// OHLC ingest/sync evidence status
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/market-data/kraken-ohlc/status`.
///
/// Read-only. Reads the latest `kraken_ohlc_ingest_*.json` (from
/// `mqk md kraken-ohlc-ingest --output-dir`) or `kraken_ohlc_sync_*.json`
/// (from `mqk md kraken-ohlc-sync --output-dir`) evidence file, selected by
/// the epoch-seconds timestamp embedded in the filename. No DB connection,
/// no provider/network call, no CLI execution, no sync/ingest triggered, no
/// daemon runtime start, no trading state mutation, no evidence staged.
///
/// `truth_state` values:
/// - `"active"`             — latest evidence file parsed successfully, passed
///   every safety check, and is fresh.
/// - `"stale"`               — evidence parsed and passed safety checks but is
///   older than `max_evidence_age_secs`, or carries no `produced_at_utc`.
/// - `"no_evidence"`        — no `kraken_ohlc_ingest_*.json` or
///   `kraken_ohlc_sync_*.json` file found in the evidence directory.
/// - `"parse_error"`        — evidence file found but JSON is malformed or has
///   an unsupported `schema_version`.
/// - `"unsafe_evidence"`    — evidence fails a fail-closed safety check (wrong
///   `provider`, an unexplained `network_call_made=true`, a
///   `completed_bar_claim`/execution-like field, an internal
///   `db_write`/`rows_inserted`/`rows_updated` inconsistency, a missing
///   required provenance field, etc. — see `kraken_ohlc_unsafe_reason` in
///   `routes/transport_quality.rs`). Never surfaced as `active` regardless of
///   freshness.
/// - `"backend_unavailable"` — evidence directory or file could not be read.
///
/// Fields not present in the selected evidence file's schema (e.g.
/// `sync_policy` when `latest_mode="ingest"`) are `None`, never fabricated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenOhlcStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub provider: Option<String>,
    /// `"ingest"` or `"sync"` — which command produced the selected evidence file.
    pub latest_mode: Option<String>,
    pub latest_schema_version: Option<String>,
    pub produced_at_utc: Option<String>,
    /// Filesystem path of the evidence file that was read. `null` when no file found.
    pub evidence_path: Option<String>,
    /// `true` when evidence is absent, unreadable, malformed, unsafe, or
    /// older than `max_evidence_age_secs`.
    pub stale_or_missing_evidence: bool,
    /// Maximum evidence age (seconds) before `truth_state` becomes `"stale"`.
    /// Default 86400 (24h); overridable via `MQK_KRAKEN_OHLC_EVIDENCE_MAX_AGE_SECS`.
    pub max_evidence_age_secs: i64,
    pub network_call_made: Option<bool>,
    pub db_write: Option<bool>,
    pub md_bars_write: Option<bool>,
    pub provider_id: Option<String>,
    pub provider_source: Option<String>,
    pub provider_symbol: Option<String>,
    pub ingest_mode: Option<String>,
    /// Only present for `latest_mode="sync"` evidence.
    pub sync_policy: Option<String>,
    pub no_update_existing: Option<bool>,
    pub symbols_requested: Vec<String>,
    pub bars_completed: Option<i64>,
    pub bars_excluded_forming: Option<i64>,
    pub bars_considered_for_sync: Option<i64>,
    pub bars_missing_new: Option<i64>,
    pub bars_existing_candidate: Option<i64>,
    pub rows_changed: Option<i64>,
    pub rows_skipped_unchanged: Option<i64>,
    pub rows_changed_skipped_due_to_no_update_existing: Option<i64>,
    pub rows_inserted: Option<i64>,
    pub rows_updated: Option<i64>,
    pub rows_skipped_if_known: Option<i64>,
    pub latest_existing_end_ts_before: Option<i64>,
    pub latest_completed_start_ts: Option<i64>,
    pub latest_completed_end_ts: Option<i64>,
    pub volume_semantics: Option<String>,
    pub volume_scale: Option<i64>,
    pub all_passed: Option<bool>,
    pub reason_code: Option<String>,
    pub fail_reasons: Vec<String>,
    /// Error description when `truth_state` is `"parse_error"`,
    /// `"unsafe_evidence"`, or `"backend_unavailable"`.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01: read-only
// re-exposure of CRYPTO-REGISTRY-03's crypto-registry-readiness CLI
// classification via `GET /api/v1/market-data/crypto-registry/readiness`.
// ---------------------------------------------------------------------------

/// Per-symbol readiness check, mirroring `mqk-cli`'s
/// `CryptoRegistrySymbolCheck`. `enabled`/`paper_trading_enabled`/
/// `live_trading_enabled` are `None` only when the symbol was not found in
/// the registry at all (distinct from `Some(false)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoRegistrySymbolReadiness {
    pub symbol: String,
    pub found: bool,
    pub asset_class_ok: bool,
    pub kraken_pair: Option<String>,
    pub kraken_result_key: Option<String>,
    pub alias_ok: bool,
    pub enabled: Option<bool>,
    pub paper_trading_enabled: Option<bool>,
    pub live_trading_enabled: Option<bool>,
    pub trading_flags_safe: bool,
    pub passed: bool,
}

/// Fixed, always-true-in-practice safety flags. Present as explicit fields
/// (rather than inferred) so a GUI/operator surface never has to assume what
/// this route did not do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoRegistryReadinessSafety {
    pub no_scheduler: bool,
    pub no_db_connection: bool,
    pub no_network_call: bool,
    pub no_trading_enabled: bool,
    pub no_config_file_mutated: bool,
}

/// Response for `GET /api/v1/market-data/crypto-registry/readiness`.
///
/// `truth_state` values:
/// - `"active"`                  — provider found, both symbols found with
///   complete Kraken aliases, all trading flags `false`.
/// - `"missing_provider"`        — `provider` (`"kraken"`) has no entry in
///   `providers_path`.
/// - `"missing_symbol"`          — a requested symbol is absent from
///   `registry_path`, or is present with the wrong `asset_class`.
/// - `"missing_alias"`           — a symbol is present but missing
///   `kraken_pair`/`kraken_result_key` in `provider_symbols`.
/// - `"unsafe_trading_enabled"`  — a symbol carries
///   `paper_trading_enabled=true` or `live_trading_enabled=true`.
/// - `"unsafe_provider_enabled"` — the provider carries `enabled=true`,
///   treated as unsafe pending an explicit, separate cutover decision
///   (`CRYPTO-REGISTRY-02`).
/// - `"parse_error"`             — `registry_path` or `providers_path`
///   could not be read/parsed.
///
/// `data_readiness_state` is `"data_ready_manual_only"` (not a failure) when
/// all checks pass and every symbol's `enabled` is `false`; `"blocked"` when
/// any check fails; `"production_default"` would require a symbol's
/// `enabled` to be `true`, which no currently-committed fixture does.
/// `trading_readiness_state` is always `"disabled"` and
/// `scheduler_readiness_state` is always `"absent"` -- this route has no
/// branch that could report otherwise, since no trading path or scheduler
/// exists in this repo to query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoRegistryReadinessResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub data_readiness_state: String,
    pub trading_readiness_state: String,
    pub scheduler_readiness_state: String,
    pub provider: String,
    pub provider_enabled: bool,
    pub api_key_required: bool,
    pub provider_asset_classes: Vec<String>,
    pub provider_implementation_status: String,
    pub registry_path: String,
    pub providers_path: String,
    pub symbols: Vec<CryptoRegistrySymbolReadiness>,
    pub all_passed: bool,
    pub reason_code: String,
    pub fail_reasons: Vec<String>,
    pub safety: CryptoRegistryReadinessSafety,
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01: read-only
// re-exposure of CRYPTO-DATA-02B's kraken-scheduler-readiness CLI
// classification via `GET /api/v1/market-data/kraken-scheduler/readiness`.
// ---------------------------------------------------------------------------

/// Per-symbol readiness check, mirroring `mqk-cli`'s
/// `KrakenSchedulerSymbolCheck`. `enabled`/`paper_trading_enabled`/
/// `live_trading_enabled` are `None` only when the symbol was not found in
/// the registry at all (distinct from `Some(false)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenSchedulerSymbolReadiness {
    pub symbol: String,
    pub found: bool,
    pub asset_class_ok: bool,
    pub alias_ok: bool,
    pub enabled: Option<bool>,
    pub paper_trading_enabled: Option<bool>,
    pub live_trading_enabled: Option<bool>,
    pub trading_flags_safe: bool,
}

/// Fixed, always-true-in-practice safety flags. Present as explicit fields
/// (rather than inferred) so a GUI/operator surface never has to assume what
/// this route did not do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenSchedulerReadinessSafety {
    pub no_scheduled_task_registered: bool,
    pub no_daemon_job_added: bool,
    pub no_network_call_made: bool,
    pub no_db_connection: bool,
    pub no_trading_enabled: bool,
    pub no_config_file_mutated: bool,
}

/// Response for `GET /api/v1/market-data/kraken-scheduler/readiness`.
///
/// `truth_state` mirrors `mqk-cli md kraken-scheduler-readiness` exactly:
/// `"active"`, `"policy_missing"`, `"policy_invalid"`, `"registry_unsafe"`,
/// `"provider_unsafe"`, `"trading_flags_unsafe"`,
/// `"scheduler_already_registered"`, `"evidence_unsafe"`, `"parse_error"`,
/// `"backend_unavailable"`.
///
/// `"active"` (`scheduler_readiness_state =
/// "scheduler_ready_manual_registration_blocked"`) never means a scheduler
/// is registered — it means every prerequisite this route can check is
/// satisfied for a future, separately authorized scheduler-registration
/// patch to be considered. This route never opens a DB connection, never
/// calls Kraken or any provider/network endpoint, never runs a CLI
/// subprocess, never registers a scheduler, never mutates any config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenSchedulerReadinessResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub scheduler_readiness_state: String,
    pub rate_limit_policy_state: String,
    pub registry_readiness_state: String,
    pub provider_readiness_state: String,
    pub evidence_readiness_state: String,
    pub provider: String,
    pub provider_enabled: bool,
    pub symbols: Vec<KrakenSchedulerSymbolReadiness>,
    pub recommended_default_cadence: String,
    pub min_seconds_between_pair_calls: i64,
    pub min_seconds_between_scheduled_runs: i64,
    pub max_ohlc_calls_per_run: i64,
    pub max_total_network_calls_per_run: i64,
    pub concurrency: String,
    pub scheduler_registration_status: String,
    pub daemon_job_status: String,
    pub network_call_made: bool,
    pub db_write: bool,
    pub trading_enabled: bool,
    pub policy_path: String,
    pub registry_path: String,
    pub providers_path: String,
    pub all_passed: bool,
    pub reason_code: String,
    pub fail_reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub safety: KrakenSchedulerReadinessSafety,
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01: read-only status
// surface for the CRYPTO-DATA-03B `kraken_ohlc_task_registration.json`
// evidence contract via `GET /api/v1/market-data/kraken-scheduler/task-status`.
// ---------------------------------------------------------------------------

/// The `safety` block embedded inside `kraken_ohlc_task_registration.json`
/// itself (written by `Register-KrakenOhlcSyncTask.ps1`), passed through
/// verbatim. These are the evidence producer's own claims about what it did
/// -- not a route-authored safety assertion. All fields are `Option<bool>`
/// because the route never fabricates a value the evidence file did not
/// itself carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenSchedulerTaskStatusEvidenceSafety {
    pub calls_runner_script_only: Option<bool>,
    pub no_daemon_runtime_broker_provider_order_references: Option<bool>,
    pub no_env_vars_embedded_in_task_action: Option<bool>,
    pub no_env_local_read: Option<bool>,
    pub task_never_started_by_this_script: Option<bool>,
}

/// Response for `GET /api/v1/market-data/kraken-scheduler/task-status`.
///
/// Read-only re-exposure of the `kraken-ohlc-task-registration-v1` evidence
/// file written by `Register-KrakenOhlcSyncTask.ps1`
/// (`CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01`). This route never
/// registers, unregisters, or starts a Windows Scheduled Task; never calls
/// Windows Task Scheduler APIs; never shells out to PowerShell; never calls
/// Kraken or any provider network endpoint; never opens a DB connection.
///
/// `truth_state`: `"active"`, `"no_evidence"`, `"parse_error"`,
/// `"unsafe_evidence"`, `"backend_unavailable"`. `"active"` never means a
/// task is registered -- see `registered`/`task_exists_after` for that
/// distinct, truthfully-surfaced fact. Every field except `canonical_route`,
/// `truth_state`, `symbols`, `env_vars_embedded`, `env_vars_required`,
/// `fail_reasons`, and `warnings` is `Option`/absent when the evidence file
/// itself did not carry that field, or when no evidence could be safely
/// read at all -- never fabricated or defaulted to a "safe-looking" value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrakenSchedulerTaskStatusResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub schema_version: Option<String>,
    pub produced_at_utc: Option<String>,
    pub mode: Option<String>,
    pub task_name: Option<String>,
    pub task_exists_before: Option<bool>,
    pub task_exists_after: Option<bool>,
    pub registered: Option<bool>,
    pub unregistered: Option<bool>,
    pub check_only: Option<bool>,
    pub task_action: Option<String>,
    pub runner_path: Option<String>,
    pub policy_path: Option<String>,
    pub registry_path: Option<String>,
    pub providers_path: Option<String>,
    pub symbols: Vec<String>,
    pub timeframe: Option<String>,
    pub at: Option<String>,
    pub scheduled_task_mutation: Option<bool>,
    pub network_call_made: Option<bool>,
    pub db_write: Option<bool>,
    pub md_bars_write: Option<bool>,
    pub env_vars_embedded: Vec<String>,
    pub env_vars_required: Vec<String>,
    pub all_passed: Option<bool>,
    pub reason_code: Option<String>,
    pub fail_reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub safety: Option<KrakenSchedulerTaskStatusEvidenceSafety>,
    pub evidence_path: Option<String>,
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
    /// Admitted (effective) symbols list, already truncated to
    /// `max_symbols_to_trade` (cap #1) when the artifact requested more
    /// than the cap allows — see `dropped_symbols`
    /// (`MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`).  Empty unless
    /// `status == "loaded_approved"`.
    pub symbols: Vec<String>,
    /// The artifact's originally-requested symbols, before any cap #1
    /// truncation — `symbols ++ dropped_symbols` in that order.  Equal to
    /// `symbols` when nothing was dropped.  Empty unless
    /// `status == "loaded_approved"`.
    pub requested_symbols: Vec<String>,
    /// Symbols dropped because `requested_symbols.len()` exceeded
    /// `max_symbols_to_trade` (cap #1) — the cap responsible for the drop
    /// is `max_symbols_to_trade` above.  Empty when nothing was dropped.
    pub dropped_symbols: Vec<String>,
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

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/account-equity-baseline (PAPER-DAILY-PNL-CAPTURE-01D)
// ---------------------------------------------------------------------------

/// Read-only operator visibility into one captured
/// `sys_account_equity_baseline` row, by `trading_date`.
///
/// `truth_state` distinguishes `"active"` (a real row exists),
/// `"not_found"` (DB queried successfully, no row for that date),
/// `"db_unavailable"` (no DB pool configured), `"invalid_request"` (bad or
/// missing `trading_date`), and `"query_failed"` (DB present but the query
/// itself errored) -- never collapses these into a bare empty payload.
#[derive(Debug, Clone, Serialize)]
pub struct AccountEquityBaselineStatusResponse {
    pub truth_state: String,
    pub trading_date: Option<String>,
    pub equity: Option<f64>,
    pub cash: Option<f64>,
    pub currency: Option<String>,
    pub captured_at_utc: Option<String>,
    pub captured_by: Option<String>,
    pub broker_snapshot_source: Option<String>,
    pub audit_event_id: Option<String>,
    pub message: String,
}

// ---------------------------------------------------------------------------
// STRATEGY-PROMOTION-REGISTRY-01C: strategy paper-promotion control surface
// ---------------------------------------------------------------------------
//
// `registered + enabled` (`sys_strategy_registry.enabled`) is NOT promotion
// approval. Only `current_state == "active_paper"` (and `tradable_paper ==
// true`) means a new paper outbox row is currently allowed for this exact
// identity. `tradable_live` is always `false` on every route in this
// module -- a paper promotion state never authorizes a LIVE run or
// live-routing path.

/// One promotion identity's current (latest) transition, with a computed
/// tradability verdict. Used by both `GET .../promotions` (one row per
/// known identity) and `GET .../promotions/history` (one row per
/// historical transition for one identity).
#[derive(Debug, Clone, Serialize)]
pub struct StrategyPromotionRow {
    pub transition_id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    /// Identity-v1 bounded fallback: always `null` in this patch.
    pub config_fingerprint: Option<String>,
    /// Always `"unavailable_in_current_runtime"` in this patch.
    pub config_identity_status: String,
    pub previous_state: Option<String>,
    pub new_state: String,
    /// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase D): the exact
    /// transition that established the evidence currently authorizing
    /// this identity's state -- resolved via `mqk_db::resolve_evidence_lineage`,
    /// never trusted from a caller. The `evidence_*` fields below are
    /// always the *resolved* evidence (this row's own, if it is itself
    /// evidence-bearing; otherwise the ancestor's), not merely this row's
    /// own possibly-`null` columns.
    pub evidence_transition_id: Option<String>,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_fingerprint: Option<String>,
    pub effective_at_utc: String,
    pub expires_at_utc: Option<String>,
    pub initiated_by: String,
    pub reason: String,
    pub created_at_utc: String,
    /// `true` only when `new_state == "active_paper"`, already effective,
    /// and not expired as of the read time.
    pub tradable_paper: bool,
    /// Always `false`. No code path in this patch can set this `true`.
    pub tradable_live: bool,
    /// Stable machine-readable reason code (see
    /// `mqk_db::PromotionReasonCode`), e.g. `"promotion_active"`,
    /// `"promotion_shadow_only"`, `"promotion_expired"`.
    pub reason_code: String,
    pub blockers: Vec<String>,
}

/// `GET /api/v1/strategy/promotions` -- current (latest) promotion state
/// for every identity that has ever had a transition. Public, read-only.
///
/// `truth_state`:
/// - `"active"` -- DB present and query succeeded; `rows` is authoritative
///   (an empty `rows` means zero identities have ever had a transition,
///   i.e. zero approved strategies -- not "unavailable").
/// - `"no_db"` -- no DB pool configured on this daemon.
/// - `"query_failed"` -- DB present but the query itself errored.
#[derive(Debug, Clone, Serialize)]
pub struct StrategyPromotionsResponse {
    pub canonical_route: String,
    pub backend: String,
    pub truth_state: String,
    pub rows: Vec<StrategyPromotionRow>,
}

/// `GET /api/v1/strategy/promotions/history?strategy_id=&symbol=&timeframe_secs=`
/// -- full transition history for one exact identity, newest first. Public,
/// read-only. Append-only: a later transition never removes or rewrites an
/// earlier row.
///
/// `truth_state`: same vocabulary as [`StrategyPromotionsResponse`], plus
/// `"invalid_request"` when `strategy_id`/`symbol`/`timeframe_secs` are
/// missing or malformed.
#[derive(Debug, Clone, Serialize)]
pub struct StrategyPromotionHistoryResponse {
    pub canonical_route: String,
    pub backend: String,
    pub truth_state: String,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub rows: Vec<StrategyPromotionRow>,
    /// Non-empty only when `truth_state == "invalid_request"`.
    pub blockers: Vec<String>,
}

/// `GET /api/v1/strategy/promotions/check?strategy_id=&symbol=&timeframe_secs=`
/// -- convenience route mirroring exactly what the runtime promotion gate
/// itself would decide for this identity right now (same shared evaluator).
/// Public, read-only.
///
/// `truth_state`: same vocabulary as [`StrategyPromotionsResponse`], plus
/// `"invalid_request"`.
#[derive(Debug, Clone, Serialize)]
pub struct StrategyPromotionCheckResponse {
    pub canonical_route: String,
    pub backend: String,
    pub truth_state: String,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub current_state: Option<String>,
    pub config_identity_status: Option<String>,
    /// See [`StrategyPromotionRow::evidence_transition_id`].
    pub evidence_transition_id: Option<String>,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_fingerprint: Option<String>,
    pub tradable_paper: bool,
    pub tradable_live: bool,
    pub reason_code: String,
    pub blockers: Vec<String>,
}

/// `POST /api/v1/strategy/promotions/transition` -- operator-authenticated
/// promotion state transition. Requires a valid Bearer token (existing
/// `token_auth_middleware`).
///
/// For `target_state` values that require fresh evidence (`shadow_approved`
/// reached from no prior state, or re-approval from `demoted`),
/// `review_dir` is required and is independently canonicalized,
/// root-bounded inside `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT`, and validated
/// against the actual review artifact content -- this route never trusts a
/// caller's claim that a candidate is `paper_candidate`.
///
/// PROMOTION-WALKFORWARD-GATE-WIRING-01: the same evidence-requiring
/// transitions additionally require `research_trial_id`,
/// `research_evidence_dir`, and `research_judge_artifact_path` -- verified,
/// AUTHORITY-anchored Research out-of-sample evidence (P7C) is required
/// ADDITIONALLY to (never instead of) the scanner/review evidence above. See
/// [`crate::research_evidence_gate`].
///
/// `effective_at_utc` (and `expires_at_utc` when present) are caller-
/// injected RFC3339 timestamps -- no `now()` is read on this route.
#[derive(Debug, Clone, Deserialize)]
pub struct StrategyPromotionTransitionRequest {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    /// One of: `shadow_approved`, `paper_approved`, `active_paper`,
    /// `demoted`, `retired`, `rejected`.
    pub target_state: String,
    /// Required when the requested transition needs fresh evidence.
    /// Must resolve inside `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT`.
    pub review_dir: Option<String>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01: required alongside `review_dir`
    /// for evidence-requiring transitions. The Research trial identity this
    /// transition claims to be backed by -- an identity claim only, never
    /// trusted on its own; independently verified against the daemon's
    /// trusted Research registry (`MQK_RESEARCH_REGISTRY_DB`, never this
    /// request) by [`crate::research_evidence_gate::evaluate_research_evidence_gate`].
    pub research_trial_id: Option<String>,
    /// Directory containing `economic_walk_forward.json` and
    /// `economic_daily_returns.csv` for `research_trial_id`. Must resolve
    /// inside `MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT`.
    pub research_evidence_dir: Option<String>,
    /// Path to the multiple-testing judge artifact JSON covering
    /// `research_trial_id`. Must resolve inside
    /// `MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT`.
    pub research_judge_artifact_path: Option<String>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: required
    /// alongside the Research fields above for evidence-requiring
    /// transitions. The candidate identity (a `BacktestReport::run_id`
    /// UUID) whose canonical, durable `BacktestReport`/`ArtifactLock`/
    /// `StressSuiteResult` evidence this transition claims to be backed by
    /// -- an identity claim only, never trusted on its own; independently
    /// resolved and cross-candidate-bound to `strategy_id` by
    /// [`crate::backtest_evidence_gate::evaluate_backtest_evidence_gate`]
    /// against the daemon's trusted artifact root
    /// (`MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT`, never this request).
    pub backtest_run_id: Option<String>,
    pub effective_at_utc: String,
    pub expires_at_utc: Option<String>,
    pub initiated_by: String,
    #[serde(default)]
    pub reason: String,
}

/// Response for `POST /api/v1/strategy/promotions/transition`.
///
/// `disposition`: `"transitioned"` (new row inserted) | `"duplicate"`
/// (identical request replayed; idempotent no-op) | `"illegal_transition"`
/// | `"evidence_invalid"` | `"rejected"` (field validation) |
/// `"transition_conflict"` (STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01:
/// a concurrent transition already advanced this identity past the
/// expected parent state -- retry by re-reading current state) |
/// `"unavailable"` (no DB / query failed).
#[derive(Debug, Clone, Serialize)]
pub struct StrategyPromotionTransitionResponse {
    pub accepted: bool,
    pub disposition: String,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub previous_state: Option<String>,
    pub target_state: String,
    pub transition_id: Option<String>,
    pub blockers: Vec<String>,
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01C-ENFORCEMENT-01: GET /api/v1/market-data/readiness
// ---------------------------------------------------------------------------

/// One resolved assignment's readiness identity + verdict, projected from
/// `mqk_daemon::daily_data_readiness::AssignmentReadiness` for the API
/// surface. See the binding contract
/// `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md` §3c
/// for the identity tuple and evaluation order this mirrors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDataReadinessAssignmentResponse {
    pub assignment_symbol: String,
    pub assignment_timeframe: String,
    pub configured_strategy_id: String,
    pub effective_runtime_strategy_id: Option<String>,
    pub effective_runtime_target_symbol: Option<String>,
    pub effective_runtime_timeframe_secs: Option<i64>,
    pub required_history_bars: Option<usize>,
    pub asset_class: Option<String>,
    pub expected_provider_id: Option<String>,
    pub expected_provider_symbol: Option<String>,
    pub actual_provider_ids: Vec<String>,
    pub actual_provider_symbols: Vec<String>,
    /// Count of `is_complete` rows in the bounded query window. `null` when
    /// the bar stage was never reached or the DB was unavailable/the query
    /// failed — never fabricated as `0`.
    pub loaded_completed_bars: Option<usize>,
    pub expected_latest_bar_ts: Option<i64>,
    pub actual_latest_bar_ts: Option<i64>,
    /// `"ok"` | `"insufficient"` | `"duplicate_detected"` | `"gap_detected"`
    /// | `"unsupported"` | `"unknown"`. Never `"ok"` when a continuity or
    /// history blocker (`insufficient_history`, `duplicate_timestamp`,
    /// `interior_gap`, `expected_latest_bar_missing`,
    /// `unsupported_intraday_continuity`, `calendar_unavailable`,
    /// `market_data_missing`) is present.
    pub continuity_state: String,
    /// `"ok"` | `"invalid"` | `"unknown"`.
    pub provenance_state: String,
    /// `"ready"` | `"blocked"` | `"db_unavailable"` | `"query_failed"`.
    pub readiness_state: String,
    pub blockers: Vec<String>,
    /// One remediation string per `blockers` entry, same order. Never
    /// suggests an ingest-job route for a timeframe the provider registry
    /// does not support.
    pub remediation: Vec<String>,
    pub configured_grace_seconds: i64,
    pub effective_grace_seconds: i64,
    pub configured_future_skew_seconds: i64,
    pub effective_future_skew_seconds: i64,
}

/// Canonical response for `GET /api/v1/market-data/readiness`.
///
/// `system/preflight`, `autonomous/readiness`, and `market-data/ingest-plan`
/// each project an additive summary sourced from this same canonical
/// evaluation (never a separate assignment parser or a simplified readiness
/// evaluator) — all four surfaces must agree on symbols, timeframes,
/// configured strategy IDs, effective runtime binding, expected provider
/// identity, `start_allowed`, and blocking reason codes.
///
/// `binding_scope`:
/// - `"configuration_preview"` — a fresh, current-configuration preview
///   built for this request. Not proof of an active runtime.
/// - `"start_attempt_binding"` — the exact bootstrap/binding pair an actual
///   runtime start attempt evaluated. This route always returns
///   `"configuration_preview"`; `"start_attempt_binding"` only appears in the
///   durable pre-start evidence JSON persisted by the runtime start gate.
///
/// # Safety invariants
/// - Read-only. No DB write, no provider/broker call, no run/outbox/event
///   creation, no scheduler start. Only bounded `md_bars` reads via the
///   shared evaluator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDataReadinessResponse {
    pub canonical_route: String,
    pub schema_version: String,
    pub evaluated_at_utc: String,
    pub binding_scope: String,
    /// `"watchlist_v2"` | `"env_single_symbol_fallback"` | `"none"`.
    pub assignment_source: String,
    /// `"applicable"` | `"not_applicable"` — applicable autonomous US
    /// equity/ETF PAPER operation is
    /// `deployment_mode()==Paper && strategy_market_data_source()==ExternalSignalIngestion`,
    /// never hardcoded to `BrokerKind::Alpaca`.
    pub applicability: String,
    pub start_allowed: bool,
    pub top_level_blocker: Option<String>,
    /// Env-configured grace/skew ceilings (§8/§9). Effective (timeframe-aware)
    /// values are surfaced per assignment, since they can differ by timeframe.
    pub configured_grace_seconds: i64,
    pub configured_future_skew_seconds: i64,
    pub calendar_source: Option<String>,
    /// `"active"` | `"stale"` | `"invalid"` | `"out_of_range"` | `"unknown"` |
    /// `"not_applicable"` (non-applicable deployments; no schedule resolved).
    pub calendar_coverage_state: String,
    /// `"YYYY-MM-DD"` (ET), or `null` when not applicable.
    pub market_date: Option<String>,
    pub session_open_utc: Option<String>,
    pub session_close_utc: Option<String>,
    pub assignments: Vec<DailyDataReadinessAssignmentResponse>,
}
