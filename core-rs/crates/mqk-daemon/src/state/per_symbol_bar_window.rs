//! PER-SYMBOL-BAR-WINDOW-01: per-symbol bar input/window foundation.
//!
//! Additive, symbol-keyed counterparts to the single-symbol
//! [`super::StrategyBarInput`] / `pending_strategy_bar_input` /
//! `fetch_recent_completed_bars_for_strategy` path, prepared for
//! `MULTI-SYMBOL-DISPATCH-LOOP-01` (Patch 4).
//!
//! # NOT WIRED — this patch only
//! Nothing here is read by `loop_runner.rs`, the live execution loop, or
//! `POST /api/v1/strategy/signal`. `AppState::pending_strategy_bar_input`
//! and `AppState::tick_strategy_dispatch` remain the only seams the runtime
//! loop calls. [`PerSymbolPendingBarInputs`] is a standalone in-memory map;
//! `AppState` does not hold one yet.
//!
//! # Types
//! - [`PerSymbolBarInput`] — symbol-keyed counterpart to `StrategyBarInput`.
//! - [`PerSymbolPendingBarInputs`] — symbol-keyed pending-input map
//!   (insert/take by normalized symbol; taking one symbol never affects
//!   another).
//! - [`PerSymbolBarWindow`] — per-symbol bar-window metadata (counts /
//!   freshness), not the bars themselves.
//! - [`PerSymbolLoadedBars`] — [`PerSymbolBarWindow`] plus the loaded
//!   [`mqk_strategy::BarStub`]s, oldest-first, ready for
//!   `RecentBarsWindow::new`.
//! - [`classify_bar_staleness`] — pure helper for cap #9
//!   (`per_symbol_bar_staleness_secs`).
//!
//! # Cross-references
//! - `docs/design/native_multi_symbol_dispatch.md` §4.4, §6 (cap #9)
//! - `core-rs/crates/mqk-daemon/src/state.rs` (`StrategyBarInput`,
//!   `tick_strategy_dispatch`, `tick_strategy_dispatch_for_symbol`)
//! - `core-rs/crates/mqk-db/src/md.rs`
//!   (`fetch_recent_completed_bars_for_strategy`)

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Symbol-keyed counterpart to [`super::StrategyBarInput`].
///
/// `symbol` is normalized (trimmed + uppercased) by
/// [`PerSymbolPendingBarInputs::insert`] before storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerSymbolBarInput {
    pub symbol: String,
    pub now_tick: u64,
    pub end_ts: i64,
    pub limit_price: Option<i64>,
    pub qty: i64,
}

/// Error returned when a symbol normalizes to the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptySymbolError;

impl std::fmt::Display for EmptySymbolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "symbol must not be empty")
    }
}

impl std::error::Error for EmptySymbolError {}

/// Symbol-keyed pending [`PerSymbolBarInput`] map.
///
/// Pure in-memory map: no DB, no broker, no runtime-loop access. Taking the
/// pending input for one symbol does not remove or modify any other
/// symbol's entry.
#[derive(Debug, Default)]
pub struct PerSymbolPendingBarInputs {
    inner: HashMap<String, PerSymbolBarInput>,
}

impl PerSymbolPendingBarInputs {
    /// Construct an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Normalize a symbol for use as a map key: trim whitespace, uppercase.
    /// No truncation.
    pub fn normalize_symbol(symbol: &str) -> String {
        symbol.trim().to_uppercase()
    }

    /// Insert or replace the pending bar input for `symbol`.
    ///
    /// `symbol` is normalized before being used as the map key; the stored
    /// `input.symbol` is overwritten with the normalized form. Returns
    /// [`EmptySymbolError`] if the normalized symbol is empty — nothing is
    /// inserted in that case.
    pub fn insert(
        &mut self,
        symbol: &str,
        mut input: PerSymbolBarInput,
    ) -> Result<(), EmptySymbolError> {
        let key = Self::normalize_symbol(symbol);
        if key.is_empty() {
            return Err(EmptySymbolError);
        }
        input.symbol = key.clone();
        self.inner.insert(key, input);
        Ok(())
    }

    /// Remove and return the pending bar input for `symbol`, if present.
    ///
    /// Normalizes `symbol` before lookup. Taking one symbol's entry does not
    /// remove or modify any other symbol's entry.
    pub fn take(&mut self, symbol: &str) -> Option<PerSymbolBarInput> {
        let key = Self::normalize_symbol(symbol);
        self.inner.remove(&key)
    }

    /// Number of pending entries currently held.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True if no symbol has a pending entry.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Per-symbol bar-window metadata: counts and freshness, not the bars
/// themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerSymbolBarWindow {
    pub symbol: String,
    pub timeframe: String,
    pub bars_loaded: usize,
    pub latest_end_ts: Option<i64>,
    pub latest_end_utc: Option<String>,
    pub is_stale: Option<bool>,
}

/// [`PerSymbolBarWindow`] metadata plus the loaded
/// [`mqk_strategy::BarStub`]s, oldest-first, ready for
/// `RecentBarsWindow::new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerSymbolLoadedBars {
    pub window: PerSymbolBarWindow,
    pub bars: Vec<mqk_strategy::BarStub>,
}

