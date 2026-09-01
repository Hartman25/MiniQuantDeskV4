//! PT-AUTO-02: Per-run autonomous signal intake bound.
//! DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: Per-run dedup state for signal-blocked alerts.
//!
//! Extracted from `state.rs` (MT-07A).  Contains the enforcement constant and
//! the accessor methods for the day-signal limit gate and B5/day-limit alert dedup.
//!
//! The backing fields are defined on `AppState`; resets on run-start are in `lifecycle.rs`.

use std::sync::atomic::Ordering;

use super::AppState;

/// PT-AUTO-02: Maximum number of strategy signals accepted per execution run.
///
/// Provides a hard per-run intake bound on the paper+alpaca signal ingestion
/// path.  After this many distinct signals are enqueued (Gate 7 Ok(true)),
/// Gate 1d refuses further signals with 409/day_limit_reached until the next
/// run start resets the counter.
///
/// 100 signals per run is conservative for a supervised paper session.  It is
/// not an economics guarantee — it is a safety bound.
pub(super) const MAX_AUTONOMOUS_SIGNALS_PER_RUN: u32 = 100;

/// MULTI-SYMBOL-DAY-ORDER-CAP-01: Read the optional per-symbol daily order
/// count limit (cap #4, design doc §6) from `MQK_PER_SYMBOL_DAY_ORDER_LIMIT`.
///
/// `None` (unset or unparsable as a non-negative integer) disables Gate 1f
/// entirely — this is the default, matching the `Option<u32> = None` default
/// for cap #4's per-symbol day order count limit in design doc §6.
pub(super) fn per_symbol_day_order_count_limit_from_env() -> Option<u32> {
    std::env::var("MQK_PER_SYMBOL_DAY_ORDER_LIMIT")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// MULTI-SYMBOL-CAPITAL-CAPS-01: Read the optional per-symbol maximum target
/// position quantity (cap #2, design doc §6) from
/// `MQK_PER_SYMBOL_MAX_POSITION_QTY`.
///
/// `None` (unset or not a positive integer) disables the B1C target-qty clamp
/// entirely — this is the default, matching the `Option<i64> = None` default
/// for cap #2's `per_symbol_max_position_qty` in design doc §6.
pub(crate) fn per_symbol_max_position_qty_from_env() -> Option<i64> {
    std::env::var("MQK_PER_SYMBOL_MAX_POSITION_QTY")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
}

/// MULTI-SYMBOL-CAPITAL-CAPS-01: Read the optional per-tick maximum number of
/// newly-accepted decisions (cap #6, design doc §6 "Cap #6 —
/// max_new_orders_per_tick") from `MQK_MAX_NEW_ORDERS_PER_TICK`.
///
/// `None` (unset or unparsable as a non-negative integer) disables cap #6
/// entirely — this is the default, matching cap #6's `Option<u32> = None`
/// default in design doc §6 (unbounded; every configured symbol is
/// dispatched every tick, today's implicit behavior). `0` is a valid,
/// parseable value — it means no new orders are accepted at all this tick.
pub(super) fn max_new_orders_per_tick_from_env() -> Option<u32> {
    std::env::var("MQK_MAX_NEW_ORDERS_PER_TICK")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// MD-STALENESS-PER-TICK-GATE-01: Read the optional per-symbol/per-bar
/// staleness threshold override (cap #9, design doc §6
/// "per_symbol_bar_staleness_guard") from `MQK_PER_SYMBOL_BAR_STALENESS_SECS`.
///
/// `None` (unset or not a non-negative integer) means "use the timeframe-aware
/// documented default" — daily/1D keeps
/// `market_data_freshness::MD_FRESHNESS_STALE_SECS` (4 trading days), while
/// intraday uses `MQK_INTRADAY_BAR_MAX_AGE_SECS` or its fail-closed default.
/// Unlike caps #2/#4/#6, the per-tick staleness gate is always-on: there is no
/// env value that disables it. For intraday, this legacy override can only
/// tighten the intraday cap; it cannot widen intraday back to the daily cap.
pub(super) fn per_symbol_bar_staleness_secs_from_env() -> Option<i64> {
    std::env::var("MQK_PER_SYMBOL_BAR_STALENESS_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n >= 0)
}

/// Normalize a symbol for use as a `day_signal_count_by_symbol` key
/// (trimmed, uppercased) so "aapl", "AAPL", and " AAPL " share one counter.
fn normalize_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

impl AppState {
    /// Returns the current per-run signal intake count.
    pub fn day_signal_count(&self) -> u32 {
        self.day_signal_count.load(Ordering::SeqCst)
    }

    /// Increment the per-run signal intake counter by one.
    ///
    /// Called from the strategy signal route on Gate 7 Ok(true) (new enqueue).
    /// Not called for duplicates (Ok(false)) or Gate failures.
    pub(crate) fn increment_day_signal_count(&self) {
        self.day_signal_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Returns `true` when the per-run signal count has reached
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`.  Gate 1d refuses signals when true.
    pub fn day_signal_limit_exceeded(&self) -> bool {
        self.day_signal_count.load(Ordering::SeqCst) >= MAX_AUTONOMOUS_SIGNALS_PER_RUN
    }

    /// Test seam: set the day signal count to an arbitrary value.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    /// Used by PT-AUTO-02 proof tests to simulate a saturated counter without
    /// submitting 100 real signals.
    pub fn set_day_signal_count_for_test(&self, count: u32) {
        self.day_signal_count.store(count, Ordering::SeqCst);
    }

    // -----------------------------------------------------------------------
    // MULTI-SYMBOL-DAY-ORDER-CAP-01: Gate 1f per-symbol order count cap
    // -----------------------------------------------------------------------

    /// Returns the current per-run order intake count for `symbol`
    /// (cap #4 counter, keyed by normalized symbol).
    pub async fn symbol_day_order_count(&self, symbol: &str) -> u32 {
        let key = normalize_symbol_key(symbol);
        self.day_signal_count_by_symbol
            .read()
            .await
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    /// Increment the per-run order intake counter for `symbol` by one.
    ///
    /// Called alongside [`increment_day_signal_count`](Self::increment_day_signal_count)
    /// on every new outbox enqueue (Gate 7 Ok(true)). Not called for
    /// duplicates or gate failures.
    pub(crate) async fn increment_symbol_day_order_count(&self, symbol: &str) {
        let key = normalize_symbol_key(symbol);
        let mut map = self.day_signal_count_by_symbol.write().await;
        *map.entry(key).or_insert(0) += 1;
    }

    /// Returns the configured per-symbol daily order count limit (cap #4),
    /// or `None` if Gate 1f is disabled.
    pub async fn per_symbol_day_order_limit(&self) -> Option<u32> {
        *self.per_symbol_day_order_limit.read().await
    }

    /// Returns `true` when `symbol`'s per-run order count has reached the
    /// configured [`per_symbol_day_order_limit`](Self::per_symbol_day_order_limit).
    ///
    /// Always `false` when Gate 1f is disabled (limit `None`) — the default.
    pub async fn symbol_day_order_limit_exceeded(&self, symbol: &str) -> bool {
        match self.per_symbol_day_order_limit().await {
            Some(limit) => self.symbol_day_order_count(symbol).await >= limit,
            None => false,
        }
    }

    /// Test seam: set the per-run order intake count for `symbol` to an
    /// arbitrary value.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    /// Used by Gate 1f proof tests to simulate a saturated per-symbol counter
    /// without driving real outbox enqueues.
    pub fn set_symbol_day_order_count_for_test(&self, symbol: &str, count: u32) {
        let key = normalize_symbol_key(symbol);
        if let Ok(mut map) = self.day_signal_count_by_symbol.try_write() {
            map.insert(key, count);
        }
    }

    /// Test seam: override the configured per-symbol daily order count limit
    /// (cap #4), bypassing `MQK_PER_SYMBOL_DAY_ORDER_LIMIT`.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub fn set_per_symbol_day_order_limit_for_test(&self, limit: Option<u32>) {
        if let Ok(mut guard) = self.per_symbol_day_order_limit.try_write() {
            *guard = limit;
        }
    }

    /// Reset all per-symbol order intake counts. Called at run start
    /// alongside `day_signal_count` so cap #4 applies per execution run, not
    /// per daemon process lifetime (same day-rollover boundary as Gate 1).
    ///
    /// `pub` (not `pub(super)`) so D07 in
    /// `scenario_multi_symbol_day_order_cap_01.rs` can exercise the reset
    /// directly without driving a full run-start lifecycle transition.
    pub async fn reset_symbol_day_order_counts(&self) {
        self.day_signal_count_by_symbol.write().await.clear();
    }

    // -----------------------------------------------------------------------
    // MULTI-SYMBOL-CAPITAL-CAPS-01: Gate-less cap #2 target-qty clamp config
    // -----------------------------------------------------------------------

    /// Returns the configured per-symbol maximum target position quantity
    /// (cap #2), or `None` if the B1C target-qty clamp is disabled.
    pub async fn per_symbol_max_position_qty(&self) -> Option<i64> {
        *self.per_symbol_max_position_qty.read().await
    }

    /// Test seam: override the configured per-symbol max position qty cap
    /// (cap #2), bypassing `MQK_PER_SYMBOL_MAX_POSITION_QTY`.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub fn set_per_symbol_max_position_qty_for_test(&self, cap: Option<i64>) {
        if let Ok(mut guard) = self.per_symbol_max_position_qty.try_write() {
            *guard = cap;
        }
    }

    // -----------------------------------------------------------------------
    // MULTI-SYMBOL-CAPITAL-CAPS-01: cap #6 per-tick new-order cap config
    // -----------------------------------------------------------------------

    /// Returns the configured per-tick maximum new-order count (cap #6), or
    /// `None` if the cap is disabled (unbounded — the default).
    pub async fn max_new_orders_per_tick(&self) -> Option<u32> {
        *self.max_new_orders_per_tick.read().await
    }

    /// Test seam: override the configured per-tick new-order cap (cap #6),
    /// bypassing `MQK_MAX_NEW_ORDERS_PER_TICK`.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub fn set_max_new_orders_per_tick_for_test(&self, cap: Option<u32>) {
        if let Ok(mut guard) = self.max_new_orders_per_tick.try_write() {
            *guard = cap;
        }
    }

    // -----------------------------------------------------------------------
    // MD-STALENESS-PER-TICK-GATE-01: cap #9 per-tick bar staleness gate config
    // -----------------------------------------------------------------------

    /// Returns the legacy daily effective per-symbol bar staleness threshold
    /// (cap #9, in seconds).
    ///
    /// New dispatch code should call
    /// [`Self::per_symbol_bar_staleness_secs_for_timeframe`] so intraday bars
    /// use the tighter max-age cap.
    pub async fn per_symbol_bar_staleness_secs(&self) -> Option<i64> {
        self.per_symbol_bar_staleness_secs_for_timeframe("1D").await
    }

    /// Returns the effective per-symbol bar staleness threshold (cap #9, in
    /// seconds) used by `dispatch_native_strategy_for_symbol_with_bar` to
    /// fail-closed-block strategy dispatch for a symbol/tick whose latest
    /// completed bar is stale or missing.
    ///
    /// Daily/1D keeps the existing 4-day tolerance. Intraday timeframes use
    /// `MQK_INTRADAY_BAR_MAX_AGE_SECS` or the safe default from
    /// `market_data_freshness`, with any legacy
    /// `MQK_PER_SYMBOL_BAR_STALENESS_SECS` value applied only if it is tighter.
    /// Always `Some` — this gate cannot be disabled via env var.
    pub async fn per_symbol_bar_staleness_secs_for_timeframe(
        &self,
        timeframe: &str,
    ) -> Option<i64> {
        let timeframe_default =
            crate::market_data_freshness::default_max_allowed_age_secs_for_timeframe(timeframe);
        let configured = *self.per_symbol_bar_staleness_secs.read().await;
        let effective = match configured {
            Some(cap) if crate::market_data_freshness::is_intraday_timeframe(timeframe) => {
                cap.min(timeframe_default)
            }
            Some(cap) => cap,
            None => timeframe_default,
        };
        Some(effective)
    }

    /// Test seam: override the effective per-symbol bar staleness threshold
    /// (cap #9), bypassing `MQK_PER_SYMBOL_BAR_STALENESS_SECS` and the
    /// timeframe-aware defaults. `None` restores the default.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub fn set_per_symbol_bar_staleness_secs_for_test(&self, cap: Option<i64>) {
        if let Ok(mut guard) = self.per_symbol_bar_staleness_secs.try_write() {
            *guard = cap;
        }
    }

    // -----------------------------------------------------------------------
    // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: dedup helpers
    // -----------------------------------------------------------------------

    /// Returns `true` the FIRST time this symbol is claimed for a B5 alert this
    /// run; `false` on subsequent calls (already alerted).
    ///
    /// Thread-safe: uses an async RwLock acquire + insert.  Called from the
    /// execution loop tick context so the await point is acceptable.
    pub(crate) async fn try_claim_b5_alert(&self, symbol: &str) -> bool {
        let mut set = self.b5_alerted_symbols.write().await;
        set.insert(symbol.to_string())
    }

    /// Returns `true` the FIRST time the day-limit Discord alert is claimed for
    /// the current run; `false` on all subsequent calls.
    ///
    /// Uses an atomic CAS so multiple concurrent signal POSTs that all hit the
    /// limit simultaneously produce at most one Discord alert.
    pub(crate) fn try_claim_day_limit_alert(&self) -> bool {
        self.day_limit_alert_fired
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// MULTI-SYMBOL-CAPITAL-CAPS-01: Returns `true` the FIRST time this symbol
    /// is claimed for a cap #2 (`per_symbol_max_position_qty`) clamp Discord
    /// alert this run; `false` on subsequent calls (already alerted).
    ///
    /// Same shape as [`try_claim_b5_alert`](Self::try_claim_b5_alert): at most
    /// one alert per (run, symbol), to avoid alert storms when a strategy
    /// repeatedly targets a qty above the cap.
    pub(crate) async fn try_claim_per_symbol_position_cap_alert(&self, symbol: &str) -> bool {
        let mut set = self.per_symbol_position_cap_alerted_symbols.write().await;
        set.insert(symbol.to_string())
    }

    /// Reset B5 dedup set, day-limit alert flag, and cap #2 position-cap
    /// alert dedup set.  Called at run start so each new run gets fresh dedup
    /// state.
    pub(super) fn reset_signal_blocked_alert_state(&self) {
        // Clear the B5 symbol set synchronously using try_write; always succeeds
        // because lifecycle.rs holds the exclusive lifecycle_op lock during start.
        if let Ok(mut set) = self.b5_alerted_symbols.try_write() {
            set.clear();
        }
        self.day_limit_alert_fired.store(false, Ordering::SeqCst);
        if let Ok(mut set) = self.per_symbol_position_cap_alerted_symbols.try_write() {
            set.clear();
        }
    }
}
