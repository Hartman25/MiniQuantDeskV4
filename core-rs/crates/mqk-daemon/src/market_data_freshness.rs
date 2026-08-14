//! DATA-FRESHNESS-READINESS-GATE-01: Market-data freshness evaluation.
//!
//! Evaluates whether md_bars contains sufficient fresh completed bars for the
//! configured strategy symbol/timeframe.  Used by autonomous readiness, system
//! preflight, and start_execution_runtime to block startup when data is missing,
//! insufficient, or stale.
//!
//! PREMARKET-DATA-READINESS-GATE-01 extends this to every symbol the current
//! deployment requires (not just one): [`required_symbols_for_freshness_gate_from_env`]
//! resolves the required set, [`evaluate_md_freshness_status_for_symbols`]
//! evaluates each independently via [`evaluate_md_freshness_status`] (no
//! duplicate DB logic), and [`aggregate_freshness_statuses`] combines the
//! results into one fail-closed [`MultiSymbolFreshnessReport`] decision.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::api_types::{MarketDataFreshnessStatus, MultiSymbolFreshnessReport};
use crate::watchlist_intake::WatchlistIntakeOutcome;

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

// ---------------------------------------------------------------------------
// PREMARKET-DATA-READINESS-GATE-01: multi-symbol aggregation
// ---------------------------------------------------------------------------

/// One symbol/timeframe pair the premarket readiness gate must verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredSymbolTimeframe {
    pub symbol: String,
    pub timeframe: String,
}

/// Normalize a required-symbol list: trim and uppercase each symbol, trim
/// each timeframe (case is significant for the `md_bars.timeframe` column,
/// so it is not folded), drop entries that are empty after trimming, and
/// collapse exact `(symbol, timeframe)` duplicates to their first
/// occurrence. Input order is otherwise preserved.
///
/// Pure: no DB, network, or broker access.
pub fn normalize_required_symbols(
    required: &[RequiredSymbolTimeframe],
) -> Vec<RequiredSymbolTimeframe> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(required.len());
    for r in required {
        let symbol = r.symbol.trim().to_uppercase();
        let timeframe = r.timeframe.trim().to_string();
        if symbol.is_empty() || timeframe.is_empty() {
            continue;
        }
        if seen.insert((symbol.clone(), timeframe.clone())) {
            out.push(RequiredSymbolTimeframe { symbol, timeframe });
        }
    }
    out
}

/// Aggregate already-evaluated per-symbol freshness statuses into a single
/// [`MultiSymbolFreshnessReport`] (PREMARKET-DATA-READINESS-GATE-01).
///
/// Pure: performs no DB, network, or broker access. `per_symbol` must
/// already be evaluated (e.g. via repeated [`evaluate_md_freshness_status`]
/// calls over a [`normalize_required_symbols`]-normalized list), and
/// `required_symbols` should be the matching normalized symbol list.
///
/// # Aggregate semantics
/// - `per_symbol` empty → `"not_applicable"`; start allowed (no symbol is
///   configured; mirrors the single-symbol gate's behaviour when env vars
///   are absent).
/// - Exactly one required symbol → `aggregate_status` is exactly that
///   symbol's own `freshness_state` (`"ok"` | `"missing"` | `"insufficient"`
///   | `"stale"` | `"unavailable"`) — byte-identical to the pre-existing
///   single-symbol gate decision (backward compatibility).
/// - More than one required symbol, more than one blocking → `"mixed_blocked"`.
/// - More than one required symbol, exactly one blocking → that symbol's own
///   blocking state (named explicitly, not a vague aggregate).
/// - More than one required symbol, none blocking, at least one
///   `"unavailable"` → `"unavailable"` (cannot fully assert ok, but not a
///   start blocker — same honesty rule as the single-symbol gate).
/// - Otherwise → `"ok"`.
///
/// `start_allowed` is true unless `aggregate_status` is one of `"missing"`,
/// `"insufficient"`, `"stale"`, or `"mixed_blocked"`.
pub fn aggregate_freshness_statuses(
    required_symbols: Vec<String>,
    per_symbol: Vec<MarketDataFreshnessStatus>,
) -> MultiSymbolFreshnessReport {
    if per_symbol.is_empty() {
        return MultiSymbolFreshnessReport {
            aggregate_status: REASON_CODE_NOT_APPLICABLE.to_string(),
            start_allowed: true,
            required_symbols: Vec::new(),
            per_symbol: Vec::new(),
            blockers: Vec::new(),
        };
    }

    let blockers: Vec<String> = per_symbol
        .iter()
        .filter(|s| s.is_start_blocker())
        .map(|s| s.reason.clone())
        .collect();
    let blocking_count = blockers.len();

    let aggregate_status = if per_symbol.len() == 1 {
        per_symbol[0].freshness_state.clone()
    } else if blocking_count == 1 {
        per_symbol
            .iter()
            .find(|s| s.is_start_blocker())
            .map(|s| s.freshness_state.clone())
            .unwrap_or_else(|| "mixed_blocked".to_string())
    } else if blocking_count > 1 {
        "mixed_blocked".to_string()
    } else if per_symbol
        .iter()
        .any(|s| s.freshness_state == "unavailable")
    {
        "unavailable".to_string()
    } else {
        REASON_CODE_OK.to_string()
    };

    let start_allowed = !matches!(
        aggregate_status.as_str(),
        "missing" | "insufficient" | "stale" | "mixed_blocked"
    );

    MultiSymbolFreshnessReport {
        aggregate_status,
        start_allowed,
        required_symbols,
        per_symbol,
        blockers,
    }
}

