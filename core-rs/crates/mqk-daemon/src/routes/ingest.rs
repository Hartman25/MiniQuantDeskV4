//! DATA-INGEST-DAEMON-JOBS-01: Daemon-managed market-data ingestion job API.
//!
//! Routes:
//!   POST /api/v1/ingest/jobs          — submit a CSV or provider sync job (operator)
//!   POST /api/v1/ingest/jobs/:job_id/cancel — cancel a queued/running job (operator)
//!   GET  /api/v1/ingest/jobs          — list ingest jobs (DB-backed when configured)
//!   GET  /api/v1/ingest/jobs/:job_id  — job status + artifact paths (public)
//!
//! DATA-INGEST-DAEMON-PROVIDER-JOBS-01:
//! - source="<registered provider>" + mode="sync_provider" accepted for dry-run and real sync.
//! - dry_run=true: resolves symbols from registry; makes ZERO provider API calls; writes nothing.
//! - dry_run=false + allow_provider_api_calls=false: refused immediately.
//! - dry_run=false + allow_provider_api_calls=true: job queued; runs via injectable provider.
//!
//! Safety invariants:
//! - No broker adapter is called. No Alpaca. No live/paper execution state.
//! - No OMS tables written. No orders/fills in live DB tables.
//! - Does not require arm_state. Does not start/stop the trading runtime.
//! - Jobs are persisted to DB when a pool is configured; in-memory store remains
//!   the no-DB fallback.
//! - Quality reports are written to exports/md_ingest/<ingest_id>/ by default.
//! - Failed jobs report errors truthfully. No hidden failures.
//! - If DB is unavailable (pool is None), CSV job fails with "no_db" error.
//! - Provider dry-run jobs never call providers or consume API credits.
//! - Provider dry-run jobs never write to DB or CSV.
//! - Real provider jobs use a registry-backed provider factory, or an injectable
//!   client (AppState.provider_client) in tests, so tests verify the real code
//!   path without making network calls.

use std::path::Path;
use std::sync::Arc;

use axum::{
    body::to_bytes,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use tokio::time::{sleep, Duration as TokioDuration};
use uuid::Uuid;

use crate::{
    api_types::{
        IngestJobAcceptedResponse, IngestJobRequest, IngestJobStatusResponse, IngestJobSummary,
        IngestJobsListResponse, IngestPlanCoverageExpectation, IngestPlanResponse,
        IngestPlanSymbolTimeframe, MarketDataFeedPollOnceRequest, MarketDataFeedPollOnceResponse,
        MarketDataFeedPollSymbolResult, MarketDataFeedSchedulerStartRequest,
        MarketDataFeedSchedulerStatusResponse, MarketDataFeedStatusResponse,
        TrackedEquitiesResponse, TrackedEquitySummary,
    },
    ingest_jobs::{
        list_persisted_ingest_jobs, load_persisted_ingest_job, persist_ingest_job_record,
        IngestJobRecord, IngestJobStatus, IngestJobStore,
    },
    market_data_freshness::{
        normalize_required_symbols, required_symbols_with_source_from_env,
        RequiredSymbolsResolution, SYMBOL_SOURCE_WATCHLIST_V2,
    },
    state::{AppState, STRATEGY_MD_TIMEFRAME_ENV},
    watchlist_intake::WatchlistIntakeOutcome,
};

const INGEST_JOB_CANCEL_REASON: &str = "cancel requested by operator";
const FEED_POLL_ROUTE: &str = "/api/v1/market-data/feed/poll-once";
const FEED_STATUS_ROUTE: &str = "/api/v1/market-data/feed/status";
const FEED_SCHEDULER_START_ROUTE: &str = "/api/v1/market-data/feed/scheduler/start";
const FEED_SCHEDULER_STOP_ROUTE: &str = "/api/v1/market-data/feed/scheduler/stop";
const FEED_SCHEDULER_STATUS_ROUTE: &str = "/api/v1/market-data/feed/scheduler/status";
const INGEST_PLAN_ROUTE: &str = "/api/v1/market-data/ingest-plan";
const SCHEDULER_RESPONSE_BODY_LIMIT_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Timeframe validation (no mqk-md dep required for daemon validation)
// ---------------------------------------------------------------------------

/// Validates the timeframe string. Returns the canonical form or an error.
fn validate_timeframe(tf: &str) -> Result<&'static str, String> {
    match tf.trim() {
        "1D" | "1d" => Ok("1D"),
        "1m" | "1min" | "1minute" => Ok("1m"),
        "5m" | "5min" | "5minute" => Ok("5m"),
        other => Err(format!(
            "unsupported timeframe '{}'. accepted: 1D | 1m | 5m",
            other
        )),
    }
}

fn normalize_poll_symbols(symbols: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let symbol = symbol.trim();
        if symbol.is_empty() {
            return Err("symbols must not contain blank entries".to_string());
        }
        out.push(symbol.to_string());
    }
    if out.is_empty() {
        return Err("symbols must contain at least one symbol".to_string());
    }
    Ok(out)
}

fn parse_poll_now(now_utc: Option<&str>) -> Result<DateTime<Utc>, String> {
    match now_utc {
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|err| format!("now_utc must be RFC3339 UTC: {err}")),
        None => Ok(Utc::now()), // allow: operator poll reference time
    }
}

fn provider_registry_path_for_poll(st: &AppState, req: &MarketDataFeedPollOnceRequest) -> String {
    req.provider_registry_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(&st.provider_registry_path)
        .to_string()
}

fn instrument_registry_path_for_poll(st: &AppState, req: &MarketDataFeedPollOnceRequest) -> String {
    req.instrument_registry_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(&st.instrument_registry_path)
        .to_string()
}

fn load_poll_provider_config(
    provider_id: &str,
    provider_registry_path: &str,
) -> Result<mqk_md::ProviderConfig, String> {
    let providers = mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
        provider_registry_path,
    ))
    .map_err(|err| format!("provider registry load failed: {err}"))?;
    let config = mqk_md::provider_registry::find_provider(&providers, provider_id)
        .ok_or_else(|| format!("unknown_provider: provider '{provider_id}' is not registered"))?;
    if !config.enabled {
        return Err(format!(
            "disabled_provider: provider '{}' is disabled",
            config.provider_id
        ));
    }
    Ok(config.clone())
}

enum LatestBarProviderClient {
    Injected(Arc<dyn mqk_md::MarketDataProvider>),
    Built(mqk_md::MarketDataProviderBox),
}

impl LatestBarProviderClient {
    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        match self {
            LatestBarProviderClient::Injected(provider) => provider.capabilities(),
            LatestBarProviderClient::Built(provider) => provider.capabilities(),
        }
    }

    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        match self {
            LatestBarProviderClient::Injected(provider) => {
                provider.fetch_latest_closed_bar(request).await
            }
            LatestBarProviderClient::Built(provider) => {
                provider.fetch_latest_closed_bar(request).await
            }
        }
    }
}

/// Shared poll-time context threaded through every `poll_response` /
/// `refused_poll_response` call within `market_data_feed_poll_once`.
struct PollContext {
    provider_id: String,
    timeframe: String,
    dry_run: bool,
    provider_api_calls_allowed: bool,
    symbols_count: usize,
    poll_time: DateTime<Utc>,
    latest_expected_closed_bar_ts: i64,
    next_poll_ts: i64,
}

fn poll_response(
    ctx: PollContext,
    truth_state: &str,
    symbols: Vec<MarketDataFeedPollSymbolResult>,
    api_calls_made: u64,
    error: Option<String>,
) -> MarketDataFeedPollOnceResponse {
    let inserted_count = symbols.iter().map(|s| s.rows_inserted).sum();
    let updated_count = symbols.iter().map(|s| s.rows_updated).sum();
    let skipped_count = symbols.iter().map(|s| s.rows_skipped).sum();
    let error_count = symbols.iter().filter(|s| s.error.is_some()).count() as u64;

    MarketDataFeedPollOnceResponse {
        canonical_route: FEED_POLL_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        provider_id: ctx.provider_id,
        timeframe: ctx.timeframe,
        dry_run: ctx.dry_run,
        provider_api_calls_allowed: ctx.provider_api_calls_allowed,
        symbols_count: ctx.symbols_count,
        poll_time_utc: ctx.poll_time.to_rfc3339(),
        latest_expected_closed_bar_ts: ctx.latest_expected_closed_bar_ts,
        next_poll_ts: ctx.next_poll_ts,
        inserted_count,
        updated_count,
        skipped_count,
        error_count,
        api_calls_made,
        symbols,
        error,
    }
}

async fn store_feed_poll_status(st: &AppState, response: &MarketDataFeedPollOnceResponse) {
    let mut status = st.market_data_feed_status.write().await;
    *status = Some(response.clone());
}

fn refused_poll_response(status: StatusCode, ctx: PollContext, error: String) -> Response {
    (
        status,
        Json(poll_response(ctx, "refused", vec![], 0, Some(error))),
    )
        .into_response()
}

fn record_to_summary(r: &IngestJobRecord) -> IngestJobSummary {
    IngestJobSummary {
        job_id: r.job_id,
        status: r.status.as_str().to_string(),
        source: r.source.clone(),
        mode: r.mode.clone(),
        timeframe: r.timeframe.clone(),
        created_at_utc: r.created_at_utc.to_rfc3339(),
        started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
        completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
        rows_read: r.rows_read,
        rows_inserted: r.rows_inserted,
        rows_rejected: r.rows_rejected,
        quality_report_path: r.quality_report_path.clone(),
        error: r.error.clone(),
        dry_run: r.dry_run,
        symbols_count: r.symbols_count,
        api_calls_made: r.api_calls_made,
        symbols_completed: r.symbols_completed,
        symbols_failed: r.symbols_failed,
    }
}

fn record_to_status_response(r: &IngestJobRecord) -> IngestJobStatusResponse {
    IngestJobStatusResponse {
        truth_state: "active".to_string(),
        job_id: r.job_id,
        status: r.status.as_str().to_string(),
        source: r.source.clone(),
        mode: r.mode.clone(),
        timeframe: r.timeframe.clone(),
        csv_path: r.csv_path.clone(),
        created_at_utc: r.created_at_utc.to_rfc3339(),
        started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
        completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
        rows_read: r.rows_read,
        rows_inserted: r.rows_inserted,
        rows_rejected: r.rows_rejected,
        quality_report_path: r.quality_report_path.clone(),
        error: r.error.clone(),
        dry_run: r.dry_run,
        provider_api_calls_allowed: r.provider_api_calls_allowed,
        api_calls_made: r.api_calls_made,
        symbols_source: r.symbols_source.clone(),
        registry_path_used: r.registry_path_used.clone(),
        symbols_count: r.symbols_count,
        planned_first_symbol: r.planned_first_symbol.clone(),
        planned_last_symbol: r.planned_last_symbol.clone(),
        asset_class: r.asset_class.clone(),
        provider_enabled: r.provider_enabled,
        provider_verification_status: r.provider_verification_status.clone(),
        symbols_completed: r.symbols_completed,
        symbols_failed: r.symbols_failed,
    }
}

