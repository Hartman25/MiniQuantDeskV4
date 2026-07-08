//! Route handlers for execution transport and market-data quality (Batch A2).
//!
//! Contains: `execution_transport`, `market_data_quality`, `market_data_coverage`,
//!           `intraday_refresh_status`, `latest_mark_status`.
//!
//! Both A2 surfaces derive entirely from daemon in-memory state — no DB dependency,
//! no lifecycle lock, no broker snapshot required.  They are always 200 OK;
//! `truth_state` / `overall_health` communicate data availability.
//!
//! `intraday_refresh_status` and `latest_mark_status` are read-only filesystem
//! access to the evidence directory. No DB writes, no broker calls, no
//! provider API calls.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;

use crate::api_types::{
    CryptoRegistryReadinessResponse, CryptoRegistryReadinessSafety, CryptoRegistrySymbolReadiness,
    ExecutionTransportResponse, IntradayRefreshStatusResponse, IntradayRefreshSymbolStatus,
    KrakenOhlcStatusResponse, KrakenSchedulerReadinessResponse, KrakenSchedulerReadinessSafety,
    KrakenSchedulerSymbolReadiness, KrakenSchedulerTaskStatusEvidenceSafety,
    KrakenSchedulerTaskStatusResponse, LatestMarkStatusResponse, LatestMarkStatusRow,
    MarketDataQualityResponse, MdBarsCoverageResponse, MdBarsCoverageRow, TransportQueueRow,
};
use crate::market_data_freshness::{
    default_max_allowed_age_secs_for_timeframe, is_intraday_timeframe,
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

const REASON_PROVIDER_RETURNED_STALE_INTRADAY_DATA: &str = "provider_returned_stale_intraday_data";
const REASON_LATEST_BAR_STALE_AFTER_REFRESH: &str = "latest_bar_stale_after_refresh";
const REASON_LATEST_COMPLETED_BAR_MISSING: &str = "latest_completed_bar_missing";
const REASON_PROVIDER_RETURNED_NO_ROWS: &str = "provider_returned_no_rows";
const REASON_PROVIDER_ERROR: &str = "provider_error";
const REASON_FRESH_AFTER_REFRESH: &str = "fresh_after_refresh";
const REASON_REFRESH_FAILED: &str = "refresh_failed";

fn evidence_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|n| n.as_i64())
}

