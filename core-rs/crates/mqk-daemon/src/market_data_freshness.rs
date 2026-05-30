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

/// Evaluate market-data freshness for the given symbol/timeframe against the DB.
///
/// `now_ts` is injected for testability; pass `Utc::now().timestamp()` in production.
///
/// Gate logic:
/// - `not_applicable` — symbol or timeframe empty; env vars not configured.
/// - `unavailable` — DB not reachable; cannot verify; pass-through (not a blocker).
/// - `missing` — 0 completed bars in `md_bars`; blocks startup.
/// - `insufficient` — fewer than `MD_FRESHNESS_MIN_BARS` completed bars; blocks startup.
/// - `stale` — latest bar older than `MD_FRESHNESS_STALE_SECS`; blocks startup.
/// - `ok` — all checks pass; startup allowed.
pub async fn evaluate_md_freshness_status(
    db: Option<&PgPool>,
    symbol: &str,
    timeframe: &str,
    now_ts: i64,
) -> MarketDataFreshnessStatus {
    if symbol.is_empty() || timeframe.is_empty() {
        return MarketDataFreshnessStatus {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            completed_rows: 0,
            min_required_rows: MD_FRESHNESS_MIN_BARS,
            latest_complete_bar_ts: None,
            freshness_state: "not_applicable".to_string(),
            reason: "MQK_STRATEGY_SYMBOL or MQK_STRATEGY_MD_TIMEFRAME is not configured; \
                     market-data freshness gate is not applicable"
                .to_string(),
        };
    }

    let pool = match db {
        Some(p) => p,
        None => {
            return MarketDataFreshnessStatus {
                symbol: symbol.to_string(),
                timeframe: timeframe.to_string(),
                completed_rows: 0,
                min_required_rows: MD_FRESHNESS_MIN_BARS,
                latest_complete_bar_ts: None,
                freshness_state: "unavailable".to_string(),
                reason: "DB is not reachable; market-data freshness cannot be verified \
                         (DATA-FRESHNESS-READINESS-GATE-01)"
                    .to_string(),
            };
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
            return MarketDataFreshnessStatus {
                symbol: symbol.to_string(),
                timeframe: timeframe.to_string(),
                completed_rows: 0,
                min_required_rows: MD_FRESHNESS_MIN_BARS,
                latest_complete_bar_ts: None,
                freshness_state: "unavailable".to_string(),
                reason: format!(
                    "md_bars query failed; freshness cannot be verified: {err} \
                     (DATA-FRESHNESS-READINESS-GATE-01)"
                ),
            };
        }
    };

    let completed_rows: i64 = row.try_get::<i64, _>("completed_rows").unwrap_or(0);
    let completed_rows_u64 = completed_rows.max(0) as u64;
    let latest_end_ts: Option<i64> = row
        .try_get::<Option<i64>, _>("latest_end_ts")
        .unwrap_or(None);

    if completed_rows_u64 == 0 {
        return MarketDataFreshnessStatus {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            completed_rows: 0,
            min_required_rows: MD_FRESHNESS_MIN_BARS,
            latest_complete_bar_ts: None,
            freshness_state: "missing".to_string(),
            reason: format!(
                "md_bars has no completed bars for {symbol}/{timeframe}; \
                 run Prep-PremarketMarketData.ps1 to ingest historical bars before \
                 starting (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
        };
    }

    if completed_rows_u64 < MD_FRESHNESS_MIN_BARS {
        let ts_str = latest_end_ts.and_then(end_ts_to_rfc3339);
        return MarketDataFreshnessStatus {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            completed_rows: completed_rows_u64,
            min_required_rows: MD_FRESHNESS_MIN_BARS,
            latest_complete_bar_ts: ts_str,
            freshness_state: "insufficient".to_string(),
            reason: format!(
                "md_bars has {completed_rows_u64} completed bar(s) for {symbol}/{timeframe} \
                 but strategy requires at least {MD_FRESHNESS_MIN_BARS}; \
                 run Prep-PremarketMarketData.ps1 to ingest more bars \
                 (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
        };
    }

    let latest_end_ts = latest_end_ts.unwrap_or(0);
    let age_secs = now_ts - latest_end_ts;
    let ts_str = end_ts_to_rfc3339(latest_end_ts);

    if age_secs > MD_FRESHNESS_STALE_SECS {
        return MarketDataFreshnessStatus {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            completed_rows: completed_rows_u64,
            min_required_rows: MD_FRESHNESS_MIN_BARS,
            latest_complete_bar_ts: ts_str,
            freshness_state: "stale".to_string(),
            reason: format!(
                "latest completed bar for {symbol}/{timeframe} is {age_secs}s old \
                 (threshold: {MD_FRESHNESS_STALE_SECS}s / 4 trading days); \
                 run Prep-PremarketMarketData.ps1 to refresh bars before starting \
                 (DATA-FRESHNESS-READINESS-GATE-01)"
            ),
        };
    }

    MarketDataFreshnessStatus {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        completed_rows: completed_rows_u64,
        min_required_rows: MD_FRESHNESS_MIN_BARS,
        latest_complete_bar_ts: ts_str,
        freshness_state: "ok".to_string(),
        reason: format!(
            "{completed_rows_u64} completed bars for {symbol}/{timeframe}; \
             latest bar is {age_secs}s old (within threshold of {MD_FRESHNESS_STALE_SECS}s)"
        ),
    }
}

fn end_ts_to_rfc3339(end_ts: i64) -> Option<String> {
    DateTime::from_timestamp(end_ts, 0).map(|dt: DateTime<Utc>| dt.to_rfc3339())
}