/// Error returned by [`load_recent_completed_bars_for_symbol_window`].
#[derive(Debug)]
pub enum PerSymbolBarWindowError {
    EmptySymbol,
    EmptyTimeframe,
    Db(anyhow::Error),
}

impl std::fmt::Display for PerSymbolBarWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySymbol => write!(f, "symbol must not be empty"),
            Self::EmptyTimeframe => write!(f, "timeframe must not be empty"),
            Self::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl std::error::Error for PerSymbolBarWindowError {}

/// Convert a unix-seconds `end_ts` to an RFC3339 UTC timestamp string.
///
/// Mirrors the conversion idiom in
/// `mqk-daemon/src/market_data_freshness.rs`.
fn end_ts_to_rfc3339(end_ts: i64) -> Option<String> {
    DateTime::from_timestamp(end_ts, 0).map(|dt: DateTime<Utc>| dt.to_rfc3339())
}

/// Pure conversion: build [`PerSymbolLoadedBars`] from already-fetched
/// [`mqk_db::MdBarRow`]s, assumed oldest-first.
///
/// Exposed separately from [`load_recent_completed_bars_for_symbol_window`]
/// so the "missing bars are represented honestly, not fabricated" and
/// "window metadata / ordering preserved" behaviors can be proven without a
/// DB connection. An empty `rows` slice produces `bars_loaded == 0`,
/// `latest_end_ts == None`, `latest_end_utc == None`, and an empty `bars`
/// vec — nothing is fabricated.
pub fn per_symbol_loaded_bars_from_rows(
    symbol: &str,
    timeframe: &str,
    rows: &[mqk_db::MdBarRow],
) -> PerSymbolLoadedBars {
    let bars_loaded = rows.len();
    let latest_end_ts = rows.last().map(|r| r.end_ts);
    let latest_end_utc = latest_end_ts.and_then(end_ts_to_rfc3339);

    let bars: Vec<mqk_strategy::BarStub> = rows
        .iter()
        .map(|r| {
            mqk_strategy::BarStub::with_ohlcv(
                r.end_ts,
                r.is_complete,
                r.open_micros,
                r.high_micros,
                r.low_micros,
                r.close_micros,
                r.volume,
            )
        })
        .collect();

    PerSymbolLoadedBars {
        window: PerSymbolBarWindow {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            bars_loaded,
            latest_end_ts,
            latest_end_utc,
            is_stale: None,
        },
        bars,
    }
}

/// Load the most recent completed bars for `symbol`/`timeframe` and build
/// [`PerSymbolLoadedBars`] (window metadata + [`mqk_strategy::BarStub`]s).
///
/// - Requires non-empty `symbol` and `timeframe` (trimmed).
/// - Delegates to [`mqk_db::fetch_recent_completed_bars_for_strategy`], which
///   already returns rows oldest-first (ascending `end_ts`), then
///   [`per_symbol_loaded_bars_from_rows`] for the conversion.
/// - Does not fabricate bars: if no completed bars exist,
///   `window.bars_loaded == 0`, `window.latest_end_ts == None`, and `bars` is
///   empty.
/// - Read-only: issues no writes, no broker/provider calls, no order
///   activity. `window.is_stale` is always `None` here — staleness is
///   classified separately via [`classify_bar_staleness`].
pub async fn load_recent_completed_bars_for_symbol_window(
    db: &PgPool,
    symbol: &str,
    timeframe: &str,
    limit: i64,
) -> Result<PerSymbolLoadedBars, PerSymbolBarWindowError> {
    let symbol = symbol.trim();
    let timeframe = timeframe.trim();
    if symbol.is_empty() {
        return Err(PerSymbolBarWindowError::EmptySymbol);
    }
    if timeframe.is_empty() {
        return Err(PerSymbolBarWindowError::EmptyTimeframe);
    }

    let rows = mqk_db::fetch_recent_completed_bars_for_strategy(db, symbol, timeframe, limit)
        .await
        .map_err(PerSymbolBarWindowError::Db)?;

    Ok(per_symbol_loaded_bars_from_rows(symbol, timeframe, &rows))
}

/// Classify bar staleness for the future cap #9
/// (`MultiSymbolRiskCaps.per_symbol_bar_staleness_secs`).
///
/// - `max_staleness_secs == None` => `None` (cap disabled; no opinion).
/// - `latest_end_ts == None` with `Some(cap)` => `Some(true)` (no bars at all
///   is stale once a cap is configured).
/// - `latest_end_ts` more than `cap` seconds before `now_ts` => `Some(true)`.
/// - `latest_end_ts` within `cap` seconds of `now_ts` => `Some(false)`.
/// - A future `latest_end_ts` (greater than `now_ts`) clamps the computed
///   age to zero — never panics, never reports stale solely due to clock
///   skew.
///
pub fn classify_bar_staleness(
    latest_end_ts: Option<i64>,
    now_ts: i64,
    max_staleness_secs: Option<i64>,
) -> Option<bool> {
    let cap = max_staleness_secs?;
    let Some(end_ts) = latest_end_ts else {
        return Some(true);
    };
    let age_secs = (now_ts - end_ts).max(0);
    Some(age_secs > cap)
}