async fn persist_and_insert_job(st: &AppState, record: IngestJobRecord) -> Result<(), Response> {
    if let Some(pool) = &st.db {
        persist_ingest_job_record(pool, &record)
            .await
            .map_err(|e| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "truth_state": "backend_unavailable",
                        "error": e
                    })),
                )
                    .into_response()
            })?;
    }

    let mut store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");
    store.insert(record.job_id, record);
    Ok(())
}

fn mutate_job<F>(jobs: &IngestJobStore, job_id: Uuid, mutate: F) -> Option<IngestJobRecord>
where
    F: FnOnce(&mut IngestJobRecord),
{
    let mut store = jobs.lock().expect("ingest_jobs lock poisoned");
    let record = store.get_mut(&job_id)?;
    let was_cancelled = record.status == IngestJobStatus::Cancelled;
    mutate(record);
    if was_cancelled {
        record.status = IngestJobStatus::Cancelled;
        if record.completed_at_utc.is_none() {
            record.completed_at_utc = Some(Utc::now());
        }
        if record.error.is_none() {
            record.error = Some(INGEST_JOB_CANCEL_REASON.to_string());
        }
    }
    Some(record.clone())
}

fn ingest_job_is_cancelled(jobs: &IngestJobStore, job_id: Uuid) -> bool {
    let store = jobs.lock().expect("ingest_jobs lock poisoned");
    store
        .get(&job_id)
        .map(|r| r.status == IngestJobStatus::Cancelled)
        .unwrap_or(false)
}

async fn persist_job_update<F>(
    jobs: &IngestJobStore,
    db_pool: Option<&sqlx::PgPool>,
    job_id: Uuid,
    mutate: F,
) where
    F: FnOnce(&mut IngestJobRecord),
{
    let Some(record) = mutate_job(jobs, job_id, mutate) else {
        return;
    };

    if let Some(pool) = db_pool {
        if let Err(err) = persist_ingest_job_record(pool, &record).await {
            tracing::error!(
                job_id = %job_id,
                error = %err,
                "ingest job persistence update failed"
            );
            let failed_record = mutate_job(jobs, job_id, |r| {
                r.status = IngestJobStatus::Failed;
                r.completed_at_utc = Some(Utc::now());
                r.error = Some(format!("ingest job persistence update failed: {err}"));
            });
            if let Some(failed_record) = failed_record {
                let _ = persist_ingest_job_record(pool, &failed_record).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn refused_provider_job_response(
    st: &Arc<AppState>,
    source: String,
    req: &IngestJobRequest,
    error: String,
    dry_run: Option<bool>,
    allow_provider: Option<bool>,
    asset_class: Option<String>,
    registry_path: Option<String>,
    provider_registry_path: Option<String>,
    provider_enabled: Option<bool>,
    provider_verification_status: Option<String>,
) -> Response {
    let job_id = Uuid::new_v4(); // allow: operator-visible refused job identifier
    let now = Utc::now(); // allow: operational job refusal timestamp
    let dry_run_value = dry_run.unwrap_or(req.dry_run);
    let allow_provider_value = allow_provider.unwrap_or(req.allow_provider_api_calls);
    let symbols_source = req
        .symbols_source
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_ascii_lowercase());
    let record = IngestJobRecord {
        job_id,
        source: source.clone(),
        mode: Some("sync_provider".to_string()),
        csv_path: None,
        timeframe: req.timeframe.trim().to_string(),
        source_label: source.clone(),
        out_dir: req
            .out_dir
            .clone()
            .unwrap_or_else(|| "exports/md_ingest".to_string()),
        status: IngestJobStatus::Refused,
        created_at_utc: now,
        started_at_utc: None,
        completed_at_utc: Some(now),
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: Some(error.clone()),
        dry_run: dry_run_value,
        provider_api_calls_allowed: allow_provider_value,
        api_calls_made: 0,
        symbols_source,
        registry_path_used: registry_path.or_else(|| {
            req.registry_path
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
        }),
        provider_registry_path_used: provider_registry_path.or_else(|| {
            req.provider_registry_path
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
        }),
        symbols_count: None,
        planned_first_symbol: None,
        planned_last_symbol: None,
        asset_class: asset_class.unwrap_or_else(|| req.asset_class.trim().to_ascii_lowercase()),
        provider_enabled,
        provider_verification_status,
        symbols_completed: None,
        symbols_failed: None,
    };

    if let Err(resp) = persist_and_insert_job(st, record).await {
        return resp;
    }

    (
        StatusCode::BAD_REQUEST,
        Json(IngestJobAcceptedResponse {
            accepted: false,
            job_id,
            status: "refused".to_string(),
            source,
            error: Some(error),
            dry_run: Some(dry_run_value),
            provider_api_calls_allowed: Some(allow_provider_value),
            symbols_count: None,
            api_calls_made: Some(0),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/market-data/feed/poll-once
// ---------------------------------------------------------------------------

pub(crate) async fn market_data_feed_poll_once(
    State(st): State<Arc<AppState>>,
    Json(req): Json<MarketDataFeedPollOnceRequest>,
) -> Response {
    let provider_id = req.provider_id.trim().to_ascii_lowercase();
    let symbols = match normalize_poll_symbols(&req.symbols) {
        Ok(symbols) => symbols,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "canonical_route": FEED_POLL_ROUTE,
                    "truth_state": "refused",
                    "error": error,
                })),
            )
                .into_response();
        }
    };
    let poll_time = match parse_poll_now(req.now_utc.as_deref()) {
        Ok(now) => now,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "canonical_route": FEED_POLL_ROUTE,
                    "truth_state": "refused",
                    "error": error,
                })),
            )
                .into_response();
        }
    };
    let timeframe = match mqk_md::Timeframe::parse(&req.timeframe) {
        Ok(timeframe) => timeframe,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "canonical_route": FEED_POLL_ROUTE,
                    "truth_state": "refused",
                    "error": error.to_string(),
                })),
            )
                .into_response();
        }
    };

    let latest_expected_closed_bar_ts =
        mqk_md::latest_closed_bar_end_ts(timeframe, poll_time.timestamp());
    let next_poll_ts = mqk_md::next_poll_time_ts(timeframe, poll_time.timestamp());
    let timeframe_str = timeframe.as_str().to_string();

    if provider_id.is_empty() {
        return refused_poll_response(
            StatusCode::BAD_REQUEST,
            PollContext {
                provider_id,
                timeframe: timeframe_str,
                dry_run: req.dry_run,
                provider_api_calls_allowed: req.allow_provider_api_calls,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            "provider_id must not be empty".to_string(),
        );
    }

    if !req.dry_run && !req.allow_provider_api_calls {
        return refused_poll_response(
            StatusCode::BAD_REQUEST,
            PollContext {
                provider_id,
                timeframe: timeframe_str,
                dry_run: false,
                provider_api_calls_allowed: false,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            "allow_provider_api_calls=true is required when dry_run=false".to_string(),
        );
    }

    let provider_registry_path = provider_registry_path_for_poll(&st, &req);
    let provider_config = match load_poll_provider_config(&provider_id, &provider_registry_path) {
        Ok(config) => config,
        Err(error) => {
            return refused_poll_response(
                StatusCode::BAD_REQUEST,
                PollContext {
                    provider_id,
                    timeframe: timeframe_str,
                    dry_run: req.dry_run,
                    provider_api_calls_allowed: req.allow_provider_api_calls,
                    symbols_count: symbols.len(),
                    poll_time,
                    latest_expected_closed_bar_ts,
                    next_poll_ts,
                },
                error,
            );
        }
    };

    if req.dry_run {
        let per_symbol = symbols
            .iter()
            .map(|symbol| MarketDataFeedPollSymbolResult {
                symbol: symbol.clone(),
                status: "dry_run".to_string(),
                expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                returned_bar_ts: None,
                rows_inserted: 0,
                rows_updated: 0,
                rows_skipped: 0,
                error: None,
            })
            .collect();
        let response = poll_response(
            PollContext {
                provider_id: provider_config.provider_id,
                timeframe: timeframe_str,
                dry_run: true,
                provider_api_calls_allowed: false,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            "dry_run",
            per_symbol,
            0,
            None,
        );
        store_feed_poll_status(&st, &response).await;
        return (StatusCode::OK, Json(response)).into_response();
    }

    let Some(pool) = st.db.clone() else {
        return refused_poll_response(
            StatusCode::SERVICE_UNAVAILABLE,
            PollContext {
                provider_id: provider_config.provider_id,
                timeframe: timeframe_str,
                dry_run: false,
                provider_api_calls_allowed: true,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            "no_db: database pool is not configured".to_string(),
        );
    };

    let provider = if let Some(provider) = st.latest_bar_provider_client.clone() {
        LatestBarProviderClient::Injected(provider)
    } else {
        match mqk_md::build_market_data_provider_from_config(&provider_config, |name| {
            std::env::var(name).ok()
        }) {
            Ok(provider) => LatestBarProviderClient::Built(provider),
            Err(error) => {
                return refused_poll_response(
                    StatusCode::BAD_REQUEST,
                    PollContext {
                        provider_id: provider_config.provider_id,
                        timeframe: timeframe_str,
                        dry_run: false,
                        provider_api_calls_allowed: true,
                        symbols_count: symbols.len(),
                        poll_time,
                        latest_expected_closed_bar_ts,
                        next_poll_ts,
                    },
                    error.to_string(),
                );
            }
        }
    };

    let capabilities = provider.capabilities();
    if !capabilities.latest_closed_bar {
        return refused_poll_response(
            StatusCode::BAD_REQUEST,
            PollContext {
                provider_id: provider_config.provider_id.clone(),
                timeframe: timeframe_str,
                dry_run: false,
                provider_api_calls_allowed: true,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            format!(
                "provider '{}' does not support capability latest_closed_bar",
                provider_config.provider_id
            ),
        );
    }
    if !capabilities.supported_timeframes.is_empty()
        && !capabilities.supported_timeframes.contains(&timeframe)
    {
        return refused_poll_response(
            StatusCode::BAD_REQUEST,
            PollContext {
                provider_id: provider_config.provider_id.clone(),
                timeframe: timeframe_str,
                dry_run: false,
                provider_api_calls_allowed: true,
                symbols_count: symbols.len(),
                poll_time,
                latest_expected_closed_bar_ts,
                next_poll_ts,
            },
            format!(
                "provider '{}' does not support timeframe '{}'",
                provider_config.provider_id,
                timeframe.as_str()
            ),
        );
    }

    // B2.4: resolve the canonical instrument registry once, up front, so
    // per-symbol provenance can be stamped from the registry's own
    // provider_symbol mapping rather than blindly reusing whatever symbol
    // string the provider echoes back. Load failure fails closed (empty
    // list — no instrument found for any symbol, no acceptance of unproven
    // provenance) rather than optimistically skipping the check.
    let instrument_registry_path = instrument_registry_path_for_poll(&st, &req);
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        &instrument_registry_path,
    ))
    .unwrap_or_default();

    let mut per_symbol = Vec::with_capacity(symbols.len());
    let mut api_calls_made = 0_u64;

    for symbol in &symbols {
        api_calls_made += 1;
        let request = mqk_md::LatestClosedBarRequest {
            symbol: symbol.clone(),
            timeframe,
            reference_ts: poll_time.timestamp(),
        };

        let latest_bar = match provider.fetch_latest_closed_bar(request).await {
            Ok(Some(bar)) => bar,
            Ok(None) => {
                per_symbol.push(MarketDataFeedPollSymbolResult {
                    symbol: symbol.clone(),
                    status: "skipped_no_bar".to_string(),
                    expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                    returned_bar_ts: None,
                    rows_inserted: 0,
                    rows_updated: 0,
                    rows_skipped: 1,
                    error: None,
                });
                continue;
            }
            Err(error) => {
                per_symbol.push(MarketDataFeedPollSymbolResult {
                    symbol: symbol.clone(),
                    status: "provider_error".to_string(),
                    expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                    returned_bar_ts: None,
                    rows_inserted: 0,
                    rows_updated: 0,
                    rows_skipped: 0,
                    error: Some(error.to_string()),
                });
                continue;
            }
        };

        if latest_bar.symbol != *symbol
            || latest_bar.timeframe != timeframe.as_str()
            || !latest_bar.is_complete
            || latest_bar.end_ts > latest_expected_closed_bar_ts
        {
            per_symbol.push(MarketDataFeedPollSymbolResult {
                symbol: symbol.clone(),
                status: "skipped_unclosed_or_unexpected_bar".to_string(),
                expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                returned_bar_ts: Some(latest_bar.end_ts),
                rows_inserted: 0,
                rows_updated: 0,
                rows_skipped: 1,
                error: None,
            });
            continue;
        }

        // B2.4: resolve the canonical instrument for this symbol and require
        // its configured provider to match the poll's selected provider —
        // never stamp `provider_symbol` from the returned local symbol
        // (unproven) when a canonical registry mapping is available.
        let matched_instrument = instruments
            .iter()
            .find(|i| i.symbol.trim().eq_ignore_ascii_case(symbol.trim()));
        let instrument = match matched_instrument {
            Some(i) => i,
            None => {
                per_symbol.push(MarketDataFeedPollSymbolResult {
                    symbol: symbol.clone(),
                    status: "skipped_instrument_not_in_registry".to_string(),
                    expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                    returned_bar_ts: Some(latest_bar.end_ts),
                    rows_inserted: 0,
                    rows_updated: 0,
                    rows_skipped: 1,
                    error: Some(format!(
                        "symbol '{symbol}' not found in instrument registry; \
                         cannot resolve canonical provider_symbol"
                    )),
                });
                continue;
            }
        };
        if !instrument
            .provider
            .trim()
            .eq_ignore_ascii_case(provider_config.provider_id.trim())
        {
            per_symbol.push(MarketDataFeedPollSymbolResult {
                symbol: symbol.clone(),
                status: "skipped_provider_mismatch".to_string(),
                expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                returned_bar_ts: Some(latest_bar.end_ts),
                rows_inserted: 0,
                rows_updated: 0,
                rows_skipped: 1,
                error: Some(format!(
                    "instrument '{symbol}' is configured for provider '{}', not the requested \
                     poll provider '{}'",
                    instrument.provider, provider_config.provider_id
                )),
            });
            continue;
        }

        let ingest_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk-md-latest-poll.v1|{}|{}|{}|{}",
                provider_config.provider_id,
                timeframe.as_str(),
                symbol,
                latest_bar.end_ts
            )
            .as_bytes(),
        );
        let provider_symbol = instrument.provider_symbol.clone();
        let db_bar = mqk_db::md::ProviderBar {
            symbol: latest_bar.symbol,
            timeframe: latest_bar.timeframe,
            end_ts: latest_bar.end_ts,
            open: latest_bar.open,
            high: latest_bar.high,
            low: latest_bar.low,
            close: latest_bar.close,
            volume: latest_bar.volume,
            is_complete: latest_bar.is_complete,
        };
        let returned_bar_ts = db_bar.end_ts;

        match mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
            &pool,
            mqk_db::md::IngestProviderBarsArgs {
                source: provider_config.provider_id.clone(),
                timeframe: timeframe.as_str().to_string(),
                ingest_id,
                bars: vec![db_bar],
            },
            mqk_db::md::MdBarProviderMetadata {
                provider_id: provider_config.provider_id.clone(),
                provider_source: Some(provider_config.provider_id.clone()),
                provider_symbol: Some(provider_symbol),
                ingest_mode: Some("latest_poll".to_string()),
                provider_bar_id: None,
                provider_updated_at_utc: None,
            },
        )
        .await
        {
            Ok(result) => per_symbol.push(MarketDataFeedPollSymbolResult {
                symbol: symbol.clone(),
                status: if result.report.coverage.rows_inserted > 0 {
                    "inserted".to_string()
                } else {
                    "updated".to_string()
                },
                expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                returned_bar_ts: Some(returned_bar_ts),
                rows_inserted: result.report.coverage.rows_inserted,
                rows_updated: result.report.coverage.rows_updated,
                rows_skipped: result.report.coverage.rows_rejected,
                error: None,
            }),
            Err(error) => per_symbol.push(MarketDataFeedPollSymbolResult {
                symbol: symbol.clone(),
                status: "db_error".to_string(),
                expected_latest_closed_bar_ts: latest_expected_closed_bar_ts,
                returned_bar_ts: Some(returned_bar_ts),
                rows_inserted: 0,
                rows_updated: 0,
                rows_skipped: 0,
                error: Some(format!("db ingest failed: {error}")),
            }),
        }
    }

    let error_count = per_symbol.iter().filter(|s| s.error.is_some()).count();
    let success_count = per_symbol
        .iter()
        .filter(|s| s.rows_inserted > 0 || s.rows_updated > 0)
        .count();
    let truth_state = if error_count == 0 {
        "completed"
    } else if success_count > 0 {
        "partial"
    } else {
        "failed"
    };
    let error = (error_count > 0).then(|| format!("{error_count} symbol(s) failed"));
    let status_code = if truth_state == "failed" {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::OK
    };
    let response = poll_response(
        PollContext {
            provider_id: provider_config.provider_id,
            timeframe: timeframe_str,
            dry_run: false,
            provider_api_calls_allowed: true,
            symbols_count: symbols.len(),
            poll_time,
            latest_expected_closed_bar_ts,
            next_poll_ts,
        },
        truth_state,
        per_symbol,
        api_calls_made,
        error,
    );
    store_feed_poll_status(&st, &response).await;
    (status_code, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/feed/status
// ---------------------------------------------------------------------------

pub(crate) async fn market_data_feed_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let last_poll = st.market_data_feed_status.read().await.clone();
    Json(MarketDataFeedStatusResponse {
        canonical_route: FEED_STATUS_ROUTE.to_string(),
        truth_state: if last_poll.is_some() {
            "active".to_string()
        } else {
            "no_poll".to_string()
        },
        limitation: "process_local_only_not_persisted".to_string(),
        last_poll,
    })
}

