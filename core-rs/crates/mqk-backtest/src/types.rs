use std::collections::BTreeMap;

use mqk_integrity::CalendarSpec;
use mqk_portfolio::Fill;
use uuid::Uuid;

use crate::corporate_actions::CorporateActionPolicy;

// ---------------------------------------------------------------------------
// Deterministic UUID namespaces (fixed constants — never change post-release)
// ---------------------------------------------------------------------------

/// Namespace for MQK backtest order IDs.
/// Bytes: "mqk_bkt_order_ns" (ASCII, padded to 16 bytes).
const BACKTEST_ORDER_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x71, 0x6b, 0x5f, 0x62, 0x6b, 0x74, 0x5f, 0x6f, 0x72, 0x64, 0x65, 0x72, 0x5f, 0x6e, 0x73,
]);

/// Namespace for MQK backtest fill IDs.
/// Bytes: "mqk_bkt_fill__ns" (ASCII, padded to 16 bytes).
const BACKTEST_FILL_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x71, 0x6b, 0x5f, 0x62, 0x6b, 0x74, 0x5f, 0x66, 0x69, 0x6c, 0x6c, 0x5f, 0x5f, 0x6e, 0x73,
]);

/// Namespace for MQK backtest config identity hashes.
/// Bytes: "mqk_bkt_cfg__ns0" (ASCII, padded to 16 bytes).
const BACKTEST_CONFIG_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x71, 0x6b, 0x5f, 0x62, 0x6b, 0x74, 0x5f, 0x63, 0x66, 0x67, 0x5f, 0x5f, 0x6e, 0x73, 0x30,
]);

// ---------------------------------------------------------------------------
// BacktestFill — Fill with per-fill provenance
// ---------------------------------------------------------------------------

/// A single fill produced by the backtest engine, with full provenance.
///
/// Extends [`mqk_portfolio::Fill`] with three provenance fields:
///
/// - `fill_id`: deterministic UUIDv5 — unique per fill, stable across replays
/// - `order_id`: deterministic UUIDv5 — identifies the originating order intent,
///   stable across replays (same bar + symbol + side + intent position → same ID)
/// - `bar_end_ts`: epoch seconds of the bar whose close triggered this fill
///
/// Implements `Deref<Target = Fill>` so all `Fill` field accesses
/// (`symbol`, `side`, `qty`, `price_micros`, `fee_micros`) work transparently
/// on `BacktestFill` values without any code changes in existing call sites.
///
/// # ID generation
///
/// ```text
/// order_id = UUIDv5(BACKTEST_ORDER_NS, "{bar_end_ts}:{symbol}:{side_char}:{intent_seq}")
/// fill_id  = UUIDv5(BACKTEST_FILL_NS,  order_id.as_bytes())
/// ```
///
/// For flatten-all fills (risk halt / drawdown flatten):
/// ```text
/// order_id = UUIDv5(BACKTEST_ORDER_NS, "flatten:{bar_end_ts}:{symbol}:{symbol_seq}")
/// fill_id  = UUIDv5(BACKTEST_FILL_NS,  order_id.as_bytes())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestFill {
    /// Deterministic per-fill UUID. Unique across fills in a run; reproducible
    /// across identical replays.
    pub fill_id: Uuid,
    /// Deterministic per-order UUID. Ties this fill back to the originating
    /// order intent (bar position + symbol + side + intent index).
    pub order_id: Uuid,
    /// Bar end timestamp (epoch seconds) at which this fill was triggered.
    pub bar_end_ts: i64,
    /// The underlying fill record used for portfolio accounting.
    pub inner: Fill,
}

impl BacktestFill {
    /// Build a deterministic order ID for a strategy-intent-driven fill.
    ///
    /// `intent_seq` is the 0-based position of this intent among all intents
    /// produced for the current bar.
    pub fn make_order_id(bar_end_ts: i64, symbol: &str, is_buy: bool, intent_seq: usize) -> Uuid {
        let side_char = if is_buy { 'B' } else { 'S' };
        let name = format!("{}:{}:{}:{}", bar_end_ts, symbol, side_char, intent_seq);
        Uuid::new_v5(&BACKTEST_ORDER_NS, name.as_bytes())
    }

