//! DATA-FRESHNESS-READINESS-GATE-01: Market-data freshness evaluation.
//!
//! Evaluates whether md_bars contains sufficient fresh completed bars for the
//! configured strategy symbol/timeframe.  Used by autonomous readiness, system
//! preflight, and start_execution_runtime to block startup when data is missing,
//! insufficient, or stale.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::api_types::MarketDataFreshnessStatus;

/// Minimum completed bars required before paper trading is allowed.
pub const MD_FRESHNESS_MIN_BARS: u64 = 5;

/// Maximum age in seconds of the latest completed bar before it is considered stale.
/// 4 trading days × 86400 s/day.  Mirrors `-MaxStalenessDays 4` in
/// `Start-PaperTradingSmoke.ps1` (DATA-FRESHNESS-READINESS-GATE-01).
pub const MD_FRESHNESS_STALE_SECS: i64 = 4 * 24 * 3600;

/// Env override for intraday completed-bar max age.
pub const INTRADAY_BAR_MAX_AGE_SECS_ENV: &str = "MQK_INTRADAY_BAR_MAX_AGE_SECS";

/// Default intraday completed-bar max age.
///
/// This allows a short delay after a completed 5m bar while refusing prior-session
/// bars during an active intraday session.
pub const DEFAULT_INTRADAY_BAR_MAX_AGE_SECS: i64 = 900;

pub const REASON_CODE_OK: &str = "ok";
pub const REASON_CODE_NOT_APPLICABLE: &str = "not_applicable";
pub const REASON_CODE_MARKET_DATA_UNAVAILABLE: &str = "market_data_unavailable";
pub const REASON_CODE_MARKET_DATA_MISSING: &str = "market_data_missing";
pub const REASON_CODE_MARKET_DATA_NOT_REFRESHED: &str = "market_data_not_refreshed";
pub const REASON_CODE_BAR_DATA_STALE: &str = "bar_data_stale";
pub const REASON_CODE_INTRADAY_BAR_NOT_CURRENT: &str = "intraday_bar_not_current";
pub const REASON_CODE_INTRADAY_BAR_STALE: &str = "intraday_bar_stale";

/// Convert a unix-seconds timestamp to RFC3339 UTC.
pub fn unix_ts_to_rfc3339(ts: i64) -> Option<String> {
    DateTime::from_timestamp(ts, 0).map(|dt: DateTime<Utc>| dt.to_rfc3339())
}

fn unix_ts_to_rfc3339_or_raw(ts: i64) -> String {
    unix_ts_to_rfc3339(ts).unwrap_or_else(|| ts.to_string())
}

/// Read the intraday max-age cap from env, falling back fail-closed to the
/// documented default when absent or invalid.
pub fn intraday_bar_max_age_secs_from_env() -> i64 {
    std::env::var(INTRADAY_BAR_MAX_AGE_SECS_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n >= 0)
        .unwrap_or(DEFAULT_INTRADAY_BAR_MAX_AGE_SECS)
}

/// Parse current daemon/md timeframe spellings into seconds.
pub fn timeframe_secs(timeframe: &str) -> Option<i64> {
    let tf = timeframe.trim().to_ascii_lowercase().replace(' ', "");
    if tf.is_empty() {
        return None;
    }

    match tf.as_str() {
        "1d" | "d1" | "1day" | "1days" | "day" | "daily" => return Some(86_400),
        "1h" | "h1" | "1hour" | "1hours" => return Some(3_600),
        "1m" | "m1" | "1min" | "1mins" | "1minute" | "1minutes" => return Some(60),
        "5m" | "m5" | "5min" | "5mins" | "5minute" | "5minutes" => return Some(300),
        "15m" | "m15" | "15min" | "15mins" | "15minute" | "15minutes" => return Some(900),
        "30m" | "m30" | "30min" | "30mins" | "30minute" | "30minutes" => return Some(1_800),
        _ => {}
    }

    for suffix in ["minutes", "minute", "mins", "min", "m"] {
        if let Some(n) = tf.strip_suffix(suffix).and_then(|s| s.parse::<i64>().ok()) {
            return (n > 0).then_some(n * 60);
        }
    }
    for suffix in ["hours", "hour", "hrs", "hr", "h"] {
        if let Some(n) = tf.strip_suffix(suffix).and_then(|s| s.parse::<i64>().ok()) {
            return (n > 0).then_some(n * 3_600);
        }
    }
    for suffix in ["days", "day", "d"] {
        if let Some(n) = tf.strip_suffix(suffix).and_then(|s| s.parse::<i64>().ok()) {
            return (n > 0).then_some(n * 86_400);
        }
    }
    for suffix in ["seconds", "second", "secs", "sec", "s"] {
        if let Some(n) = tf.strip_suffix(suffix).and_then(|s| s.parse::<i64>().ok()) {
            return (n > 0).then_some(n);
        }
    }

    None
}

