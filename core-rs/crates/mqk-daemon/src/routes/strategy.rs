//! Strategy route handlers.
//!
//! Contains: strategy_summary (via summary), strategy_suppressions, strategy_signal.

// MT-07E: strategy summary handler extracted to reduce file size.
mod summary;
pub(crate) use summary::strategy_summary;

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::api_types::{
    StrategySignalRequest, StrategySignalResponse, StrategySuppressionRow,
    StrategySuppressionsResponse,
};
use mqk_integrity::CalendarSpec;

use super::helpers::write_signal_admission_event;
use crate::notify::CriticalAlertPayload;
use crate::state::{AlpacaWsContinuityState, AppState, StrategyBarInput, StrategyMarketDataSource};

// ---------------------------------------------------------------------------
// RTS-07: Outbox provenance mark
// ---------------------------------------------------------------------------

/// Provenance value written into every outbox row sourced from the strategy
/// signal ingestion path.  Carried in `order_json["signal_source"]` and
/// persisted in `oms_outbox.order_json`.
///
/// The orchestrator's Phase 1 dispatch does **not** filter on this field —
/// all outbox rows are claimed and dispatched unconditionally.  The value is
/// metadata for audit and tracing only.
pub(crate) const OUTBOX_SIGNAL_SOURCE: &str = "external_signal_ingestion";

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/suppressions
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_suppressions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategySuppressionsResponse {
                canonical_route: "/api/v1/strategy/suppressions".to_string(),
                backend: "postgres.sys_strategy_suppressions".to_string(),
                truth_state: "no_db".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let records = match mqk_db::fetch_strategy_suppressions(db).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!("fetch_strategy_suppressions failed: {err}");
            return (
                StatusCode::OK,
                Json(StrategySuppressionsResponse {
                    canonical_route: "/api/v1/strategy/suppressions".to_string(),
                    backend: "postgres.sys_strategy_suppressions".to_string(),
                    truth_state: "no_db".to_string(),
                    rows: Vec::new(),
                }),
            )
                .into_response();
        }
    };

    let rows = records
        .into_iter()
        .map(|r| StrategySuppressionRow {
            suppression_id: r.suppression_id.to_string(),
            strategy_id: r.strategy_id,
            state: r.state,
            trigger_domain: r.trigger_domain,
            trigger_reason: r.trigger_reason,
            started_at: r.started_at_utc.to_rfc3339(),
            cleared_at: r.cleared_at_utc.map(|t| t.to_rfc3339()),
            note: r.note,
        })
        .collect();

    (
        StatusCode::OK,
        Json(StrategySuppressionsResponse {
            canonical_route: "/api/v1/strategy/suppressions".to_string(),
            backend: "postgres.sys_strategy_suppressions".to_string(),
            truth_state: "active".to_string(),
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/strategy/signal
// ---------------------------------------------------------------------------
// PT-DAY-01: Strategy-driven broker-backed paper execution entry point.
//
// Accepts strategy signals from external sources (research-py, operator tooling)
// and enqueues them to the execution outbox for dispatch by the orchestrator.
//
// Gate sequence (fail-closed):
//   1.  signal_ingestion_configured — ExternalSignalIngestion must be wired
//   1e. capital_budget              — per-strategy budget authorized (TV-04B)
//   1f. position_sizing             — implied notional within broker/account cap (TV-04C)
//   1b. alpaca_ws_continuity        — must be Live (PT-DAY-02)
//   1c. nyse_session                — must be regular session (PT-DAY-03)
//   1d. day_signal_limit            — per-run intake bound not exceeded (PT-AUTO-02)
//   2.  db_present                  — no DB → 503
//   3.  arm_state == ARMED          — not armed → 403
//   4.  active_run                  — no active run → 409
//   5.  runtime_state == running    — not running → 409
//   6.  strategy_not_suppressed     — active suppression → 409
//   7.  outbox_enqueue              — durable idempotent write
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_signal(
    State(st): State<Arc<AppState>>,
    Json(body): Json<StrategySignalRequest>,
) -> Response {
    let validated = match validate_strategy_signal(body) {
        Ok(v) => v,
        Err((signal_id, strategy_id, blockers)) => {
            return signal_response(
                StatusCode::BAD_REQUEST,
                false,
                "rejected",
                signal_id,
                strategy_id,
                None,
                blockers,
            );
        }
    };

    // Gate 1: signal ingestion must be configured for this deployment.
    if !matches!(
        st.strategy_market_data_source(),
        StrategyMarketDataSource::ExternalSignalIngestion
    ) {
        return signal_response(
            StatusCode::SERVICE_UNAVAILABLE,
            false,
            "unavailable",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec!["strategy signal ingestion is not configured for this deployment".to_string()],
        );
    }

    // Gate 1e: capital allocation policy / per-strategy budget (TV-04B).
    //
    // If MQK_CAPITAL_POLICY_PATH is configured, the strategy must have an
    // explicit budget entry with budget_authorized=true.
    //
    // Placed before Gate 1b (WS continuity) because budget denial is a
    // pure filesystem check and is cheaper than the network-state check.
    // Placing it early also means budget-denied signals never consume
    // continuity or session-time checks.
    //
    // PolicyNotConfigured → no budget enforcement active; pass through.
    // BudgetAuthorized    → strategy has explicit budget authorization; pass.
    // BudgetDenied        → strategy is not capital-authorized; 403 fail-closed.
    // PolicyInvalid       → policy configured but structurally invalid; 503.
    {
        use crate::capital_policy::evaluate_strategy_budget_from_env;
        let budget = evaluate_strategy_budget_from_env(&validated.strategy_id);
        if !budget.is_signal_safe() {
            let (status, disposition, blocker) = match &budget {
                crate::capital_policy::StrategyBudgetOutcome::BudgetDenied { reason } => (
                    StatusCode::FORBIDDEN,
                    "budget_denied",
                    format!("strategy signal refused: {reason}"),
                ),
                crate::capital_policy::StrategyBudgetOutcome::PolicyInvalid { reason } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    format!(
                        "strategy signal unavailable: capital allocation policy \
                         is configured but invalid: {reason}"
                    ),
                ),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "strategy signal unavailable: capital policy evaluation failed".to_string(),
                ),
            };
            return signal_response(
                status,
                false,
                disposition,
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![blocker],
            );
        }
    }

    // Gate 1f: position sizing realism under broker/account limits (TV-04C).
    //
    // Called after Gate 1e (budget authorized). Adds the explicit distinction:
    // budget-authorized ≠ size-executable.
    //
    // The evaluator reads max_position_notional_usd from the strategy's policy
    // entry.  For limit orders the implied notional (qty × limit_price) is
    // checked against the cap.  For market orders (no price reference) the
    // outcome is SizingUnverifiable — passed through honestly, not silently
    // authorized.
    //
    // NotConfigured      → no sizing policy; pass through.
    // NoSizingConstraint → entry has no notional cap; pass through.
    // SizingAuthorized   → within cap; pass.
    // SizingUnverifiable → market order; pass (honest, cannot deny the unmeasured).
    // SizingDenied       → over cap; 403 sizing_denied.
    // PolicyInvalid      → 503 unavailable.
    {
        use crate::capital_policy::evaluate_position_sizing_from_env;
        let sizing = evaluate_position_sizing_from_env(
            &validated.strategy_id,
            validated.qty,
            validated.limit_price,
        );
        if !sizing.is_signal_safe() {
            let (status, disposition, blocker) = match &sizing {
                crate::capital_policy::PositionSizingOutcome::SizingDenied { reason } => (
                    StatusCode::FORBIDDEN,
                    "sizing_denied",
                    format!("strategy signal refused: {reason}"),
                ),
                crate::capital_policy::PositionSizingOutcome::PolicyInvalid { reason } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    format!(
                        "strategy signal unavailable: position sizing evaluation found \
                         invalid policy: {reason}"
                    ),
                ),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "strategy signal unavailable: position sizing evaluation failed".to_string(),
                ),
            };
            return signal_response(
                status,
                false,
                disposition,
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![blocker],
            );
        }
    }

    // Gate 1g: portfolio risk — exposure and capital exhaustion (TV-04E).
    //
    // Called after Gate 1f (sizing authorized). Adds portfolio-level controls:
    //   - Exposure: single-order notional as a fraction of the portfolio cap.
    //   - Exhaustion: order notional vs. capital reserve floor.
    //   - Drift: not measurable at signal time; RiskUnverifiable is a pass-through.
    //
    // Placed before Gate 1b (WS continuity) because portfolio risk is a pure
    // filesystem check and cheaper than the network-state check.
    //
    // NotConfigured      → no policy; pass through.
    // NoRiskConstraints  → entry has no risk fields; pass through.
    // Authorized         → all checks passed.
    // RiskUnverifiable   → market order / drift unverifiable; honest pass.
    // ExposureDenied     → 403 exposure_denied.
    // ExhaustionDenied   → 403 exhaustion_denied.
    // PolicyInvalid      → 503 unavailable.
    {
        use crate::capital_policy::evaluate_portfolio_risk_from_env;
        let risk = evaluate_portfolio_risk_from_env(
            &validated.strategy_id,
            validated.qty,
            validated.limit_price,
        );
        if !risk.is_signal_safe() {
            let (status, disposition, blocker) = match &risk {
                crate::capital_policy::PortfolioRiskOutcome::ExposureDenied { reason } => (
                    StatusCode::FORBIDDEN,
                    "exposure_denied",
                    format!("strategy signal refused: {reason}"),
                ),
                crate::capital_policy::PortfolioRiskOutcome::ExhaustionDenied { reason } => (
                    StatusCode::FORBIDDEN,
                    "exhaustion_denied",
                    format!("strategy signal refused: {reason}"),
                ),
                crate::capital_policy::PortfolioRiskOutcome::PolicyInvalid { reason } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    format!(
                        "strategy signal unavailable: portfolio risk evaluation found \
                         invalid policy: {reason}"
                    ),
                ),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "strategy signal unavailable: portfolio risk evaluation failed".to_string(),
                ),
            };
            return signal_response(
                status,
                false,
                disposition,
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![blocker],
            );
        }
    }

    // Gate 1b: WS continuity must be Live for Alpaca signal ingestion (PT-DAY-02).
    //
    // A GapDetected or ColdStartUnproven state means broker event delivery is
    // unreliable.  Accepting signals in this window would create outbox rows that
    // the orchestrator may dispatch without receiving fills, producing unmonitored
    // positions.  Fail closed: refuse until the WS transport re-establishes Live.
    let ws_continuity = st.alpaca_ws_continuity().await;
    match &ws_continuity {
        AlpacaWsContinuityState::NotApplicable => {}
        AlpacaWsContinuityState::Live { .. } => {}
        AlpacaWsContinuityState::ColdStartUnproven => {
            return signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![
                    "strategy signal refused: Alpaca WS continuity is unproven (cold start); \
                     wait for WS transport to establish Live before submitting signals"
                        .to_string(),
                ],
            );
        }
        AlpacaWsContinuityState::GapDetected { detail, .. } => {
            let msg = format!(
                "strategy signal refused: Alpaca WS continuity gap detected — \
                 broker event delivery is unreliable until WS transport re-establishes Live: \
                 {detail}"
            );
            // PT-DAY-04 / DIS-01: Escalate on the first refusal per gap window.
            //
            // try_claim_gap_escalation() is an atomic swap — exactly one caller
            // receives true even under concurrent signal POSTs.  Subsequent
            // refusals while the gap persists do not re-notify (already done).
            // The flag resets when continuity transitions back to Live.
            //
            // If update_ws_continuity already claimed the escalation at transport
            // level (DIS-01 wire in state.rs), this path is a no-op for this gap
            // window.  If the gap was seeded from a persisted cursor at boot
            // (BRK-07R) without a new update_ws_continuity call, this is the
            // first-escalation path.
            if st.try_claim_gap_escalation() {
                let notifier = st.discord_notifier.clone();
                let env = Some(st.deployment_mode().as_api_label().to_string());
                let signal_detail = format!("signal_refused:{} | {detail}", validated.signal_id);
                let ts = chrono::Utc::now().to_rfc3339(); // allow: ops-metadata escalation timestamp
                tokio::spawn(async move {
                    notifier
                        .notify_critical_alert(&CriticalAlertPayload {
                            alert_class: "paper.ws_continuity.gap_detected".to_string(),
                            severity: "critical".to_string(),
                            summary: "Alpaca WS gap detected; signal refused — fill delivery \
                                      unreliable until WS transport re-establishes Live."
                                .to_string(),
                            detail: Some(signal_detail),
                            environment: env,
                            run_id: None,
                            ts_utc: ts,
                        })
                        .await;
                });
            }
            return signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "continuity_gap",
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![msg],
            );
        }
    }

    // Gate 1c: NYSE session must be regular for strategy signal ingestion (PT-DAY-03).
    //
    // ExternalSignalIngestion is wired to the real Alpaca paper broker, which
    // operates on NYSE market hours.  Signals submitted outside regular session
    // hours (premarket, after-hours, weekends, holidays) would be dispatched by
    // the orchestrator without a live fill feed, creating unmonitored positions
    // that open or are modified unattended.  Fail closed: refuse until the
    // exchange session is regular.
    //
    // Calendar: CalendarSpec::NyseWeekdays is used explicitly here regardless
    // of the daemon's configured calendar_spec (which is AlwaysOn for Paper
    // mode — appropriate for in-process bar-driven paper, not broker-backed
    // paper).  ExternalSignalIngestion is always NYSE-backed via Alpaca.
    let session_ts = st.session_now_ts().await;
    let session = CalendarSpec::NyseWeekdays.classify_market_session(session_ts);
    if session != "regular" {
        return signal_response(
            StatusCode::CONFLICT,
            false,
            "outside_session",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec![format!(
                "strategy signal refused: NYSE market session is '{session}', not 'regular'; \
                 signals are only accepted during regular session hours (09:30–16:00 ET, \
                 NYSE weekdays excluding holidays)"
            )],
        );
    }

    // Gate 1d: per-run autonomous signal intake bound (PT-AUTO-02).
    //
    // Refuses further signals once the per-run counter reaches
    // MAX_AUTONOMOUS_SIGNALS_PER_RUN.  The counter resets at each
    // start_execution_runtime call so the bound is per-run, not per-process.
    //
    // Placed pre-lifecycle-guard so it is pure in-memory and always reachable
    // without DB, even in tests.  409/day_limit_reached tells the signal
    // producer to stop for the remainder of this run.
    if st.day_signal_limit_exceeded() {
        return signal_response(
            StatusCode::CONFLICT,
            false,
            "day_limit_reached",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec![format!(
                "strategy signal refused: autonomous day signal limit reached \
                 ({} signals accepted this run); \
                 no further signals will be accepted until the next run start",
                st.day_signal_count()
            )],
        );
    }

    let _lifecycle = st.lifecycle_guard().await;

    // Gate 2: DB must be present.
    let Some(db) = st.db.as_ref() else {
        return signal_response(
            StatusCode::SERVICE_UNAVAILABLE,
            false,
            "unavailable",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        );
    };

    // Gate 3: arm state must be ARMED.
    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((state, reason))) => (state, reason),
        Ok(None) => {
            return signal_response(
                StatusCode::FORBIDDEN,
                false,
                "rejected",
                validated.signal_id,
                validated.strategy_id,
                None,
                vec!["strategy signal refused: durable arm state is not armed; \
                      fresh systems default to disarmed until explicitly armed"
                    .to_string()],
            );
        }
        Err(err) => {
            return signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![format!(
                    "strategy signal unavailable: arm-state truth could not be loaded: {err}"
                )],
            );
        }
    };

    if durable_arm_state != "ARMED" {
        let blocker = match durable_arm_reason.as_deref() {
            Some("OperatorHalt") => {
                "strategy signal refused: durable arm state is halted".to_string()
            }
            Some(reason) => {
                format!("strategy signal refused: durable arm state is disarmed ({reason})")
            }
            None => "strategy signal refused: durable arm state is not armed".to_string(),
        };
        return signal_response(
            StatusCode::FORBIDDEN,
            false,
            "rejected",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec![blocker],
        );
    }

    // Gates 4+5: active run must exist and be in running state.
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.signal_id,
                validated.strategy_id,
                None,
                vec![err.to_string()],
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return signal_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            validated.signal_id,
            validated.strategy_id,
            None,
            vec!["strategy signal refused: no active durable run is available".to_string()],
        );
    };

    if status.state != "running" {
        let mut blockers = vec![format!(
            "strategy signal refused: runtime state '{}' is not accepting signals",
            status.state
        )];
        if let Some(note) = status.notes {
            blockers.push(note);
        }
        return signal_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            validated.signal_id,
            validated.strategy_id,
            Some(active_run_id),
            blockers,
        );
    }

    // Gate 6: strategy must not be actively suppressed.
    match mqk_db::fetch_strategy_suppressions(db).await {
        Ok(rows) => {
            if let Some(sup) = rows
                .iter()
                .find(|r| r.strategy_id == validated.strategy_id && r.state == "active")
            {
                let blocker = format!(
                    "strategy signal refused: strategy '{}' is suppressed ({}): {}",
                    validated.strategy_id, sup.trigger_domain, sup.trigger_reason
                );
                return signal_response(
                    StatusCode::CONFLICT,
                    false,
                    "suppressed",
                    validated.signal_id,
                    validated.strategy_id,
                    Some(active_run_id),
                    vec![blocker],
                );
            }
        }
        Err(err) => {
            return signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.signal_id,
                validated.strategy_id,
                Some(active_run_id),
                vec![format!(
                    "strategy signal unavailable: suppression check failed: {err}"
                )],
            );
        }
    }

    // B1B: Deposit bar input for the execution loop to dispatch on the next tick.
    //
    // The execution loop is the canonical runtime-owned on_bar dispatch path.
    // This route DEPOSITS the input only; it does NOT directly call on_bar.
    // `AppState::tick_strategy_dispatch` (called from loop_runner.rs) is the
    // authoritative dispatch seam — on_bar fires in the execution loop's tick
    // context, not in this HTTP handler.
    //
    // Fail-closed: if no active bootstrap exists at dispatch time, the loop's
    // tick_strategy_dispatch returns None and no callback is made.
    // B1C will consume the result from tick_strategy_dispatch.
    {
        let now_tick = st.day_signal_count() as u64;
        let end_ts = st.session_now_ts().await;
        st.deposit_strategy_bar_input(StrategyBarInput {
            now_tick,
            end_ts,
            limit_price: validated.limit_price,
            qty: validated.qty,
        })
        .await;
    }

    // Gate 7: enqueue to outbox (idempotent).
    let order_json = validated.order_json();
    match mqk_db::outbox_enqueue(db, active_run_id, &validated.signal_id, order_json).await {
        Ok(true) => {
            // PT-AUTO-02: count only new enqueues; duplicates do not consume quota.
            st.increment_day_signal_count();
            // JOUR-01: Write durable signal-admission audit event (best-effort, non-fatal).
            // The outbox write above is authoritative; this creates a supervision record.
            write_signal_admission_event(
                &st,
                active_run_id,
                &validated.signal_id,
                &validated.strategy_id,
                &validated.symbol,
                &validated.side,
                validated.qty,
            )
            .await;
            signal_response(
                StatusCode::OK,
                true,
                "enqueued",
                validated.signal_id,
                validated.strategy_id,
                Some(active_run_id),
                vec![],
            )
        }
        Ok(false) => {
            let dup_note = format!(
                "signal_id '{}' already exists; no new outbox row was created",
                validated.signal_id
            );
            signal_response(
                StatusCode::OK,
                false,
                "duplicate",
                validated.signal_id,
                validated.strategy_id,
                Some(active_run_id),
                vec![dup_note],
            )
        }
        Err(err) => signal_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
            "unavailable",
            validated.signal_id,
            validated.strategy_id,
            Some(active_run_id),
            vec![format!("outbox enqueue failed: {err}")],
        ),
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ValidatedStrategySignal {
    signal_id: String,
    strategy_id: String,
    symbol: String,
    side: String,
    qty: i64,
    order_type: String,
    time_in_force: String,
    limit_price: Option<i64>,
}

