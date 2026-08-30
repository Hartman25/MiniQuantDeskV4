//! Strategy route handlers.
//!
//! Contains: strategy_summary (via summary), multi_symbol_dispatch_summary,
//! strategy_suppressions, strategy_signal, strategy_dry_run_status (via
//! dry_run_status).

// MT-07E: strategy summary handler extracted to reduce file size.
mod summary;
pub(crate) use summary::strategy_summary;

// MULTI-STRATEGY-DRY-RUN-STATUS-01: dry-run diagnostics status handler.
mod dry_run_status;
pub(crate) use dry_run_status::strategy_dry_run_status;

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::api_types::{
    MultiSymbolDispatchSummaryResponse, PerSymbolDispatchRow, StrategySignalRequest,
    StrategySignalResponse, StrategySuppressionRow, StrategySuppressionsResponse,
};
use mqk_integrity::CalendarSpec;

use super::helpers::{
    write_signal_admission_event, write_signal_refusal_event, SignalRefusalAudit,
};
use crate::notify::CriticalAlertPayload;
use crate::state::{AlpacaWsContinuityState, AppState, StrategyBarInput, StrategyMarketDataSource};
use chrono::Utc;
use mqk_runtime::observability::ExecutionSnapshot;

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

pub(crate) const MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE: &str =
    "/api/v1/strategy/multi-symbol-dispatch-summary";

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
// GET /api/v1/strategy/multi-symbol-dispatch-summary
// ---------------------------------------------------------------------------

pub(crate) async fn multi_symbol_dispatch_summary(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let response = build_dispatch_summary_response(&st).await;
    (StatusCode::OK, Json(response))
}