/// Evaluate market-data freshness for every required symbol/timeframe pair
/// against the DB and aggregate the results
/// (PREMARKET-DATA-READINESS-GATE-01).
///
/// Extends [`evaluate_md_freshness_status`] (single symbol) to a required
/// set: `required` is normalized via [`normalize_required_symbols`], each
/// resulting pair is evaluated independently with the exact same pure/DB
/// logic as the single-symbol gate (one query per unique pair — no
/// duplicate DB logic, no broker/order/outbox access), and the results are
/// combined via [`aggregate_freshness_statuses`].
///
/// `now_ts` is injected for testability; pass `Utc::now().timestamp()` in
/// production.
pub async fn evaluate_md_freshness_status_for_symbols(
    db: Option<&PgPool>,
    required: &[RequiredSymbolTimeframe],
    now_ts: i64,
) -> MultiSymbolFreshnessReport {
    let normalized = normalize_required_symbols(required);
    if normalized.is_empty() {
        return aggregate_freshness_statuses(Vec::new(), Vec::new());
    }

    let mut per_symbol = Vec::with_capacity(normalized.len());
    for req in &normalized {
        per_symbol
            .push(evaluate_md_freshness_status(db, &req.symbol, &req.timeframe, now_ts).await);
    }
    let required_symbols = normalized.into_iter().map(|r| r.symbol).collect();

    aggregate_freshness_statuses(required_symbols, per_symbol)
}

/// Resolve the symbol/timeframe pairs the premarket readiness gate must
/// verify, from the environment (PREMARKET-DATA-READINESS-GATE-01).
///
/// # Source preference
/// 1. An approved `watchlist-v2` artifact (`MQK_PAPER_WATCHLIST_PATH`,
///    [`crate::watchlist_intake::evaluate_watchlist_intake_from_env`]) — the
///    same multi-symbol source `loop_runner.rs` already prefers for actual
///    dispatch. Every symbol in `artifact.symbols` is paired with the shared
///    `MQK_STRATEGY_MD_TIMEFRAME` (per-symbol timeframe overrides are not
///    yet supported anywhere in the runtime — Tier A is one global
///    timeframe; see `crate::state::multi_symbol_config`).
/// 2. Otherwise, the legacy single `MQK_STRATEGY_SYMBOL` /
///    `MQK_STRATEGY_MD_TIMEFRAME` pair — the exact source the freshness
///    gate has always used.
///
/// Deliberately independent of `MQK_STRATEGY_IDS` / strategy-id
/// configuration: freshness is a property of (symbol, timeframe) data
/// availability in `md_bars`, not of which strategy a symbol is mapped to.
/// `crate::state::multi_symbol_config::build_legacy_single_symbol_config`
/// requires a strategy id and is deliberately not reused here — doing so
/// would make this gate go silently `not_applicable` whenever
/// `MQK_STRATEGY_IDS` is unset while `MQK_STRATEGY_SYMBOL` is set, which is
/// not how the pre-existing single-symbol gate behaved.
///
/// Returns an empty list when neither source yields a usable symbol or the
/// shared timeframe is absent — callers must treat an empty list as
/// `"not_applicable"`, exactly matching prior single-symbol behavior when
/// env vars were absent.
///
/// Thin wrapper over [`required_symbols_with_source_from_env`] — kept for
/// existing callers that only need the resolved list, not its provenance.
pub fn required_symbols_for_freshness_gate_from_env() -> Vec<RequiredSymbolTimeframe> {
    required_symbols_with_source_from_env().required
}

// ---------------------------------------------------------------------------
// OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01: structurally-pending first bar
// ---------------------------------------------------------------------------