impl ValidatedStrategySignal {
    fn order_json(&self) -> serde_json::Value {
        serde_json::json!({
            "symbol": self.symbol,
            "side": self.side,
            "qty": self.qty,
            "order_type": self.order_type,
            "time_in_force": self.time_in_force,
            "limit_price": self.limit_price,
            "strategy_id": self.strategy_id,
            "signal_source": OUTBOX_SIGNAL_SOURCE,
        })
    }
}

fn validate_strategy_signal(
    body: StrategySignalRequest,
) -> Result<ValidatedStrategySignal, (String, String, Vec<String>)> {
    let signal_id = body.signal_id.trim().to_string();
    let strategy_id = body.strategy_id.trim().to_string();
    let mut blockers = Vec::new();

    if signal_id.is_empty() {
        blockers.push("signal_id is required".to_string());
    }
    if strategy_id.is_empty() {
        blockers.push("strategy_id is required".to_string());
    }

    let symbol = body.symbol.trim().to_string();
    if symbol.is_empty() {
        blockers.push("symbol must not be blank".to_string());
    }

    // B8: Asset-class scope gate.
    //
    // Only "equity" is supported on the canonical execution path.  Options,
    // futures, crypto, and FX are not wired into execution, portfolio, risk,
    // or broker adapter paths.  When `asset_class` is absent, equity is
    // implied (backward-compatible default).  Any explicit non-equity value
    // is rejected fail-closed here rather than allowed to silently reach the
    // outbox.
    if let Some(ac) = body.asset_class.as_deref() {
        let ac_norm = ac.trim().to_ascii_lowercase();
        if ac_norm != "equity" {
            blockers.push(format!(
                "asset_class '{}' is not supported: only 'equity' is supported \
                 on this execution path; options, futures, crypto, and fx are not wired",
                ac.trim()
            ));
        }
    }

    let side = body.side.trim().to_ascii_lowercase();
    if !matches!(side.as_str(), "buy" | "sell") {
        blockers.push("side must be one of: buy, sell".to_string());
    }

    let qty = match parse_signal_integer_field("qty", &body.qty) {
        Ok(v) if v <= 0 => {
            blockers.push("qty must be positive".to_string());
            None
        }
        Ok(v) if v > i32::MAX as i64 => {
            blockers.push("qty is out of range for broker request".to_string());
            None
        }
        Ok(v) => Some(v),
        Err(e) => {
            blockers.push(e);
            None
        }
    };

    let order_type = body
        .order_type
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("market")
        .to_ascii_lowercase();
    if !matches!(order_type.as_str(), "market" | "limit") {
        blockers.push("order_type must be one of: market, limit".to_string());
    }

    let time_in_force = body
        .time_in_force
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("day")
        .to_ascii_lowercase();
    if !matches!(
        time_in_force.as_str(),
        "day" | "gtc" | "ioc" | "fok" | "opg" | "cls"
    ) {
        blockers.push("time_in_force must be one of: day, gtc, ioc, fok, opg, cls".to_string());
    }

    let limit_price = match body.limit_price.as_ref() {
        Some(v) => match parse_signal_integer_field("limit_price", v) {
            Ok(p) if p <= 0 => {
                blockers.push("limit_price must be positive".to_string());
                None
            }
            Ok(p) => Some(p),
            Err(e) => {
                blockers.push(e);
                None
            }
        },
        None => None,
    };

    match order_type.as_str() {
        "market" if body.limit_price.is_some() => {
            blockers.push("market order must not carry limit_price".to_string());
        }
        "limit" if limit_price.is_none() && !blockers.iter().any(|b| b.contains("limit_price")) => {
            blockers.push("limit order must carry limit_price".to_string());
        }
        _ => {}
    }

    if !blockers.is_empty() {
        return Err((signal_id, strategy_id, blockers));
    }

    Ok(ValidatedStrategySignal {
        signal_id,
        strategy_id,
        symbol,
        side,
        qty: qty.expect("validated qty"),
        order_type,
        time_in_force,
        limit_price,
    })
}