#[derive(Clone)]
struct MarketDataFeedSchedulerPollConfig {
    provider_id: String,
    symbols: Vec<String>,
    timeframe: mqk_md::Timeframe,
    dry_run: bool,
    allow_provider_api_calls: bool,
    provider_registry_path: Option<String>,
}

fn ts_to_utc_string(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(ts, 0)
        .expect("scheduler timestamp must be representable")
        .to_rfc3339()
}

fn scheduler_status_response(
    canonical_route: &str,
    truth_state: &str,
    scheduler: &crate::state::MarketDataFeedSchedulerRuntimeState,
) -> MarketDataFeedSchedulerStatusResponse {
    MarketDataFeedSchedulerStatusResponse {
        canonical_route: canonical_route.to_string(),
        truth_state: truth_state.to_string(),
        limitation: "process_local_only_not_persisted".to_string(),
        running: scheduler.running,
        provider_id: scheduler.provider_id.clone(),
        timeframe: scheduler.timeframe.map(|tf| tf.as_str().to_string()),
        symbols: scheduler.symbols.clone(),
        last_poll_utc: scheduler.last_poll_ts.map(ts_to_utc_string),
        next_poll_utc: scheduler.next_poll_ts.map(ts_to_utc_string),
        latest_expected_closed_bar_utc: scheduler
            .latest_expected_closed_bar_ts
            .map(ts_to_utc_string),
        last_result: scheduler.last_result.clone(),
        last_error: scheduler.last_error.clone(),
        started_at_utc: scheduler.started_at_ts.map(ts_to_utc_string),
        stopped_at_utc: scheduler.stopped_at_ts.map(ts_to_utc_string),
        poll_count: scheduler.poll_count,
        inserted_count: scheduler.inserted_count,
        unchanged_or_skipped_count: scheduler.unchanged_or_skipped_count,
        error_count: scheduler.error_count,
    }
}

fn scheduler_refused_response(
    status: StatusCode,
    canonical_route: &str,
    error: String,
) -> Response {
    (
        status,
        Json(MarketDataFeedSchedulerStatusResponse {
            canonical_route: canonical_route.to_string(),
            truth_state: "refused".to_string(),
            limitation: "process_local_only_not_persisted".to_string(),
            running: false,
            provider_id: None,
            timeframe: None,
            symbols: vec![],
            last_poll_utc: None,
            next_poll_utc: None,
            latest_expected_closed_bar_utc: None,
            last_result: None,
            last_error: Some(error),
            started_at_utc: None,
            stopped_at_utc: None,
            poll_count: 0,
            inserted_count: 0,
            unchanged_or_skipped_count: 0,
            error_count: 1,
        }),
    )
        .into_response()
}

async fn execute_scheduler_poll_once(
    st: Arc<AppState>,
    config: MarketDataFeedSchedulerPollConfig,
    poll_time: DateTime<Utc>,
) -> Result<(StatusCode, MarketDataFeedPollOnceResponse), String> {
    let response = market_data_feed_poll_once(
        State(st),
        Json(MarketDataFeedPollOnceRequest {
            provider_id: config.provider_id,
            symbols: config.symbols,
            timeframe: config.timeframe.as_str().to_string(),
            dry_run: config.dry_run,
            allow_provider_api_calls: config.allow_provider_api_calls,
            now_utc: Some(poll_time.to_rfc3339()),
            provider_registry_path: config.provider_registry_path,
            instrument_registry_path: None,
        }),
    )
    .await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), SCHEDULER_RESPONSE_BODY_LIMIT_BYTES)
        .await
        .map_err(|err| format!("scheduler poll response read failed: {err}"))?;
    let parsed = serde_json::from_slice::<MarketDataFeedPollOnceResponse>(&bytes)
        .map_err(|err| format!("scheduler poll response parse failed: {err}"))?;
    Ok((status, parsed))
}