pub(crate) async fn build_dispatch_summary_response(
    state: &AppState,
) -> MultiSymbolDispatchSummaryResponse {
    let target_states = state.per_symbol_target_states().await;
    let configured_symbol_count = target_states.len();
    let runtime_execution_mode = match configured_symbol_count {
        0 => "unknown",
        1 => "single_symbol",
        _ => "multi_symbol",
    }
    .to_string();
    let truth_state = if target_states.is_empty() {
        "no_snapshot"
    } else {
        "active"
    }
    .to_string();

    let day_order_limit = state.per_symbol_day_order_limit().await;
    let mut per_symbol = Vec::with_capacity(target_states.len());
    for row in target_states {
        per_symbol.push(PerSymbolDispatchRow {
            day_order_count: state.symbol_day_order_count(&row.symbol).await,
            day_order_limit,
            // Cap #9 has a classification helper, but no trusted current
            // in-memory per-symbol staleness source is wired into this route.
            bar_staleness_secs: None,
            symbol: row.symbol,
            strategy_id: row.strategy_id,
            current_qty: row.current_qty,
            target_qty: row.target_qty,
            delta: row.delta,
            no_order_reason: row.no_order_reason,
            last_decision_id: row.last_decision_id,
            last_decision_disposition: row.last_decision_disposition,
        });
    }

    MultiSymbolDispatchSummaryResponse {
        canonical_route: MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE.to_string(),
        backend: "daemon.runtime_state".to_string(),
        truth_state,
        runtime_execution_mode,
        configured_symbol_count,
        per_symbol,
    }
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
//   1g. portfolio_risk              — exposure and capital exhaustion (TV-04E)
//   1h. event_risk_blackout         — symbol-level static blackout periods (EVENT-RISK-GATE-01)
//   1h2.earnings_calendar           — symbol-level earnings proximity blackout (EVENT-RISK-CALENDAR-01)
//   1i. sector_risk                 — optional per-sector live gross exposure cap (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`); shared with internal Gate 1h (decision.rs) via capital_policy::sector_risk_gate (ETF-RISK-EXTERNAL-SIGNAL-GATE-01)
//   1b. alpaca_ws_continuity        — must be Live (PT-DAY-02)
//   1c. nyse_session                — must be regular session (PT-DAY-03)
//   1d. day_signal_limit            — per-run intake bound not exceeded (PT-AUTO-02)
//   2.  db_present                  — no DB → 503
//   2b. paper_promotion             — STRATEGY-PROMOTION-REGISTRY-01D: exact (strategy_id, symbol, timeframe_secs) identity must be `active_paper`; shared with internal Gate 3b (decision.rs) via promotion_gate::evaluate_paper_promotion_gate
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
            tracing::warn!(
                gate = "validation",
                signal_id = %signal_id,
                strategy_id = %strategy_id,
                disposition = "rejected",
                ?blockers,
                "strategy signal refused"
            );
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
        return refused_signal_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "gate_1_ingestion",
            "unavailable",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![
                    "strategy signal ingestion is not configured for this deployment".to_string(),
                ],
            },
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
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on capital budget denial
            // (high-value: strategy is actively blocked from executing).
            if disposition == "budget_denied" {
                let notifier = st.discord_notifier.clone();
                let env = Some(st.deployment_mode().as_api_label().to_string());
                let blocker_copy = blocker.clone();
                let summary = format!(
                    "signal.blocked [budget_denied] strategy={} symbol={} | {}",
                    validated.strategy_id, validated.symbol, blocker_copy
                );
                let ts = Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            stage: "signal.blocked".to_string(),
                            run_id: None,
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!("gate=gate_1e_budget reason={blocker_copy}")),
                            environment: env,
                            summary,
                            ts_utc: ts,
                        })
                        .await;
                });
            }
            return refused_signal_response(
                status,
                "gate_1e_budget",
                disposition,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
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
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on position sizing denial.
            if disposition == "sizing_denied" {
                let notifier = st.discord_notifier.clone();
                let env = Some(st.deployment_mode().as_api_label().to_string());
                let blocker_copy = blocker.clone();
                let summary = format!(
                    "signal.blocked [sizing_denied] strategy={} symbol={} | {}",
                    validated.strategy_id, validated.symbol, blocker_copy
                );
                let ts = Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            stage: "signal.blocked".to_string(),
                            run_id: None,
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!("gate=gate_1f_sizing reason={blocker_copy}")),
                            environment: env,
                            summary,
                            ts_utc: ts,
                        })
                        .await;
                });
            }
            return refused_signal_response(
                status,
                "gate_1f_sizing",
                disposition,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
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
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on portfolio risk denial
            // (exposure or exhaustion blocked; high-value operator signal).
            if matches!(disposition, "exposure_denied" | "exhaustion_denied") {
                let notifier = st.discord_notifier.clone();
                let env = Some(st.deployment_mode().as_api_label().to_string());
                let blocker_copy = blocker.clone();
                let disp = disposition.to_string();
                let summary = format!(
                    "signal.blocked [{}] strategy={} symbol={} | {}",
                    disp, validated.strategy_id, validated.symbol, blocker_copy
                );
                let ts = Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            stage: "signal.blocked".to_string(),
                            run_id: None,
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!(
                                "gate=gate_1g_risk disposition={disp} reason={blocker_copy}"
                            )),
                            environment: env,
                            summary,
                            ts_utc: ts,
                        })
                        .await;
                });
            }
            return refused_signal_response(
                status,
                "gate_1g_risk",
                disposition,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
            );
        }
    }

    // Gate 1h: event-risk blackout check (EVENT-RISK-GATE-01).
    //
    // `session_ts` is obtained here (Unix epoch seconds from AppState) and reused
    // by Gate 1c's NYSE session check below, avoiding a redundant async call.
    //
    // If MQK_EVENT_RISK_BLACKOUT_PATH is configured, the signal's symbol is checked
    // against operator-declared blackout periods at the current session timestamp.
    // Placed before Gate 1b (WS continuity) because this is a pure filesystem
    // check, cheaper than the network-state check.
    //
    // NotConfigured → no blackout file; pass through.
    // Authorized    → symbol not in any blackout period; pass.
    // Blackout      → symbol in a declared blackout period; 403 fail-closed.
    // Unavailable   → file configured but unreadable/unparseable; 503 fail-closed.
    let session_ts = st.session_now_ts().await;
    {
        use crate::event_risk_blackout::evaluate_event_risk_blackout_from_env;
        let blackout = evaluate_event_risk_blackout_from_env(&validated.symbol, session_ts);
        if !blackout.is_signal_safe() {
            let (status, disposition, blocker) = match &blackout {
                crate::event_risk_blackout::EventRiskBlackoutOutcome::Blackout { note } => (
                    StatusCode::FORBIDDEN,
                    "event_risk_blackout",
                    format!("strategy signal refused: {note}"),
                ),
                crate::event_risk_blackout::EventRiskBlackoutOutcome::Unavailable { reason } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    format!(
                        "strategy signal unavailable: event-risk blackout file is \
                         configured but cannot be read: {reason}"
                    ),
                ),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "strategy signal unavailable: event-risk blackout evaluation failed"
                        .to_string(),
                ),
            };
            return refused_signal_response(
                status,
                "gate_1h_event_risk",
                disposition,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
            );
        }
    }

    // Gate 1h2: earnings-calendar proximity check (EVENT-RISK-CALENDAR-01).
    //
    // If MQK_EARNINGS_CALENDAR_PATH is configured, the signal's symbol is checked
    // against operator-declared earnings events.  The evaluator derives the effective
    // blackout window from each entry's earnings_ts + pre_window_secs + post_window_secs.
    //
    // Distinct from Gate 1h (blackout-v1): this source encodes earnings events with
    // operator-specified proximity windows rather than raw timestamp ranges.  Both gates
    // can be configured simultaneously; either refusal blocks the signal.
    //
    // Reuses session_ts captured at Gate 1h — no additional async call required.
    //
    // NotConfigured    → no calendar file; pass through.
    // Authorized       → symbol not within any earnings proximity window; pass.
    // EarningsBlackout → within declared window; 403 fail-closed.
    // Unavailable      → file configured but unreadable/unparseable; 503 fail-closed.
    {
        use crate::earnings_calendar::evaluate_earnings_calendar_from_env;
        let calendar = evaluate_earnings_calendar_from_env(&validated.symbol, session_ts);
        if !calendar.is_signal_safe() {
            let (status, disposition, blocker) = match &calendar {
                crate::earnings_calendar::EarningsCalendarOutcome::EarningsBlackout { note } => (
                    StatusCode::FORBIDDEN,
                    "event_risk_calendar",
                    format!("strategy signal refused: {note}"),
                ),
                crate::earnings_calendar::EarningsCalendarOutcome::Unavailable { reason } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    format!(
                        "strategy signal unavailable: earnings calendar file is \
                         configured but cannot be read: {reason}"
                    ),
                ),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "strategy signal unavailable: earnings calendar evaluation failed".to_string(),
                ),
            };
            return refused_signal_response(
                status,
                "gate_1h_calendar",
                disposition,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
            );
        }
    }

    // Gate 1i: ETF-RISK-EXTERNAL-SIGNAL-GATE-01 — optional per-sector live
    // gross exposure cap (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`).
    //
    // Shares its registry/snapshot/marks mechanics with the internal
    // decision path's Gate 1h (`decision.rs`) via
    // `capital_policy::sector_risk_gate::evaluate_sector_risk_gate` — one
    // mechanism, two callers, so sector exposure risk cannot drift between
    // an internally-generated order and an externally-submitted signal.
    //
    // Default-off: an unset/empty env var disables this gate entirely; no
    // DB or instrument-registry access occurs for this check in that case,
    // and existing external signal behavior is unchanged.
    //
    // `signed_delta_qty` is derived from `validated.side`/`validated.qty`
    // inside the shared gate — never from a caller-supplied claim — so
    // risk-reducing status is determined the same honest way as the
    // internal path: from real current/prospective weights, not metadata.
    //
    // `sector_limit_exceeded` is a verified breach → 403 (denied). Every
    // other deny outcome (`sector_config_invalid`, `unavailable`,
    // `sector_weights_missing`, `sector_nav_unavailable`) means the gate
    // could not verify safety → 503, fail-closed without claiming a breach
    // that was never actually computed.
    {
        use crate::capital_policy::sector_risk_gate::evaluate_sector_risk_gate;
        let result =
            evaluate_sector_risk_gate(&st, &validated.symbol, &validated.side, validated.qty).await;

        if !result.allowed {
            let status = if result.reason_code == "sector_limit_exceeded" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            let blocker = format!(
                "strategy signal refused: {}",
                result
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("sector risk gate denied ({})", result.reason_code))
            );
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on sector-risk denial —
            // same high-value signal as the other signal.blocked gates above.
            let notifier = st.discord_notifier.clone();
            let env = Some(st.deployment_mode().as_api_label().to_string());
            let symbol_owned = validated.symbol.clone();
            let strategy_owned = validated.strategy_id.clone();
            let blocker_copy = blocker.clone();
            let reason_code_copy = result.reason_code.clone();
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: Some(symbol_owned.clone()),
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!(
                            "gate=external_gate_1h_sector_risk reason={blocker_copy}"
                        )),
                        environment: env,
                        summary: format!(
                            "signal.blocked [{reason_code_copy}] strategy={strategy_owned} \
                             symbol={symbol_owned} | {blocker_copy}"
                        ),
                        ts_utc: Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                    })
                    .await;
            });
            return refused_signal_response(
                status,
                "external_gate_1h_sector_risk",
                &result.reason_code,
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![blocker],
                },
            );
        }
    }

    // Gate 1j: short-entry policy and shortable preflight.
    //
    // The external signal route accepts only buy/sell, so this gate derives
    // direction from current execution snapshot quantity plus the signed order
    // delta. Caller-supplied metadata cannot bypass it. A sell from flat or a
    // sell larger than the current long position is denied before outbox unless
    // the fail-closed short-entry policy and read-only broker shortable
    // preflight both allow the transition.
    if let Err((status, disposition, blocker)) =
        evaluate_external_signal_short_entry_gate(&st, &validated).await
    {
        return refused_signal_response(
            status,
            "external_gate_1j_short_entry",
            &disposition,
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![blocker],
            },
        );
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
            return refused_signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gate_1b_ws_cold_start",
                "unavailable",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![
                        "strategy signal refused: Alpaca WS continuity is unproven (cold start); \
                     wait for WS transport to establish Live before submitting signals"
                            .to_string(),
                    ],
                },
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
            return refused_signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gate_1b_ws_gap",
                "continuity_gap",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![msg],
                },
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
    //
    // session_ts is obtained at Gate 1h above; reused here to avoid a redundant
    // async call.
    let session = CalendarSpec::NyseWeekdays.classify_market_session(session_ts);
    if session != "regular" {
        return refused_signal_response(
            StatusCode::CONFLICT,
            "gate_1c_session",
            "outside_session",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![format!(
                    "strategy signal refused: NYSE market session is '{session}', not 'regular'; \
                 signals are only accepted during regular session hours (09:30–16:00 ET, \
                 NYSE weekdays excluding holidays)"
                )],
            },
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
        // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert once per run when the day
        // signal limit is hit.  Deduped: only the first refusal sends Discord.
        if st.try_claim_day_limit_alert() {
            let notifier = st.discord_notifier.clone();
            let env = Some(st.deployment_mode().as_api_label().to_string());
            let count = st.day_signal_count();
            let ts = Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: None,
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!("gate=gate_1d_day_limit count={count}")),
                        environment: env,
                        summary: format!(
                            "signal.blocked [day_limit_reached] {count} signals accepted \
                             this run; no further signals until next run start"
                        ),
                        ts_utc: ts,
                    })
                    .await;
            });
        }
        return refused_signal_response(
            StatusCode::CONFLICT,
            "gate_1d_day_limit",
            "day_limit_reached",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![format!(
                    "strategy signal refused: autonomous day signal limit reached \
                 ({} signals accepted this run); \
                 no further signals will be accepted until the next run start",
                    st.day_signal_count()
                )],
            },
        );
    }

    let _lifecycle = st.lifecycle_guard().await;

    // Gate 2: DB must be present.
    let Some(db) = st.db.as_ref() else {
        return refused_signal_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "gate_2_db",
            "unavailable",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![
                    "durable execution DB truth is unavailable on this daemon".to_string()
                ],
            },
        );
    };

    // Gate 2b: STRATEGY-PROMOTION-REGISTRY-01D — strategy must be
    // paper-promoted (active_paper) for this exact
    // (strategy_id, symbol, timeframe_secs) identity.
    //
    // Placed immediately after Gate 2 (DB present), before any further
    // durable-state gate, so a promotion refusal never consumes arm-state/
    // run/suppression checks below it. Shared with the internal decision
    // seam (`decision.rs` Gate 3b) via
    // `promotion_gate::evaluate_paper_promotion_gate` — one mechanism, two
    // callers, so promotion enforcement cannot drift between an
    // internally-generated order and an externally-submitted signal.
    {
        let timeframe_secs = match validated.timeframe_secs {
            Some(v) if v > 0 => v,
            _ => {
                return refused_signal_response(
                    StatusCode::FORBIDDEN,
                    "gate_2b_paper_promotion",
                    "promotion_timeframe_unknown",
                    RefusedSignalArgs {
                        signal_id: validated.signal_id,
                        strategy_id: validated.strategy_id,
                        symbol: validated.symbol.clone(),
                        active_run_id: None,
                        blockers: vec![
                            "strategy signal refused: timeframe_secs is required and must be \
                             positive to evaluate paper-promotion identity; this route never \
                             assumes a default timeframe"
                                .to_string(),
                        ],
                    },
                );
            }
        };
        // RUNTIME-PROMOTION-EVIDENCE-BINDING-01 (C2): the external signal
        // carries no live strategy host to query — the only authoritative
        // semantic identity available is re-derived server-side, right now,
        // through the exact same registry-construction seam C1's promotion
        // route and this gate's other caller (decision.rs) ultimately trace
        // back to. Never trusts anything from the signal request itself
        // (there is no such field in `validated` to trust). `Err` (unknown
        // strategy_id, or a real registered timeframe that disagrees with
        // this request's own `timeframe_secs`) means no server-authoritative
        // semantic identity exists for this signal — `None` here can never
        // match any promoted fingerprint, so the gate fails closed.
        let current_semantic_fingerprint =
            crate::strategy_config_identity::resolve_server_semantic_fingerprint(
                &validated.strategy_id,
                &validated.symbol,
                timeframe_secs,
            )
            .ok();
        let promotion = crate::promotion_gate::evaluate_paper_promotion_gate(
            db,
            crate::promotion_gate::PromotionRunMode::from(st.deployment_mode()),
            &validated.strategy_id,
            &validated.symbol,
            timeframe_secs,
            current_semantic_fingerprint.as_deref(),
        )
        .await;
        if !promotion.paper_tradable {
            let status = match promotion.reason_code {
                mqk_db::PromotionReasonCode::PromotionDbUnavailable
                | mqk_db::PromotionReasonCode::PromotionQueryFailed => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                _ => StatusCode::FORBIDDEN,
            };
            return refused_signal_response(
                status,
                "gate_2b_paper_promotion",
                promotion.reason_code.code(),
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![format!("strategy signal refused: {}", promotion.blocker)],
                },
            );
        }
    }

    // Gate 3: arm state must be ARMED.
    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((state, reason))) => (state, reason),
        Ok(None) => {
            return refused_signal_response(
                StatusCode::FORBIDDEN,
                "gate_3_arm",
                "rejected",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec!["strategy signal refused: durable arm state is not armed; \
                      fresh systems default to disarmed until explicitly armed"
                        .to_string()],
                },
            );
        }
        Err(err) => {
            return refused_signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gate_3_arm",
                "unavailable",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![format!(
                        "strategy signal unavailable: arm-state truth could not be loaded: {err}"
                    )],
                },
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
        return refused_signal_response(
            StatusCode::FORBIDDEN,
            "gate_3_arm",
            "rejected",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![blocker],
            },
        );
    }

    // Gates 4+5: active run must exist and be in running state.
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return refused_signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gate_4_snapshot",
                "unavailable",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: None,
                    blockers: vec![err.to_string()],
                },
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return refused_signal_response(
            StatusCode::CONFLICT,
            "gate_4_active_run",
            "unavailable",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: None,
                blockers: vec![
                    "strategy signal refused: no active durable run is available".to_string(),
                ],
            },
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
        write_signal_refusal_event(
            &st,
            active_run_id,
            &SignalRefusalAudit {
                gate: "gate_5_state",
                disposition: "unavailable",
                signal_id: &validated.signal_id,
                strategy_id: &validated.strategy_id,
                symbol: &validated.symbol,
                blockers: &blockers,
            },
        )
        .await;
        return refused_signal_response(
            StatusCode::CONFLICT,
            "gate_5_state",
            "unavailable",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol.clone(),
                active_run_id: Some(active_run_id),
                blockers,
            },
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
                let blockers = vec![blocker.clone()];
                write_signal_refusal_event(
                    &st,
                    active_run_id,
                    &SignalRefusalAudit {
                        gate: "gate_6_suppressed",
                        disposition: "suppressed",
                        signal_id: &validated.signal_id,
                        strategy_id: &validated.strategy_id,
                        symbol: &validated.symbol,
                        blockers: &blockers,
                    },
                )
                .await;
                // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert when a strategy is
                // suppressed.  Suppression is an operator-set condition and requires
                // explicit resolution — surfacing it on Discord helps diagnosis.
                {
                    let notifier = st.discord_notifier.clone();
                    let env = Some(st.deployment_mode().as_api_label().to_string());
                    let run_id_short = format!("{:.8}", active_run_id.to_string());
                    let summary = format!(
                        "signal.blocked [suppressed] strategy={} symbol={} run={} | {}",
                        validated.strategy_id, validated.symbol, run_id_short, blocker
                    );
                    let ts = Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                    tokio::spawn(async move {
                        notifier
                            .notify_trade_event(&crate::notify::TradeEventPayload {
                                stage: "signal.blocked".to_string(),
                                run_id: Some(run_id_short),
                                symbol: None,
                                side: None,
                                qty: None,
                                price_micros: None,
                                order_id: None,
                                detail: Some(format!("gate=gate_6_suppressed reason={blocker}")),
                                environment: env,
                                summary,
                                ts_utc: ts,
                            })
                            .await;
                    });
                }
                return refused_signal_response(
                    StatusCode::CONFLICT,
                    "gate_6_suppressed",
                    "suppressed",
                    RefusedSignalArgs {
                        signal_id: validated.signal_id,
                        strategy_id: validated.strategy_id,
                        symbol: validated.symbol.clone(),
                        active_run_id: Some(active_run_id),
                        blockers,
                    },
                );
            }
        }
        Err(err) => {
            let blockers = vec![format!(
                "strategy signal unavailable: suppression check failed: {err}"
            )];
            write_signal_refusal_event(
                &st,
                active_run_id,
                &SignalRefusalAudit {
                    gate: "gate_6_suppression_check",
                    disposition: "unavailable",
                    signal_id: &validated.signal_id,
                    strategy_id: &validated.strategy_id,
                    symbol: &validated.symbol,
                    blockers: &blockers,
                },
            )
            .await;
            return refused_signal_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gate_6_suppression_check",
                "unavailable",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol.clone(),
                    active_run_id: Some(active_run_id),
                    blockers,
                },
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
    match mqk_db::outbox_enqueue_for_running_run(
        db,
        active_run_id,
        &validated.signal_id,
        order_json,
    )
    .await
    {
        Ok(mqk_db::OutboxEnqueueOutcome::Enqueued) => {
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
            // DISCORD-TRADE-LIFECYCLE-ALERTS-01: signal admitted → outbox queued.
            // Fired after durable outbox write — best-effort, non-fatal.
            {
                let notifier = st.discord_notifier.clone();
                let env = st.deployment_mode().as_db_mode().to_string();
                let symbol = validated.symbol.clone();
                let side = format!("{:?}", validated.side);
                let qty = validated.qty;
                let signal_id = validated.signal_id.clone();
                let run_id_short = format!("{:.8}", active_run_id.to_string());
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            stage: "signal.admitted".to_string(),
                            run_id: Some(run_id_short),
                            symbol: Some(symbol.clone()),
                            side: Some(side.clone()),
                            qty: Some(qty),
                            price_micros: None,
                            order_id: None,
                            detail: Some(signal_id.clone()),
                            environment: Some(env),
                            summary: format!(
                                "signal admitted → outbox queued: {side} {symbol} qty={qty} signal={signal_id}"
                            ),
                            ts_utc: Utc::now().to_rfc3339(),
                        })
                        .await;
                });
            }
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
        Ok(mqk_db::OutboxEnqueueOutcome::Duplicate) => {
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
        Ok(mqk_db::OutboxEnqueueOutcome::RunNotRunning { actual_status }) => {
            refused_signal_response(
                StatusCode::CONFLICT,
                "gate_7_outbox",
                "unavailable",
                RefusedSignalArgs {
                    signal_id: validated.signal_id,
                    strategy_id: validated.strategy_id,
                    symbol: validated.symbol,
                    active_run_id: Some(active_run_id),
                    blockers: vec![format!(
                        "signal refused: durable run status is '{actual_status}', not RUNNING"
                    )],
                },
            )
        }
        Err(err) => refused_signal_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "gate_7_outbox",
            "unavailable",
            RefusedSignalArgs {
                signal_id: validated.signal_id,
                strategy_id: validated.strategy_id,
                symbol: validated.symbol,
                active_run_id: Some(active_run_id),
                blockers: vec![format!("outbox enqueue failed: {err}")],
            },
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
    /// STRATEGY-PROMOTION-REGISTRY-01D: intentionally NOT validated at
    /// Gate 0 (field validation runs before every other gate, including
    /// gates that existing tests exercise independently of promotion).
    /// Presence/positivity is checked at Gate 2b, immediately before it is
    /// used — see the module's gate-sequence doc comment.
    timeframe_secs: Option<i64>,
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
        timeframe_secs: body.timeframe_secs,
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

async fn evaluate_external_signal_short_entry_gate(
    st: &AppState,
    validated: &ValidatedStrategySignal,
) -> Result<(), (StatusCode, String, String)> {
    use crate::capital_policy::{
        classify_order_intent, evaluate_short_entry_policy_with_preflight,
        order_intent_to_short_entry_intent, short_entry_config_from_env, ShortEntryPolicyDecision,
    };

    let signed_delta = match validated.side.as_str() {
        "sell" => -validated.qty,
        "buy" => validated.qty,
        _ => return Ok(()),
    };
    let current_qty = match st.current_execution_snapshot().await {
        Some(snapshot) => current_qty_for_symbol(&snapshot, &validated.symbol).map_err(|err| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "short_entry_policy_invalid".to_string(),
                format!("strategy signal unavailable: {err}"),
            )
        })?,
        None => 0,
    };
    let order_intent = classify_order_intent(current_qty, signed_delta);
    if !order_intent.requires_short_entry_policy() {
        return Ok(());
    }

    let config = short_entry_config_from_env().map_err(|err| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "short_entry_policy_invalid".to_string(),
            format!("strategy signal unavailable: short_entry_policy invalid: {err}"),
        )
    })?;
    let preflight = shortability_preflight_for_policy(st, &validated.symbol);
    let projected_qty = current_qty.saturating_add(signed_delta);
    let projected_short_shares = if projected_qty < 0 {
        Some(projected_qty.saturating_abs())
    } else {
        Some(0)
    };
    let projected_short_notional_usd =
        projected_short_shares.and_then(|shares| match validated.limit_price {
            Some(price_micros) if shares > 0 => {
                Some((shares as f64) * (price_micros as f64) / 1_000_000.0)
            }
            _ => None,
        });

    let decision = evaluate_short_entry_policy_with_preflight(
        &config,
        order_intent_to_short_entry_intent(order_intent),
        matches!(st.deployment_mode(), crate::state::DeploymentMode::Paper),
        &preflight,
        projected_short_shares,
        projected_short_notional_usd,
    );

    match decision {
        ShortEntryPolicyDecision::NotShortOpen
        | ShortEntryPolicyDecision::ShortOpenAllowed { .. } => Ok(()),
        ShortEntryPolicyDecision::ShortOpenBlocked {
            reason_code,
            detail,
        } => {
            let (status, disposition) = short_entry_blocker_status(reason_code);
            Err((
                status,
                disposition.to_string(),
                format!(
                    "strategy signal refused: short-entry gate denied {order_intent:?} \
                     for symbol={} current_qty={} delta={} reason_code={reason_code}: {detail}",
                    validated.symbol, current_qty, signed_delta
                ),
            ))
        }
    }
}

