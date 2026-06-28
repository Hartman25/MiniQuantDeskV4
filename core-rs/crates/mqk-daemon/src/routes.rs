//! Axum router and all HTTP handlers for mqk-daemon.
//!
//! `build_router` is the single entry point; `main.rs` calls it and attaches
//! middleware layers.  All handlers are `pub(crate)` so the scenario tests in
//! `tests/` can compose the router directly.
//!
//! # Module layout
//!
//! | Module            | Contents                                              |
//! |-------------------|-------------------------------------------------------|
//! | `control`         | Existing control sub-router (unchanged)               |
//! | `helpers`         | Shared pure functions used by multiple route modules  |
//! | `system`          | health, status, preflight, metadata, leadership,      |
//! |                   | session, config-fingerprint, config-diffs             |
//! | `system_artifact` | artifact-intake, run-artifact, parity-evidence,       |
//! |                   | topology (MT-01 split from system)                    |
//! | `control_plane`  | run_start/stop/halt, integrity arm/disarm, ops_action,|
//! |                  | ops_catalog, ops_mode_change_guidance                 |
//! | `execution`      | execution_summary, execution_orders, order submit/cancel |
//! | `portfolio`      | portfolio_summary/positions/orders/fills/live-weights,|
//! |                  | risk                                                  |
//! | `reconcile`      | reconcile_status, reconcile_mismatches                |
//! | `strategy`       | strategy_summary, strategy_suppressions,              |
//! |                   | strategy_dry_run_status                               |
//! | `audit_ops`      | audit_operator_actions, audit_artifacts,              |
//! |                  | ops_operator_timeline                                 |
//! | `oms_metrics`    | oms_overview, metrics_dashboards                      |
//! | `trading`        | /v1/trading/*, diagnostics_snapshot, stream           |
//! | `transport_quality` | execution_transport, market_data_quality           |

pub(crate) mod alerts_events;
pub(crate) mod audit_ops;
pub(crate) mod autonomous_paper_status;
pub(crate) mod backtests;
pub mod control;
pub(crate) mod control_plane;
pub(crate) mod execution;
pub(crate) mod execution_flow;
pub(crate) mod execution_order_analysis;
pub(crate) mod helpers;
pub(crate) mod ingest;
pub(crate) mod oms_metrics;
pub(crate) mod paper_journal;
pub(crate) mod portfolio;
pub(crate) mod reconcile;
pub(crate) mod repair;
pub(crate) mod strategy;
pub(crate) mod system;
pub(crate) mod system_artifact;
pub(crate) mod trading;
pub(crate) mod transport_quality;
pub(crate) mod watchlist;

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use crate::{
    api_types::{GateRefusedResponse, ShortablePreflightResponse},
    state::{AppState, BrokerAssetShortablePreflightOutcome, OperatorAuthMode},
};

// ---------------------------------------------------------------------------
// S7-1: Token auth middleware
// ---------------------------------------------------------------------------

/// Axum middleware that enforces fail-closed operator authentication.
///
/// # RT-03R — Fail-Closed Auth in Production
///
/// The daemon now distinguishes three explicit privileged-route postures:
///
/// - [`OperatorAuthMode::TokenRequired`] — a valid `Authorization: Bearer ...`
///   header is mandatory.
/// - [`OperatorAuthMode::ExplicitDevNoToken`] — a caller intentionally opted
///   into local no-token development; privileged routes are reachable.
/// - [`OperatorAuthMode::MissingTokenFailClosed`] — no trustworthy operator
///   token is configured, so privileged routes are denied explicitly.
///
/// Loopback bind is not treated as sufficient authorization for privileged
/// actions. Missing operator auth now fails closed instead of passing through.
async fn token_auth_middleware(
    State(st): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    match st.operator_auth_mode() {
        OperatorAuthMode::TokenRequired(expected_token) => {
            let provided = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "));

            if provided != Some(expected_token.as_str()) {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(GateRefusedResponse {
                        error: "GATE_REFUSED: valid Bearer token required on operator routes"
                            .to_string(),
                        gate: "operator_token".to_string(),
                    }),
                )
                    .into_response();
            }

            next.run(req).await
        }
        OperatorAuthMode::ExplicitDevNoToken => next.run(req).await,
        OperatorAuthMode::MissingTokenFailClosed => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GateRefusedResponse {
                error: "GATE_REFUSED: operator token missing; privileged routes stay disabled until MQK_OPERATOR_TOKEN is configured or explicit debug-only dev mode is selected"
                    .to_string(),
                gate: "operator_auth_config".to_string(),
            }),
        )
            .into_response(),
    }
}