async fn apply_scheduler_poll_result(
    st: &Arc<AppState>,
    timeframe: mqk_md::Timeframe,
    poll_time: DateTime<Utc>,
    result: Result<(StatusCode, MarketDataFeedPollOnceResponse), String>,
) {
    let mut scheduler = st.market_data_feed_scheduler.lock().await;
    if !scheduler.running {
        return;
    }

    scheduler.last_poll_ts = Some(poll_time.timestamp());
    scheduler.latest_expected_closed_bar_ts = Some(mqk_md::latest_closed_bar_end_ts(
        timeframe,
        poll_time.timestamp(),
    ));
    scheduler.next_poll_ts = Some(mqk_md::next_poll_time_ts(timeframe, poll_time.timestamp()));
    scheduler.poll_count += 1;

    match result {
        Ok((status, response)) => {
            scheduler.inserted_count += response.inserted_count;
            scheduler.unchanged_or_skipped_count += response.updated_count + response.skipped_count;
            if status.is_success() {
                scheduler.last_error = response.error.clone();
                scheduler.error_count += response.error_count;
            } else {
                let error = response
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("poll-once returned HTTP {status}"));
                scheduler.last_error = Some(error);
                scheduler.error_count += response.error_count.max(1);
            }
            scheduler.last_result = Some(response);
        }
        Err(error) => {
            scheduler.last_error = Some(error);
            scheduler.error_count += 1;
        }
    }
}

async fn market_data_feed_scheduler_loop(
    st: Arc<AppState>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let (next_poll_ts, config) = {
            let scheduler = st.market_data_feed_scheduler.lock().await;
            if !scheduler.running {
                return;
            }
            let Some(timeframe) = scheduler.timeframe else {
                return;
            };
            let Some(provider_id) = scheduler.provider_id.clone() else {
                return;
            };
            (
                scheduler
                    .next_poll_ts
                    .unwrap_or_else(|| Utc::now().timestamp()),
                MarketDataFeedSchedulerPollConfig {
                    provider_id,
                    symbols: scheduler.symbols.clone(),
                    timeframe,
                    dry_run: scheduler.dry_run,
                    allow_provider_api_calls: scheduler.allow_provider_api_calls,
                    provider_registry_path: scheduler.provider_registry_path.clone(),
                },
            )
        };

        let now_ts = Utc::now().timestamp();
        let effective_next_poll_ts = if next_poll_ts <= now_ts {
            mqk_md::next_poll_time_ts(config.timeframe, now_ts)
        } else {
            next_poll_ts
        };
        let wait_secs = effective_next_poll_ts.saturating_sub(now_ts) as u64;
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    return;
                }
            }
            _ = sleep(TokioDuration::from_secs(wait_secs)) => {}
        }

        if *stop_rx.borrow() {
            return;
        }

        let poll_time = Utc::now();
        let result = execute_scheduler_poll_once(st.clone(), config.clone(), poll_time).await;
        apply_scheduler_poll_result(&st, config.timeframe, poll_time, result).await;
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/market-data/feed/scheduler/start
// ---------------------------------------------------------------------------

pub(crate) async fn market_data_feed_scheduler_start(
    State(st): State<Arc<AppState>>,
    Json(req): Json<MarketDataFeedSchedulerStartRequest>,
) -> Response {
    let provider_id = req.provider_id.trim().to_ascii_lowercase();
    if provider_id.is_empty() {
        return scheduler_refused_response(
            StatusCode::BAD_REQUEST,
            FEED_SCHEDULER_START_ROUTE,
            "provider_id must not be empty".to_string(),
        );
    }

    let symbols = match normalize_poll_symbols(&req.symbols) {
        Ok(symbols) => symbols,
        Err(error) => {
            return scheduler_refused_response(
                StatusCode::BAD_REQUEST,
                FEED_SCHEDULER_START_ROUTE,
                error,
            );
        }
    };
    let timeframe = match mqk_md::Timeframe::parse(&req.timeframe) {
        Ok(timeframe) => timeframe,
        Err(error) => {
            return scheduler_refused_response(
                StatusCode::BAD_REQUEST,
                FEED_SCHEDULER_START_ROUTE,
                error.to_string(),
            );
        }
    };
    let start_time = match parse_poll_now(req.now_utc.as_deref()) {
        Ok(now) => now,
        Err(error) => {
            return scheduler_refused_response(
                StatusCode::BAD_REQUEST,
                FEED_SCHEDULER_START_ROUTE,
                error,
            );
        }
    };

    if !req.dry_run && !req.allow_provider_api_calls {
        return scheduler_refused_response(
            StatusCode::BAD_REQUEST,
            FEED_SCHEDULER_START_ROUTE,
            "allow_provider_api_calls=true is required when dry_run=false".to_string(),
        );
    }

    let provider_registry_path = req
        .provider_registry_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(&st.provider_registry_path)
        .to_string();
    let provider_config = match load_poll_provider_config(&provider_id, &provider_registry_path) {
        Ok(config) => config,
        Err(error) => {
            return scheduler_refused_response(
                StatusCode::BAD_REQUEST,
                FEED_SCHEDULER_START_ROUTE,
                error,
            );
        }
    };

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let latest_expected_closed_bar_ts =
        mqk_md::latest_closed_bar_end_ts(timeframe, start_time.timestamp());
    let next_poll_ts = mqk_md::next_poll_time_ts(timeframe, start_time.timestamp());
    {
        let mut scheduler = st.market_data_feed_scheduler.lock().await;
        if scheduler.running {
            scheduler.last_error = Some("latest-bar scheduler is already running".to_string());
            let response = scheduler_status_response(
                FEED_SCHEDULER_START_ROUTE,
                "already_running",
                &scheduler,
            );
            return (StatusCode::CONFLICT, Json(response)).into_response();
        }

        *scheduler = crate::state::MarketDataFeedSchedulerRuntimeState {
            running: true,
            provider_id: Some(provider_config.provider_id.clone()),
            timeframe: Some(timeframe),
            symbols: symbols.clone(),
            dry_run: req.dry_run,
            allow_provider_api_calls: req.allow_provider_api_calls,
            provider_registry_path: Some(provider_registry_path.clone()),
            last_poll_ts: None,
            next_poll_ts: Some(next_poll_ts),
            latest_expected_closed_bar_ts: Some(latest_expected_closed_bar_ts),
            last_result: None,
            last_error: None,
            started_at_ts: Some(start_time.timestamp()),
            stopped_at_ts: None,
            poll_count: 0,
            inserted_count: 0,
            unchanged_or_skipped_count: 0,
            error_count: 0,
            stop_tx: Some(stop_tx),
            task: None,
        };
    }

    let config = MarketDataFeedSchedulerPollConfig {
        provider_id: provider_config.provider_id,
        symbols,
        timeframe,
        dry_run: req.dry_run,
        allow_provider_api_calls: req.allow_provider_api_calls,
        provider_registry_path: Some(provider_registry_path),
    };

    if req.poll_immediately {
        let result = execute_scheduler_poll_once(st.clone(), config.clone(), start_time).await;
        apply_scheduler_poll_result(&st, timeframe, start_time, result).await;
    }

    let task = tokio::spawn(market_data_feed_scheduler_loop(st.clone(), stop_rx));
    {
        let mut scheduler = st.market_data_feed_scheduler.lock().await;
        if scheduler.running {
            scheduler.task = Some(task);
        } else {
            task.abort();
        }
        let response = scheduler_status_response(FEED_SCHEDULER_START_ROUTE, "started", &scheduler);
        (StatusCode::OK, Json(response)).into_response()
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/market-data/feed/scheduler/stop
// ---------------------------------------------------------------------------

pub(crate) async fn market_data_feed_scheduler_stop(State(st): State<Arc<AppState>>) -> Response {
    let mut scheduler = st.market_data_feed_scheduler.lock().await;
    if scheduler.running {
        scheduler.running = false;
        scheduler.stopped_at_ts = Some(Utc::now().timestamp());
        let _ = scheduler.stop_tx.as_ref().map(|tx| tx.send(true));
        if let Some(task) = scheduler.task.take() {
            task.abort();
        }
        scheduler.stop_tx = None;
        scheduler.next_poll_ts = None;
    }
    let truth_state = if scheduler.stopped_at_ts.is_some() {
        "stopped"
    } else {
        "not_running"
    };
    let response = scheduler_status_response(FEED_SCHEDULER_STOP_ROUTE, truth_state, &scheduler);
    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/feed/scheduler/status
// ---------------------------------------------------------------------------

pub(crate) async fn market_data_feed_scheduler_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let scheduler = st.market_data_feed_scheduler.lock().await;
    let truth_state = if scheduler.running {
        "running"
    } else if scheduler.stopped_at_ts.is_some() {
        "stopped"
    } else {
        "not_started"
    };
    Json(scheduler_status_response(
        FEED_SCHEDULER_STATUS_ROUTE,
        truth_state,
        &scheduler,
    ))
}

// ---------------------------------------------------------------------------
// POST /api/v1/ingest/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_job_submit(
    State(st): State<Arc<AppState>>,
    Json(req): Json<IngestJobRequest>,
) -> Response {
    let source = req.source.trim().to_ascii_lowercase();

    if source == "csv" {
        return handle_csv_job(st, req, source).await;
    }

    let mode = req
        .mode
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mode == "sync_provider" {
        return handle_provider_sync_job(st, req, source).await;
    }

    if !source.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(IngestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                source: source.clone(),
                error: Some(format!(
                    "provider source '{}' requires mode='sync_provider'; \
                     got mode='{}'",
                    source,
                    req.mode.as_deref().unwrap_or("(absent)")
                )),
                dry_run: None,
                provider_api_calls_allowed: None,
                symbols_count: None,
                api_calls_made: None,
            }),
        )
            .into_response();
    }

    // Empty source or otherwise malformed non-provider request.
    (
        StatusCode::BAD_REQUEST,
        Json(IngestJobAcceptedResponse {
            accepted: false,
            job_id: Uuid::nil(),
            status: "refused".to_string(),
            source: source.clone(),
            error: Some(format!(
                "source '{}' is not implemented in this version. \
                 supported: 'csv' or registered providers with mode='sync_provider'.",
                source
            )),
            dry_run: None,
            provider_api_calls_allowed: None,
            symbols_count: None,
            api_calls_made: None,
        }),
    )
        .into_response()
}

enum IngestCancelOutcome {
    Accepted(IngestJobRecord),
    AlreadyTerminal(IngestJobRecord),
}