/// True for known intraday timeframes and unknown non-empty timeframes.
///
/// Unknown non-empty values use the tighter intraday cap fail-closed; only
/// recognized daily-or-higher intervals retain the broad daily tolerance.
pub fn is_intraday_timeframe(timeframe: &str) -> bool {
    let trimmed = timeframe.trim();
    if trimmed.is_empty() {
        return false;
    }
    match timeframe_secs(trimmed) {
        Some(secs) => secs < 86_400,
        None => true,
    }
}

/// Effective default max age for a completed bar on the given timeframe.
pub fn default_max_allowed_age_secs_for_timeframe(timeframe: &str) -> i64 {
    if is_intraday_timeframe(timeframe) {
        intraday_bar_max_age_secs_from_env()
    } else {
        MD_FRESHNESS_STALE_SECS
    }
}

#[allow(clippy::too_many_arguments)]
fn status(
    symbol: &str,
    timeframe: &str,
    completed_rows: u64,
    latest_end_ts: Option<i64>,
    freshness_state: &str,
    reason_code: &str,
    reason: String,
    now_ts: i64,
    max_allowed_age_seconds: i64,
) -> MarketDataFreshnessStatus {
    let latest_completed_bar_ts = latest_end_ts.and_then(unix_ts_to_rfc3339);
    MarketDataFreshnessStatus {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        completed_rows,
        min_required_rows: MD_FRESHNESS_MIN_BARS,
        latest_complete_bar_ts: latest_completed_bar_ts.clone(),
        latest_completed_bar_ts,
        now_utc: unix_ts_to_rfc3339_or_raw(now_ts),
        age_seconds: latest_end_ts.map(|ts| (now_ts - ts).max(0)),
        max_allowed_age_seconds,
        freshness_state: freshness_state.to_string(),
        reason_code: reason_code.to_string(),
        reason,
    }
}

