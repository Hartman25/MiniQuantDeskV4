use std::collections::BTreeMap;

use mqk_integrity::CalendarSpec;
use mqk_portfolio::Fill;

use crate::corporate_actions::CorporateActionPolicy;

/// Stress profile for conservative fill pricing.
///
/// # Slippage model (Patch B5 — Slippage Realism v1)
///
/// Effective slippage per fill:
/// ```text
/// bar_spread_bps         = (high - low) * 10_000 / close   (volatility proxy)
/// vol_component          = bar_spread_bps * volatility_mult_bps / 10_000
/// effective_slippage_bps = slippage_bps + vol_component
/// ```
/// - `slippage_bps` is a deterministic minimum floor (calibrated or stress-tested).
/// - `volatility_mult_bps` scales slippage with actual bar volatility so that
///   wide-spread (volatile) bars incur more slippage than narrow ones.
///   A value of `0` disables the volatility component (pre-B5 behavior).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StressProfile {
    /// Flat slippage floor in basis points (1 bps = 0.01%).
    /// Applied to fill prices: BUY fills at higher price, SELL fills at lower price.
    /// Default 0 = no flat slippage.
    pub slippage_bps: i64,

    /// Patch B5 — fraction of the bar's price spread added as extra slippage, in bps.
    ///
    /// `10_000` = 100% of the spread; `5_000` = 50%; `0` = disabled.
    /// Wide-spread bars automatically incur more slippage, making the model
    /// conservative for volatile market conditions.
    pub volatility_mult_bps: i64,
}

/// Backtest configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestConfig {
    /// Bar timeframe in seconds (must match strategy spec).
    pub timeframe_secs: i64,

    /// Maximum number of recent bars to keep in the strategy context window.
    pub bar_history_len: usize,

    /// Initial cash balance in micros.
    pub initial_cash_micros: i64,

    /// Shadow mode: if true, strategy runs but trades are not executed.
    pub shadow_mode: bool,

    // --- Risk parameters ---
    /// Daily loss limit in micros (0 = disabled).
    pub daily_loss_limit_micros: i64,

    /// Max drawdown limit in micros (0 = disabled).
    pub max_drawdown_limit_micros: i64,

    /// Max rejects in a window before halt.
    pub reject_storm_max_rejects: u32,

    /// PDT auto-enforcement enabled.
    pub pdt_enabled: bool,

    /// Kill switch type for missing protective stop.
    pub kill_switch_flattens: bool,

    /// Max gross exposure multiplier vs equity, in micros (1.0 => 1_000_000).
    /// Used by PATCH 13 engine isolation allocation caps.
    pub max_gross_exposure_mult_micros: i64,

    /// Stress profile for conservative fill pricing.
    pub stress: StressProfile,

    // --- PATCH 22: Integrity gate ---
    /// If true, enable integrity checks per bar (stale/gap/disagreement).
    /// When integrity disarms or halts, execution is blocked.
    pub integrity_enabled: bool,

    /// Stale data threshold in ticks (bar count). 0 = disabled.
    /// When now_tick - last_feed_tick > this, integrity DISARMS.
    pub integrity_stale_threshold_ticks: u64,

    /// Number of missing bars tolerated before integrity halts (0 = fail on any gap).
    pub integrity_gap_tolerance_bars: u32,

    /// If true, enforce feed disagreement detection in integrity engine.
    pub integrity_enforce_feed_disagreement: bool,

    /// Patch B3 — trading session calendar for session-aware gap detection.
    /// Defaults to `AlwaysOn` (preserves pre-B3 behavior).
    pub integrity_calendar: CalendarSpec,

    /// Patch B4 — corporate action policy.
    ///
    /// Enforces an explicit choice: either the caller guarantees adjusted data
    /// (`Allow`) or declares which (symbol, period) pairs are forbidden
    /// (`ForbidPeriods`). Defaults to `Allow` for backward compatibility.
    pub corporate_action_policy: CorporateActionPolicy,
}

impl BacktestConfig {
    /// Reasonable defaults for testing.
    pub fn test_defaults() -> Self {
        Self {
            timeframe_secs: 60,
            bar_history_len: 50,
            initial_cash_micros: 100_000_000_000, // 100k USD
            shadow_mode: false,
            daily_loss_limit_micros: 0,
            max_drawdown_limit_micros: 0,
            reject_storm_max_rejects: 100,
            pdt_enabled: false,
            kill_switch_flattens: true,
            max_gross_exposure_mult_micros: 1_000_000, // 1.0x equity
            stress: StressProfile {
                slippage_bps: 0,
                volatility_mult_bps: 0,
            },
            // PATCH 22: integrity off by default (backwards compat)
            integrity_enabled: false,
            integrity_stale_threshold_ticks: 0,
            integrity_gap_tolerance_bars: 0,
            integrity_enforce_feed_disagreement: false,
            // Patch B3: AlwaysOn preserves pre-B3 behavior
            integrity_calendar: CalendarSpec::AlwaysOn,
            // Patch B4: Allow preserves pre-B4 behavior
            corporate_action_policy: CorporateActionPolicy::Allow,
        }
    }
}

/// A single bar in the backtest input sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestBar {
    pub symbol: String,
    /// Bar end timestamp (epoch seconds).
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    /// If false, the bar is incomplete and must be rejected.
    pub is_complete: bool,
    /// Deterministic trading day identifier (e.g. YYYYMMDD).
    pub day_id: u32,
    /// Deterministic reject window identifier (e.g. minute bucket).
    pub reject_window_id: u32,
}

impl BacktestBar {
    pub fn new(
        symbol: impl Into<String>,
        end_ts: i64,
        open_micros: i64,
        high_micros: i64,
        low_micros: i64,
        close_micros: i64,
        volume: i64,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            end_ts,
            open_micros,
            high_micros,
            low_micros,
            close_micros,
            volume,
            is_complete: true,
            day_id: 20250101,
            reject_window_id: 0,
        }
    }
}

/// Backtest report produced after a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestReport {
    /// Whether the backtest halted early.
    pub halted: bool,
    /// Reason for halt (if any).
    pub halt_reason: Option<String>,
    /// Equity curve: (end_ts, equity_micros) pairs.
    pub equity_curve: Vec<(i64, i64)>,
    /// All fills executed during the backtest.
    pub fills: Vec<Fill>,
    /// Last known price per symbol.
    pub last_prices: BTreeMap<String, i64>,
    /// PATCH 22: Whether integrity disarmed (stale feed / gap blocked execution).
    pub execution_blocked: bool,
}