fn cancel_record_in_memory(jobs: &IngestJobStore, job_id: Uuid) -> Option<IngestCancelOutcome> {
    let mut store = jobs.lock().expect("ingest_jobs lock poisoned");
    let record = store.get_mut(&job_id)?;
    if record.status.is_terminal() {
        return Some(IngestCancelOutcome::AlreadyTerminal(record.clone()));
    }

    record.status = IngestJobStatus::Cancelled;
    record.completed_at_utc = Some(Utc::now());
    record.error = Some(INGEST_JOB_CANCEL_REASON.to_string());
    Some(IngestCancelOutcome::Accepted(record.clone()))
}

fn cancel_route_response(
    status_code: StatusCode,
    truth_state: &str,
    accepted: bool,
    job: &IngestJobRecord,
) -> Response {
    (
        status_code,
        Json(serde_json::json!({
            "canonical_route": "/api/v1/ingest/jobs/:job_id/cancel",
            "truth_state": truth_state,
            "accepted": accepted,
            "job_id": job.job_id,
            "status": job.status.as_str(),
            "error": job.error.clone(),
        })),
    )
        .into_response()
}

fn not_found_cancel_response(job_id: Uuid) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "canonical_route": "/api/v1/ingest/jobs/:job_id/cancel",
            "truth_state": "not_found",
            "accepted": false,
            "job_id": job_id,
            "error": format!("job_id {} not found", job_id),
        })),
    )
        .into_response()
}

fn backend_unavailable_cancel_response(job_id: Uuid, error: String) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "canonical_route": "/api/v1/ingest/jobs/:job_id/cancel",
            "truth_state": "backend_unavailable",
            "accepted": false,
            "job_id": job_id,
            "error": error,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/ingest/jobs/:job_id/cancel
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_job_cancel(
    State(st): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Response {
    if let Some(outcome) = cancel_record_in_memory(&st.ingest_jobs, job_id) {
        return match outcome {
            IngestCancelOutcome::AlreadyTerminal(record) => {
                cancel_route_response(StatusCode::OK, "already_terminal", false, &record)
            }
            IngestCancelOutcome::Accepted(record) => {
                if let Some(pool) = &st.db {
                    if let Err(e) = persist_ingest_job_record(pool, &record).await {
                        return backend_unavailable_cancel_response(job_id, e);
                    }
                }
                cancel_route_response(StatusCode::ACCEPTED, "cancel_accepted", true, &record)
            }
        };
    }

    let Some(pool) = &st.db else {
        return not_found_cancel_response(job_id);
    };

    let mut record = match load_persisted_ingest_job(pool, job_id).await {
        Ok(Some(record)) => record,
        Ok(None) => return not_found_cancel_response(job_id),
        Err(e) => return backend_unavailable_cancel_response(job_id, e),
    };

    if record.status.is_terminal() {
        return cancel_route_response(StatusCode::OK, "already_terminal", false, &record);
    }

    record.status = IngestJobStatus::Cancelled;
    record.completed_at_utc = Some(Utc::now());
    record.error = Some(INGEST_JOB_CANCEL_REASON.to_string());

    if let Err(e) = persist_ingest_job_record(pool, &record).await {
        return backend_unavailable_cancel_response(job_id, e);
    }

    {
        let mut store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");
        store.insert(job_id, record.clone());
    }

    cancel_route_response(StatusCode::ACCEPTED, "cancel_accepted", true, &record)
}

// ---------------------------------------------------------------------------
// CSV job handler
// ---------------------------------------------------------------------------

async fn handle_csv_job(st: Arc<AppState>, req: IngestJobRequest, source: String) -> Response {
    // csv_path is required and non-empty.
    let csv_path = match &req.csv_path {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(IngestJobAcceptedResponse {
                    accepted: false,
                    job_id: Uuid::nil(),
                    status: "refused".to_string(),
                    source: source.clone(),
                    error: Some(
                        "csv_path is required for source='csv' and must not be empty".to_string(),
                    ),
                    dry_run: None,
                    provider_api_calls_allowed: None,
                    symbols_count: None,
                    api_calls_made: None,
                }),
            )
                .into_response();
        }
    };

    // Timeframe validation.
    let timeframe = match validate_timeframe(&req.timeframe) {
        Ok(tf) => tf.to_string(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(IngestJobAcceptedResponse {
                    accepted: false,
                    job_id: Uuid::nil(),
                    status: "refused".to_string(),
                    source: source.clone(),
                    error: Some(e),
                    dry_run: None,
                    provider_api_calls_allowed: None,
                    symbols_count: None,
                    api_calls_made: None,
                }),
            )
                .into_response();
        }
    };

    // CSV path must exist at submission time (fail fast, not silently).
    if !Path::new(&csv_path).exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(IngestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                source: source.clone(),
                error: Some(format!("csv_path not found: {}", csv_path)),
                dry_run: None,
                provider_api_calls_allowed: None,
                symbols_count: None,
                api_calls_made: None,
            }),
        )
            .into_response();
    }

    let source_label = req
        .source_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("csv")
        .to_string();

    let out_dir = req
        .out_dir
        .clone()
        .unwrap_or_else(|| "exports/md_ingest".to_string());

    // Create job record.
    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp
    let record = IngestJobRecord {
        job_id,
        source: source.clone(),
        mode: None,
        csv_path: Some(csv_path.clone()),
        timeframe: timeframe.clone(),
        source_label: source_label.clone(),
        out_dir: out_dir.clone(),
        status: IngestJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: None,
        dry_run: false,
        provider_api_calls_allowed: false,
        api_calls_made: 0,
        symbols_source: None,
        registry_path_used: None,
        provider_registry_path_used: None,
        symbols_count: None,
        planned_first_symbol: None,
        planned_last_symbol: None,
        asset_class: "equity".to_string(),
        provider_enabled: None,
        provider_verification_status: None,
        symbols_completed: None,
        symbols_failed: None,
    };

    if let Err(resp) = persist_and_insert_job(&st, record).await {
        return resp;
    }

    // Clone state for background task.
    let jobs = Arc::clone(&st.ingest_jobs);
    let db_pool = st.db.clone();

    tokio::spawn(async move {
        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }

        // Mark running.
        persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
            r.status = IngestJobStatus::Running;
            r.started_at_utc = Some(Utc::now()); // allow: operational
        })
        .await;

        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }

        let result =
            run_csv_ingest_async(db_pool.clone(), csv_path, timeframe, source_label, out_dir).await;

        // Mark completed / failed.
        persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
            r.completed_at_utc = Some(Utc::now()); // allow: operational
            match result {
                Ok(outcome) => {
                    r.status = IngestJobStatus::Completed;
                    r.rows_read = Some(outcome.rows_read);
                    r.rows_inserted = Some(outcome.rows_inserted);
                    r.rows_rejected = Some(outcome.rows_rejected);
                    r.quality_report_path = outcome.quality_report_path;
                }
                Err(e) => {
                    r.status = IngestJobStatus::Failed;
                    r.error = Some(e);
                }
            }
        })
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(IngestJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            source,
            error: None,
            dry_run: None,
            provider_api_calls_allowed: None,
            symbols_count: None,
            api_calls_made: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Provider sync job handler (DATA-INGEST-DAEMON-PROVIDER-JOBS-01)
// ---------------------------------------------------------------------------

/// Handle source="<registered provider>" + mode="sync_provider" job submissions.
///
/// Safety:
/// - dry_run=true (default): resolves symbols from registry; makes ZERO provider
///   API calls; writes NOTHING to DB or CSV. Transitions to dry_run_completed.
/// - dry_run=false + allow_provider_api_calls=false (default): refused immediately.
/// - dry_run=false + allow_provider_api_calls=true: job queued; real provider
///   sync runs asynchronously via an injectable provider client (zero-network in tests).
///
/// DATA-PROVIDER-FOUNDATION-01 additions:
/// - Validates asset_class against known values.
/// - Loads provider registry (if available) to validate provider enabled + asset_class support.
/// - Reports provider_enabled and provider_verification_status in job status.
async fn handle_provider_sync_job(
    st: Arc<AppState>,
    req: IngestJobRequest,
    source: String,
) -> Response {
    let dry_run = req.dry_run;
    let allow_provider = req.allow_provider_api_calls;

    // Gate: dry_run=false without explicit allow → refused.
    if !dry_run && !allow_provider {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            "provider API calls are not allowed: \
             set allow_provider_api_calls=true to permit real ingestion, \
             or use dry_run=true (default) to validate without provider calls."
                .to_string(),
            Some(false),
            Some(false),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
    }

    // asset_class validation (synchronous, no file I/O).
    let asset_class = req.asset_class.trim().to_ascii_lowercase();
    const VALID_ASSET_CLASSES: &[&str] =
        &["equity", "etf", "crypto", "futures", "options", "forex"];
    if !VALID_ASSET_CLASSES.contains(&asset_class.as_str()) {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            format!(
                "unsupported asset_class '{}'. accepted: {}",
                asset_class,
                VALID_ASSET_CLASSES.join(", ")
            ),
            Some(dry_run),
            Some(allow_provider),
            Some(asset_class),
            None,
            None,
            None,
            None,
        )
        .await;
    }

    // Provider registry validation (synchronous, no provider construction).
    // Validates provider is registered, enabled, and supports the requested asset_class.
    let provider_registry_path = req
        .provider_registry_path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&st.provider_registry_path)
        .to_string();

    let (provider_config, provider_enabled_val, provider_verification_status_val) =
        match mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
            &provider_registry_path,
        )) {
            Ok(providers) => match mqk_md::provider_registry::find_provider(&providers, &source) {
                None => {
                    return refused_provider_job_response(
                        &st,
                        source.clone(),
                        &req,
                        format!(
                            "provider '{}' is not registered in the provider registry. \
                                 Check config/providers/providers.json.",
                            source
                        ),
                        Some(dry_run),
                        Some(allow_provider),
                        Some(asset_class.clone()),
                        None,
                        Some(provider_registry_path.clone()),
                        None,
                        None,
                    )
                    .await;
                }
                Some(p) => {
                    if !p.enabled {
                        return refused_provider_job_response(
                            &st,
                            source.clone(),
                            &req,
                            format!(
                                "provider '{}' is disabled in the provider registry \
                                     (enabled=false). No real ingestion is possible until \
                                     the provider is implemented and enabled.",
                                source
                            ),
                            Some(dry_run),
                            Some(allow_provider),
                            Some(asset_class.clone()),
                            None,
                            Some(provider_registry_path.clone()),
                            Some(p.enabled),
                            Some(p.verification_status.clone()),
                        )
                        .await;
                    }
                    if !p.supports_asset_class(&asset_class) {
                        return refused_provider_job_response(
                            &st,
                            source.clone(),
                            &req,
                            format!(
                                "provider '{}' does not support asset_class='{}'. \
                                     Supported by this provider: {}",
                                source,
                                asset_class,
                                p.asset_classes.join(", ")
                            ),
                            Some(dry_run),
                            Some(allow_provider),
                            Some(asset_class.clone()),
                            None,
                            Some(provider_registry_path.clone()),
                            Some(p.enabled),
                            Some(p.verification_status.clone()),
                        )
                        .await;
                    }
                    (
                        p.clone(),
                        Some(p.enabled),
                        Some(p.verification_status.clone()),
                    )
                }
            },
            Err(e) => {
                return refused_provider_job_response(
                    &st,
                    source.clone(),
                    &req,
                    format!("provider registry load failed: {}", e),
                    Some(dry_run),
                    Some(allow_provider),
                    Some(asset_class.clone()),
                    None,
                    Some(provider_registry_path.clone()),
                    None,
                    None,
                )
                .await;
            }
        };

    // symbols_source validation.
    let symbols_source = req
        .symbols_source
        .as_deref()
        .unwrap_or("registry")
        .trim()
        .to_ascii_lowercase();
    if symbols_source != "registry" {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            format!(
                "symbols_source '{}' is not supported. \
                 Only 'registry' is implemented in this version.",
                symbols_source
            ),
            Some(dry_run),
            Some(allow_provider),
            Some(asset_class),
            None,
            Some(provider_registry_path),
            provider_enabled_val,
            provider_verification_status_val,
        )
        .await;
    }

    // Timeframe validation.
    let timeframe = match validate_timeframe(&req.timeframe) {
        Ok(tf) => tf.to_string(),
        Err(e) => {
            return refused_provider_job_response(
                &st,
                source,
                &req,
                e,
                Some(dry_run),
                Some(allow_provider),
                Some(asset_class),
                None,
                Some(provider_registry_path),
                provider_enabled_val,
                provider_verification_status_val,
            )
            .await;
        }
    };

    let provider_capabilities = match mqk_md::capabilities_from_provider_config(&provider_config) {
        Ok(capabilities) => capabilities,
        Err(e) => {
            return refused_provider_job_response(
                &st,
                source,
                &req,
                e.to_string(),
                Some(dry_run),
                Some(allow_provider),
                Some(asset_class),
                None,
                Some(provider_registry_path),
                provider_enabled_val,
                provider_verification_status_val,
            )
            .await;
        }
    };

    if !provider_capabilities.historical_bars {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            format!(
                "provider '{}' does not support capability historical_bars",
                provider_config.provider_id
            ),
            Some(dry_run),
            Some(allow_provider),
            Some(asset_class),
            None,
            Some(provider_registry_path),
            provider_enabled_val,
            provider_verification_status_val,
        )
        .await;
    }

    let parsed_timeframe = match mqk_md::Timeframe::parse(&timeframe) {
        Ok(t) => t,
        Err(e) => {
            return refused_provider_job_response(
                &st,
                source,
                &req,
                format!("timeframe parse error: {}", e),
                Some(dry_run),
                Some(allow_provider),
                Some(asset_class),
                None,
                Some(provider_registry_path),
                provider_enabled_val,
                provider_verification_status_val,
            )
            .await;
        }
    };

    if !provider_capabilities
        .supported_timeframes
        .contains(&parsed_timeframe)
    {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            format!(
                "provider '{}' does not support timeframe '{}'",
                provider_config.provider_id, timeframe
            ),
            Some(dry_run),
            Some(allow_provider),
            Some(asset_class),
            None,
            Some(provider_registry_path),
            provider_enabled_val,
            provider_verification_status_val,
        )
        .await;
    }

    // Registry path: use request override, otherwise fall back to AppState default.
    let registry_path = req
        .registry_path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&st.instrument_registry_path)
        .to_string();

    let out_dir = req
        .out_dir
        .clone()
        .unwrap_or_else(|| "exports/md_ingest".to_string());

    // --- DRY-RUN PATH ---
    if dry_run {
        return handle_provider_dry_run(
            st,
            source,
            asset_class,
            registry_path,
            provider_registry_path,
            out_dir,
            timeframe,
            provider_enabled_val,
            provider_verification_status_val,
        )
        .await;
    }

    // --- REAL PROVIDER SYNC PATH (dry_run=false + allow_provider=true) ---

    // Parse date range: end defaults to today; start defaults to 30 days ago.
    let end_d = match &req.end {
        Some(s) => match NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                return refused_provider_job_response(
                    &st,
                    source,
                    &req,
                    format!("invalid end date '{}'; expected YYYY-MM-DD", s),
                    Some(false),
                    Some(true),
                    Some(asset_class),
                    Some(registry_path),
                    Some(provider_registry_path),
                    provider_enabled_val,
                    provider_verification_status_val,
                )
                .await;
            }
        },
        None => Utc::now().date_naive(),
    };

    let start_d = match &req.start {
        Some(s) => match NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                return refused_provider_job_response(
                    &st,
                    source,
                    &req,
                    format!("invalid start date '{}'; expected YYYY-MM-DD", s),
                    Some(false),
                    Some(true),
                    Some(asset_class),
                    Some(registry_path),
                    Some(provider_registry_path),
                    provider_enabled_val,
                    provider_verification_status_val,
                )
                .await;
            }
        },
        // Default: 30-day lookback (safe and bounded for any timeframe).
        None => end_d - Duration::days(30),
    };

    if start_d > end_d {
        return refused_provider_job_response(
            &st,
            source,
            &req,
            format!("start ({}) must be <= end ({})", start_d, end_d),
            Some(false),
            Some(true),
            Some(asset_class),
            Some(registry_path),
            Some(provider_registry_path),
            provider_enabled_val,
            provider_verification_status_val,
        )
        .await;
    }

    // Deterministic ingest_id for this job: keyed on source, timeframe, date range.
    // Using start_d + end_d ensures the same params produce the same ingest_id.
    let ingest_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-sync.daemon.v1|{}|{}|{}|{}",
            source, timeframe, start_d, end_d
        )
        .as_bytes(),
    );

    let tf = parsed_timeframe;

    // Create job record (Queued).
    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp
    let record = IngestJobRecord {
        job_id,
        source: source.clone(),
        mode: Some("sync_provider".to_string()),
        csv_path: None,
        timeframe: timeframe.clone(),
        source_label: source.clone(),
        out_dir: out_dir.clone(),
        status: IngestJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: None,
        dry_run: false,
        provider_api_calls_allowed: true,
        api_calls_made: 0,
        symbols_source: Some("registry".to_string()),
        registry_path_used: Some(registry_path.clone()),
        provider_registry_path_used: Some(provider_registry_path.clone()),
        symbols_count: None,
        planned_first_symbol: None,
        planned_last_symbol: None,
        asset_class: asset_class.clone(),
        provider_enabled: provider_enabled_val,
        provider_verification_status: provider_verification_status_val,
        symbols_completed: None,
        symbols_failed: None,
    };

    if let Err(resp) = persist_and_insert_job(&st, record).await {
        return resp;
    }

    let jobs = Arc::clone(&st.ingest_jobs);
    let api_credits_per_minute = req.api_credits_per_minute;
    let api_credits_per_day = req.api_credits_per_day;
    let provider_client = st.provider_client.clone();
    let db_pool = st.db.clone();
    let source_for_task = source.clone();
    let registry_path_for_task = registry_path.clone();
    let provider_registry_path_for_task = provider_registry_path.clone();

    tokio::spawn(async move {
        run_real_provider_sync(
            jobs,
            job_id,
            provider_client,
            db_pool,
            source_for_task,
            registry_path_for_task,
            provider_registry_path_for_task,
            tf,
            start_d,
            end_d,
            api_credits_per_minute,
            api_credits_per_day,
            ingest_id,
        )
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(IngestJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            source,
            error: None,
            dry_run: Some(false),
            provider_api_calls_allowed: Some(true),
            symbols_count: None,
            api_calls_made: Some(0),
        }),
    )
        .into_response()
}

