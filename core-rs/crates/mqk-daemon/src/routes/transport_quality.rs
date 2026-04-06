//! Route handlers for execution transport and market-data quality (Batch A2).
//!
//! Contains: `execution_transport`, `market_data_quality`.
//!
//! Both surfaces derive entirely from daemon in-memory state — no DB dependency,
//! no lifecycle lock, no broker snapshot required.  They are always 200 OK;
//! `truth_state` / `overall_health` communicate data availability.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;

use crate::api_types::{ExecutionTransportResponse, MarketDataQualityResponse, TransportQueueRow};
use crate::state::{AlpacaWsContinuityState, AppState, StrategyMarketDataSource};

// ---------------------------------------------------------------------------
// GET /api/v1/execution/transport (A2)
// ---------------------------------------------------------------------------

/// Surface honest execution transport truth derived from the current execution
/// snapshot.
///
/// `truth_state = "no_snapshot"` when no execution loop is active (run not
/// started or daemon freshly booted).  All counts are zero and must NOT be
/// interpreted as authoritative-zero.
///
/// `truth_state = "active"` when an execution snapshot is present.  Counts
/// are authoritative for the current snapshot window.
pub(crate) async fn execution_transport(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.execution_snapshot.read().await.clone();

    let (truth_state, outbox_depth, inbox_depth, max_claim_age_ms, dispatch_retries, orphaned_claims, queues) =
        match snap {
            None => (
                "no_snapshot".to_string(),
                0usize,
                0usize,
                0u64,
                0usize,
                0usize,
                Vec::new(),
            ),
            Some(snapshot) => {
                let now = Utc::now();

                let outbox_depth = snapshot.pending_outbox.len();
                let inbox_depth = snapshot.recent_inbox_events.len();

                let dispatch_retries = snapshot
                    .pending_outbox
                    .iter()
                    .filter(|o| o.status == "FAILED" || o.status == "AMBIGUOUS")
                    .count();

                // Age of the oldest CLAIMED row (held by the orchestrator but not yet
                // dispatched to the broker).  Long claim ages indicate a stalled dispatch loop.
                let max_claim_age_ms = snapshot
                    .pending_outbox
                    .iter()
                    .filter(|o| o.status == "CLAIMED")
                    .filter_map(|o| {
                        o.claimed_at_utc
                            .map(|t| (now - t).num_milliseconds().max(0) as u64)
                    })
                    .max()
                    .unwrap_or(0);

                // CLAIMED rows stale > 30 s without progressing to DISPATCHING/SENT.
                let orphaned_claims = snapshot
                    .pending_outbox
                    .iter()
                    .filter(|o| o.status == "CLAIMED")
                    .filter(|o| {
                        o.claimed_at_utc
                            .map(|t| (now - t).num_seconds() > 30)
                            .unwrap_or(false)
                    })
                    .count();

                let outbox_oldest_age_ms = snapshot
                    .pending_outbox
                    .iter()
                    .map(|o| (now - o.created_at_utc).num_milliseconds().max(0) as u64)
                    .max()
                    .unwrap_or(0);

                let inbox_oldest_unapplied_age_ms = snapshot
                    .recent_inbox_events
                    .iter()
                    .filter(|e| !e.applied)
                    .map(|e| (now - e.received_at_utc).num_milliseconds().max(0) as u64)
                    .max()
                    .unwrap_or(0);

                let unapplied_inbox = snapshot
                    .recent_inbox_events
                    .iter()
                    .filter(|e| !e.applied)
                    .count();

                let outbox_status = if outbox_depth == 0 {
                    "idle"
                } else if dispatch_retries > 0 {
                    "retrying"
                } else {
                    "active"
                };

                let inbox_status = if inbox_depth == 0 {
                    "idle"
                } else if unapplied_inbox > 0 {
                    "pending"
                } else {
                    "applied"
                };

                let queues = vec![
                    TransportQueueRow {
                        queue_id: "outbox".to_string(),
                        direction: "outbox".to_string(),
                        status: outbox_status.to_string(),
                        depth: outbox_depth,
                        oldest_age_ms: outbox_oldest_age_ms,
                        retry_count: dispatch_retries,
                        duplicate_events: 0,
                        orphaned_claims,
                        lag_ms: None,
                        last_activity_at: None,
                        notes: String::new(),
                    },
                    TransportQueueRow {
                        queue_id: "inbox".to_string(),
                        direction: "inbox".to_string(),
                        status: inbox_status.to_string(),
                        depth: inbox_depth,
                        oldest_age_ms: inbox_oldest_unapplied_age_ms,
                        retry_count: 0,
                        duplicate_events: 0,
                        orphaned_claims: 0,
                        lag_ms: None,
                        last_activity_at: None,
                        notes: String::new(),
                    },
                ];

                (
                    "active".to_string(),
                    outbox_depth,
                    inbox_depth,
                    max_claim_age_ms,
                    dispatch_retries,
                    orphaned_claims,
                    queues,
                )
            }
        };

    (
        StatusCode::OK,
        Json(ExecutionTransportResponse {
            canonical_route: "/api/v1/execution/transport".to_string(),
            truth_state,
            outbox_depth,
            inbox_depth,
            max_claim_age_ms,
            dispatch_retries,
            orphaned_claims,
            duplicate_inbox_events: 0,
            queues,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/quality (A2)
// ---------------------------------------------------------------------------

/// Surface honest market-data quality truth for the current daemon configuration.
///
/// Derives entirely from `strategy_market_data_source` (the configured ingestion
/// policy) and `alpaca_ws_continuity` (WS transport health for the paper+alpaca
/// path).
///
/// `truth_state` is always `"active"` — both fields are always present in daemon
/// memory.  Use `overall_health` to distinguish "ok" from "not_configured".
///
/// Counts (`stale_symbol_count`, `missing_bar_count`, etc.) are always 0 — per-
/// symbol quality tracking does not exist in the current implementation.  Setting
/// them to 0 is honest: these metrics are not tracked, not "zero issues confirmed."
pub(crate) async fn market_data_quality(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let source = st.strategy_market_data_source();
    let ws_state = st.alpaca_ws_continuity().await;

    // overall_health is WS-continuity-aware for ExternalSignalIngestion:
    //   Live            → "ok"
    //   ColdStartUnproven / NotApplicable → "warning" (not yet proven safe)
    //   GapDetected     → "critical" (active data gap; not safe to trade)
    let overall_health = match source {
        StrategyMarketDataSource::NotConfigured => "not_configured",
        StrategyMarketDataSource::ExternalSignalIngestion => match &ws_state {
            AlpacaWsContinuityState::Live { .. } => "ok",
            AlpacaWsContinuityState::ColdStartUnproven => "warning",
            AlpacaWsContinuityState::GapDetected { .. } => "critical",
            AlpacaWsContinuityState::NotApplicable => "warning",
        },
    };

    let ws_continuity = match &ws_state {
        AlpacaWsContinuityState::NotApplicable => "not_applicable",
        AlpacaWsContinuityState::ColdStartUnproven => "cold_start_unproven",
        AlpacaWsContinuityState::Live { .. } => "live",
        AlpacaWsContinuityState::GapDetected { .. } => "gap_detected",
    };

    (
        StatusCode::OK,
        Json(MarketDataQualityResponse {
            canonical_route: "/api/v1/market-data/quality".to_string(),
            truth_state: "active".to_string(),
            overall_health: overall_health.to_string(),
            freshness_sla_ms: 0,
            stale_symbol_count: 0,
            missing_bar_count: 0,
            venue_disagreement_count: 0,
            strategy_blocks: 0,
            venues: vec![],
            issues: vec![],
            market_data_source: source.as_health_str().to_string(),
            ws_continuity: ws_continuity.to_string(),
        }),
    )
        .into_response()
}
