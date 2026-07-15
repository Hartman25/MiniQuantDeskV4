//! DAILY-DATA-READINESS-01C-ENFORCEMENT-01: `GET /api/v1/market-data/readiness`.
//!
//! Binding contract:
//! `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md`.
//!
//! Read-only projection of `crate::daily_data_readiness`'s strict evaluator.
//! [`compute_daily_data_readiness_response`] is the single canonical
//! composition path this route, `system/preflight`, `autonomous/readiness`,
//! and `market-data/ingest-plan` all call — never a separate assignment
//! parser or a simplified readiness evaluator (§C.4).
//!
//! # Safety invariants
//! - Read-only. No provider call, no broker call, no ingest job, no
//!   scheduler start, no runtime start, no arm, no run/outbox row, no
//!   readiness event persisted. Only bounded `md_bars` reads via the shared
//!   evaluator.
//! - Always labels `binding_scope = "configuration_preview"` — never proof of
//!   an active runtime.

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Utc};

use crate::api_types::{DailyDataReadinessAssignmentResponse, DailyDataReadinessResponse};
use crate::daily_data_readiness::{self, AssignmentReadiness};
use crate::state::market_calendar::{resolve_market_session_schedule, CalendarCoverageState};
use crate::state::{AppState, DeploymentMode, MultiSymbolConfigSource, StrategyMarketDataSource};

pub(crate) const MARKET_DATA_READINESS_ROUTE: &str = "/api/v1/market-data/readiness";

fn coverage_state_str(state: CalendarCoverageState) -> &'static str {
    match state {
        CalendarCoverageState::Active => "active",
        CalendarCoverageState::Stale => "stale",
        CalendarCoverageState::Invalid => "invalid",
        CalendarCoverageState::OutOfRange => "out_of_range",
        CalendarCoverageState::Unknown => "unknown",
    }
}

fn assignment_source_str(source: &MultiSymbolConfigSource) -> &'static str {
    match source {
        MultiSymbolConfigSource::EnvSingleSymbolFallback => "env_single_symbol_fallback",
        MultiSymbolConfigSource::WatchlistArtifactV2 { .. } => "watchlist_v2",
    }
}