fn current_qty_for_symbol(snapshot: &ExecutionSnapshot, symbol: &str) -> Result<i64, String> {
    let target = symbol.trim().to_ascii_uppercase();
    let mut found: Option<i64> = None;
    for position in &snapshot.portfolio.positions {
        if position.symbol.trim().to_ascii_uppercase() != target {
            continue;
        }
        if found.is_some() {
            return Err(format!(
                "short-entry gate cannot determine position truth: duplicate position rows for symbol '{target}'"
            ));
        }
        found = Some(position.net_qty);
    }
    Ok(found.unwrap_or(0))
}

fn shortability_preflight_for_policy(
    st: &AppState,
    symbol: &str,
) -> crate::capital_policy::ShortabilityPreflightResult {
    use crate::capital_policy::{ShortabilityPreflightResult, ShortabilitySource};
    use crate::state::BrokerAssetShortablePreflightOutcome;

    match st.broker_asset_shortable_preflight(symbol) {
        BrokerAssetShortablePreflightOutcome::Active(asset) => {
            if !asset.tradable {
                ShortabilityPreflightResult::NotTradable {
                    symbol: asset.symbol,
                }
            } else if !asset.shortable {
                ShortabilityPreflightResult::NotShortable {
                    symbol: asset.symbol,
                }
            } else if asset.easy_to_borrow == Some(false) {
                ShortabilityPreflightResult::NotEasyToBorrow {
                    symbol: asset.symbol,
                }
            } else {
                ShortabilityPreflightResult::Confirmed {
                    symbol: asset.symbol,
                    tradable: asset.tradable,
                    shortable: asset.shortable,
                    easy_to_borrow: asset.easy_to_borrow,
                    source: ShortabilitySource::AlpacaAsset,
                }
            }
        }
        BrokerAssetShortablePreflightOutcome::NotConfigured => {
            ShortabilityPreflightResult::Unavailable {
                symbol: symbol.to_string(),
                reason: "shortable preflight fetcher is not configured".to_string(),
            }
        }
        BrokerAssetShortablePreflightOutcome::UnsupportedAdapter => {
            ShortabilityPreflightResult::Unavailable {
                symbol: symbol.to_string(),
                reason: "selected broker adapter does not support shortable preflight".to_string(),
            }
        }
        BrokerAssetShortablePreflightOutcome::SymbolNotFound => {
            ShortabilityPreflightResult::SymbolNotFound {
                symbol: symbol.to_string(),
            }
        }
        BrokerAssetShortablePreflightOutcome::QueryFailed(err) => {
            ShortabilityPreflightResult::Unavailable {
                symbol: symbol.to_string(),
                reason: err,
            }
        }
    }
}