fn parse_signal_integer_field(name: &str, value: &serde_json::Value) -> Result<i64, String> {
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| format!("{name} must be an integer without lossy conversion")),
        serde_json::Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{name} must be an integer without lossy conversion")),
        _ => Err(format!("{name} must be an integer-compatible value")),
    }
}

/// RTS-07: Returns `true` when the disposition represents a *new* execution
/// intent placed in the outbox (Gate 7 `Ok(true)`).
///
/// Only the `(accepted=true, disposition="enqueued")` pair qualifies.  All
/// other outcomes — gate failures, duplicates, validation errors — leave the
/// outbox and runtime state unchanged.
fn is_intent_placed(accepted: bool, disposition: &str) -> bool {
    accepted && disposition == "enqueued"
}

fn signal_response(
    status: StatusCode,
    accepted: bool,
    disposition: &str,
    signal_id: String,
    strategy_id: String,
    active_run_id: Option<uuid::Uuid>,
    blockers: Vec<String>,
) -> Response {
    let intent_placed = is_intent_placed(accepted, disposition);
    (
        status,
        Json(StrategySignalResponse {
            accepted,
            disposition: disposition.to_string(),
            signal_id,
            strategy_id,
            active_run_id,
            blockers,
            intent_placed,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RTS-07 U01: `is_intent_placed` is true only for the new-enqueue outcome.
    ///
    /// Proves that the `intent_placed` field in `StrategySignalResponse` is
    /// derived correctly for every outcome the signal route can produce.
    ///
    /// This is the authoritative unit proof for the strategy→intent binding:
    /// Gate 7 `Ok(true)` is the *only* path that sets `intent_placed = true`.
    #[test]
    fn u01_is_intent_placed_true_only_on_new_enqueue() {
        // Sole true case: Gate 7 Ok(true)
        assert!(
            is_intent_placed(true, "enqueued"),
            "Gate-7 Ok(true) must yield intent_placed=true"
        );

        // Duplicate: outbox row already exists; no new intent placed.
        assert!(
            !is_intent_placed(false, "duplicate"),
            "duplicate must not yield intent_placed=true"
        );

        // Gate failure cases
        assert!(!is_intent_placed(false, "rejected"));
        assert!(!is_intent_placed(false, "unavailable"));
        assert!(!is_intent_placed(false, "suppressed"));
        assert!(!is_intent_placed(false, "budget_denied"));
        assert!(!is_intent_placed(false, "sizing_denied"));
        assert!(!is_intent_placed(false, "exposure_denied"));
        assert!(!is_intent_placed(false, "exhaustion_denied"));
        assert!(!is_intent_placed(false, "continuity_gap"));
        assert!(!is_intent_placed(false, "outside_session"));
        assert!(!is_intent_placed(false, "day_limit_reached"));

        // Defensive: accepted=true with any disposition other than "enqueued"
        // must not yield intent_placed=true (no such case is produced today but
        // the derivation must be robust).
        assert!(!is_intent_placed(true, "duplicate"));
        assert!(!is_intent_placed(true, "unavailable"));
    }
}