/// Dry-run path for handle_provider_sync_job.
///
/// Resolves symbols from the registry; makes zero provider calls; writes nothing.
#[allow(clippy::too_many_arguments)]
async fn handle_provider_dry_run(
    st: Arc<AppState>,
    source: String,
    asset_class: String,
    registry_path: String,
    provider_registry_path: String,
    out_dir: String,
    timeframe: String,
    provider_enabled_val: Option<bool>,
    provider_verification_status_val: Option<String>,
) -> Response {
    // Create job record (Queued).
    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp
    let record = IngestJobRecord {
        job_id,
        source: source.clone(),
        mode: Some("sync_provider".to_string()),
        csv_path: None,
        timeframe: timeframe.clone(),
        source_label: source.clone(),
        out_dir,
        status: IngestJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: None,
        dry_run: true,
        provider_api_calls_allowed: false,
        api_calls_made: 0,
        symbols_source: Some("registry".to_string()),
        registry_path_used: Some(registry_path.clone()),
        provider_registry_path_used: Some(provider_registry_path.clone()),
        symbols_count: None,
        planned_first_symbol: None,
        planned_last_symbol: None,
        asset_class: asset_class.clone(),
        provider_enabled: provider_enabled_val,
        provider_verification_status: provider_verification_status_val,
        symbols_completed: None,
        symbols_failed: None,
    };

    if let Err(resp) = persist_and_insert_job(&st, record).await {
        return resp;
    }

    // Background task: resolve symbols from registry (pure fs read, no network).
    let jobs = Arc::clone(&st.ingest_jobs);
    let db_pool = st.db.clone();
    let source_for_task = source.clone();
    tokio::spawn(async move {
        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }

        // Mark running.
        persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
            r.status = IngestJobStatus::Running;
            r.started_at_utc = Some(Utc::now()); // allow: operational
        })
        .await;

        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }

        // Resolve symbols — pure filesystem read; zero network calls. Scoped
        // to instruments actually assigned to `source_for_task` (§B2.3) so a
        // dry-run preview agrees with what the real sync path would fetch.
        let result =
            resolve_provider_scoped_equities(&registry_path, &source_for_task).map(|equities| {
                let count = equities.len();
                let first = equities.first().map(|e| e.symbol.clone());
                let last = equities.last().map(|e| e.symbol.clone());
                (count, first, last)
            });

        // Mark terminal.
        persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
            r.completed_at_utc = Some(Utc::now()); // allow: operational
            match result {
                Ok((count, first, last)) => {
                    r.status = IngestJobStatus::DryRunCompleted;
                    r.symbols_count = Some(count);
                    r.planned_first_symbol = first;
                    r.planned_last_symbol = last;
                    // Explicit: zero API calls made (dry-run invariant).
                    r.api_calls_made = 0;
                }
                Err(e) => {
                    r.status = IngestJobStatus::Failed;
                    r.error = Some(format!("registry load failed: {}", e));
                }
            }
        })
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(IngestJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            source,
            error: None,
            dry_run: Some(true),
            provider_api_calls_allowed: Some(false),
            symbols_count: None, // resolved asynchronously; poll job status
            api_calls_made: Some(0),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Real provider sync background task
// ---------------------------------------------------------------------------

/// Resolve enabled equity instruments assigned to `source`'s exact configured
/// provider (case-insensitive, trimmed) from the registry at `registry_path`
/// (§B2.3, DAILY-DATA-READINESS-01B-PROVIDER-CONTRACT-INTEGRATION-01).
///
/// The prior behavior (`enabled_equities()` with no provider filter) resolved
/// *every* enabled equity regardless of which provider the registry actually
/// assigns it to — meaning a `source="alpaca"` sync job would request every
/// registry symbol from Alpaca even though today's registry assigns every
/// enabled equity to `"twelvedata"`. This filters to only instruments whose
/// own `provider` field matches the requested job's provider, so a sync job
/// never claims provenance for an instrument it was never configured to
/// fetch. Deterministic ordering (sorted by symbol) mirrors
/// `enabled_equities()`.
fn resolve_provider_scoped_equities(
    registry_path: &str,
    source: &str,
) -> Result<Vec<mqk_md::instrument_registry::TrackedInstrument>, String> {
    let instruments =
        mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(registry_path))
            .map_err(|e| e.to_string())?;
    let mut filtered: Vec<mqk_md::instrument_registry::TrackedInstrument> = instruments
        .into_iter()
        .filter(|i| {
            i.enabled
                && i.asset_class == "equity"
                && i.provider.trim().eq_ignore_ascii_case(source.trim())
        })
        .collect();
    filtered.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(filtered)
}