    /// Build a deterministic order ID for a flatten-all fill (risk halt).
    ///
    /// `symbol_seq` is the 0-based position of this symbol in the sorted
    /// flatten iteration (BTreeMap order is alphabetical, hence deterministic).
    pub fn make_flatten_order_id(bar_end_ts: i64, symbol: &str, symbol_seq: usize) -> Uuid {
        let name = format!("flatten:{}:{}:{}", bar_end_ts, symbol, symbol_seq);
        Uuid::new_v5(&BACKTEST_ORDER_NS, name.as_bytes())
    }

    /// Derive a deterministic fill ID from the order ID.
    ///
    /// Since each backtest order results in exactly one simulated fill,
    /// the fill ID is a deterministic function of the order ID.
    pub fn make_fill_id(order_id: &Uuid) -> Uuid {
        Uuid::new_v5(&BACKTEST_FILL_NS, order_id.as_bytes())
    }
}

impl std::ops::Deref for BacktestFill {
    type Target = Fill;
    fn deref(&self) -> &Fill {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// BacktestOrder — order intent record (filled OR rejected)
// ---------------------------------------------------------------------------

/// Side of a backtest order intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BacktestOrderSide {
    Buy,
    Sell,
}

/// Outcome status of a backtest order intent.
///
/// BKT-04P: every order intent is recorded regardless of whether risk allowed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    /// Risk allowed the order; a corresponding fill was produced.
    Filled,
    /// Risk rejected the order; no fill was produced.
    Rejected,
    /// This order triggered (or was caught by) a risk halt. No fill for
    /// the intent itself, but a flatten-all sequence may follow.
    HaltTriggered,
}

/// An order intent record produced by the backtest engine.
///
/// BKT-04P: emitted for every intent (strategy-driven or flatten-all),
/// whether risk allowed or rejected it. Enables a complete audit trail
/// of what the strategy wanted vs. what was actually executed.
///
/// `order_id` is the same deterministic UUIDv5 used in `BacktestFill.order_id`
/// for fills that correspond to this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestOrder {
    /// Deterministic per-order UUID (same namespace as `BacktestFill.order_id`).
    pub order_id: Uuid,
    /// Bar end timestamp (epoch seconds) at which this order was generated.
    pub bar_end_ts: i64,
    /// Symbol this order targets.
    pub symbol: String,
    /// Direction.
    pub side: BacktestOrderSide,
    /// Quantity in shares/units (always positive).
    pub qty: i64,
    /// Outcome status.
    pub status: OrderStatus,
}

// ---------------------------------------------------------------------------
// StressProfile
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// CommissionModel
// ---------------------------------------------------------------------------

/// Per-fill commission model for cost realism.
///
/// # BKT-03P — Commission/fee modeling
///
/// Effective fee per fill:
/// ```text
/// fee = per_share_micros * qty  +  notional * bps_of_notional / 10_000
/// ```
///
/// Both components may be used simultaneously, or only one, or neither.
///
/// - `per_share_micros`: flat per-share (or per-unit) fee in micros.
///   Mimics interactive-brokers-style "per share" rate.
///   `0` = disabled.
///
/// - `bps_of_notional`: fee as basis points of fill notional value.
///   Mimics percentage-of-notional schemes.
///   `0` = disabled.
///
/// The result is a non-negative fee in micros deducted from cash when
/// a fill is applied.  This is intentionally fail-closed: any positive
/// commission reduces equity, making backtest P&L conservative rather
/// than optimistic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommissionModel {
    /// Flat fee per share (unit) in micros.  0 = disabled.
    pub per_share_micros: i64,
    /// Fee as basis points of fill notional value.  0 = disabled.
    pub bps_of_notional: i64,
}

impl CommissionModel {
    /// No commission (zero fees).
    pub const ZERO: Self = Self {
        per_share_micros: 0,
        bps_of_notional: 0,
    };

    /// Compute the fee for a fill.
    ///
    /// `qty` is always positive.  `fill_price_micros` is price per share in micros.
    /// Returns a non-negative fee in micros.
    pub fn compute_fee(&self, qty: i64, fill_price_micros: i64) -> i64 {
        if qty <= 0 {
            return 0;
        }
        let per_share = self.per_share_micros.saturating_mul(qty);
        let notional = (fill_price_micros as i128) * (qty as i128);
        let bps_fee = if self.bps_of_notional > 0 {
            let raw = notional * (self.bps_of_notional as i128) / 10_000i128;
            raw.min(i64::MAX as i128) as i64
        } else {
            0
        };
        per_share.saturating_add(bps_fee).max(0)
    }
}

