//! Route handlers for execution transport and market-data quality (Batch A2).
//!
//! Contains: `execution_transport`, `market_data_quality`, `market_data_coverage`,
//!           `intraday_refresh_status`.
//!
//! Both A2 surfaces derive entirely from daemon in-memory state — no DB dependency,
//! no lifecycle lock, no broker snapshot required.  They are always 200 OK;
//! `truth_state` / `overall_health` communicate data availability.
//!
//! `intraday_refresh_status` is read-only filesystem access to the evidence directory.
//! No DB writes, no broker calls, no provider API calls.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;

use crate::api_types::{
    ExecutionTransportResponse, IntradayRefreshStatusResponse, IntradayRefreshSymbolStatus,
    MarketDataQualityResponse, MdBarsCoverageResponse, MdBarsCoverageRow, TransportQueueRow,
};
use crate::state::{AlpacaWsContinuityState, AppState, StrategyMarketDataSource};

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/coverage (DATA-INGEST-GUI-RESULTS-01)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub(crate) struct CoverageParams {
    pub timeframe: Option<String>,
}

/// Read-only view: what md_bars data exists locally, grouped by (symbol, timeframe).
///
/// Public route (no operator auth). Read-only DB query. No broker adapter.
/// No live/paper execution state touched.
///
/// `truth_state`:
/// - `"active"` — DB has rows matching the filter.
/// - `"empty"`  — DB responded but returned zero rows.
/// - `"db_unavailable"` — no DB pool configured.
/// - `"unavailable"` — pool present but query failed.
pub(crate) async fn market_data_coverage(
    Query(params): Query<CoverageParams>,
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let route = "/api/v1/market-data/coverage";

    // Normalize: empty string → None (all timeframes). Echo the normalized value.
    let timeframe_echo: Option<String> = params.timeframe.filter(|s| !s.is_empty());
    let tf: Option<&str> = timeframe_echo.as_deref();

    let pool = match st.db.as_ref() {
        None => {
            return (
                StatusCode::OK,
                Json(MdBarsCoverageResponse {
                    canonical_route: route.to_string(),
                    truth_state: "db_unavailable".to_string(),
                    timeframe: timeframe_echo,
                    rows: vec![],
                    error: Some("database pool not configured".to_string()),
                }),
            )
                .into_response();
        }
        Some(p) => p.clone(),
    };

    match mqk_db::md::fetch_md_bars_coverage(&pool, tf).await {
        Ok(db_rows) if db_rows.is_empty() => (
            StatusCode::OK,
            Json(MdBarsCoverageResponse {
                canonical_route: route.to_string(),
                truth_state: "empty".to_string(),
                timeframe: timeframe_echo,
                rows: vec![],
                error: None,
            }),
        )
            .into_response(),
        Ok(db_rows) => {
            let rows = db_rows
                .into_iter()
                .map(|r| MdBarsCoverageRow {
                    symbol: r.symbol,
                    timeframe: r.timeframe,
                    bars: r.bars,
                    min_end_ts: r.min_end_ts,
                    max_end_ts: r.max_end_ts,
                    latest_ingested_at: r.latest_ingested_at,
                })
                .collect();
            (
                StatusCode::OK,
                Json(MdBarsCoverageResponse {
                    canonical_route: route.to_string(),
                    truth_state: "active".to_string(),
                    timeframe: timeframe_echo,
                    rows,
                    error: None,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(MdBarsCoverageResponse {
                canonical_route: route.to_string(),
                truth_state: "unavailable".to_string(),
                timeframe: timeframe_echo,
                rows: vec![],
                error: Some(format!("coverage query failed: {e}")),
            }),
        )
            .into_response(),
    }
}

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

    let (
        truth_state,
        outbox_depth,
        inbox_depth,
        max_claim_age_ms,
        dispatch_retries,
        orphaned_claims,
        queues,
    ) = match snap {
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

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/intraday-refresh/status
// (INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01)
// ---------------------------------------------------------------------------

const INTRADAY_REFRESH_ROUTE: &str = "/api/v1/market-data/intraday-refresh/status";
const INTRADAY_EVIDENCE_PREFIX: &str = "intraday_refresh_";
const INTRADAY_EVIDENCE_SUFFIX: &str = ".json";
const INTRADAY_EVIDENCE_SCHEMA_VERSION: &str = "intraday-refresh-v1";
/// Evidence older than this is flagged stale.
const INTRADAY_EVIDENCE_STALE_SECS: i64 = 86_400;

/// Parse one symbol object from the evidence JSON.
///
/// All fields are optional — evidence files may omit provider fields (check_only mode).
fn parse_refresh_symbol(v: &serde_json::Value) -> IntradayRefreshSymbolStatus {
    let fail_reasons: Vec<String> = v
        .get("fail_reasons")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    IntradayRefreshSymbolStatus {
        symbol: v
            .get("symbol")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        gate: v
            .get("gate")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        completed_count: v.get("completed_count").and_then(|n| n.as_i64()),
        latest_completed_bar_ts: v
            .get("max_ts_iso")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        staleness_min: v.get("staleness_min").and_then(|n| n.as_i64()),
        provider_source: v
            .get("provider_source")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        provider_configured: v.get("provider_configured").and_then(|b| b.as_bool()),
        provider_attempted: v.get("provider_attempted").and_then(|b| b.as_bool()),
        provider_success: v.get("provider_success").and_then(|b| b.as_bool()),
        rows_inserted: v.get("provider_rows_inserted").and_then(|n| n.as_i64()),
        rows_updated: v.get("provider_rows_updated").and_then(|n| n.as_i64()),
        rows_filtered_incomplete: v
            .get("provider_rows_dropped_incomplete")
            .and_then(|n| n.as_i64()),
        rows_filtered_in_progress: v
            .get("provider_rows_dropped_current")
            .and_then(|n| n.as_i64()),
        fail_reasons,
    }
}

/// Determine whether evidence produced at `produced_at_utc` is stale.
///
/// Returns `true` when the timestamp is absent, unparseable, or older than
/// `INTRADAY_EVIDENCE_STALE_SECS`.
fn is_evidence_stale(produced_at_utc: Option<&str>) -> bool {
    match produced_at_utc {
        None => true,
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Err(_) => true,
            Ok(produced_at) => {
                let age = (Utc::now() - produced_at.with_timezone(&Utc)).num_seconds();
                age > INTRADAY_EVIDENCE_STALE_SECS
            }
        },
    }
}

/// `GET /api/v1/market-data/intraday-refresh/status`
///
/// Read-only. Reads the latest `intraday_refresh_*.json` evidence file from
/// `st.md_refresh_evidence_dir` (env var `MQK_MD_REFRESH_EVIDENCE_DIR`,
/// default `exports/market_data`).
///
/// Safety:
/// - No provider API calls.
/// - No DB reads or writes.
/// - No broker interaction.
/// - No order/OMS/outbox state accessed.
/// - Missing or malformed evidence never panics; surfaces truth_state honestly.
pub(crate) async fn intraday_refresh_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let evidence_dir = st.md_refresh_evidence_dir.clone();
    let dir_path = std::path::Path::new(&evidence_dir);

    let read_dir = match std::fs::read_dir(dir_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(IntradayRefreshStatusResponse {
                    canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
                    truth_state: "backend_unavailable".to_string(),
                    evidence_path: None,
                    stale_or_missing_evidence: true,
                    schema_version: None,
                    produced_at_utc: None,
                    mode: None,
                    source: None,
                    timeframe: None,
                    all_passed: None,
                    reason: None,
                    symbols: vec![],
                    error: Some(format!("evidence directory unreadable: {}", e)),
                }),
            )
                .into_response();
        }
        Ok(rd) => rd,
    };

    let mut candidates: Vec<String> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(INTRADAY_EVIDENCE_PREFIX)
                && name.ends_with(INTRADAY_EVIDENCE_SUFFIX)
            {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    if candidates.is_empty() {
        return (
            StatusCode::OK,
            Json(IntradayRefreshStatusResponse {
                canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
                truth_state: "no_evidence".to_string(),
                evidence_path: None,
                stale_or_missing_evidence: true,
                schema_version: None,
                produced_at_utc: None,
                mode: None,
                source: None,
                timeframe: None,
                all_passed: None,
                reason: None,
                symbols: vec![],
                error: None,
            }),
        )
            .into_response();
    }

    // Alphabetical sort = chronological (timestamp in filename).
    candidates.sort();
    let latest_name = candidates.last().expect("candidates non-empty");
    let latest_path = dir_path.join(latest_name);
    let evidence_path_str = latest_path.to_string_lossy().to_string();

    let content = match std::fs::read_to_string(&latest_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(IntradayRefreshStatusResponse {
                    canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
                    truth_state: "backend_unavailable".to_string(),
                    evidence_path: Some(evidence_path_str),
                    stale_or_missing_evidence: true,
                    schema_version: None,
                    produced_at_utc: None,
                    mode: None,
                    source: None,
                    timeframe: None,
                    all_passed: None,
                    reason: None,
                    symbols: vec![],
                    error: Some(format!("evidence file unreadable: {}", e)),
                }),
            )
                .into_response();
        }
        Ok(c) => c,
    };

    let raw: serde_json::Value = match serde_json::from_str(&content) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(IntradayRefreshStatusResponse {
                    canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
                    truth_state: "parse_error".to_string(),
                    evidence_path: Some(evidence_path_str),
                    stale_or_missing_evidence: true,
                    schema_version: None,
                    produced_at_utc: None,
                    mode: None,
                    source: None,
                    timeframe: None,
                    all_passed: None,
                    reason: None,
                    symbols: vec![],
                    error: Some(format!("evidence JSON parse failed: {}", e)),
                }),
            )
                .into_response();
        }
        Ok(v) => v,
    };

    let schema_version = raw
        .get("schema_version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if schema_version.as_deref() != Some(INTRADAY_EVIDENCE_SCHEMA_VERSION) {
        return (
            StatusCode::OK,
            Json(IntradayRefreshStatusResponse {
                canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
                truth_state: "parse_error".to_string(),
                evidence_path: Some(evidence_path_str),
                stale_or_missing_evidence: true,
                schema_version,
                produced_at_utc: None,
                mode: None,
                source: None,
                timeframe: None,
                all_passed: None,
                reason: None,
                symbols: vec![],
                error: Some(format!(
                    "unsupported schema_version (expected '{}')",
                    INTRADAY_EVIDENCE_SCHEMA_VERSION
                )),
            }),
        )
            .into_response();
    }

    let produced_at_utc = raw
        .get("produced_at_utc")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stale_or_missing_evidence = is_evidence_stale(produced_at_utc.as_deref());

    let symbols: Vec<IntradayRefreshSymbolStatus> = raw
        .get("symbols")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(parse_refresh_symbol).collect())
        .unwrap_or_default();

    (
        StatusCode::OK,
        Json(IntradayRefreshStatusResponse {
            canonical_route: INTRADAY_REFRESH_ROUTE.to_string(),
            truth_state: "active".to_string(),
            evidence_path: Some(evidence_path_str),
            stale_or_missing_evidence,
            schema_version,
            produced_at_utc,
            mode: raw
                .get("mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            source: raw
                .get("source")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            timeframe: raw
                .get("timeframe")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            all_passed: raw.get("all_passed").and_then(|v| v.as_bool()),
            reason: raw
                .get("reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            symbols,
            error: None,
        }),
    )
        .into_response()
}