/// True when `now_ts` falls within the structurally-guaranteed publication
/// window for the *first* bar of the current trading session on `timeframe`
/// — the session has opened, but the first bar's interval plus publication
/// grace has not yet elapsed, so no current-session bar can possibly exist
/// yet. Every completed bar this evaluator can see is therefore necessarily
/// from a prior session, and a flat wall-clock "stale" verdict from
/// [`evaluate_md_freshness_snapshot`] reflects that structural fact, not a
/// genuine data problem.
///
/// `false` on a non-trading day (`schedule.is_trading_day == false`) or
/// before the session has opened — neither condition means "awaiting the
/// first bar", and both are already gated elsewhere (`AwaitingSessionOpen`/
/// `NonTradingDay`) before this evaluator would ever run.
///
/// Once `now_ts` reaches `session_open_utc + timeframe_secs +
/// effective_grace_seconds`, the first bar should exist; from that instant
/// on this returns `false` and ordinary staleness rules resume unchanged
/// (OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01).
pub fn is_awaiting_first_session_bar(
    schedule: &crate::state::market_calendar::MarketSessionSchedule,
    timeframe_secs: i64,
    effective_grace_seconds: i64,
    now_ts: i64,
) -> bool {
    if !schedule.is_trading_day {
        return false;
    }
    let open_ts = schedule.session_open_utc.timestamp();
    if now_ts < open_ts {
        return false;
    }
    now_ts < open_ts + timeframe_secs + effective_grace_seconds
}

#[cfg(test)]
mod opening_bar_tests {
    use super::*;
    use crate::state::market_calendar::CalendarCoverageState;

    fn schedule(
        session_open_utc_ts: i64,
        is_trading_day: bool,
    ) -> crate::state::market_calendar::MarketSessionSchedule {
        crate::state::market_calendar::MarketSessionSchedule {
            market_date: (2024, 4, 15),
            session_open_utc: DateTime::<Utc>::from_timestamp(session_open_utc_ts, 0).unwrap(),
            session_close_utc: DateTime::<Utc>::from_timestamp(session_open_utc_ts + 6 * 3600 + 1800, 0)
                .unwrap(),
            previous_trading_date: (2024, 4, 12),
            is_early_close: false,
            is_trading_day,
            calendar_source: "test",
            coverage_state: CalendarCoverageState::Active,
        }
    }

    const OPEN_TS: i64 = 1_713_180_600; // 2024-04-15 13:30 UTC (09:30 EDT)

    #[test]
    fn obf_01_true_45_seconds_after_open_for_5m_timeframe() {
        let sched = schedule(OPEN_TS, true);
        assert!(is_awaiting_first_session_bar(&sched, 300, 300, OPEN_TS + 45));
    }

    #[test]
    fn obf_02_false_once_first_bar_interval_plus_grace_has_elapsed() {
        let sched = schedule(OPEN_TS, true);
        assert!(!is_awaiting_first_session_bar(
            &sched,
            300,
            300,
            OPEN_TS + 300 + 300
        ));
    }

    #[test]
    fn obf_03_false_well_inside_the_session() {
        let sched = schedule(OPEN_TS, true);
        assert!(!is_awaiting_first_session_bar(
            &sched,
            300,
            300,
            OPEN_TS + 3600
        ));
    }

    #[test]
    fn obf_04_false_on_a_non_trading_day_regardless_of_wall_clock() {
        let sched = schedule(OPEN_TS, false);
        assert!(!is_awaiting_first_session_bar(&sched, 300, 300, OPEN_TS + 45));
    }

    #[test]
    fn obf_05_false_before_session_open() {
        let sched = schedule(OPEN_TS, true);
        assert!(!is_awaiting_first_session_bar(
            &sched,
            300,
            300,
            OPEN_TS - 10
        ));
    }

    #[test]
    fn obf_06_boundary_instant_itself_is_no_longer_pending() {
        let sched = schedule(OPEN_TS, true);
        // now_ts == open + interval + grace exactly: the window is a
        // half-open [open, open+interval+grace) — the boundary instant
        // itself is not "awaiting" (the bar should exist by now).
        assert!(!is_awaiting_first_session_bar(
            &sched,
            300,
            300,
            OPEN_TS + 600
        ));
    }
}

/// Source label: an approved `watchlist-v2` artifact supplied the required
/// symbols (WATCHLIST-INGEST-PLAN-01).
pub const SYMBOL_SOURCE_WATCHLIST_V2: &str = "watchlist_v2";

/// Source label: the legacy single `MQK_STRATEGY_SYMBOL` env var supplied
/// the required symbol (WATCHLIST-INGEST-PLAN-01).
pub const SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL: &str = "env_strategy_symbol";

/// Source label: no symbol source is configured/usable
/// (WATCHLIST-INGEST-PLAN-01).
pub const SYMBOL_SOURCE_NONE: &str = "none";

