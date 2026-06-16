//! BACKTEST-DAEMON-JOBS-01: Daemon-managed offline backtest job API.
//!
//! Routes:
//!   POST /api/v1/backtests/jobs          — submit a CSV backtest job (operator)
//!   GET  /api/v1/backtests/jobs          — list all in-memory jobs (public)
//!   GET  /api/v1/backtests/jobs/:job_id  — job status + artifact paths (public)
//!
//! Safety invariants:
//! - No broker adapter is called. No Alpaca. No live/paper execution.
//! - No OMS tables written. No orders/fills in live DB tables.
//! - Does not require arm_state. Does not start/stop the trading runtime.
//! - Jobs are in-memory only (process-lifetime); no DB persistence.
//! - Artifacts are written to the filesystem under exports/backtests/<run_id>/.
//! - Failed jobs report errors truthfully. No hidden failures.

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    api_types::{
        BacktestJobAcceptedResponse, BacktestJobRequest, BacktestJobStatusResponse,
        BacktestJobSummary, BacktestJobsListResponse,
    },
    backtest_jobs::{BacktestJobRecord, BacktestJobStatus},
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /api/v1/backtests/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn backtest_job_submit(
    State(st): State<Arc<AppState>>,
    Json(req): Json<BacktestJobRequest>,
) -> Response {
    // --- Input validation (fail truthfully, never optimistically pass) ---
    if req.bars_path.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some("bars_path is required and must not be empty".to_string()),
            }),
        )
            .into_response();
    }
    if req.strategy.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some("strategy is required and must not be empty".to_string()),
            }),
        )
            .into_response();
    }
    if req.symbol.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some("symbol is required and must not be empty".to_string()),
            }),
        )
            .into_response();
    }
    if req.timeframe_secs <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some("timeframe_secs must be > 0".to_string()),
            }),
        )
            .into_response();
    }
    if req.initial_cash_micros <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some("initial_cash_micros must be > 0".to_string()),
            }),
        )
            .into_response();
    }
    // Bars path must exist at submission time (fail fast, not silently).
    if !Path::new(&req.bars_path).exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(BacktestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                artifact_dir: None,
                error: Some(format!("bars_path not found: {}", req.bars_path)),
            }),
        )
            .into_response();
    }

    // --- Create job record ---
    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp
    let record = BacktestJobRecord {
        job_id,
        strategy: req.strategy.clone(),
        symbol: req.symbol.clone(),
        bars_path: req.bars_path.clone(),
        timeframe_secs: req.timeframe_secs,
        initial_cash_micros: req.initial_cash_micros,
        status: BacktestJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        artifact_dir: None,
        manifest_path: None,
        metrics_path: None,
        error: None,
    };

    {
        let mut store = st
            .backtest_jobs
            .lock()
            .expect("backtest_jobs lock poisoned");
        store.insert(job_id, record);
    }

    // --- Resolve artifact output directory ---
    let out_dir = req
        .out_dir
        .clone()
        .unwrap_or_else(|| "exports/backtests".to_string());

    // --- Clone everything needed for the background task ---
    let jobs = Arc::clone(&st.backtest_jobs);
    let bars_path = req.bars_path.clone();
    let strategy = req.strategy.clone();
    let symbol = req.symbol.clone();
    let timeframe_secs = req.timeframe_secs;
    let initial_cash_micros = req.initial_cash_micros;
    let integrity_enabled = req.integrity_enabled.unwrap_or(false);
    let integrity_stale_threshold_ticks = req.integrity_stale_threshold_ticks;
    let shadow = req.shadow.unwrap_or(false);

    // --- Spawn background task ---
    tokio::spawn(async move {
        // Update to Running
        {
            let mut store = jobs.lock().expect("backtest_jobs lock poisoned");
            if let Some(r) = store.get_mut(&job_id) {
                r.status = BacktestJobStatus::Running;
                r.started_at_utc = Some(Utc::now()); // allow: operational
            }
        }

        // Run blocking backtest on a dedicated thread.
        let result = tokio::task::spawn_blocking(move || {
            run_backtest_csv_blocking(
                bars_path,
                strategy,
                symbol,
                timeframe_secs,
                initial_cash_micros,
                integrity_enabled,
                integrity_stale_threshold_ticks,
                shadow,
                out_dir,
            )
        })
        .await
        .unwrap_or_else(|join_err| Err(format!("backtest task panicked: {join_err}")));

        // Update completed / failed
        {
            let mut store = jobs.lock().expect("backtest_jobs lock poisoned");
            if let Some(r) = store.get_mut(&job_id) {
                r.completed_at_utc = Some(Utc::now()); // allow: operational
                match result {
                    Ok((artifact_dir, manifest_path, metrics_path)) => {
                        r.status = BacktestJobStatus::Completed;
                        r.artifact_dir = Some(artifact_dir);
                        r.manifest_path = Some(manifest_path);
                        r.metrics_path = Some(metrics_path);
                    }
                    Err(e) => {
                        r.status = BacktestJobStatus::Failed;
                        r.error = Some(e);
                    }
                }
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(BacktestJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            artifact_dir: None,
            error: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/backtests/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn backtest_jobs_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let store = st
        .backtest_jobs
        .lock()
        .expect("backtest_jobs lock poisoned");

    let mut jobs: Vec<BacktestJobSummary> = store
        .values()
        .map(|r| BacktestJobSummary {
            job_id: r.job_id,
            status: r.status.as_str().to_string(),
            strategy: r.strategy.clone(),
            symbol: r.symbol.clone(),
            created_at_utc: r.created_at_utc.to_rfc3339(),
            started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
            completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
            artifact_dir: r.artifact_dir.clone(),
            error: r.error.clone(),
        })
        .collect();

    // Deterministic ordering: newest first by created_at_utc.
    jobs.sort_by(|a, b| b.created_at_utc.cmp(&a.created_at_utc));

    Json(BacktestJobsListResponse {
        truth_state: "active".to_string(),
        jobs,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/backtests/jobs/:job_id
// ---------------------------------------------------------------------------

pub(crate) async fn backtest_job_status(
    State(st): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Response {
    let store = st
        .backtest_jobs
        .lock()
        .expect("backtest_jobs lock poisoned");

    match store.get(&job_id) {
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("job_id {} not found", job_id),
                "truth_state": "not_found"
            })),
        )
            .into_response(),
        Some(r) => (
            StatusCode::OK,
            Json(BacktestJobStatusResponse {
                truth_state: "active".to_string(),
                job_id: r.job_id,
                status: r.status.as_str().to_string(),
                strategy: r.strategy.clone(),
                symbol: r.symbol.clone(),
                created_at_utc: r.created_at_utc.to_rfc3339(),
                started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
                completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
                artifact_dir: r.artifact_dir.clone(),
                manifest_path: r.manifest_path.clone(),
                metrics_path: r.metrics_path.clone(),
                error: r.error.clone(),
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Blocking backtest runner (no async, no broker, no live execution)
// ---------------------------------------------------------------------------

/// Timeframe-aware default for the integrity stale-data threshold (in ticks /
/// seconds), used when the operator does not supply an explicit value.
///
/// BACKTEST-DAILY-STALE-DEFAULT-FIX-01: daily (>= 1 day) bars legitimately gap
/// across weekends and holidays. A Friday→Monday gap is ~259_200 s; longer
/// holiday weekends exceed that. A 4-day (345_600 s) default safely survives
/// those gaps and matches the recommended value documented in
/// `docs/runbooks/backtest_workflow.md`. The previous 172_800 s (2-day) default
/// was below a normal weekend gap and could falsely block daily proof runs.
///
/// Intraday timeframes keep the runtime-mirrored 120 s threshold
/// (`runtime.stale_data_threshold_seconds`) — they are not widened here.
fn default_integrity_stale_threshold_ticks(timeframe_secs: i64) -> u64 {
    if timeframe_secs >= 86_400 {
        345_600
    } else {
        120
    }
}

/// Resolve the integrity stale-data threshold to apply for a backtest run.
///
/// An explicit operator-supplied value always wins and is never widened or
/// clamped (request validation remains the fail-closed authority on bad
/// values). When absent, the timeframe-aware default is applied.
fn resolve_integrity_stale_threshold(explicit: Option<u64>, timeframe_secs: i64) -> u64 {
    explicit.unwrap_or_else(|| default_integrity_stale_threshold_ticks(timeframe_secs))
}

/// Run a CSV backtest synchronously and write artifacts.
///
/// Returns `(artifact_dir, manifest_path, metrics_path)` on success.
/// Returns `Err(message)` on any failure — never panics.
///
/// Safety: this function does NOT touch any broker adapter, OMS table,
/// live execution path, or Alpaca API. It is pure offline research work.
#[allow(clippy::too_many_arguments)]
fn run_backtest_csv_blocking(
    bars_path: String,
    strategy: String,
    symbol: String,
    timeframe_secs: i64,
    initial_cash_micros: i64,
    integrity_enabled: bool,
    integrity_stale_threshold_ticks: Option<u64>,
    shadow: bool,
    out_dir: String,
) -> Result<(String, String, String), String> {
    // Load bars from CSV.
    let bars = mqk_backtest::load_csv_file(&bars_path)
        .map_err(|e| format!("load bars csv failed: {}: {}", bars_path, e))?;

    // Build engine config.
    let mut cfg = mqk_backtest::BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = timeframe_secs;
    cfg.initial_cash_micros = initial_cash_micros;
    cfg.shadow_mode = shadow;
    cfg.integrity_enabled = integrity_enabled;
    // Timeframe-aware stale threshold: conservative_defaults() uses 120 s which
    // immediately triggers execution_blocked=true on daily bar gaps (86400 s).
    // Apply operator-supplied value when present; otherwise default by timeframe.
    cfg.integrity_stale_threshold_ticks =
        resolve_integrity_stale_threshold(integrity_stale_threshold_ticks, timeframe_secs);

    // Resolve strategy from built-in registry.
    let mut reg = mqk_strategy::PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies(&mut reg, &symbol).map_err(|e| {
        format!(
            "register_builtin_strategies failed for symbol={}: {}",
            symbol, e
        )
    })?;

    let strategy_instance = reg.instantiate(&strategy).map_err(|_| {
        let available: Vec<_> = reg.list().iter().map(|m| m.name.as_str()).collect();
        format!(
            "unknown strategy '{}'; available: {}",
            strategy,
            available.join(", ")
        )
    })?;

    let mut engine = mqk_backtest::BacktestEngine::new(cfg);
    engine
        .add_strategy(strategy_instance)
        .map_err(|e| format!("add_strategy failed for '{}': {}", strategy, e))?;

    // Run backtest — deterministic replay.
    let report = engine
        .run(&bars)
        .map_err(|e| format!("backtest run failed: {}", e))?;

    // Derive provenance metadata (same pattern as CLI bkt.rs).
    let config_hash = report.config_id.to_string();
    let git_hash = bkt_git_hash();
    let host_fp = bkt_host_fingerprint();

    // Initialize run artifact directory.
    let exports_root = Path::new(&out_dir);
    let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(timeframe_secs),
        git_hash: &git_hash,
        config_hash: &config_hash,
        host_fingerprint: &host_fp,
        now_utc: Utc::now(), // allow: operational manifest timestamp
    })
    .map_err(|e| format!("init run artifacts failed: {}: {}", out_dir, e))?;

    // Write backtest report artifacts.
    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash_micros)
        .map_err(|e| {
            format!(
                "write backtest artifacts failed: {}: {}",
                init_result.run_dir.display(),
                e
            )
        })?;

    // Verify the critical artifact files exist before claiming success.
    let manifest = init_result.manifest_path.clone();
    let metrics = init_result.run_dir.join("metrics.json");
    if !manifest.exists() {
        return Err(format!(
            "manifest.json not found after write: {}",
            manifest.display()
        ));
    }
    if !metrics.exists() {
        return Err(format!(
            "metrics.json not found after write: {}",
            metrics.display()
        ));
    }

    Ok((
        init_result.run_dir.display().to_string(),
        manifest.display().to_string(),
        metrics.display().to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Operational helpers (same as CLI bkt.rs helpers)
// ---------------------------------------------------------------------------

fn bkt_git_hash() -> String {
    use std::process::Command;
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

fn bkt_host_fingerprint() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    format!(
        "{}@{}:{}/{}",
        username,
        hostname,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

// ---------------------------------------------------------------------------
// BACKTEST-DAILY-STALE-DEFAULT-FIX-01: stale-threshold defaulting unit tests.
//
// These prove the daemon's *actual* default constant and override resolution,
// which are not observable from the JSON job API (the value is not echoed in
// metrics.json). The engine-level value justification for daily/weekend gaps is
// proven separately as BJ-D04 in tests/scenario_backtest_jobs_01.rs.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod stale_threshold_default_tests {
    use super::{default_integrity_stale_threshold_ticks, resolve_integrity_stale_threshold};

    // BJ-D01: daily timeframe default stale threshold is 345_600 or greater.
    #[test]
    fn bj_d01_daily_default_is_at_least_345600() {
        // Exactly one trading day and longer all use the daily default.
        for tf in [86_400i64, 604_800, 2_592_000] {
            let got = default_integrity_stale_threshold_ticks(tf);
            assert!(
                got >= 345_600,
                "daily default for tf={tf}s must be >= 345600, got {got}"
            );
            assert_eq!(
                got, 345_600,
                "daily default for tf={tf}s must be exactly 345600 (matches runbook)"
            );
        }
        // Resolution with no explicit value must yield the daily default.
        assert_eq!(
            resolve_integrity_stale_threshold(None, 86_400),
            345_600,
            "daily resolution with no explicit value must be 345600"
        );
    }

    // BJ-D02: an explicit operator-supplied value still overrides the default.
    #[test]
    fn bj_d02_explicit_value_overrides_daily_default() {
        // A small explicit value must win even on a daily timeframe.
        assert_eq!(
            resolve_integrity_stale_threshold(Some(120), 86_400),
            120,
            "explicit 120 must override the daily default"
        );
        // A large explicit value must also be honored verbatim (not clamped).
        assert_eq!(
            resolve_integrity_stale_threshold(Some(1_000_000), 86_400),
            1_000_000,
            "explicit value must be applied verbatim, never widened or clamped"
        );
        // Override applies to intraday too.
        assert_eq!(
            resolve_integrity_stale_threshold(Some(300), 60),
            300,
            "explicit value must override the intraday default"
        );
    }

    // BJ-D03: intraday default behavior is unchanged (still 120 s, not widened).
    #[test]
    fn bj_d03_intraday_default_unchanged() {
        for tf in [1i64, 60, 300, 3_600, 86_399] {
            assert_eq!(
                default_integrity_stale_threshold_ticks(tf),
                120,
                "intraday/sub-daily tf={tf}s default must remain 120 (unchanged)"
            );
        }
        assert_eq!(
            resolve_integrity_stale_threshold(None, 60),
            120,
            "intraday resolution with no explicit value must be 120"
        );
    }
}