fn evidence_string(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Parse one symbol object from the evidence JSON and compute a conservative
/// route-level post-refresh verdict from durable evidence fields.
///
/// All fields are optional — evidence files may omit provider fields (check_only mode).
fn parse_refresh_symbol(
    v: &serde_json::Value,
    evidence_timeframe: Option<&str>,
) -> IntradayRefreshSymbolStatus {
    let mut fail_reasons: Vec<String> = v
        .get("fail_reasons")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let row_timeframe = evidence_string(v, "timeframe")
        .or_else(|| evidence_timeframe.map(|s| s.to_string()))
        .unwrap_or_default();
    let max_allowed_age_secs = evidence_i64(v, "max_allowed_age_secs")
        .or_else(|| evidence_i64(v, "max_allowed_age_seconds"))
        .or_else(|| {
            (!row_timeframe.is_empty())
                .then(|| default_max_allowed_age_secs_for_timeframe(&row_timeframe))
        });
    let latest_completed_bar_age_secs = evidence_i64(v, "latest_completed_bar_age_secs")
        .or_else(|| evidence_i64(v, "latest_completed_bar_age_seconds"))
        .or_else(|| evidence_i64(v, "staleness_min").and_then(|m| (m >= 0).then_some(m * 60)));
    let provider_success = v.get("provider_success").and_then(|b| b.as_bool());
    let rows_read = evidence_i64(v, "provider_rows_read").or_else(|| evidence_i64(v, "rows_read"));
    let rows_ok = evidence_i64(v, "provider_rows_ok").or_else(|| evidence_i64(v, "rows_ok"));
    let completed_count = evidence_i64(v, "completed_count");
    let gate = evidence_string(v, "gate");

    let mut freshness_truth_state = evidence_string(v, "freshness_truth_state");
    let mut reason_code = evidence_string(v, "reason_code");
    let mut passed = v
        .get("passed")
        .and_then(|b| b.as_bool())
        .unwrap_or_else(|| gate.as_deref() != Some("FAIL") && fail_reasons.is_empty());

    let derived_failure = if provider_success == Some(false) {
        Some((REASON_PROVIDER_ERROR, REASON_PROVIDER_ERROR))
    } else if provider_success == Some(true) && rows_read == Some(0) {
        Some((
            REASON_PROVIDER_RETURNED_NO_ROWS,
            REASON_PROVIDER_RETURNED_NO_ROWS,
        ))
    } else if completed_count == Some(0) {
        Some((
            REASON_LATEST_COMPLETED_BAR_MISSING,
            REASON_LATEST_COMPLETED_BAR_MISSING,
        ))
    } else if let (Some(age), Some(max_age)) = (latest_completed_bar_age_secs, max_allowed_age_secs)
    {
        if age > max_age {
            if is_intraday_timeframe(&row_timeframe) && provider_success == Some(true) {
                Some((
                    "stale_after_refresh",
                    REASON_PROVIDER_RETURNED_STALE_INTRADAY_DATA,
                ))
            } else {
                Some(("stale_after_refresh", REASON_LATEST_BAR_STALE_AFTER_REFRESH))
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some((state, reason)) = derived_failure {
        passed = false;
        freshness_truth_state = Some(state.to_string());
        reason_code = Some(reason.to_string());
        if !fail_reasons.iter().any(|r| r == reason) {
            fail_reasons.push(reason.to_string());
        }
    } else if passed && freshness_truth_state.is_none() && reason_code.is_none() {
        freshness_truth_state = Some(REASON_FRESH_AFTER_REFRESH.to_string());
        reason_code = Some(REASON_FRESH_AFTER_REFRESH.to_string());
    } else if !passed {
        if freshness_truth_state.is_none() {
            freshness_truth_state = Some(REASON_REFRESH_FAILED.to_string());
        }
        if reason_code.is_none() {
            reason_code = Some(REASON_REFRESH_FAILED.to_string());
        }
    }

    IntradayRefreshSymbolStatus {
        symbol: v
            .get("symbol")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        gate,
        completed_count,
        latest_completed_bar_ts: v
            .get("latest_completed_bar_ts")
            .or_else(|| v.get("max_ts_iso"))
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        staleness_min: evidence_i64(v, "staleness_min"),
        provider_source: evidence_string(v, "provider_source")
            .or_else(|| evidence_string(v, "provider")),
        provider_configured: v.get("provider_configured").and_then(|b| b.as_bool()),
        provider_attempted: v.get("provider_attempted").and_then(|b| b.as_bool()),
        provider_success,
        rows_read,
        rows_ok,
        rows_inserted: evidence_i64(v, "provider_rows_inserted")
            .or_else(|| evidence_i64(v, "rows_inserted")),
        rows_updated: evidence_i64(v, "provider_rows_updated")
            .or_else(|| evidence_i64(v, "rows_updated")),
        rows_filtered_incomplete: v
            .get("provider_rows_dropped_incomplete")
            .and_then(|n| n.as_i64()),
        rows_filtered_in_progress: v
            .get("provider_rows_dropped_current")
            .and_then(|n| n.as_i64()),
        latest_completed_bar_age_secs,
        max_allowed_age_secs,
        freshness_truth_state,
        reason_code,
        passed,
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

    let evidence_timeframe = raw.get("timeframe").and_then(|v| v.as_str());
    let symbols: Vec<IntradayRefreshSymbolStatus> = raw
        .get("symbols")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|symbol| parse_refresh_symbol(symbol, evidence_timeframe))
                .collect()
        })
        .unwrap_or_default();
    let evidence_all_passed = raw.get("all_passed").and_then(|v| v.as_bool());
    let all_passed =
        evidence_all_passed.map(|passed| passed && symbols.iter().all(|symbol| symbol.passed));

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
            all_passed,
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

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/latest-marks/status
// (CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED)
// ---------------------------------------------------------------------------

const LATEST_MARK_ROUTE: &str = "/api/v1/market-data/latest-marks/status";
const LATEST_MARK_EVIDENCE_PREFIX: &str = "coinlore_latest_mark_";
const LATEST_MARK_EVIDENCE_SUFFIX: &str = ".json";
const LATEST_MARK_SCHEMA_VERSION: &str = "coinlore-latest-mark-v1";
/// Default max evidence age (seconds) before `truth_state` becomes `"stale"`.
const LATEST_MARK_DEFAULT_MAX_AGE_SECS: i64 = 86_400;
const ENV_LATEST_MARK_MAX_AGE_SECS: &str = "MQK_LATEST_MARK_EVIDENCE_MAX_AGE_SECS";

/// Bar-like field names a latest-mark evidence file (or any mark within it)
/// must never carry. Presence of any of these on a mark means the evidence
/// is claiming (or could be mistaken for claiming) a completed OHLCV bar,
/// which this route must refuse to surface as active.
const FORBIDDEN_MARK_FIELDS: [&str; 6] = ["open", "high", "low", "close", "is_complete", "end_ts"];

fn latest_mark_max_age_secs() -> i64 {
    std::env::var(ENV_LATEST_MARK_MAX_AGE_SECS)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(LATEST_MARK_DEFAULT_MAX_AGE_SECS)
}

/// Build a `LatestMarkStatusResponse` for a non-`active` outcome (missing,
/// unreadable, malformed, or unsafe evidence). `stale_or_missing_evidence`
/// is always `true` for these outcomes.
fn latest_mark_response_for_non_active(
    truth_state: &str,
    max_evidence_age_secs: i64,
    evidence_path: Option<String>,
    error: Option<String>,
) -> LatestMarkStatusResponse {
    LatestMarkStatusResponse {
        canonical_route: LATEST_MARK_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        provider: None,
        produced_at_utc: None,
        evidence_path,
        stale_or_missing_evidence: true,
        max_evidence_age_secs,
        network_call_made: None,
        db_write: None,
        md_bars_write: None,
        completed_bar_claim: None,
        provider_enabled: None,
        symbols_requested: vec![],
        marks: vec![],
        all_passed: None,
        reason_code: None,
        fail_reasons: vec![],
        error,
    }
}

fn latest_mark_row_from_value(v: &serde_json::Value) -> LatestMarkStatusRow {
    LatestMarkStatusRow {
        canonical_symbol: v
            .get("canonical_symbol")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        provider_id: v
            .get("provider_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        provider_symbol: v
            .get("provider_symbol")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        provider_coin_id: v
            .get("provider_coin_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        price_usd: v
            .get("price_usd")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        volume24_usd: v
            .get("volume24_usd")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        as_of_client_request_ts: v.get("as_of_client_request_ts").and_then(|x| x.as_i64()),
        provider_ts: v.get("provider_ts").and_then(|x| x.as_i64()),
        truth_state: v
            .get("truth_state")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        kind: v
            .get("kind")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    }
}

fn value_string_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/v1/market-data/latest-marks/status`
///
/// Read-only. Reads the latest `coinlore_latest_mark_*.json` evidence file
/// from `st.md_refresh_evidence_dir` (env var `MQK_MD_REFRESH_EVIDENCE_DIR`,
/// default `exports/market_data` — the same directory
/// `intraday_refresh_status` reads, filtered to a distinct filename prefix).
///
/// Safety:
/// - No provider/network API calls.
/// - No DB reads or writes.
/// - No broker interaction.
/// - No order/OMS/outbox state accessed.
/// - No CLI execution, no daemon runtime start.
/// - Missing or malformed evidence never panics; surfaces truth_state honestly.
/// - Evidence claiming `db_write`/`md_bars_write`/`completed_bar_claim=true`,
///   or a mark carrying a bar-like field, is surfaced as `"unsafe_evidence"`,
///   never `"active"` — this is a fail-closed check independent of the
///   CLI's own invariants, so a hand-edited or future-buggy evidence file
///   cannot make this route lie about what it is looking at.
pub(crate) async fn latest_mark_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let max_age_secs = latest_mark_max_age_secs();
    let evidence_dir = st.md_refresh_evidence_dir.clone();
    let dir_path = std::path::Path::new(&evidence_dir);

    let read_dir = match std::fs::read_dir(dir_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(latest_mark_response_for_non_active(
                    "backend_unavailable",
                    max_age_secs,
                    None,
                    Some(format!("evidence directory unreadable: {e}")),
                )),
            )
                .into_response();
        }
        Ok(rd) => rd,
    };

    let mut candidates: Vec<String> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(LATEST_MARK_EVIDENCE_PREFIX)
                && name.ends_with(LATEST_MARK_EVIDENCE_SUFFIX)
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
            Json(latest_mark_response_for_non_active(
                "no_evidence",
                max_age_secs,
                None,
                None,
            )),
        )
            .into_response();
    }

    // Alphabetical sort = chronological (epoch-seconds timestamp in filename).
    candidates.sort();
    let latest_name = candidates.last().expect("candidates non-empty");
    let latest_path = dir_path.join(latest_name);
    let evidence_path_str = latest_path.to_string_lossy().to_string();

    let content = match std::fs::read_to_string(&latest_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(latest_mark_response_for_non_active(
                    "backend_unavailable",
                    max_age_secs,
                    Some(evidence_path_str),
                    Some(format!("evidence file unreadable: {e}")),
                )),
            )
                .into_response();
        }
        Ok(c) => c,
    };

    let raw: serde_json::Value = match serde_json::from_str(&content) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(latest_mark_response_for_non_active(
                    "parse_error",
                    max_age_secs,
                    Some(evidence_path_str),
                    Some(format!("evidence JSON parse failed: {e}")),
                )),
            )
                .into_response();
        }
        Ok(v) => v,
    };

    let schema_version = raw.get("schema_version").and_then(|v| v.as_str());
    if schema_version != Some(LATEST_MARK_SCHEMA_VERSION) {
        return (
            StatusCode::OK,
            Json(latest_mark_response_for_non_active(
                "parse_error",
                max_age_secs,
                Some(evidence_path_str),
                Some(format!(
                    "unsupported schema_version (expected '{LATEST_MARK_SCHEMA_VERSION}')"
                )),
            )),
        )
            .into_response();
    }

    // Fail-closed safety check: evidence must not claim a DB/md_bars write or
    // a completed-bar status, regardless of what the CLI that produced it
    // promises to do -- this route does not trust the producer, it verifies.
    let claims_db_write = raw
        .get("db_write")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let claims_md_bars_write = raw
        .get("md_bars_write")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let claims_completed_bar = raw
        .get("completed_bar_claim")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if claims_db_write || claims_md_bars_write || claims_completed_bar {
        return (
            StatusCode::OK,
            Json(latest_mark_response_for_non_active(
                "unsafe_evidence",
                max_age_secs,
                Some(evidence_path_str),
                Some(
                    "evidence claims a db_write/md_bars_write/completed_bar_claim; refusing to surface as active"
                        .to_string(),
                ),
            )),
        )
            .into_response();
    }

    let marks_raw: Vec<serde_json::Value> = raw
        .get("marks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for mark in &marks_raw {
        if let Some(obj) = mark.as_object() {
            for forbidden in FORBIDDEN_MARK_FIELDS {
                if obj.contains_key(forbidden) {
                    return (
                        StatusCode::OK,
                        Json(latest_mark_response_for_non_active(
                            "unsafe_evidence",
                            max_age_secs,
                            Some(evidence_path_str),
                            Some(format!(
                                "mark contains forbidden bar-like field '{forbidden}'"
                            )),
                        )),
                    )
                        .into_response();
                }
            }
        }
    }

    let produced_at_utc = raw
        .get("produced_at_utc")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stale_or_missing_evidence = match &produced_at_utc {
        None => true,
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Err(_) => true,
            Ok(produced_at) => {
                (Utc::now() - produced_at.with_timezone(&Utc)).num_seconds() > max_age_secs
            }
        },
    };

    let marks: Vec<LatestMarkStatusRow> =
        marks_raw.iter().map(latest_mark_row_from_value).collect();
    let symbols_requested = value_string_array(&raw, "symbols_requested");
    let fail_reasons = value_string_array(&raw, "fail_reasons");

    let truth_state = if stale_or_missing_evidence {
        "stale"
    } else {
        "active"
    };

    (
        StatusCode::OK,
        Json(LatestMarkStatusResponse {
            canonical_route: LATEST_MARK_ROUTE.to_string(),
            truth_state: truth_state.to_string(),
            provider: raw
                .get("provider")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            produced_at_utc,
            evidence_path: Some(evidence_path_str),
            stale_or_missing_evidence,
            max_evidence_age_secs: max_age_secs,
            network_call_made: raw.get("network_call_made").and_then(|v| v.as_bool()),
            db_write: Some(claims_db_write),
            md_bars_write: Some(claims_md_bars_write),
            completed_bar_claim: Some(claims_completed_bar),
            provider_enabled: raw.get("provider_enabled").and_then(|v| v.as_bool()),
            symbols_requested,
            marks,
            all_passed: raw.get("all_passed").and_then(|v| v.as_bool()),
            reason_code: raw
                .get("reason_code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            fail_reasons,
            error: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/kraken-ohlc/status
// (CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01)
// ---------------------------------------------------------------------------

const KRAKEN_OHLC_ROUTE: &str = "/api/v1/market-data/kraken-ohlc/status";
const KRAKEN_OHLC_INGEST_PREFIX: &str = "kraken_ohlc_ingest_";
const KRAKEN_OHLC_SYNC_PREFIX: &str = "kraken_ohlc_sync_";
const KRAKEN_OHLC_EVIDENCE_SUFFIX: &str = ".json";
const KRAKEN_OHLC_INGEST_SCHEMA_VERSION: &str = "kraken-ohlc-ingest-v1";
const KRAKEN_OHLC_SYNC_SCHEMA_VERSION: &str = "kraken-ohlc-sync-v2";
/// Default max evidence age (seconds) before `truth_state` becomes `"stale"`.
const KRAKEN_OHLC_DEFAULT_MAX_AGE_SECS: i64 = 86_400;
const ENV_KRAKEN_OHLC_MAX_AGE_SECS: &str = "MQK_KRAKEN_OHLC_EVIDENCE_MAX_AGE_SECS";

/// Top-level evidence fields whose presence would imply this route is
/// looking at an order/broker/execution artifact, not a market-data
/// ingest/sync evidence file. Kraken ingest/sync evidence never carries any
/// of these; presence is treated as a fail-closed safety violation.
const KRAKEN_OHLC_FORBIDDEN_EXECUTION_FIELDS: [&str; 8] = [
    "order_id",
    "broker_order_id",
    "fill_price",
    "position_id",
    "account_id",
    "side",
    "limit_price",
    "stop_price",
];

fn kraken_ohlc_max_age_secs() -> i64 {
    std::env::var(ENV_KRAKEN_OHLC_MAX_AGE_SECS)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(KRAKEN_OHLC_DEFAULT_MAX_AGE_SECS)
}

/// Build a `KrakenOhlcStatusResponse` for a non-`active` outcome (missing,
/// unreadable, malformed, or unsafe evidence). `stale_or_missing_evidence`
/// is always `true` for these outcomes.
fn kraken_ohlc_response_for_non_active(
    truth_state: &str,
    max_evidence_age_secs: i64,
    evidence_path: Option<String>,
    error: Option<String>,
) -> KrakenOhlcStatusResponse {
    KrakenOhlcStatusResponse {
        canonical_route: KRAKEN_OHLC_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        provider: None,
        latest_mode: None,
        latest_schema_version: None,
        produced_at_utc: None,
        evidence_path,
        stale_or_missing_evidence: true,
        max_evidence_age_secs,
        network_call_made: None,
        db_write: None,
        md_bars_write: None,
        provider_id: None,
        provider_source: None,
        provider_symbol: None,
        ingest_mode: None,
        sync_policy: None,
        no_update_existing: None,
        symbols_requested: vec![],
        bars_completed: None,
        bars_excluded_forming: None,
        bars_considered_for_sync: None,
        bars_missing_new: None,
        bars_existing_candidate: None,
        rows_changed: None,
        rows_skipped_unchanged: None,
        rows_changed_skipped_due_to_no_update_existing: None,
        rows_inserted: None,
        rows_updated: None,
        rows_skipped_if_known: None,
        latest_existing_end_ts_before: None,
        latest_completed_start_ts: None,
        latest_completed_end_ts: None,
        volume_semantics: None,
        volume_scale: None,
        all_passed: None,
        reason_code: None,
        fail_reasons: vec![],
        error,
    }
}

/// Fail-closed safety check, independent of the CLI's own invariants: this
/// route does not trust the producer of the evidence file, it verifies.
/// Returns `Some(reason)` when the evidence must never be surfaced as
/// `"active"`, regardless of freshness.
fn kraken_ohlc_unsafe_reason(raw: &serde_json::Value) -> Option<String> {
    let provider = raw.get("provider").and_then(|v| v.as_str());
    if provider != Some(mqk_md::KRAKEN_PROVIDER_ID) {
        return Some(format!(
            "evidence provider field is not '{}': {:?}",
            mqk_md::KRAKEN_PROVIDER_ID,
            provider
        ));
    }

    // network_call_made=true is only safe when the evidence's own `mode`
    // field names the explicit operator opt-in path the CLI uses
    // (`mode="network_smoke"`, only ever set when
    // MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1 was set at the time the CLI ran).
    let network_call_made = raw
        .get("network_call_made")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mode = raw.get("mode").and_then(|v| v.as_str());
    if network_call_made && mode != Some("network_smoke") {
        return Some(
            "evidence claims network_call_made=true without the expected network_smoke opt-in mode"
                .to_string(),
        );
    }

    if raw
        .get("completed_bar_claim")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some(
            "evidence carries a completed_bar_claim=true field, which Kraken ingest/sync evidence never sets"
                .to_string(),
        );
    }

    if raw
        .get("forming_candle_excluded")
        .and_then(|v| v.as_bool())
        == Some(false)
    {
        return Some("evidence claims forming_candle_excluded=false".to_string());
    }

    let rows_inserted = raw.get("rows_inserted").and_then(|v| v.as_i64()).unwrap_or(0);
    let rows_updated = raw.get("rows_updated").and_then(|v| v.as_i64()).unwrap_or(0);
    let db_write = raw.get("db_write").and_then(|v| v.as_bool());
    if db_write == Some(false) && (rows_inserted > 0 || rows_updated > 0) {
        return Some(
            "evidence claims db_write=false but reports rows_inserted/rows_updated > 0".to_string(),
        );
    }

    let md_bars_write = raw.get("md_bars_write").and_then(|v| v.as_bool());
    let bars_excluded_forming = raw.get("bars_excluded_forming").and_then(|v| v.as_i64());
    if md_bars_write == Some(true) && bars_excluded_forming == Some(0) {
        return Some(
            "evidence claims md_bars_write=true with bars_excluded_forming=0, inconsistent with the committed Kraken fixtures (which always carry one forming row)"
                .to_string(),
        );
    }

    for required in ["volume_semantics", "provider_id", "provider_source", "ingest_mode"] {
        if raw.get(required).and_then(|v| v.as_str()).is_none() {
            return Some(format!("evidence is missing required field '{required}'"));
        }
    }

    if let Some(obj) = raw.as_object() {
        for forbidden in KRAKEN_OHLC_FORBIDDEN_EXECUTION_FIELDS {
            if obj.contains_key(forbidden) {
                return Some(format!(
                    "evidence contains forbidden trading/execution field '{forbidden}'"
                ));
            }
        }
    }

    None
}

/// `GET /api/v1/market-data/kraken-ohlc/status`
///
/// Read-only. Reads the latest `kraken_ohlc_ingest_*.json` or
/// `kraken_ohlc_sync_*.json` evidence file (selected by the epoch-seconds
/// timestamp embedded in the filename, not by alphabetical filename order --
/// `"ingest"` sorts before `"sync"` lexically, which would otherwise pick a
/// stale ingest file over a newer sync file) from
/// `st.md_refresh_evidence_dir` (env var `MQK_MD_REFRESH_EVIDENCE_DIR`,
/// default `exports/market_data` -- the same directory
/// `latest_mark_status`/`intraday_refresh_status` read, filtered to these
/// two distinct filename prefixes).
///
/// Safety:
/// - No provider/network API calls (Kraken or otherwise).
/// - No DB reads or writes.
/// - No broker interaction.
/// - No order/OMS/outbox state accessed.
/// - No CLI execution, no sync/ingest triggered, no daemon runtime start.
/// - Never stages or writes evidence -- read-only.
/// - Missing or malformed evidence never panics; surfaces truth_state honestly.
/// - Evidence failing any check in `kraken_ohlc_unsafe_reason` is surfaced as
///   `"unsafe_evidence"`, never `"active"`, regardless of freshness.
pub(crate) async fn kraken_ohlc_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let max_age_secs = kraken_ohlc_max_age_secs();
    let evidence_dir = st.md_refresh_evidence_dir.clone();
    let dir_path = std::path::Path::new(&evidence_dir);

    let read_dir = match std::fs::read_dir(dir_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(kraken_ohlc_response_for_non_active(
                    "backend_unavailable",
                    max_age_secs,
                    None,
                    Some(format!("evidence directory unreadable: {e}")),
                )),
            )
                .into_response();
        }
        Ok(rd) => rd,
    };

    let mut candidates: Vec<(i64, &'static str, String)> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let (mode, prefix): (&'static str, &'static str) = if name.starts_with(KRAKEN_OHLC_INGEST_PREFIX)
        {
            ("ingest", KRAKEN_OHLC_INGEST_PREFIX)
        } else if name.starts_with(KRAKEN_OHLC_SYNC_PREFIX) {
            ("sync", KRAKEN_OHLC_SYNC_PREFIX)
        } else {
            continue;
        };
        if !name.ends_with(KRAKEN_OHLC_EVIDENCE_SUFFIX) {
            continue;
        }
        let epoch: Option<i64> = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(KRAKEN_OHLC_EVIDENCE_SUFFIX))
            .and_then(|s| s.parse::<i64>().ok());
        if let Some(epoch) = epoch {
            candidates.push((epoch, mode, name));
        }
    }

    if candidates.is_empty() {
        return (
            StatusCode::OK,
            Json(kraken_ohlc_response_for_non_active(
                "no_evidence",
                max_age_secs,
                None,
                None,
            )),
        )
            .into_response();
    }

    candidates.sort_by_key(|(epoch, _, _)| *epoch);
    let (_, latest_mode, latest_name) = candidates.last().expect("candidates non-empty");
    let latest_path = dir_path.join(latest_name);
    let evidence_path_str = latest_path.to_string_lossy().to_string();

    let content = match std::fs::read_to_string(&latest_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(kraken_ohlc_response_for_non_active(
                    "backend_unavailable",
                    max_age_secs,
                    Some(evidence_path_str),
                    Some(format!("evidence file unreadable: {e}")),
                )),
            )
                .into_response();
        }
        Ok(c) => c,
    };

    let raw: serde_json::Value = match serde_json::from_str(&content) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(kraken_ohlc_response_for_non_active(
                    "parse_error",
                    max_age_secs,
                    Some(evidence_path_str),
                    Some(format!("evidence JSON parse failed: {e}")),
                )),
            )
                .into_response();
        }
        Ok(v) => v,
    };

    let schema_version = raw
        .get("schema_version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let schema_ok = matches!(
        schema_version.as_deref(),
        Some(KRAKEN_OHLC_INGEST_SCHEMA_VERSION) | Some(KRAKEN_OHLC_SYNC_SCHEMA_VERSION)
    );
    if !schema_ok {
        return (
            StatusCode::OK,
            Json(kraken_ohlc_response_for_non_active(
                "parse_error",
                max_age_secs,
                Some(evidence_path_str),
                Some(format!(
                    "unsupported schema_version (expected '{KRAKEN_OHLC_INGEST_SCHEMA_VERSION}' or '{KRAKEN_OHLC_SYNC_SCHEMA_VERSION}', got {schema_version:?})"
                )),
            )),
        )
            .into_response();
    }

    if let Some(reason) = kraken_ohlc_unsafe_reason(&raw) {
        return (
            StatusCode::OK,
            Json(kraken_ohlc_response_for_non_active(
                "unsafe_evidence",
                max_age_secs,
                Some(evidence_path_str),
                Some(reason),
            )),
        )
            .into_response();
    }

    let produced_at_utc = raw
        .get("produced_at_utc")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stale_or_missing_evidence = match &produced_at_utc {
        None => true,
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Err(_) => true,
            Ok(produced_at) => {
                (Utc::now() - produced_at.with_timezone(&Utc)).num_seconds() > max_age_secs
            }
        },
    };
    let truth_state = if stale_or_missing_evidence {
        "stale"
    } else {
        "active"
    };

    (
        StatusCode::OK,
        Json(KrakenOhlcStatusResponse {
            canonical_route: KRAKEN_OHLC_ROUTE.to_string(),
            truth_state: truth_state.to_string(),
            provider: raw
                .get("provider")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            latest_mode: Some((*latest_mode).to_string()),
            latest_schema_version: schema_version,
            produced_at_utc,
            evidence_path: Some(evidence_path_str),
            stale_or_missing_evidence,
            max_evidence_age_secs: max_age_secs,
            network_call_made: raw.get("network_call_made").and_then(|v| v.as_bool()),
            db_write: raw.get("db_write").and_then(|v| v.as_bool()),
            md_bars_write: raw.get("md_bars_write").and_then(|v| v.as_bool()),
            provider_id: raw
                .get("provider_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            provider_source: raw
                .get("provider_source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            provider_symbol: raw
                .get("provider_symbol")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            ingest_mode: raw
                .get("ingest_mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            sync_policy: raw
                .get("sync_policy")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            no_update_existing: raw.get("no_update_existing").and_then(|v| v.as_bool()),
            symbols_requested: value_string_array(&raw, "symbols_requested"),
            bars_completed: raw.get("bars_completed").and_then(|v| v.as_i64()),
            bars_excluded_forming: raw.get("bars_excluded_forming").and_then(|v| v.as_i64()),
            bars_considered_for_sync: raw.get("bars_considered_for_sync").and_then(|v| v.as_i64()),
            bars_missing_new: raw.get("bars_missing_new").and_then(|v| v.as_i64()),
            bars_existing_candidate: raw
                .get("bars_existing_candidate")
                .and_then(|v| v.as_i64()),
            rows_changed: raw.get("rows_changed").and_then(|v| v.as_i64()),
            rows_skipped_unchanged: raw.get("rows_skipped_unchanged").and_then(|v| v.as_i64()),
            rows_changed_skipped_due_to_no_update_existing: raw
                .get("rows_changed_skipped_due_to_no_update_existing")
                .and_then(|v| v.as_i64()),
            rows_inserted: raw.get("rows_inserted").and_then(|v| v.as_i64()),
            rows_updated: raw.get("rows_updated").and_then(|v| v.as_i64()),
            rows_skipped_if_known: raw.get("rows_skipped_if_known").and_then(|v| v.as_i64()),
            latest_existing_end_ts_before: raw
                .get("latest_existing_end_ts_before")
                .and_then(|v| v.as_i64()),
            latest_completed_start_ts: raw
                .get("latest_completed_start_ts")
                .and_then(|v| v.as_i64()),
            latest_completed_end_ts: raw.get("latest_completed_end_ts").and_then(|v| v.as_i64()),
            volume_semantics: raw
                .get("volume_semantics")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            volume_scale: raw.get("volume_scale").and_then(|v| v.as_i64()),
            all_passed: raw.get("all_passed").and_then(|v| v.as_bool()),
            reason_code: raw
                .get("reason_code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            fail_reasons: value_string_array(&raw, "fail_reasons"),
            error: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/crypto-registry/readiness
// (CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01)
//
// Read-only re-exposure of the same classification
// `mqk-cli md crypto-registry-readiness` (CRYPTO-REGISTRY-03) performs,
// reusing the identical `mqk_md::provider_registry`/
// `mqk_md::instrument_registry_v2` pure loaders -- no CLI subprocess, no DB
// connection, no provider/network call, no scheduler, no config-file
// mutation, no trading state. `provider_enabled=false` and per-symbol
// `enabled=false` are expected, correct states and classify as
// `data_ready_manual_only`, not a failure.
// ---------------------------------------------------------------------------

/// Registry-v2 fixture path used when `MQK_INSTRUMENT_REGISTRY_V2_PATH` is
/// not configured -- the same, single, already-committed crypto fixture
/// `CRYPTO-REGISTRY-03`'s CLI documentation examples read. Read-only; never
/// mutated by this route.
const CRYPTO_REGISTRY_READINESS_DEFAULT_REGISTRY_PATH: &str =
    "config/instruments/instruments_v2.crypto_local_marks.example.json";

const CRYPTO_REGISTRY_READINESS_PROVIDER: &str = "kraken";
const CRYPTO_REGISTRY_READINESS_SYMBOLS: &[&str] = &["BTC/USD", "ETH/USD"];

pub(crate) async fn crypto_registry_readiness(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let registry_path = st
        .instrument_registry_v2_path
        .clone()
        .unwrap_or_else(|| CRYPTO_REGISTRY_READINESS_DEFAULT_REGISTRY_PATH.to_string());
    let providers_path = st.provider_registry_path.clone();
    let provider_id = CRYPTO_REGISTRY_READINESS_PROVIDER;

    let mut fail_reasons: Vec<String> = Vec::new();
    let mut truth_state = "active";

    let provider_configs = mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
        &providers_path,
    ));
    let (provider_enabled, api_key_required, provider_asset_classes, provider_implementation_status) =
        match &provider_configs {
            Ok(cfgs) => match mqk_md::provider_registry::find_provider(cfgs, provider_id) {
                Some(cfg) => {
                    if cfg.enabled {
                        truth_state = "unsafe_provider_enabled";
                        fail_reasons.push(format!(
                            "provider '{provider_id}' is enabled=true; expected false pending an explicit, separate cutover decision (CRYPTO-REGISTRY-02)"
                        ));
                    }
                    (
                        cfg.enabled,
                        cfg.api_key_required,
                        cfg.asset_classes.clone(),
                        cfg.implementation_status.clone(),
                    )
                }
                None => {
                    truth_state = "missing_provider";
                    fail_reasons.push(format!(
                        "provider '{provider_id}' not found in {providers_path}"
                    ));
                    (false, false, Vec::new(), String::new())
                }
            },
            Err(err) => {
                truth_state = "parse_error";
                fail_reasons.push(format!("providers registry load failed: {err}"));
                (false, false, Vec::new(), String::new())
            }
        };

    let registry_v2 =
        mqk_md::instrument_registry_v2::load_instrument_registry_v2(std::path::Path::new(
            &registry_path,
        ));

    let mut symbols: Vec<CryptoRegistrySymbolReadiness> =
        Vec::with_capacity(CRYPTO_REGISTRY_READINESS_SYMBOLS.len());
    match &registry_v2 {
        Err(err) => {
            if truth_state == "active" {
                truth_state = "parse_error";
            }
            fail_reasons.push(format!("registry load failed: {err}"));
            for symbol in CRYPTO_REGISTRY_READINESS_SYMBOLS {
                symbols.push(CryptoRegistrySymbolReadiness {
                    symbol: symbol.to_string(),
                    found: false,
                    asset_class_ok: false,
                    kraken_pair: None,
                    kraken_result_key: None,
                    alias_ok: false,
                    enabled: None,
                    paper_trading_enabled: None,
                    live_trading_enabled: None,
                    trading_flags_safe: false,
                    passed: false,
                });
            }
        }
        Ok(registry) => {
            for symbol in CRYPTO_REGISTRY_READINESS_SYMBOLS {
                let inst = registry.instruments.iter().find(|i| i.symbol == *symbol);
                let check = match inst {
                    None => {
                        if truth_state == "active" {
                            truth_state = "missing_symbol";
                        }
                        fail_reasons.push(format!(
                            "symbol '{symbol}' not found in {registry_path}"
                        ));
                        CryptoRegistrySymbolReadiness {
                            symbol: symbol.to_string(),
                            found: false,
                            asset_class_ok: false,
                            kraken_pair: None,
                            kraken_result_key: None,
                            alias_ok: false,
                            enabled: None,
                            paper_trading_enabled: None,
                            live_trading_enabled: None,
                            trading_flags_safe: false,
                            passed: false,
                        }
                    }
                    Some(inst) => {
                        let asset_class_ok = inst.asset_class == "crypto";
                        if !asset_class_ok && truth_state == "active" {
                            truth_state = "missing_symbol";
                            fail_reasons.push(format!(
                                "symbol '{symbol}' asset_class is '{}', expected 'crypto'",
                                inst.asset_class
                            ));
                        }
                        let kraken_pair = inst.provider_symbols.get("kraken_pair").cloned();
                        let kraken_result_key =
                            inst.provider_symbols.get("kraken_result_key").cloned();
                        let alias_ok = kraken_pair.is_some() && kraken_result_key.is_some();
                        if !alias_ok {
                            if truth_state == "active" {
                                truth_state = "missing_alias";
                            }
                            fail_reasons.push(format!(
                                "symbol '{symbol}' missing kraken_pair/kraken_result_key alias"
                            ));
                        }
                        let trading_flags_safe =
                            !inst.paper_trading_enabled && !inst.live_trading_enabled;
                        if inst.paper_trading_enabled {
                            truth_state = "unsafe_trading_enabled";
                            fail_reasons
                                .push(format!("symbol '{symbol}' paper_trading_enabled=true"));
                        }
                        if inst.live_trading_enabled {
                            truth_state = "unsafe_trading_enabled";
                            fail_reasons
                                .push(format!("symbol '{symbol}' live_trading_enabled=true"));
                        }
                        let passed = asset_class_ok && alias_ok && trading_flags_safe;
                        CryptoRegistrySymbolReadiness {
                            symbol: symbol.to_string(),
                            found: true,
                            asset_class_ok,
                            kraken_pair,
                            kraken_result_key,
                            alias_ok,
                            enabled: Some(inst.enabled),
                            paper_trading_enabled: Some(inst.paper_trading_enabled),
                            live_trading_enabled: Some(inst.live_trading_enabled),
                            trading_flags_safe,
                            passed,
                        }
                    }
                };
                symbols.push(check);
            }
        }
    }

    let all_symbols_disabled = symbols.iter().all(|c| c.enabled == Some(false));
    let all_passed = truth_state == "active" && fail_reasons.is_empty();

    let data_readiness_state = if !all_passed {
        "blocked"
    } else if all_symbols_disabled {
        "data_ready_manual_only"
    } else {
        "production_default"
    };

    let reason_code = if all_passed {
        "crypto_registry_readiness_data_ready_manual_only"
    } else {
        match truth_state {
            "missing_provider" => "provider_not_found",
            "missing_symbol" => "symbol_not_found_or_wrong_asset_class",
            "missing_alias" => "kraken_alias_incomplete",
            "unsafe_trading_enabled" => "trading_flag_unexpectedly_true",
            "unsafe_provider_enabled" => "provider_unexpectedly_enabled",
            "parse_error" => "registry_or_providers_parse_error",
            _ => "readiness_check_failed",
        }
    };

    let response = CryptoRegistryReadinessResponse {
        canonical_route: "/api/v1/market-data/crypto-registry/readiness".to_string(),
        truth_state: truth_state.to_string(),
        data_readiness_state: data_readiness_state.to_string(),
        trading_readiness_state: "disabled".to_string(),
        scheduler_readiness_state: "absent".to_string(),
        provider: provider_id.to_string(),
        provider_enabled,
        api_key_required,
        provider_asset_classes,
        provider_implementation_status,
        registry_path,
        providers_path,
        symbols,
        all_passed,
        reason_code: reason_code.to_string(),
        fail_reasons,
        safety: CryptoRegistryReadinessSafety {
            no_scheduler: true,
            no_db_connection: true,
            no_network_call: true,
            no_trading_enabled: true,
            no_config_file_mutated: true,
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/kraken-scheduler/readiness
// (CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01)
// ---------------------------------------------------------------------------

const KRAKEN_SCHEDULER_READINESS_ROUTE: &str = "/api/v1/market-data/kraken-scheduler/readiness";
const KRAKEN_SCHEDULER_READINESS_PROVIDER: &str = "kraken";
const KRAKEN_SCHEDULER_READINESS_SYMBOLS: &[&str] = &["BTC/USD", "ETH/USD"];
const KRAKEN_SCHEDULER_POLICY_SCHEMA_VERSION: &str =
    "crypto-data-02a-kraken-scheduler-rate-limit-decision-v1";

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct KrakenSchedulerPolicySafetyDoc {
    #[serde(default)]
    no_scheduled_task_registered: bool,
    #[serde(default)]
    no_daemon_job_added: bool,
    #[serde(default)]
    no_network_call_to_kraken_api: bool,
    #[serde(default)]
    no_db_write: bool,
    #[serde(default)]
    no_trading_enabled: bool,
    #[serde(default)]
    kraken_provider_enabled: bool,
    #[serde(default)]
    paper_trading_enabled: bool,
    #[serde(default)]
    live_trading_enabled: bool,
}

/// Mirrors `mqk-cli`'s `KrakenSchedulerPolicy` -- deserialized directly from
/// `KRAKEN_SCHEDULER_READINESS_POLICY_PATH`; never mutated, never written
/// back.
#[derive(Debug, Clone, serde::Deserialize)]
struct KrakenSchedulerPolicyDoc {
    #[serde(default)]
    schema_version: String,
    #[serde(default)]
    recommended_default_cadence: String,
    #[serde(default)]
    min_seconds_between_pair_calls: i64,
    #[serde(default)]
    min_seconds_between_scheduled_runs: i64,
    #[serde(default)]
    max_ohlc_calls_per_run: i64,
    #[serde(default)]
    max_total_network_calls_per_run: i64,
    #[serde(default)]
    concurrency: String,
    #[serde(default)]
    scheduler_registration_status: String,
    #[serde(default)]
    safety: KrakenSchedulerPolicySafetyDoc,
}

/// Mirrors `mqk-cli`'s `validate_kraken_scheduler_policy_contract`. Kept as
/// an independent reimplementation (not a shared crate dependency) exactly
/// like `crypto_registry_readiness` above mirrors its own CLI sibling --
/// this route must never trust the CLI's own validation, it re-derives it.
fn validate_kraken_scheduler_policy_doc(policy: &KrakenSchedulerPolicyDoc) -> Vec<String> {
    let mut violations = Vec::new();
    if policy.schema_version != KRAKEN_SCHEDULER_POLICY_SCHEMA_VERSION {
        violations.push(format!(
            "schema_version must be '{KRAKEN_SCHEDULER_POLICY_SCHEMA_VERSION}', got '{}'",
            policy.schema_version
        ));
    }
    if policy.min_seconds_between_pair_calls < 2 {
        violations.push(format!(
            "min_seconds_between_pair_calls must be >= 2, got {}",
            policy.min_seconds_between_pair_calls
        ));
    }
    if policy.min_seconds_between_scheduled_runs < 86400 {
        violations.push(format!(
            "min_seconds_between_scheduled_runs must be >= 86400, got {}",
            policy.min_seconds_between_scheduled_runs
        ));
    }
    if policy.max_ohlc_calls_per_run > 2 {
        violations.push(format!(
            "max_ohlc_calls_per_run must be <= 2, got {}",
            policy.max_ohlc_calls_per_run
        ));
    }
    if policy.max_total_network_calls_per_run > 4 {
        violations.push(format!(
            "max_total_network_calls_per_run must be <= 4, got {}",
            policy.max_total_network_calls_per_run
        ));
    }
    if policy.concurrency != "sequential_only" {
        violations.push(format!(
            "concurrency must be 'sequential_only', got '{}'",
            policy.concurrency
        ));
    }
    let safety = &policy.safety;
    if !safety.no_scheduled_task_registered
        || !safety.no_daemon_job_added
        || !safety.no_network_call_to_kraken_api
        || !safety.no_db_write
        || !safety.no_trading_enabled
    {
        violations.push(
            "safety.no_scheduled_task_registered/no_daemon_job_added/no_network_call_to_kraken_api/no_db_write/no_trading_enabled must all be true"
                .to_string(),
        );
    }
    if safety.kraken_provider_enabled || safety.paper_trading_enabled || safety.live_trading_enabled
    {
        violations.push(
            "safety.kraken_provider_enabled/paper_trading_enabled/live_trading_enabled must all be false"
                .to_string(),
        );
    }
    violations
}

/// Finds the most recent `kraken_ohlc_ingest_<epoch>.json` or
/// `kraken_ohlc_sync_<epoch>.json` file in `dir` by the epoch-seconds
/// embedded in the filename, mirroring `kraken_ohlc_status`'s own selection
/// rule above (kept as an independent copy since that function's evidence
/// parsing is intertwined with its own response type).
fn find_latest_kraken_ohlc_evidence_for_scheduler(
    dir: &std::path::Path,
) -> Option<(std::path::PathBuf, i64)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::path::PathBuf, i64)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str())?.to_string();
        let stem = name.strip_suffix(KRAKEN_OHLC_EVIDENCE_SUFFIX)?;
        let epoch_str = stem
            .strip_prefix(KRAKEN_OHLC_INGEST_PREFIX)
            .or_else(|| stem.strip_prefix(KRAKEN_OHLC_SYNC_PREFIX))?;
        let epoch: i64 = epoch_str.parse().ok()?;
        if best.as_ref().map(|(_, e)| epoch > *e).unwrap_or(true) {
            best = Some((path, epoch));
        }
    }
    best
}

/// `GET /api/v1/market-data/kraken-scheduler/readiness`
///
/// Read-only re-exposure of `mqk-cli md kraken-scheduler-readiness`'s
/// classification: whether a *future*, not-yet-authorized Kraken scheduled
/// sync is currently allowed by the `CRYPTO-DATA-02A` policy, the current
/// provider/registry config, and (if present) the latest Kraken OHLC
/// evidence. Evidence is inspected read-only (never required to be fresh by
/// this route -- missing/unsafe evidence never blocks `active` here, only
/// downgrades `evidence_readiness_state`) since this route has no operator
/// flag equivalent to the CLI's `--require-fresh-evidence`.
///
/// `"active"` never means a scheduler is registered. This route never opens
/// a DB connection, never calls Kraken or any provider/network endpoint,
/// never runs a CLI subprocess, never registers a scheduler, never mutates
/// any config file.
pub(crate) async fn kraken_scheduler_readiness(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let policy_path = st.kraken_scheduler_policy_path.clone();
    let registry_path = st
        .instrument_registry_v2_path
        .clone()
        .unwrap_or_else(|| CRYPTO_REGISTRY_READINESS_DEFAULT_REGISTRY_PATH.to_string());
    let providers_path = st.provider_registry_path.clone();
    let provider_id = KRAKEN_SCHEDULER_READINESS_PROVIDER;

    let mut fail_reasons: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut truth_state = "active";

    // -- 1. Policy: existence, parse, contract, scheduler_registration_status --
    let mut rate_limit_policy_state = "missing";
    let mut policy: Option<KrakenSchedulerPolicyDoc> = None;
    let policy_fs_path = std::path::Path::new(&policy_path);
    if !policy_fs_path.exists() {
        truth_state = "policy_missing";
        fail_reasons.push(format!("policy file not found: {policy_path}"));
    } else {
        match std::fs::read_to_string(policy_fs_path)
            .ok()
            .and_then(|s| serde_json::from_str::<KrakenSchedulerPolicyDoc>(&s).ok())
        {
            None => {
                truth_state = "policy_invalid";
                rate_limit_policy_state = "invalid";
                fail_reasons.push(format!(
                    "policy file is not valid JSON or does not match the expected schema: {policy_path}"
                ));
            }
            Some(p) => {
                let violations = validate_kraken_scheduler_policy_doc(&p);
                if violations.is_empty() {
                    rate_limit_policy_state = "valid";
                } else {
                    rate_limit_policy_state = "invalid";
                    if truth_state == "active" {
                        truth_state = "policy_invalid";
                    }
                    fail_reasons.extend(violations);
                }
                if p.scheduler_registration_status != "not_registered" {
                    if truth_state == "active" {
                        truth_state = "scheduler_already_registered";
                    }
                    fail_reasons.push(format!(
                        "policy scheduler_registration_status is '{}', expected 'not_registered'",
                        p.scheduler_registration_status
                    ));
                }
                policy = Some(p);
            }
        }
    }

    // -- 2. Providers: existence, parse, provider entry, enabled flag --
    let mut provider_readiness_state = "unknown";
    let mut provider_enabled = false;
    let provider_configs =
        mqk_md::provider_registry::load_provider_registry(std::path::Path::new(&providers_path));
    match &provider_configs {
        Ok(cfgs) => match mqk_md::provider_registry::find_provider(cfgs, provider_id) {
            Some(cfg) => {
                provider_enabled = cfg.enabled;
                if cfg.enabled {
                    provider_readiness_state = "unsafe";
                    if truth_state == "active" {
                        truth_state = "provider_unsafe";
                    }
                    fail_reasons.push(format!(
                        "provider '{provider_id}' is enabled=true; expected false pending a separate, explicit cutover decision (CRYPTO-REGISTRY-02)"
                    ));
                } else {
                    provider_readiness_state = "ready";
                }
            }
            None => {
                provider_readiness_state = "unsafe";
                if truth_state == "active" {
                    truth_state = "provider_unsafe";
                }
                fail_reasons.push(format!(
                    "provider '{provider_id}' not found in {providers_path}"
                ));
            }
        },
        Err(err) => {
            if truth_state == "active" {
                truth_state = "parse_error";
            }
            fail_reasons.push(format!("providers registry load failed: {err}"));
        }
    }

    // -- 3. Registry: existence, parse, per-symbol alias/asset-class/trading flags --
    let mut registry_readiness_state = "unknown";
    let registry_v2 = mqk_md::instrument_registry_v2::load_instrument_registry_v2(
        std::path::Path::new(&registry_path),
    );
    let mut symbols: Vec<KrakenSchedulerSymbolReadiness> =
        Vec::with_capacity(KRAKEN_SCHEDULER_READINESS_SYMBOLS.len());
    match &registry_v2 {
        Err(err) => {
            if truth_state == "active" {
                truth_state = "parse_error";
            }
            fail_reasons.push(format!("registry load failed: {err}"));
            for symbol in KRAKEN_SCHEDULER_READINESS_SYMBOLS {
                symbols.push(KrakenSchedulerSymbolReadiness {
                    symbol: symbol.to_string(),
                    found: false,
                    asset_class_ok: false,
                    alias_ok: false,
                    enabled: None,
                    paper_trading_enabled: None,
                    live_trading_enabled: None,
                    trading_flags_safe: false,
                });
            }
        }
        Ok(registry) => {
            let mut registry_ok = true;
            let mut trading_unsafe = false;
            for symbol in KRAKEN_SCHEDULER_READINESS_SYMBOLS {
                let inst = registry.instruments.iter().find(|i| i.symbol == *symbol);
                let check = match inst {
                    None => {
                        registry_ok = false;
                        fail_reasons.push(format!("symbol '{symbol}' not found in {registry_path}"));
                        KrakenSchedulerSymbolReadiness {
                            symbol: symbol.to_string(),
                            found: false,
                            asset_class_ok: false,
                            alias_ok: false,
                            enabled: None,
                            paper_trading_enabled: None,
                            live_trading_enabled: None,
                            trading_flags_safe: false,
                        }
                    }
                    Some(inst) => {
                        let asset_class_ok = inst.asset_class == "crypto";
                        let alias_ok = inst.provider_symbols.contains_key("kraken_pair")
                            && inst.provider_symbols.contains_key("kraken_result_key");
                        if !asset_class_ok || !alias_ok {
                            registry_ok = false;
                            fail_reasons.push(format!(
                                "symbol '{symbol}' failed registry checks (asset_class_ok={asset_class_ok}, alias_ok={alias_ok})"
                            ));
                        }
                        if inst.paper_trading_enabled || inst.live_trading_enabled {
                            trading_unsafe = true;
                        }
                        let trading_flags_safe =
                            !inst.paper_trading_enabled && !inst.live_trading_enabled;
                        KrakenSchedulerSymbolReadiness {
                            symbol: symbol.to_string(),
                            found: true,
                            asset_class_ok,
                            alias_ok,
                            enabled: Some(inst.enabled),
                            paper_trading_enabled: Some(inst.paper_trading_enabled),
                            live_trading_enabled: Some(inst.live_trading_enabled),
                            trading_flags_safe,
                        }
                    }
                };
                symbols.push(check);
            }
            registry_readiness_state = if registry_ok { "ready" } else { "unsafe" };
            if !registry_ok && truth_state == "active" {
                truth_state = "registry_unsafe";
            }
            if trading_unsafe {
                if truth_state == "active" {
                    truth_state = "trading_flags_unsafe";
                }
                fail_reasons.push(
                    "one or more requested symbols has paper_trading_enabled or live_trading_enabled = true"
                        .to_string(),
                );
            }
        }
    }

    // -- 4. Evidence (read-only, never required by this route) --
    let evidence_dir = st.md_refresh_evidence_dir.clone();
    let evidence_readiness_state = match find_latest_kraken_ohlc_evidence_for_scheduler(
        std::path::Path::new(&evidence_dir),
    ) {
        None => "missing",
        Some((path, _epoch)) => {
            let parsed = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            match parsed {
                None => "unsafe",
                Some(v) => {
                    let provider_ok = v.get("provider").and_then(|p| p.as_str()) == Some("kraken");
                    let evidence_all_passed =
                        v.get("all_passed").and_then(|b| b.as_bool()).unwrap_or(false);
                    if !provider_ok || !evidence_all_passed {
                        "unsafe"
                    } else {
                        "safe"
                    }
                }
            }
        }
    };
    if evidence_readiness_state == "missing" || evidence_readiness_state == "unsafe" {
        warnings.push(format!(
            "Kraken OHLC evidence is {evidence_readiness_state} in {evidence_dir}; this route never requires fresh evidence to report active"
        ));
    }

    let all_passed = truth_state == "active" && fail_reasons.is_empty();
    let scheduler_readiness_state = if all_passed {
        "scheduler_ready_manual_registration_blocked"
    } else if matches!(
        truth_state,
        "policy_missing" | "policy_invalid" | "scheduler_already_registered"
    ) {
        "policy_blocked"
    } else {
        "scheduler_not_ready"
    };

    let reason_code = if all_passed {
        "kraken_scheduler_prerequisites_satisfied_manual_registration_blocked"
    } else {
        match truth_state {
            "policy_missing" => "scheduler_policy_file_missing",
            "policy_invalid" => "scheduler_policy_contract_violation",
            "scheduler_already_registered" => "policy_declares_scheduler_already_registered",
            "registry_unsafe" => "registry_alias_or_asset_class_check_failed",
            "provider_unsafe" => "provider_not_found_or_unexpectedly_enabled",
            "trading_flags_unsafe" => "trading_flag_unexpectedly_true",
            "parse_error" => "registry_or_providers_file_parse_error",
            _ => "readiness_check_failed",
        }
    };

    let (recommended_default_cadence, min_pair, min_run, max_ohlc, max_total, concurrency) =
        match &policy {
            Some(p) => (
                p.recommended_default_cadence.clone(),
                p.min_seconds_between_pair_calls,
                p.min_seconds_between_scheduled_runs,
                p.max_ohlc_calls_per_run,
                p.max_total_network_calls_per_run,
                p.concurrency.clone(),
            ),
            None => (String::new(), 0, 0, 0, 0, String::new()),
        };
    let scheduler_registration_status = policy
        .as_ref()
        .map(|p| p.scheduler_registration_status.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let response = KrakenSchedulerReadinessResponse {
        canonical_route: KRAKEN_SCHEDULER_READINESS_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        scheduler_readiness_state: scheduler_readiness_state.to_string(),
        rate_limit_policy_state: rate_limit_policy_state.to_string(),
        registry_readiness_state: registry_readiness_state.to_string(),
        provider_readiness_state: provider_readiness_state.to_string(),
        evidence_readiness_state: evidence_readiness_state.to_string(),
        provider: provider_id.to_string(),
        provider_enabled,
        symbols,
        recommended_default_cadence,
        min_seconds_between_pair_calls: min_pair,
        min_seconds_between_scheduled_runs: min_run,
        max_ohlc_calls_per_run: max_ohlc,
        max_total_network_calls_per_run: max_total,
        concurrency,
        scheduler_registration_status,
        daemon_job_status: "absent".to_string(),
        network_call_made: false,
        db_write: false,
        trading_enabled: false,
        policy_path,
        registry_path,
        providers_path,
        all_passed,
        reason_code: reason_code.to_string(),
        fail_reasons,
        warnings,
        safety: KrakenSchedulerReadinessSafety {
            no_scheduled_task_registered: true,
            no_daemon_job_added: true,
            no_network_call_made: true,
            no_db_connection: true,
            no_trading_enabled: true,
            no_config_file_mutated: true,
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/kraken-scheduler/task-status
// (CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01)
// ---------------------------------------------------------------------------

const KRAKEN_SCHEDULER_TASK_STATUS_ROUTE: &str =
    "/api/v1/market-data/kraken-scheduler/task-status";
const KRAKEN_SCHEDULER_TASK_STATUS_EVIDENCE_FILENAME: &str = "kraken_ohlc_task_registration.json";
const KRAKEN_SCHEDULER_TASK_STATUS_SCHEMA_VERSION: &str = "kraken-ohlc-task-registration-v1";

/// Top-level evidence fields whose presence would imply this route is
/// looking at a broker/order/risk/runtime artifact, not a task-registration
/// evidence file. `Register-KrakenOhlcSyncTask.ps1` never emits any of
/// these; presence is treated as a fail-closed safety violation.
const KRAKEN_SCHEDULER_TASK_STATUS_FORBIDDEN_FIELDS: [&str; 8] = [
    "order_id",
    "broker_order_id",
    "fill_price",
    "position_id",
    "account_id",
    "side",
    "limit_price",
    "stop_price",
];

fn kraken_scheduler_task_status_response_for_non_active(
    truth_state: &str,
    evidence_path: Option<String>,
    error: Option<String>,
) -> KrakenSchedulerTaskStatusResponse {
    KrakenSchedulerTaskStatusResponse {
        canonical_route: KRAKEN_SCHEDULER_TASK_STATUS_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        schema_version: None,
        produced_at_utc: None,
        mode: None,
        task_name: None,
        task_exists_before: None,
        task_exists_after: None,
        registered: None,
        unregistered: None,
        check_only: None,
        task_action: None,
        runner_path: None,
        policy_path: None,
        registry_path: None,
        providers_path: None,
        symbols: vec![],
        timeframe: None,
        at: None,
        scheduled_task_mutation: None,
        network_call_made: None,
        db_write: None,
        md_bars_write: None,
        env_vars_embedded: vec![],
        env_vars_required: vec![],
        all_passed: None,
        reason_code: None,
        fail_reasons: vec![],
        warnings: vec![],
        safety: None,
        evidence_path,
        error,
    }
}

/// Fail-closed safety check, independent of the CLI/PowerShell producer's
/// own invariants: this route does not trust the writer of the evidence
/// file, it verifies. Returns `Some(reason)` when the evidence must never be
/// surfaced as `"active"`. Callers must check `schema_version` separately
/// before calling this (a schema mismatch is classified as `"parse_error"`,
/// matching this codebase's existing `kraken-ohlc/status` /
/// `intraday-refresh/status` convention).
fn kraken_scheduler_task_status_unsafe_reason(raw: &serde_json::Value) -> Option<String> {
    let mode = raw.get("mode").and_then(|v| v.as_str());

    let scheduled_task_mutation = raw
        .get("scheduled_task_mutation")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if mode == Some("check_only") && scheduled_task_mutation {
        return Some(
            "evidence claims mode=\"check_only\" but scheduled_task_mutation=true".to_string(),
        );
    }

    let registered = raw
        .get("registered")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if mode == Some("check_only") && registered {
        return Some("evidence claims mode=\"check_only\" but registered=true".to_string());
    }

    if raw
        .get("network_call_made")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some("evidence claims network_call_made=true".to_string());
    }
    if raw.get("db_write").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Some("evidence claims db_write=true".to_string());
    }
    if raw
        .get("md_bars_write")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some("evidence claims md_bars_write=true".to_string());
    }

    let env_vars_embedded_nonempty = raw
        .get("env_vars_embedded")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if env_vars_embedded_nonempty {
        return Some("evidence claims a non-empty env_vars_embedded".to_string());
    }

    let task_name = raw
        .get("task_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());
    if task_name.is_none() {
        return Some("evidence is missing a non-empty task_name".to_string());
    }

    let task_action = raw
        .get("task_action")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());
    if task_action.is_none() {
        return Some("evidence is missing a non-empty task_action".to_string());
    }

    let runner_path = raw
        .get("runner_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());
    if runner_path.is_none() {
        return Some("evidence is missing a non-empty runner_path".to_string());
    }

    if let Some(action) = task_action {
        if !action.contains("Run-KrakenOhlcSync.ps1") {
            return Some(
                "evidence task_action does not reference Run-KrakenOhlcSync.ps1".to_string(),
            );
        }
    }

    if raw
        .get("task_started")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some("evidence claims the task was started".to_string());
    }
    if raw
        .get("daemon_job_added")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some("evidence claims a daemon job was added".to_string());
    }
    for trading_flag in [
        "trading_enabled",
        "paper_trading_enabled",
        "live_trading_enabled",
    ] {
        if raw
            .get(trading_flag)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Some(format!("evidence claims {trading_flag}=true"));
        }
    }

    if let Some(obj) = raw.as_object() {
        for forbidden in KRAKEN_SCHEDULER_TASK_STATUS_FORBIDDEN_FIELDS {
            if obj.contains_key(forbidden) {
                return Some(format!(
                    "evidence contains forbidden broker/order/risk/runtime field '{forbidden}'"
                ));
            }
        }
    }

    None
}

fn kraken_scheduler_task_status_str_vec(raw: &serde_json::Value, key: &str) -> Vec<String> {
    raw.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/v1/market-data/kraken-scheduler/task-status`
///
/// Read-only. Reads the fixed `kraken_ohlc_task_registration.json` evidence
/// file (written by `Register-KrakenOhlcSyncTask.ps1`,
/// `CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01`) from
/// `st.md_refresh_evidence_dir` (env var `MQK_MD_REFRESH_EVIDENCE_DIR`,
/// default `exports/market_data` -- the same directory
/// `kraken-ohlc/status`/`latest-marks/status`/`intraday-refresh/status`
/// read, here filtered to this one fixed filename so it never collides with
/// the runner's own `kraken_ohlc_scheduled_runner_<epoch>.json` evidence).
///
/// This route is task-**registration** evidence visibility only, distinct
/// from `Run-KrakenOhlcSync.ps1`'s own runner evidence.
///
/// Safety:
/// - Never calls Windows Task Scheduler APIs.
/// - Never shells out to PowerShell or runs any script.
/// - Never calls Kraken or any provider/network endpoint.
/// - Never opens a DB connection.
/// - Never registers, unregisters, or starts a scheduled task.
/// - Never mutates scheduler or trading state.
/// - Missing or malformed evidence never panics; surfaces truth_state honestly.
/// - Evidence failing any check in `kraken_scheduler_task_status_unsafe_reason`
///   is surfaced as `"unsafe_evidence"`, never `"active"`.
pub(crate) async fn kraken_scheduler_task_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let evidence_dir = st.md_refresh_evidence_dir.clone();
    let dir_path = std::path::Path::new(&evidence_dir);

    if let Err(e) = std::fs::read_dir(dir_path) {
        return (
            StatusCode::OK,
            Json(kraken_scheduler_task_status_response_for_non_active(
                "backend_unavailable",
                None,
                Some(format!("evidence directory unreadable: {e}")),
            )),
        )
            .into_response();
    }

    let evidence_path = dir_path.join(KRAKEN_SCHEDULER_TASK_STATUS_EVIDENCE_FILENAME);
    if !evidence_path.exists() {
        return (
            StatusCode::OK,
            Json(kraken_scheduler_task_status_response_for_non_active(
                "no_evidence",
                None,
                None,
            )),
        )
            .into_response();
    }
    let evidence_path_str = evidence_path.to_string_lossy().to_string();

    let content = match std::fs::read_to_string(&evidence_path) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(kraken_scheduler_task_status_response_for_non_active(
                    "backend_unavailable",
                    Some(evidence_path_str),
                    Some(format!("evidence file unreadable: {e}")),
                )),
            )
                .into_response();
        }
        Ok(c) => c,
    };

    let raw: serde_json::Value = match serde_json::from_str(&content) {
        Err(e) => {
            return (
                StatusCode::OK,
                Json(kraken_scheduler_task_status_response_for_non_active(
                    "parse_error",
                    Some(evidence_path_str),
                    Some(format!("evidence JSON parse failed: {e}")),
                )),
            )
                .into_response();
        }
        Ok(v) => v,
    };

    let schema_version = raw
        .get("schema_version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if schema_version.as_deref() != Some(KRAKEN_SCHEDULER_TASK_STATUS_SCHEMA_VERSION) {
        return (
            StatusCode::OK,
            Json(kraken_scheduler_task_status_response_for_non_active(
                "parse_error",
                Some(evidence_path_str),
                Some(format!(
                    "unsupported schema_version (expected '{KRAKEN_SCHEDULER_TASK_STATUS_SCHEMA_VERSION}', got {schema_version:?})"
                )),
            )),
        )
            .into_response();
    }

    let (truth_state, error) = match kraken_scheduler_task_status_unsafe_reason(&raw) {
        Some(reason) => ("unsafe_evidence", Some(reason)),
        None => ("active", None),
    };

    let safety = raw
        .get("safety")
        .map(|s| KrakenSchedulerTaskStatusEvidenceSafety {
            calls_runner_script_only: s.get("calls_runner_script_only").and_then(|v| v.as_bool()),
            no_daemon_runtime_broker_provider_order_references: s
                .get("no_daemon_runtime_broker_provider_order_references")
                .and_then(|v| v.as_bool()),
            no_env_vars_embedded_in_task_action: s
                .get("no_env_vars_embedded_in_task_action")
                .and_then(|v| v.as_bool()),
            no_env_local_read: s.get("no_env_local_read").and_then(|v| v.as_bool()),
            task_never_started_by_this_script: s
                .get("task_never_started_by_this_script")
                .and_then(|v| v.as_bool()),
        });

    let response = KrakenSchedulerTaskStatusResponse {
        canonical_route: KRAKEN_SCHEDULER_TASK_STATUS_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        schema_version,
        produced_at_utc: raw
            .get("produced_at_utc")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        mode: raw.get("mode").and_then(|v| v.as_str()).map(|s| s.to_string()),
        task_name: raw
            .get("task_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        task_exists_before: raw.get("task_exists_before").and_then(|v| v.as_bool()),
        task_exists_after: raw.get("task_exists_after").and_then(|v| v.as_bool()),
        registered: raw.get("registered").and_then(|v| v.as_bool()),
        unregistered: raw.get("unregistered").and_then(|v| v.as_bool()),
        check_only: raw.get("check_only").and_then(|v| v.as_bool()),
        task_action: raw
            .get("task_action")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        runner_path: raw
            .get("runner_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        policy_path: raw
            .get("policy_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        registry_path: raw
            .get("registry_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        providers_path: raw
            .get("providers_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        symbols: kraken_scheduler_task_status_str_vec(&raw, "symbols"),
        timeframe: raw
            .get("timeframe")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        at: raw.get("at").and_then(|v| v.as_str()).map(|s| s.to_string()),
        scheduled_task_mutation: raw.get("scheduled_task_mutation").and_then(|v| v.as_bool()),
        network_call_made: raw.get("network_call_made").and_then(|v| v.as_bool()),
        db_write: raw.get("db_write").and_then(|v| v.as_bool()),
        md_bars_write: raw.get("md_bars_write").and_then(|v| v.as_bool()),
        env_vars_embedded: kraken_scheduler_task_status_str_vec(&raw, "env_vars_embedded"),
        env_vars_required: kraken_scheduler_task_status_str_vec(&raw, "env_vars_required"),
        all_passed: raw.get("all_passed").and_then(|v| v.as_bool()),
        reason_code: raw
            .get("reason_code")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        fail_reasons: kraken_scheduler_task_status_str_vec(&raw, "fail_reasons"),
        warnings: kraken_scheduler_task_status_str_vec(&raw, "warnings"),
        safety,
        evidence_path: Some(evidence_path_str),
        error,
    };

    (StatusCode::OK, Json(response)).into_response()
}
