use std::collections::BTreeMap;

use mqk_portfolio::Fill;

/// Stress profile for conservative fill pricing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StressProfile {
    /// Slippage in basis points (1 bps = 0.01%).
    /// Applied to fill prices: BUY fills at higher price, SELL fills at lower price.
    pub slippage_bps: i64,
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
            stress: StressProfile { slippage_bps: 0 },
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
}