/// Evaluate market-data freshness from an already-known completed-row snapshot.
///
/// This is the pure core used by the DB-backed readiness evaluator and focused
/// scenario tests. It performs no DB, provider, broker, or order activity.
pub fn evaluate_md_freshness_snapshot(
    symbol: &str,
    timeframe: &str,
    completed_rows_u64: u64,
    latest_end_ts: Option<i64>,
    now_ts: i64,
) -> MarketDataFreshnessStatus {
    let max_allowed_age_seconds = default_max_allowed_age_secs_for_timeframe(timeframe);
    let now_utc = unix_ts_to_rfc3339_or_raw(now_ts);
    let intraday = is_intraday_timeframe(timeframe);

    if completed_rows_u64 == 0 {
        let reason_code = if intraday {
            REASON_CODE_INTRADAY_BAR_NOT_CURRENT
        } else {
            REASON_CODE_MARKET_DATA_MISSING
        };
        return status(
            symbol,
            timeframe,
            0,
            None,
            "missing",
            reason_code,
            format!(
                "{reason_code}: md_bars has no completed bars for {symbol}/{timeframe}; \
                 latest_completed_bar_ts=null, now_utc={now_utc}, age_seconds=null, \
                 max_allowed_age_seconds={max_allowed_age_seconds}; run \
                 Prep-PremarketMarketData.ps1 to ingest historical bars before starting \
                 (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
            now_ts,
            max_allowed_age_seconds,
        );
    }

    if completed_rows_u64 < MD_FRESHNESS_MIN_BARS {
        let reason_code = if intraday {
            REASON_CODE_MARKET_DATA_NOT_REFRESHED
        } else {
            "market_data_insufficient"
        };
        let latest_completed_bar_ts = latest_end_ts
            .and_then(unix_ts_to_rfc3339)
            .unwrap_or_else(|| "null".to_string());
        let age_seconds = latest_end_ts
            .map(|ts| (now_ts - ts).max(0).to_string())
            .unwrap_or_else(|| "null".to_string());
        return status(
            symbol,
            timeframe,
            completed_rows_u64,
            latest_end_ts,
            "insufficient",
            reason_code,
            format!(
                "{reason_code}: md_bars has {completed_rows_u64} completed bar(s) for \
                 {symbol}/{timeframe} but strategy requires at least \
                 {MD_FRESHNESS_MIN_BARS}; latest_completed_bar_ts={latest_completed_bar_ts}, \
                 now_utc={now_utc}, age_seconds={age_seconds}, \
                 max_allowed_age_seconds={max_allowed_age_seconds}; run \
                 Prep-PremarketMarketData.ps1 to ingest more bars \
                 (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
            now_ts,
            max_allowed_age_seconds,
        );
    }

    let latest_end_ts = latest_end_ts.unwrap_or(0);
    let age_secs = (now_ts - latest_end_ts).max(0);
    let latest_completed_bar_ts =
        unix_ts_to_rfc3339(latest_end_ts).unwrap_or_else(|| latest_end_ts.to_string());

    if age_secs > max_allowed_age_seconds {
        let reason_code = if intraday {
            REASON_CODE_INTRADAY_BAR_STALE
        } else {
            REASON_CODE_BAR_DATA_STALE
        };
        return status(
            symbol,
            timeframe,
            completed_rows_u64,
            Some(latest_end_ts),
            "stale",
            reason_code,
            format!(
                "{reason_code}: latest completed bar for {symbol}/{timeframe} is \
                 {age_secs}s old; latest_completed_bar_ts={latest_completed_bar_ts}, \
                 now_utc={now_utc}, age_seconds={age_secs}, \
                 max_allowed_age_seconds={max_allowed_age_seconds}; refresh completed \
                 bars before autonomous dispatch (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
            now_ts,
            max_allowed_age_seconds,
        );
    }

    status(
        symbol,
        timeframe,
        completed_rows_u64,
        Some(latest_end_ts),
        "ok",
        REASON_CODE_OK,
        format!(
            "{completed_rows_u64} completed bars for {symbol}/{timeframe}; \
             latest_completed_bar_ts={latest_completed_bar_ts}, now_utc={now_utc}, \
             age_seconds={age_secs}, max_allowed_age_seconds={max_allowed_age_seconds}"
        ),
        now_ts,
        max_allowed_age_seconds,
    )
}

/// Evaluate market-data freshness for the given symbol/timeframe against the DB.
///
/// `now_ts` is injected for testability; pass `Utc::now().timestamp()` in production.
///
/// Gate logic:
/// - `not_applicable` — symbol or timeframe empty; env vars not configured.
/// - `unavailable` — DB not reachable; cannot verify; pass-through (not a blocker).
/// - `missing` — 0 completed bars in `md_bars`; blocks startup.
/// - `insufficient` — fewer than `MD_FRESHNESS_MIN_BARS` completed bars; blocks startup.
/// - `stale` — latest bar older than the timeframe-aware max age; blocks startup.
/// - `ok` — all checks pass; startup allowed.
pub async fn evaluate_md_freshness_status(
    db: Option<&PgPool>,
    symbol: &str,
    timeframe: &str,
    now_ts: i64,
) -> MarketDataFreshnessStatus {
    let max_allowed_age_seconds = default_max_allowed_age_secs_for_timeframe(timeframe);
    let now_utc = unix_ts_to_rfc3339_or_raw(now_ts);

    if symbol.is_empty() || timeframe.is_empty() {
        return status(
            symbol,
            timeframe,
            0,
            None,
            "not_applicable",
            REASON_CODE_NOT_APPLICABLE,
            "MQK_STRATEGY_SYMBOL or MQK_STRATEGY_MD_TIMEFRAME is not configured; \
             market-data freshness gate is not applicable"
                .to_string(),
            now_ts,
            max_allowed_age_seconds,
        );
    }

    let pool = match db {
        Some(p) => p,
        None => {
            return status(
                symbol,
                timeframe,
                0,
                None,
                "unavailable",
                REASON_CODE_MARKET_DATA_UNAVAILABLE,
                format!(
                    "DB is not reachable; market-data freshness cannot be verified \
                     (now_utc={now_utc}, max_allowed_age_seconds={max_allowed_age_seconds}) \
                     (DATA-FRESHNESS-READINESS-GATE-01)"
                ),
                now_ts,
                max_allowed_age_seconds,
            );
        }
    };

    let query_result = sqlx::query(
        r#"
        select
            count(*) filter (where is_complete = true)    as completed_rows,
            max(end_ts)  filter (where is_complete = true) as latest_end_ts
        from md_bars
        where symbol   = $1
          and timeframe = $2
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .fetch_one(pool)
    .await;

    let row = match query_result {
        Ok(r) => r,
        Err(err) => {
            return status(
                symbol,
                timeframe,
                0,
                None,
                "unavailable",
                REASON_CODE_MARKET_DATA_UNAVAILABLE,
                format!(
                    "md_bars query failed; freshness cannot be verified: {err} \
                     (now_utc={now_utc}, max_allowed_age_seconds={max_allowed_age_seconds}) \
                     (DATA-FRESHNESS-READINESS-GATE-01)"
                ),
                now_ts,
                max_allowed_age_seconds,
            );
        }
    };

    let completed_rows: i64 = row.try_get::<i64, _>("completed_rows").unwrap_or(0);
    let completed_rows_u64 = completed_rows.max(0) as u64;
    let latest_end_ts: Option<i64> = row
        .try_get::<Option<i64>, _>("latest_end_ts")
        .unwrap_or(None);

    evaluate_md_freshness_snapshot(symbol, timeframe, completed_rows_u64, latest_end_ts, now_ts)
}