enum ProviderSyncClient {
    Historical(std::sync::Arc<dyn mqk_md::HistoricalProvider>),
    MarketData(mqk_md::MarketDataProviderBox),
}

impl ProviderSyncClient {
    async fn fetch_bars(
        &self,
        req: mqk_md::FetchBarsRequest,
    ) -> anyhow::Result<Vec<mqk_md::ProviderBar>> {
        match self {
            ProviderSyncClient::Historical(provider) => provider.fetch_bars(req).await,
            ProviderSyncClient::MarketData(provider) => provider
                .fetch_historical_bars(mqk_md::HistoricalBarsRequest {
                    symbols: req.symbols,
                    timeframe: req.timeframe,
                    start: req.start,
                    end: req.end,
                })
                .await
                .map_err(anyhow::Error::from),
        }
    }
}

/// Executes the real provider sync job asynchronously.
///
/// Safety:
/// - Uses registry-backed provider construction in production.
/// - Uses injectable `provider_client`; tests pass a fake that makes zero network calls.
/// - API credit guardrails are checked before each symbol fetch.
/// - One failed symbol does not abort the batch (per-symbol error tracking).
/// - Global failures (missing client, registry unreadable, DB unavailable) fail the job.
/// - Writes through the canonical `mqk_db::md::ingest_provider_bars_to_md_bars` path.
#[allow(clippy::too_many_arguments)]
async fn run_real_provider_sync(
    jobs: crate::ingest_jobs::IngestJobStore,
    job_id: Uuid,
    provider_client: Option<std::sync::Arc<dyn mqk_md::HistoricalProvider>>,
    db_pool: Option<sqlx::PgPool>,
    source: String,
    registry_path: String,
    provider_registry_path: String,
    timeframe: mqk_md::Timeframe,
    start_d: NaiveDate,
    end_d: NaiveDate,
    api_credits_per_minute: Option<i64>,
    api_credits_per_day: Option<i64>,
    ingest_id: Uuid,
) {
    if ingest_job_is_cancelled(&jobs, job_id) {
        return;
    }

    // Mark running.
    persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
        r.status = IngestJobStatus::Running;
        r.started_at_utc = Some(Utc::now()); // allow: operational
    })
    .await;

    if ingest_job_is_cancelled(&jobs, job_id) {
        return;
    }

    // Resolve provider client (injected or built from registry + env).
    let provider = if let Some(p) = provider_client {
        ProviderSyncClient::Historical(p)
    } else {
        match mqk_md::provider_registry::load_provider_registry(std::path::Path::new(
            &provider_registry_path,
        ))
        .map_err(|err| err.to_string())
        .and_then(|providers| {
            mqk_md::build_market_data_provider_from_env(&source, &providers)
                .map(ProviderSyncClient::MarketData)
                .map_err(|err| err.to_string())
        }) {
            Ok(provider) => provider,
            Err(err) => {
                persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
                    r.status = IngestJobStatus::Failed;
                    r.completed_at_utc = Some(Utc::now()); // allow: operational
                    r.error = Some(format!(
                        "provider construction failed for '{}': {}",
                        source, err
                    ));
                })
                .await;
                return;
            }
        }
    };

    // Resolve symbols from registry — scoped to instruments actually
    // assigned to `source`'s exact configured provider (§B2.3). Never claim
    // provenance for an instrument this job was never configured to fetch.
    let instruments: Vec<mqk_md::instrument_registry::TrackedInstrument> =
        match resolve_provider_scoped_equities(&registry_path, &source) {
            Ok(instruments) => {
                if instruments.is_empty() {
                    persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
                        r.status = IngestJobStatus::Failed;
                        r.completed_at_utc = Some(Utc::now()); // allow: operational
                        r.error = Some(format!(
                            "registry contains no enabled equity symbols assigned to provider '{}'",
                            source
                        ));
                    })
                    .await;
                    return;
                }
                let first = instruments.first().map(|e| e.symbol.clone());
                let last = instruments.last().map(|e| e.symbol.clone());
                let count = instruments.len();
                persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
                    r.symbols_count = Some(count);
                    r.planned_first_symbol = first;
                    r.planned_last_symbol = last;
                    r.symbols_completed = Some(0);
                    r.symbols_failed = Some(0);
                })
                .await;
                instruments
            }
            Err(e) => {
                persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
                    r.status = IngestJobStatus::Failed;
                    r.completed_at_utc = Some(Utc::now()); // allow: operational
                    r.error = Some(format!("registry load failed: {}", e));
                })
                .await;
                return;
            }
        };

    // Per-instrument fetch + insert loop. Each instrument is fetched and
    // inserted independently (rather than batched into one aggregate insert)
    // so `MdBarProviderMetadata.provider_symbol` can carry that instrument's
    // own canonical provider symbol (§B2.3) — the DB metadata API applies one
    // `MdBarProviderMetadata` per `IngestProviderBarsArgs` call, so per-
    // instrument insertion is required to avoid stamping one symbol's
    // provenance onto another's bars.
    let mut api_calls_made: i64 = 0;
    let mut symbols_completed: usize = 0;
    let mut symbols_failed: usize = 0;
    let mut total_rows_inserted: i64 = 0;
    let mut total_rows_rejected: i64 = 0;
    let mut guardrail_msg: Option<String> = None;
    let mut db_error: Option<String> = None;

    for instrument in &instruments {
        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }

        // Check per-day guardrail before making the next call.
        if let Some(max_day) = api_credits_per_day {
            if api_calls_made >= max_day {
                guardrail_msg = Some(format!(
                    "api_credits_per_day guardrail reached: {} calls made, limit {}; \
                     remaining {} symbols skipped",
                    api_calls_made,
                    max_day,
                    instruments.len() - symbols_completed - symbols_failed
                ));
                break;
            }
        }
        // Check per-minute guardrail (used as a per-batch cap in daemon context).
        if let Some(max_min) = api_credits_per_minute {
            if api_calls_made >= max_min {
                guardrail_msg = Some(format!(
                    "api_credits_per_minute guardrail reached: {} calls made, limit {}; \
                     remaining {} symbols skipped",
                    api_calls_made,
                    max_min,
                    instruments.len() - symbols_completed - symbols_failed
                ));
                break;
            }
        }

        // B2.3: request via the provider's own symbol contract, not the
        // local canonical symbol — they are not guaranteed identical.
        let req = mqk_md::FetchBarsRequest {
            symbols: vec![instrument.provider_symbol.clone()],
            timeframe,
            start: start_d,
            end: end_d,
        };

        api_calls_made += 1;

        match provider.fetch_bars(req).await {
            Ok(raw_bars) => {
                let (completed, _) = mqk_md::filter_completed_provider_bars(
                    raw_bars,
                    timeframe,
                    Utc::now().timestamp(), // allow: operational completion filter
                );

                // Validate returned bars actually belong to the requested
                // instrument/provider mapping (§B2.3) — never stamp one
                // symbol's provider metadata onto another symbol's bars.
                // Remap the accepted bars' `symbol` to the canonical local
                // symbol before storage; `md_bars.symbol` is always the
                // local symbol, never the provider symbol.
                let mut matched_bars: Vec<mqk_db::md::ProviderBar> =
                    Vec::with_capacity(completed.len());
                let mut mismatched = 0usize;
                for b in completed {
                    if b.symbol
                        .trim()
                        .eq_ignore_ascii_case(instrument.provider_symbol.trim())
                    {
                        matched_bars.push(mqk_db::md::ProviderBar {
                            symbol: instrument.symbol.clone(),
                            timeframe: b.timeframe,
                            end_ts: b.end_ts,
                            open: b.open,
                            high: b.high,
                            low: b.low,
                            close: b.close,
                            volume: b.volume,
                            is_complete: b.is_complete,
                        });
                    } else {
                        mismatched += 1;
                    }
                }
                if mismatched > 0 {
                    tracing::warn!(
                        job_id = %job_id,
                        symbol = %instrument.symbol,
                        provider_symbol = %instrument.provider_symbol,
                        mismatched,
                        "provider returned bars for an unmapped symbol; rejected"
                    );
                }

                if matched_bars.is_empty() {
                    symbols_completed += 1;
                } else {
                    match &db_pool {
                        None => {
                            db_error = Some("no_db: database pool is not configured".to_string());
                            symbols_failed += 1;
                        }
                        Some(pool) => {
                            let symbol_ingest_id = Uuid::new_v5(
                                &Uuid::NAMESPACE_DNS,
                                format!("{}|{}", ingest_id, instrument.symbol).as_bytes(),
                            );
                            match mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
                                pool,
                                mqk_db::md::IngestProviderBarsArgs {
                                    source: source.clone(),
                                    timeframe: timeframe.as_str().to_string(),
                                    ingest_id: symbol_ingest_id,
                                    bars: matched_bars,
                                },
                                mqk_db::md::MdBarProviderMetadata {
                                    provider_id: source.clone(),
                                    provider_source: Some(source.clone()),
                                    provider_symbol: Some(instrument.provider_symbol.clone()),
                                    ingest_mode: Some("historical_backfill".to_string()),
                                    provider_bar_id: None,
                                    provider_updated_at_utc: None,
                                },
                            )
                            .await
                            {
                                Ok(res) => {
                                    total_rows_inserted += res.report.coverage.rows_inserted as i64;
                                    total_rows_rejected += res.report.coverage.rows_rejected as i64;
                                    symbols_completed += 1;
                                }
                                Err(e) => {
                                    db_error = Some(format!(
                                        "db ingest failed for {}: {}",
                                        instrument.symbol, e
                                    ));
                                    symbols_failed += 1;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    job_id = %job_id,
                    symbol = %instrument.symbol,
                    error = %e,
                    "provider fetch failed for symbol; continuing batch"
                );
                symbols_failed += 1;
            }
        }

        // Progress update after each symbol.
        persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
            r.api_calls_made = api_calls_made;
            r.symbols_completed = Some(symbols_completed);
            r.symbols_failed = Some(symbols_failed);
        })
        .await;

        if ingest_job_is_cancelled(&jobs, job_id) {
            return;
        }
        if db_error.is_some() {
            break;
        }
    }

    if ingest_job_is_cancelled(&jobs, job_id) {
        return;
    }

    let rows_inserted = Some(total_rows_inserted);
    let rows_rejected = Some(total_rows_rejected);

    // Determine terminal status.
    let final_status = if db_error.is_some() {
        IngestJobStatus::Failed
    } else if symbols_failed == 0 && guardrail_msg.is_none() {
        IngestJobStatus::Completed
    } else if symbols_completed > 0 {
        IngestJobStatus::Partial
    } else {
        IngestJobStatus::Failed
    };

    let final_error = db_error.or(guardrail_msg);

    persist_job_update(&jobs, db_pool.as_ref(), job_id, |r| {
        r.completed_at_utc = Some(Utc::now()); // allow: operational
        r.status = final_status;
        r.api_calls_made = api_calls_made;
        r.symbols_completed = Some(symbols_completed);
        r.symbols_failed = Some(symbols_failed);
        r.rows_inserted = rows_inserted;
        r.rows_rejected = rows_rejected;
        r.error = final_error;
    })
    .await;
}