fn format_market_date(date: (i64, i64, i64)) -> String {
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

/// Project one production [`AssignmentReadiness`] onto the API response
/// shape. Pure, lossless except for `&'static str` -> `String`.
pub(crate) fn assignment_to_response(
    a: &AssignmentReadiness,
) -> DailyDataReadinessAssignmentResponse {
    DailyDataReadinessAssignmentResponse {
        assignment_symbol: a.assignment_symbol.clone(),
        assignment_timeframe: a.assignment_timeframe.clone(),
        configured_strategy_id: a.configured_strategy_id.clone(),
        effective_runtime_strategy_id: a.effective_runtime_strategy_id.clone(),
        effective_runtime_target_symbol: a.effective_runtime_target_symbol.clone(),
        effective_runtime_timeframe_secs: a.effective_runtime_timeframe_secs,
        required_history_bars: a.required_history_bars,
        asset_class: a.asset_class.clone(),
        expected_provider_id: a.expected_provider_id.clone(),
        expected_provider_symbol: a.expected_provider_symbol.clone(),
        actual_provider_ids: a.actual_provider_ids.clone(),
        actual_provider_symbols: a.actual_provider_symbols.clone(),
        loaded_completed_bars: a.loaded_completed_bars,
        expected_latest_bar_ts: a.expected_latest_bar_ts,
        actual_latest_bar_ts: a.actual_latest_bar_ts,
        continuity_state: a.continuity_state.to_string(),
        provenance_state: a.provenance_state.to_string(),
        readiness_state: a.readiness_state.to_string(),
        blockers: a.blockers.iter().map(|b| b.to_string()).collect(),
        remediation: a.remediation.clone(),
        configured_grace_seconds: a.configured_grace_seconds,
        effective_grace_seconds: a.effective_grace_seconds,
        configured_future_skew_seconds: a.configured_future_skew_seconds,
        effective_future_skew_seconds: a.effective_future_skew_seconds,
    }
}

/// `true` iff the deployment is applicable autonomous US equity/ETF PAPER
/// operation (§C.5) — `deployment_mode()==Paper &&
/// strategy_market_data_source()==ExternalSignalIngestion`, never hardcoded
/// to `BrokerKind::Alpaca`. The same predicate the runtime start gate and
/// the pre-existing `PREMARKET-DATA-READINESS-GATE-01` legacy gate use.
pub(crate) fn is_applicable_autonomous_paper(st: &AppState) -> bool {
    st.deployment_mode() == DeploymentMode::Paper
        && st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion
}

/// The single canonical composition path (§C.1/§C.4): resolve the canonical
/// assignment source, build a fresh `configuration_preview` effective runtime
/// binding, load the shared provider/instrument/calendar/strategy-registry
/// context, evaluate strict readiness, and project the result onto the API
/// response shape. Every read-only surface (`market-data/readiness`,
/// `system/preflight`, `autonomous/readiness`, `market-data/ingest-plan`)
/// calls this — never a separate assignment parser or a simplified
/// evaluator, so they can never disagree.
///
/// Read-only: no provider/broker call, no DB write, no run/outbox/event
/// creation. `db` is threaded through only for the bounded `md_bars` reads
/// the shared evaluator itself performs.
pub(crate) async fn compute_daily_data_readiness_response(
    st: &AppState,
    now_utc: DateTime<Utc>,
) -> DailyDataReadinessResponse {
    let configured_grace_seconds = daily_data_readiness::configured_grace_seconds_from_env();
    let configured_future_skew_seconds =
        daily_data_readiness::configured_future_skew_seconds_from_env();

    if !is_applicable_autonomous_paper(st) {
        return DailyDataReadinessResponse {
            canonical_route: MARKET_DATA_READINESS_ROUTE.to_string(),
            schema_version: daily_data_readiness::READINESS_RESPONSE_SCHEMA_VERSION.to_string(),
            evaluated_at_utc: now_utc.to_rfc3339(),
            binding_scope: daily_data_readiness::BINDING_SCOPE_CONFIGURATION_PREVIEW.to_string(),
            assignment_source: "none".to_string(),
            applicability: "not_applicable".to_string(),
            start_allowed: false,
            top_level_blocker: Some("not_applicable".to_string()),
            configured_grace_seconds,
            configured_future_skew_seconds,
            calendar_source: None,
            calendar_coverage_state: "not_applicable".to_string(),
            market_date: None,
            session_open_utc: None,
            session_close_utc: None,
            assignments: vec![],
        };
    }

    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(cfg) => cfg,
        Err(_) => {
            return DailyDataReadinessResponse {
                canonical_route: MARKET_DATA_READINESS_ROUTE.to_string(),
                schema_version: daily_data_readiness::READINESS_RESPONSE_SCHEMA_VERSION.to_string(),
                evaluated_at_utc: now_utc.to_rfc3339(),
                binding_scope: daily_data_readiness::BINDING_SCOPE_CONFIGURATION_PREVIEW
                    .to_string(),
                assignment_source: "none".to_string(),
                applicability: "applicable".to_string(),
                start_allowed: false,
                top_level_blocker: Some(
                    daily_data_readiness::REASON_REQUIRED_ASSIGNMENTS_MISSING.to_string(),
                ),
                configured_grace_seconds,
                configured_future_skew_seconds,
                calendar_source: None,
                calendar_coverage_state: "unknown".to_string(),
                market_date: None,
                session_open_utc: None,
                session_close_utc: None,
                assignments: vec![],
            };
        }
    };

    let fleet_ids = daily_data_readiness::fleet_ids_from_env();
    let (_bootstrap, binding) =
        mqk_runtime::native_strategy::bootstrap_with_effective_binding(fleet_ids.as_deref());

    let context = daily_data_readiness::load_readiness_context_from_env();
    let schedule = resolve_market_session_schedule(context.calendar_provider.as_ref(), now_utc);

    let report = daily_data_readiness::evaluate_readiness_with_binding(
        st.db.as_ref(),
        &config,
        &binding,
        &context,
        now_utc,
    )
    .await;

    DailyDataReadinessResponse {
        canonical_route: MARKET_DATA_READINESS_ROUTE.to_string(),
        schema_version: daily_data_readiness::READINESS_RESPONSE_SCHEMA_VERSION.to_string(),
        evaluated_at_utc: now_utc.to_rfc3339(),
        binding_scope: daily_data_readiness::BINDING_SCOPE_CONFIGURATION_PREVIEW.to_string(),
        assignment_source: assignment_source_str(&config.source).to_string(),
        applicability: "applicable".to_string(),
        start_allowed: report.start_allowed,
        top_level_blocker: report.top_level_blocker.map(|b| b.to_string()),
        configured_grace_seconds,
        configured_future_skew_seconds,
        calendar_source: Some(schedule.calendar_source.to_string()),
        calendar_coverage_state: coverage_state_str(schedule.coverage_state).to_string(),
        market_date: Some(format_market_date(schedule.market_date)),
        session_open_utc: Some(schedule.session_open_utc.to_rfc3339()),
        session_close_utc: Some(schedule.session_close_utc.to_rfc3339()),
        assignments: report
            .assignments
            .iter()
            .map(assignment_to_response)
            .collect(),
    }
}

/// `GET /api/v1/market-data/readiness` — read-only, public (no auth), mirrors
/// the `market-data/ingest-plan` route's auth posture.
pub(crate) async fn market_data_readiness_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let now_utc = Utc::now();
    Json(compute_daily_data_readiness_response(&st, now_utc).await)
}