async fn broker_asset_shortable_preflight(
    State(st): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let normalized = symbol.trim().to_ascii_uppercase();
    let canonical_route = "/api/v1/broker/assets/:symbol/shortable-preflight".to_string();
    if normalized.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ShortablePreflightResponse {
                canonical_route,
                symbol: normalized,
                asset_class: None,
                tradable: None,
                shortable: None,
                marginable: None,
                easy_to_borrow: None,
                truth_state: "query_failed".to_string(),
                source: None,
                message: "symbol path parameter must not be blank".to_string(),
            }),
        );
    }

    let response = match st.broker_asset_shortable_preflight(&normalized) {
        BrokerAssetShortablePreflightOutcome::Active(asset) => ShortablePreflightResponse {
            canonical_route,
            symbol: asset.symbol,
            asset_class: Some(asset.asset_class),
            tradable: Some(asset.tradable),
            shortable: Some(asset.shortable),
            marginable: asset.marginable,
            easy_to_borrow: asset.easy_to_borrow,
            truth_state: "active".to_string(),
            source: Some(asset.source),
            message: "Read-only broker asset shortable preflight.".to_string(),
        },
        BrokerAssetShortablePreflightOutcome::NotConfigured => ShortablePreflightResponse {
            canonical_route,
            symbol: normalized,
            asset_class: Some("equity".to_string()),
            tradable: None,
            shortable: None,
            marginable: None,
            easy_to_borrow: None,
            truth_state: "not_configured".to_string(),
            source: None,
            message: "Read-only broker asset shortable preflight is not configured.".to_string(),
        },
        BrokerAssetShortablePreflightOutcome::UnsupportedAdapter => ShortablePreflightResponse {
            canonical_route,
            symbol: normalized,
            asset_class: Some("equity".to_string()),
            tradable: None,
            shortable: None,
            marginable: None,
            easy_to_borrow: None,
            truth_state: "unsupported_adapter".to_string(),
            source: None,
            message: "Selected broker adapter does not support asset shortable preflight."
                .to_string(),
        },
        BrokerAssetShortablePreflightOutcome::SymbolNotFound => ShortablePreflightResponse {
            canonical_route,
            symbol: normalized,
            asset_class: Some("equity".to_string()),
            tradable: None,
            shortable: None,
            marginable: None,
            easy_to_borrow: None,
            truth_state: "symbol_not_found".to_string(),
            source: None,
            message: "Broker asset registry did not contain the requested symbol.".to_string(),
        },
        BrokerAssetShortablePreflightOutcome::QueryFailed(err) => {
            let truth_state = if err
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("broker_unavailable")
            {
                "broker_unavailable"
            } else {
                "query_failed"
            };
            ShortablePreflightResponse {
                canonical_route,
                symbol: normalized,
                asset_class: Some("equity".to_string()),
                tradable: None,
                shortable: None,
                marginable: None,
                easy_to_borrow: None,
                truth_state: truth_state.to_string(),
                source: None,
                message: format!("Read-only broker asset shortable preflight query failed: {err}"),
            }
        }
    };
    (StatusCode::OK, Json(response))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the complete application router wired to the given shared state.