/// Result of resolving the required symbol/timeframe set from the
/// environment, with explicit provenance (WATCHLIST-INGEST-PLAN-01).
#[derive(Debug, Clone)]
pub struct RequiredSymbolsResolution {
    /// The resolved required symbol/timeframe pairs (unnormalized — callers
    /// that need normalization should still apply
    /// [`normalize_required_symbols`]).
    pub required: Vec<RequiredSymbolTimeframe>,
    /// Which source produced `required`: [`SYMBOL_SOURCE_WATCHLIST_V2`],
    /// [`SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL`], or [`SYMBOL_SOURCE_NONE`].
    pub source: &'static str,
    /// The watchlist intake outcome evaluated during resolution, regardless
    /// of whether it ended up being the active source. Lets callers explain
    /// *why* a configured watchlist was/was not used (e.g.
    /// `Missing`/`Invalid`/`LoadedNotApproved`/v1) without re-evaluating the
    /// artifact themselves.
    pub watchlist_outcome: WatchlistIntakeOutcome,
}

/// Resolve the symbol/timeframe pairs the premarket readiness gate must
/// verify, from the environment, together with explicit source provenance
/// (WATCHLIST-INGEST-PLAN-01).
///
/// This is the same source-preference logic documented on
/// [`required_symbols_for_freshness_gate_from_env`] (that function is now a
/// thin wrapper over this one) — so the premarket readiness gate and any
/// provenance-aware caller (e.g. the ingest-plan surface) can never disagree
/// about which symbols are required or where they came from.
///
/// Thin wrapper over [`required_symbols_with_source`] — reads the watchlist
/// artifact and the legacy `MQK_STRATEGY_SYMBOL`/timeframe env vars exactly
/// once each, then delegates to the pure core. Kept for existing callers
/// that only need the resolved list/provenance, not a caller-supplied
/// already-resolved watchlist outcome.
pub fn required_symbols_with_source_from_env() -> RequiredSymbolsResolution {
    let timeframe = std::env::var(crate::state::STRATEGY_MD_TIMEFRAME_ENV).ok();
    let watchlist_outcome = crate::watchlist_intake::evaluate_watchlist_intake_from_env();
    let legacy_symbol = std::env::var("MQK_STRATEGY_SYMBOL").ok();
    required_symbols_with_source(
        &watchlist_outcome,
        timeframe.as_deref(),
        legacy_symbol.as_deref(),
    )
}

/// BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 1: pure core of
/// [`required_symbols_with_source_from_env`], taking an already-resolved
/// `watchlist_outcome` and the legacy symbol/timeframe inputs instead of
/// reading the watchlist artifact or env itself.
///
/// `StartAttemptAuthoritySnapshot::resolve` (`state/lifecycle.rs`) calls
/// this with the exact same `WatchlistIntakeOutcome` and legacy inputs
/// [`crate::state::multi_symbol_config::MultiSymbolConfigRawInputs`]
/// resolved for [`crate::state::MultiSymbolRuntimeConfig`] construction —
/// so the premarket freshness gate and the multi-symbol dispatch
/// assignment can never disagree about which watchlist file content or
/// legacy env value was in effect for a given start attempt (no second,
/// independently-timed watchlist-artifact read).
pub(crate) fn required_symbols_with_source(
    watchlist_outcome: &WatchlistIntakeOutcome,
    timeframe: Option<&str>,
    legacy_symbol: Option<&str>,
) -> RequiredSymbolsResolution {
    let timeframe = timeframe.map(|v| v.trim().to_string()).unwrap_or_default();

    if timeframe.is_empty() {
        return RequiredSymbolsResolution {
            required: Vec::new(),
            source: SYMBOL_SOURCE_NONE,
            watchlist_outcome: watchlist_outcome.clone(),
        };
    }

    if let WatchlistIntakeOutcome::LoadedApproved { artifact } = watchlist_outcome {
        if artifact.schema_version == crate::watchlist_intake::WATCHLIST_SCHEMA_VERSION_V2
            && !artifact.symbols.is_empty()
        {
            let required = artifact
                .symbols
                .iter()
                .map(|symbol| RequiredSymbolTimeframe {
                    symbol: symbol.clone(),
                    timeframe: timeframe.clone(),
                })
                .collect();
            return RequiredSymbolsResolution {
                required,
                source: SYMBOL_SOURCE_WATCHLIST_V2,
                watchlist_outcome: watchlist_outcome.clone(),
            };
        }
    }

    let symbol = legacy_symbol
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if symbol.is_empty() {
        return RequiredSymbolsResolution {
            required: Vec::new(),
            source: SYMBOL_SOURCE_NONE,
            watchlist_outcome: watchlist_outcome.clone(),
        };
    }

    RequiredSymbolsResolution {
        required: vec![RequiredSymbolTimeframe { symbol, timeframe }],
        source: SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL,
        watchlist_outcome: watchlist_outcome.clone(),
    }
}