fn short_entry_blocker_status(reason_code: &str) -> (StatusCode, &'static str) {
    match reason_code {
        "short_entries_disabled"
        | "paper_short_entries_disabled"
        | "live_short_entries_blocked" => (StatusCode::FORBIDDEN, "short_entry_disabled"),
        "shortable_check_required" | "shortable_check_unavailable" => (
            StatusCode::SERVICE_UNAVAILABLE,
            "shortable_preflight_unavailable",
        ),
        "shortable_symbol_not_found"
        | "shortable_not_tradable"
        | "shortable_not_shortable"
        | "shortable_not_easy_to_borrow" => (StatusCode::FORBIDDEN, "symbol_not_shortable"),
        "max_short_shares_exceeded" | "max_short_notional_exceeded" => {
            (StatusCode::FORBIDDEN, "short_entry_policy_denied")
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "short_entry_policy_invalid",
        ),
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

/// SIGNAL-REFUSAL-OBS-01: Emit a structured `warn!` log for every gate refusal
/// and delegate to `signal_response` for the HTTP response.
///
/// Every refusal path calls this instead of `signal_response` directly so that
/// all refused signals produce a durable log entry with the gate label,
/// strategy identity, symbol, and blockers — enabling morning-after diagnosis
/// without relying solely on the caller's HTTP response capture.
struct RefusedSignalArgs {
    signal_id: String,
    strategy_id: String,
    symbol: String,
    active_run_id: Option<uuid::Uuid>,
    blockers: Vec<String>,
}

fn refused_signal_response(
    status: StatusCode,
    gate: &str,
    disposition: &str,
    args: RefusedSignalArgs,
) -> Response {
    tracing::warn!(
        gate = gate,
        signal_id = %args.signal_id,
        strategy_id = %args.strategy_id,
        symbol = %args.symbol,
        disposition = disposition,
        blockers = ?args.blockers,
        "strategy signal refused"
    );
    signal_response(
        status,
        false,
        disposition,
        args.signal_id,
        args.strategy_id,
        args.active_run_id,
        args.blockers,
    )
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