// ---------------------------------------------------------------------------
// Ingest outcome (internal)
// ---------------------------------------------------------------------------

struct IngestOutcome {
    rows_read: i64,
    rows_inserted: i64,
    rows_rejected: i64,
    quality_report_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Async CSV ingest execution
// ---------------------------------------------------------------------------

/// Run CSV ingest against the DB and write a quality report artifact.
///
/// Safety:
/// - No broker adapter calls. No OMS tables. No live/paper execution state.
/// - Calls `mqk_db::md::ingest_csv_to_md_bars` using the daemon's DB pool.
/// - If pool is None, returns Err("no_db: ...").
/// - Returns Err(message) on any failure — never panics.
async fn run_csv_ingest_async(
    db_pool: Option<sqlx::PgPool>,
    csv_path: String,
    timeframe: String,
    source_label: String,
    out_dir: String,
) -> Result<IngestOutcome, String> {
    let pool = match db_pool {
        Some(p) => p,
        None => return Err("no_db: database pool is not configured".to_string()),
    };

    // Deterministic ingest_id (D1-5 rule: no Uuid::new_v4 in ingestion path).
    // Uses same key format as the CLI for cross-tool idempotency.
    let ingest_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-ingest.csv.v1|{}|{}|{}",
            source_label, csv_path, timeframe
        )
        .as_bytes(),
    );

    let res = mqk_db::md::ingest_csv_to_md_bars(
        &pool,
        mqk_db::md::IngestCsvArgs {
            path: std::path::PathBuf::from(&csv_path),
            timeframe: timeframe.clone(),
            source: source_label.clone(),
            ingest_id,
        },
    )
    .await
    .map_err(|e| format!("ingest_csv failed: {}", e))?;

    // Write quality report artifact.
    let report_dir = std::path::Path::new(&out_dir).join(res.ingest_id.to_string());
    std::fs::create_dir_all(&report_dir).map_err(|e| format!("create report dir failed: {}", e))?;

    let report_path = report_dir.join("data_quality.json");
    let json_str = serde_json::to_string_pretty(&res.report)
        .map_err(|e| format!("serialize report failed: {}", e))?;
    std::fs::write(&report_path, json_str).map_err(|e| format!("write report failed: {}", e))?;

    let coverage = &res.report.coverage;
    Ok(IngestOutcome {
        rows_read: coverage.rows_read as i64,
        rows_inserted: coverage.rows_inserted as i64,
        rows_rejected: coverage.rows_rejected as i64,
        quality_report_path: Some(report_path.to_string_lossy().to_string()),
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_jobs_list(State(st): State<Arc<AppState>>) -> Response {
    if let Some(pool) = &st.db {
        return match list_persisted_ingest_jobs(pool).await {
            Ok(records) => {
                let jobs = records.iter().map(record_to_summary).collect();
                Json(IngestJobsListResponse {
                    truth_state: "active".to_string(),
                    jobs,
                })
                .into_response()
            }
            Err(e) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "truth_state": "backend_unavailable",
                    "error": e,
                    "jobs": []
                })),
            )
                .into_response(),
        };
    }

    let store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");

    let mut jobs: Vec<IngestJobSummary> = store.values().map(record_to_summary).collect();

    // Deterministic ordering: newest first by created_at_utc, then job_id.
    jobs.sort_by(|a, b| {
        b.created_at_utc
            .cmp(&a.created_at_utc)
            .then_with(|| a.job_id.cmp(&b.job_id))
    });

    Json(IngestJobsListResponse {
        truth_state: "active".to_string(),
        jobs,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/tracked-equities
// ---------------------------------------------------------------------------

/// Return the enabled equity universe from the canonical instrument registry.
///
/// Safety invariants:
/// - No broker adapter. No provider API calls. No DB writes.
/// - Does not touch live/paper execution state. No arm_state required.
/// - Read-only filesystem access to the registry JSON file.
/// - Provider sync job is NOT triggered. No API credits consumed.
pub(crate) async fn tracked_equities_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);

    let instruments = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(e) => {
            let (truth_state, error_msg) = if path.exists() {
                ("registry_invalid", format!("registry parse failed: {}", e))
            } else {
                (
                    "registry_unavailable",
                    format!("registry file not found: {}", registry_path),
                )
            };
            return Json(TrackedEquitiesResponse {
                canonical_route: "/api/v1/ingest/tracked-equities".to_string(),
                truth_state: truth_state.to_string(),
                registry_path,
                count: 0,
                symbols: vec![],
                first_symbol: None,
                last_symbol: None,
                error: Some(error_msg),
            });
        }
    };

    let equities = mqk_md::instrument_registry::enabled_equities(&instruments);
    let count = equities.len();
    let first_symbol = equities.first().map(|e| e.symbol.clone());
    let last_symbol = equities.last().map(|e| e.symbol.clone());

    let symbols: Vec<TrackedEquitySummary> = equities
        .into_iter()
        .map(|e| TrackedEquitySummary {
            symbol: e.symbol.clone(),
            instrument_id: e.instrument_id.clone(),
            provider: e.provider.clone(),
            venue: e.venue.clone(),
            timeframes: e.timeframes.clone(),
        })
        .collect();

    Json(TrackedEquitiesResponse {
        canonical_route: "/api/v1/ingest/tracked-equities".to_string(),
        truth_state: "active".to_string(),
        registry_path,
        count,
        symbols,
        first_symbol,
        last_symbol,
        error: None,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/ingest-plan — WATCHLIST-INGEST-PLAN-01
// ---------------------------------------------------------------------------

/// Build the `IngestPlanResponse` from a resolved symbol set and the
/// configured timeframe. Pure helper — extracted for testability (mirrors
/// `routes/watchlist.rs::build_watchlist_status_response`).
///
/// Safety: takes no DB/provider/broker handles and performs no I/O — every
/// input is already resolved by the caller from env vars + the watchlist
/// artifact reader.
fn build_ingest_plan_response(
    resolution: RequiredSymbolsResolution,
    timeframe_configured: Option<String>,
    checked_at_utc: String,
) -> IngestPlanResponse {
    let normalized = normalize_required_symbols(&resolution.required);
    let required_symbols: Vec<String> = normalized.iter().map(|r| r.symbol.clone()).collect();
    let required_symbol_timeframes: Vec<IngestPlanSymbolTimeframe> = normalized
        .iter()
        .map(|r| IngestPlanSymbolTimeframe {
            symbol: r.symbol.clone(),
            timeframe: r.timeframe.clone(),
            source: resolution.source.to_string(),
        })
        .collect();

    let watchlist_configured = !matches!(
        resolution.watchlist_outcome,
        WatchlistIntakeOutcome::NotConfigured
    );
    let watchlist_is_active_source = resolution.source == SYMBOL_SOURCE_WATCHLIST_V2;

    let mut warnings = Vec::new();
    if timeframe_configured.is_none() {
        warnings.push(
            "MQK_STRATEGY_MD_TIMEFRAME is not configured; ingest plan cannot resolve a \
             required timeframe regardless of symbol source"
                .to_string(),
        );
    }
    if watchlist_configured && !watchlist_is_active_source {
        let fallback_desc = if required_symbols.is_empty() {
            "no source (no required symbols resolved)".to_string()
        } else {
            format!("source '{}'", resolution.source)
        };
        warnings.push(format!(
            "MQK_PAPER_WATCHLIST_PATH is configured but is not the active ingest-plan source \
             (status={}); falling back to {}",
            resolution.watchlist_outcome.status_label(),
            fallback_desc,
        ));
    }

    let truth_state = if watchlist_configured && !watchlist_is_active_source {
        "degraded"
    } else if required_symbols.is_empty() {
        "not_configured"
    } else {
        "active"
    };

    IngestPlanResponse {
        canonical_route: INGEST_PLAN_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        symbol_source: resolution.source.to_string(),
        required_symbols,
        timeframe: timeframe_configured,
        required_symbol_timeframes,
        coverage_expectation: IngestPlanCoverageExpectation {
            uses_market_data_readiness: true,
            uses_md_bars: true,
        },
        warnings,
        checked_at_utc,
    }
}

/// Read-only required-symbol/timeframe ingest plan, naming its source.
///
/// Answers: which symbols/timeframe does the bot require for trading
/// readiness, where did that list come from, and which source should
/// premarket ingest and the GUI coverage panel expect.
///
/// # Safety invariants
/// - Read-only. No DB, no provider/broker calls, no network access.
/// - Does not touch live/paper execution state. No arm_state required.
/// - Never uses the full instrument registry as the required-symbol source.
pub(crate) async fn market_data_ingest_plan() -> impl IntoResponse {
    let resolution = required_symbols_with_source_from_env();
    let timeframe_configured = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let checked_at_utc = Utc::now().to_rfc3339();

    Json(build_ingest_plan_response(
        resolution,
        timeframe_configured,
        checked_at_utc,
    ))
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/jobs/:job_id
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_job_status(
    State(st): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Response {
    if let Some(pool) = &st.db {
        return match load_persisted_ingest_job(pool, job_id).await {
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("job_id {} not found", job_id),
                    "truth_state": "not_found"
                })),
            )
                .into_response(),
            Ok(Some(r)) => (StatusCode::OK, Json(record_to_status_response(&r))).into_response(),
            Err(e) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "truth_state": "backend_unavailable",
                    "error": e
                })),
            )
                .into_response(),
        };
    }

    let store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");

    match store.get(&job_id) {
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("job_id {} not found", job_id),
                "truth_state": "not_found"
            })),
        )
            .into_response(),
        Some(r) => (StatusCode::OK, Json(record_to_status_response(r))).into_response(),
    }
}