///
/// # Route classification (S7-1)
///
/// | Category      | Methods                              | Auth required |
/// |---------------|--------------------------------------|---------------|
/// | Telemetry     | GET /v1/health, /v1/status, /v1/stream | No          |
/// | Trading read  | GET /v1/trading/*                    | No            |
/// | Operator      | POST/DELETE /v1/run/*, /v1/integrity/*, /v1/trading/snapshot, /control/* | Yes |
///
/// Operator routes are wrapped in [`token_auth_middleware`]. Telemetry and
/// read routes are on the public sub-router — no middleware is applied.
///
/// Middleware layers (CORS, tracing) are **not** applied here; `main.rs`
/// attaches them after this call so tests can use the bare router.
pub fn build_router(state: Arc<AppState>) -> Router {
    use alerts_events::{
        alert_triage_ack, alerts_active, alerts_triage, create_incident, events_feed, incidents,
        resolve_incident,
    };
    use audit_ops::{audit_artifacts, audit_operator_actions, ops_operator_timeline};
    use autonomous_paper_status::autonomous_paper_status;
    use backtests::{
        backtest_economics_suggestion, backtest_job_status, backtest_job_submit, backtest_jobs_list,
    };
    use control_plane::{
        integrity_arm, integrity_disarm, ops_action, ops_catalog, ops_mode_change_guidance,
        run_halt, run_start, run_stop,
    };
    use execution::{
        execution_fill_quality, execution_order_cancel, execution_order_submit, execution_orders,
        execution_signal_evaluations, execution_summary,
    };
    use execution_flow::execution_flow;
    use execution_order_analysis::{
        execution_event_risk_status, execution_order_causality, execution_order_chart,
        execution_order_replay, execution_order_timeline, execution_order_trace, execution_outbox,
        execution_protection_status, execution_replace_cancel_chains,
    };
    use ingest::{
        ingest_job_cancel, ingest_job_status, ingest_job_submit, ingest_jobs_list,
        market_data_feed_poll_once, market_data_feed_scheduler_start,
        market_data_feed_scheduler_status, market_data_feed_scheduler_stop,
        market_data_feed_status, market_data_ingest_plan, tracked_equities_list,
    };
    use oms_metrics::{metrics_dashboards, oms_overview};
    use paper_journal::paper_journal;
    use portfolio::{
        portfolio_fills, portfolio_live_weights, portfolio_open_orders, portfolio_positions,
        portfolio_summary, risk_denials, risk_summary,
    };
    use reconcile::{reconcile_mismatches, reconcile_status};
    use repair::{
        repair_adopt_broker_position_baseline, repair_halted_run_fill_apply,
        repair_halted_run_fill_plan, repair_halted_run_fill_rest_recovery,
        repair_halted_run_portfolio_snapshot, repair_outbox_ambiguous, repair_ws_gap_fill_recovery,
    };
    use strategy::{
        multi_symbol_dispatch_summary, strategy_dry_run_status, strategy_signal, strategy_summary,
        strategy_suppressions,
    };
    use system::{
        autonomous_readiness, health, status_handler, system_config_diffs,
        system_config_fingerprint, system_instrument_registry_v2_source_status,
        system_instrument_registry_v2_status, system_instrument_sessions_parity,
        system_instrument_sessions_status, system_metadata, system_preflight,
        system_runtime_leadership, system_session, system_status,
    };
    use system_artifact::{
        system_artifact_intake, system_parity_evidence, system_run_artifact, system_topology,
    };
    use trading::{
        diagnostics_snapshot, stream, trading_account, trading_fills, trading_orders,
        trading_positions, trading_snapshot, trading_snapshot_clear, trading_snapshot_set,
    };
    use transport_quality::{
        execution_transport, intraday_refresh_status, market_data_coverage, market_data_quality,
    };
    use watchlist::{watchlist_admission_check, watchlist_status};

    // --- Public (unauthenticated) routes — read-only telemetry & data. ---
    let public = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_handler))
        .route("/v1/stream", get(stream))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/preflight", get(system_preflight))
        .route("/api/v1/system/metadata", get(system_metadata))
        .route(
            "/api/v1/system/runtime-leadership",
            get(system_runtime_leadership),
        )
        // ASSET-CORE-01C: read-only v1->v2 instrument-registry conversion/validation
        // status (public, no auth). No DB, no provider/broker calls, no writes.
        // production_cutover_enabled and trading_uses_v2 are always false.
        .route(
            "/api/v1/system/instrument-registry-v2/status",
            get(system_instrument_registry_v2_status),
        )
        // ASSET-CORE-05B: read-only per-instrument session-profile diagnostics.
        // No DB, provider/broker calls, runtime start, writes, or trading cutover.
        .route(
            "/api/v1/system/instrument-sessions/status",
            get(system_instrument_sessions_status),
        )
        // ASSET-CORE-05C: read-only shadow parity comparing production
        // session truth against the ASSET-CORE-05B per-instrument
        // session-profile seam. No DB, provider/broker calls, runtime start,
        // writes, or trading cutover. production_cutover_enabled,
        // runtime_uses_session_v2, and trading_uses_session_v2 are always
        // false; shadow_only is always true.
        .route(
            "/api/v1/system/instrument-sessions/parity",
            get(system_instrument_sessions_parity),
        )
        // ASSET-CORE-01D: read-only status for the *separate* v2 registry source
        // configured via MQK_INSTRUMENT_REGISTRY_V2_PATH (public, no auth). No DB,
        // no provider/broker calls, no writes. used_for_trading and the
        // enabled_for_live_trading/enabled_for_paper_trading flags are always false.
        .route(
            "/api/v1/system/instrument-registry-v2-source/status",
            get(system_instrument_registry_v2_source_status),
        )
        .route("/api/v1/execution/summary", get(execution_summary))
        .route("/api/v1/execution/orders", get(execution_orders))
        .route("/api/v1/execution/outbox", get(execution_outbox))
        .route("/api/v1/execution/flow", get(execution_flow))
        .route(
            "/api/v1/execution/fill-quality",
            get(execution_fill_quality),
        )
        .route(
            "/api/v1/execution/signal-evaluations",
            get(execution_signal_evaluations),
        )
        .route(
            "/api/v1/execution/orders/:order_id/timeline",
            get(execution_order_timeline),
        )
        .route(
            "/api/v1/execution/orders/:order_id/trace",
            get(execution_order_trace),
        )
        .route(
            "/api/v1/execution/orders/:order_id/replay",
            get(execution_order_replay),
        )
        .route(
            "/api/v1/execution/orders/:order_id/chart",
            get(execution_order_chart),
        )
        .route(
            "/api/v1/execution/orders/:order_id/causality",
            get(execution_order_causality),
        )
        .route("/api/v1/portfolio/summary", get(portfolio_summary))
        .route("/api/v1/portfolio/positions", get(portfolio_positions))
        .route("/api/v1/portfolio/orders/open", get(portfolio_open_orders))
        .route("/api/v1/portfolio/fills", get(portfolio_fills))
        .route(
            "/api/v1/portfolio/live-weights",
            get(portfolio_live_weights),
        )
        .route("/api/v1/risk/summary", get(risk_summary))
        .route("/api/v1/risk/denials", get(risk_denials))
        .route("/api/v1/reconcile/status", get(reconcile_status))
        .route("/api/v1/reconcile/mismatches", get(reconcile_mismatches))
        .route("/api/v1/system/topology", get(system_topology))
        .route("/api/v1/system/session", get(system_session))
        .route(
            "/api/v1/system/config-fingerprint",
            get(system_config_fingerprint),
        )
        .route("/api/v1/system/config-diffs", get(system_config_diffs))
        .route("/api/v1/strategy/summary", get(strategy_summary))
        .route(
            "/api/v1/broker/assets/:symbol/shortable-preflight",
            get(broker_asset_shortable_preflight),
        )
        .route("/api/v1/strategy/suppressions", get(strategy_suppressions))
        .route(
            "/api/v1/strategy/multi-symbol-dispatch-summary",
            get(multi_symbol_dispatch_summary),
        )
        // MULTI-STRATEGY-DRY-RUN-STATUS-01: read-only dry-run diagnostics (public, no auth).
        // No broker calls, no DB mutations, no orders. submitted is always false.
        .route(
            "/api/v1/strategy/dry-run/status",
            get(strategy_dry_run_status),
        )
        .route(
            "/api/v1/audit/operator-actions",
            get(audit_operator_actions),
        )
        .route("/api/v1/audit/artifacts", get(audit_artifacts))
        .route("/api/v1/ops/operator-timeline", get(ops_operator_timeline))
        .route("/api/v1/ops/catalog", get(ops_catalog))
        .route(
            "/api/v1/ops/mode-change-guidance",
            get(ops_mode_change_guidance),
        )
        .route("/api/v1/alerts/active", get(alerts_active))
        .route("/api/v1/alerts/triage", get(alerts_triage))
        .route("/api/v1/incidents", get(incidents))
        .route("/api/v1/events/feed", get(events_feed))
        .route("/api/v1/paper/journal", get(paper_journal))
        .route("/api/v1/oms/overview", get(oms_overview))
        .route("/api/v1/metrics/dashboards", get(metrics_dashboards))
        .route(
            "/api/v1/system/artifact-intake",
            get(system_artifact_intake),
        )
        .route("/api/v1/system/run-artifact", get(system_run_artifact))
        .route(
            "/api/v1/system/parity-evidence",
            get(system_parity_evidence),
        )
        .route("/api/v1/autonomous/readiness", get(autonomous_readiness))
        // PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01: read-only paper status summary.
        // No broker calls, no DB mutations, no orders. fail-soft on missing subsystems.
        .route(
            "/api/v1/autonomous/paper-status",
            get(autonomous_paper_status),
        )
        .route(
            "/api/v1/execution/replace-cancel-chains",
            get(execution_replace_cancel_chains),
        )
        .route(
            "/api/v1/execution/protection-status",
            get(execution_protection_status),
        )
        .route(
            "/api/v1/execution/event-risk-status",
            get(execution_event_risk_status),
        )
        .route("/api/v1/execution/transport", get(execution_transport))
        .route("/api/v1/market-data/quality", get(market_data_quality))
        // DATA-INGEST-GUI-RESULTS-01: read-only md_bars coverage (public, no auth)
        .route("/api/v1/market-data/coverage", get(market_data_coverage))
        // INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01: read-only refresher evidence (public, no auth)
        .route(
            "/api/v1/market-data/intraday-refresh/status",
            get(intraday_refresh_status),
        )
        .route(
            "/api/v1/market-data/feed/status",
            get(market_data_feed_status),
        )
        .route(
            "/api/v1/market-data/feed/scheduler/status",
            get(market_data_feed_scheduler_status),
        )
        // WATCHLIST-INGEST-PLAN-01: read-only required-symbol ingest plan (public, no auth)
        // No DB, no provider/broker calls. Reuses the PREMARKET-DATA-READINESS-GATE-01 resolver.
        .route(
            "/api/v1/market-data/ingest-plan",
            get(market_data_ingest_plan),
        )
        .route("/v1/trading/account", get(trading_account))
        .route("/v1/trading/positions", get(trading_positions))
        .route("/v1/trading/orders", get(trading_orders))
        .route("/v1/trading/fills", get(trading_fills))
        .route("/v1/trading/snapshot", get(trading_snapshot))
        .route("/v1/diagnostics/snapshot", get(diagnostics_snapshot))
        // BACKTEST-DAEMON-JOBS-01: read-only backtest job status (public, no auth)
        .route("/api/v1/backtests/jobs", get(backtest_jobs_list))
        .route("/api/v1/backtests/jobs/:job_id", get(backtest_job_status))
        .route(
            "/api/v1/backtests/economics-suggestion",
            get(backtest_economics_suggestion),
        )
        // DATA-INGEST-DAEMON-JOBS-01: read-only ingest job status (public, no auth)
        .route("/api/v1/ingest/jobs", get(ingest_jobs_list))
        .route("/api/v1/ingest/jobs/:job_id", get(ingest_job_status))
        // DATA-INGEST-GUI-SYNC-ALL-01: read-only tracked-equity registry preview (public, no auth)
        // No provider calls. No DB writes. No execution state touched.
        .route(
            "/api/v1/ingest/tracked-equities",
            get(tracked_equities_list),
        )
        // PAPER-HANDOFF-READONLY-01: read-only watchlist artifact status (public, no auth).
        // No broker calls, no DB mutations, no orders. approved_for_live always false.
        .route("/api/v1/watchlist/status", get(watchlist_status))
        // PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01: dry-run admission check (public, no auth).
        // Pure read-only. No broker, no DB, no orders, no outbox/inbox writes.
        // approved_for_live always false. note always "dry_run_only_not_enforced".
        // NOT wired into POST /api/v1/strategy/signal — enforcement deferred to PAPER-HANDOFF-ENFORCE-01.
        .route(
            "/api/v1/watchlist/admission-check",
            get(watchlist_admission_check),
        );

    // --- Operator (authenticated) routes — mutating state changes. ---
    let operator = Router::new()
        .route("/v1/run/start", post(run_start))
        .route("/v1/run/stop", post(run_stop))
        .route("/v1/run/halt", post(run_halt))
        .route("/v1/integrity/arm", post(integrity_arm))
        .route("/v1/integrity/disarm", post(integrity_disarm))
        .route("/api/v1/execution/orders", post(execution_order_submit))
        .route(
            "/api/v1/execution/orders/:order_id/cancel",
            post(execution_order_cancel),
        )
        .route("/api/v1/ops/action", post(ops_action))
        .route(
            "/api/v1/ops/repair/outbox-ambiguous",
            post(repair_outbox_ambiguous),
        )
        .route(
            "/api/v1/ops/repair/halted-run-fill-plan",
            get(repair_halted_run_fill_plan),
        )
        .route(
            "/api/v1/ops/repair/halted-run-fill-apply",
            post(repair_halted_run_fill_apply),
        )
        .route(
            "/api/v1/ops/repair/halted-run-fill-rest-recovery",
            post(repair_halted_run_fill_rest_recovery),
        )
        .route(
            "/api/v1/ops/repair/halted-run-portfolio-snapshot",
            post(repair_halted_run_portfolio_snapshot),
        )
        .route(
            "/api/v1/ops/repair/ws-gap-fill-recovery",
            post(repair_ws_gap_fill_recovery),
        )
        .route(
            "/api/v1/ops/repair/adopt-broker-position-baseline",
            post(repair_adopt_broker_position_baseline),
        )
        .route("/api/v1/alerts/triage/ack", post(alert_triage_ack))
        .route("/api/v1/incidents", post(create_incident))
        .route("/api/v1/incidents/:id/resolve", post(resolve_incident))
        .route("/api/v1/strategy/signal", post(strategy_signal))
        .route("/v1/trading/snapshot", post(trading_snapshot_set))
        .route(
            "/v1/trading/snapshot",
            axum::routing::delete(trading_snapshot_clear),
        )
        // BACKTEST-DAEMON-JOBS-01: submit backtest job (operator, requires auth)
        .route("/api/v1/backtests/jobs", post(backtest_job_submit))
        // DATA-INGEST-DAEMON-JOBS-01: submit ingest job (operator, requires auth)
        // Route identity: market-data ingestion / research-data-control.
        // Not trading execution. Does not require arm_state.
        .route("/api/v1/ingest/jobs", post(ingest_job_submit))
        .route(
            "/api/v1/ingest/jobs/:job_id/cancel",
            post(ingest_job_cancel),
        )
        .route(
            "/api/v1/market-data/feed/poll-once",
            post(market_data_feed_poll_once),
        )
        .route(
            "/api/v1/market-data/feed/scheduler/start",
            post(market_data_feed_scheduler_start),
        )
        .route(
            "/api/v1/market-data/feed/scheduler/stop",
            post(market_data_feed_scheduler_stop),
        )
        .merge(control::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            token_auth_middleware,
        ));

    Router::new()
        .merge(public)
        .merge(operator)
        .with_state(state)
}