// ---------------------------------------------------------------------------
// BacktestConfig
// ---------------------------------------------------------------------------

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

    /// BKT-03P — commission/fee model applied at fill time.
    ///
    /// Defaults to `CommissionModel::ZERO` in `test_defaults` (backward-compat).
    /// `conservative_defaults` uses a realistic flat-per-share commission.
    pub commission: CommissionModel,

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
    /// Reasonable defaults **for unit tests only**.
    ///
    /// # ⚠ Not for real evaluation
    ///
    /// This constructor deliberately disables safety features — integrity checks,
    /// risk limits, and slippage — to keep unit-test scenarios predictable and
    /// isolated from system state. It must **never** be used as the default config
    /// for CLI backtests, promotion runs, or any "run in anger" evaluation.
    ///
    /// Use [`BacktestConfig::conservative_defaults`] for real evaluation.
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
            // BKT-03P: zero commission for unit tests (predictable P&L)
            commission: CommissionModel::ZERO,
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

    /// Conservative defaults for real evaluation ("run in anger" mode).
    ///
    /// # PATCH F2 — conservative-first posture
    ///
    /// These defaults are calibrated against `config/defaults/base.yaml` and apply
    /// fail-closed settings for every safety knob. They are suitable as the
    /// starting point for CLI backtests and promotion evaluation when no explicit
    /// override is provided.
    ///
    /// Key differences from [`BacktestConfig::test_defaults`]:
    ///
    /// | Setting                           | `test_defaults` | `conservative_defaults` |
    /// |-----------------------------------|-----------------|-------------------------|
    /// | `integrity_enabled`               | `false`         | `true`                  |
    /// | `integrity_stale_threshold_ticks` | `0` (disabled)  | `120` s                 |
    /// | `integrity_gap_tolerance_bars`    | `0` (any gap halts) | `0` (any gap halts) |
    /// | `integrity_enforce_feed_disagreement` | `false`     | `true`                  |
    /// | `pdt_enabled`                     | `false`         | `true`                  |
    /// | `daily_loss_limit_micros`         | `0` (disabled)  | 2 % of equity           |
    /// | `max_drawdown_limit_micros`       | `0` (disabled)  | 18 % of equity          |
    /// | `reject_storm_max_rejects`        | `100`           | `5`                     |
    /// | `stress.slippage_bps`             | `0`             | `5`                     |
    /// | `stress.volatility_mult_bps`      | `0`             | `5_000` (50 % spread)   |
    /// | `corporate_action_policy`         | `Allow`         | `ForbidPeriods([])`     |
    ///
    /// Stale threshold (120 s) mirrors `runtime.stale_data_threshold_seconds: 120`
    /// in `base.yaml`. Slippage values mirror `execution.base_slippage_bps: 5` and
    /// `execution.volatility_multiplier: 0.5`. Risk limits mirror
    /// `risk.daily_loss_limit: 0.02` and `risk.max_drawdown: 0.18` applied to the
    /// default 100 k initial equity.
    pub fn conservative_defaults() -> Self {
        Self {
            timeframe_secs: 60,
            bar_history_len: 50,
            initial_cash_micros: 100_000_000_000, // 100k USD
            shadow_mode: false,
            // 2 % of 100 k = $2 000 (base.yaml risk.daily_loss_limit: 0.02)
            daily_loss_limit_micros: 2_000_000_000,
            // 18 % of 100 k = $18 000 (base.yaml risk.max_drawdown: 0.18)
            max_drawdown_limit_micros: 18_000_000_000,
            // base.yaml risk.reject_storm.max_rejects: 5
            reject_storm_max_rejects: 5,
            pdt_enabled: true,
            kill_switch_flattens: true,
            // base.yaml risk.max_leverage: 1.0
            max_gross_exposure_mult_micros: 1_000_000,
            // base.yaml execution.base_slippage_bps: 5
            // base.yaml execution.volatility_multiplier: 0.5 → 5_000 bps (50 % of spread)
            stress: StressProfile {
                slippage_bps: 5,
                volatility_mult_bps: 5_000,
            },
            // BKT-03P: $0.005/share flat (IB tiered-1 conservative proxy; 5000 micros)
            commission: CommissionModel {
                per_share_micros: 5_000,
                bps_of_notional: 0,
            },
            // Integrity ON — mirrors runtime.stale_data_threshold_seconds: 120
            integrity_enabled: true,
            integrity_stale_threshold_ticks: 120,
            // base.yaml data.fail_on_gap: true, data.gap_tolerance_bars: 0
            integrity_gap_tolerance_bars: 0,
            // base.yaml data.feed_disagreement_policy: "HALT_NEW"
            integrity_enforce_feed_disagreement: true,
            integrity_calendar: CalendarSpec::AlwaysOn,
            // ForbidPeriods(empty): no active exclusions yet, but the policy is set
            // for the caller to extend with known corporate-action windows.
            corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![]),
        }
    }

    /// Compute a deterministic config identity hash.
    ///
    /// Returns a `Uuid` (UUIDv5) derived from a canonical string of all
    /// `BacktestConfig` fields.  Identical configs produce the same UUID;
    /// any changed field produces a different UUID.
    ///
    /// Suitable as the `config_hash` input for run identity derivation and
    /// artifact manifests.  Call `.to_string()` to get a hex-formatted string.
    ///
    /// # Format stability
    ///
    /// The canonical string is prefixed with `"v1|"` so that any future
    /// schema change can use a different prefix, making old and new hashes
    /// mutually incomparable without ambiguity.
    pub fn config_id(&self) -> Uuid {
        let ca_str = match &self.corporate_action_policy {
            CorporateActionPolicy::Allow => "ca:allow".to_string(),
            CorporateActionPolicy::ForbidPeriods(v) => {
                let entries = v
                    .iter()
                    .map(|e| format!("{}:{}-{}", e.symbol, e.start_ts, e.end_ts))
                    .collect::<Vec<_>>()
                    .join(";");
                format!("ca:forbid:{}", entries)
            }
        };
        // CalendarSpec derives Debug; format! gives stable enum variant names.
        let cal_str = format!("{:?}", self.integrity_calendar);
        let canonical = format!(
            "v1|ts={ts}|hist={hist}|cash={cash}|shadow={shadow}|dll={dll}|mdd={mdd}|\
             rs={rs}|pdt={pdt}|ks={ks}|exp={exp}|slip={slip}|vol={vol}|\
             comm_ps={comm_ps}|comm_bps={comm_bps}|\
             int={int}|stale={stale}|gap={gap}|disagree={disagree}|cal={cal}|{ca}",
            ts = self.timeframe_secs,
            hist = self.bar_history_len,
            cash = self.initial_cash_micros,
            shadow = self.shadow_mode as u8,
            dll = self.daily_loss_limit_micros,
            mdd = self.max_drawdown_limit_micros,
            rs = self.reject_storm_max_rejects,
            pdt = self.pdt_enabled as u8,
            ks = self.kill_switch_flattens as u8,
            exp = self.max_gross_exposure_mult_micros,
            slip = self.stress.slippage_bps,
            vol = self.stress.volatility_mult_bps,
            comm_ps = self.commission.per_share_micros,
            comm_bps = self.commission.bps_of_notional,
            int = self.integrity_enabled as u8,
            stale = self.integrity_stale_threshold_ticks,
            gap = self.integrity_gap_tolerance_bars,
            disagree = self.integrity_enforce_feed_disagreement as u8,
            cal = cal_str,
            ca = ca_str,
        );
        Uuid::new_v5(&BACKTEST_CONFIG_NS, canonical.as_bytes())
    }
}

// ---------------------------------------------------------------------------
// BacktestBar
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// BacktestReport
// ---------------------------------------------------------------------------

/// Backtest report produced after a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestReport {
    /// Whether the backtest halted early.
    pub halted: bool,
    /// Reason for halt (if any).
    pub halt_reason: Option<String>,
    /// Equity curve: (end_ts, equity_micros) pairs.
    pub equity_curve: Vec<(i64, i64)>,
    /// All order intents generated during the backtest (filled AND rejected).
    ///
    /// BKT-04P: one row per intent, regardless of risk outcome.
    /// `order_id` matches `BacktestFill.order_id` for filled orders.
    pub orders: Vec<BacktestOrder>,
    /// All fills executed during the backtest, with per-fill provenance.
    ///
    /// BKT-01P: each fill carries `fill_id`, `order_id`, and `bar_end_ts`.
    /// Implements `Deref<Target = Fill>` for transparent field access.
    pub fills: Vec<BacktestFill>,
    /// Last known price per symbol.
    pub last_prices: BTreeMap<String, i64>,
    /// PATCH 22: Whether integrity disarmed (stale feed / gap blocked execution).
    pub execution_blocked: bool,
}
