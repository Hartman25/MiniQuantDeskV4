use std::collections::BTreeMap;

use mqk_integrity::CalendarSpec;
use mqk_portfolio::Fill;
use uuid::Uuid;

use crate::corporate_actions::CorporateActionPolicy;
use crate::economics::{BacktestEconomicsReport, BacktestInstrumentEconomics};

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

/// Namespace for MQK backtest input-data identity hashes.
/// Bytes: "mqk_bkt_input_ns" (ASCII, 16 bytes exactly).
const BACKTEST_INPUT_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x71, 0x6b, 0x5f, 0x62, 0x6b, 0x74, 0x5f, 0x69, 0x6e, 0x70, 0x75, 0x74, 0x5f, 0x6e, 0x73,
]);

// ---------------------------------------------------------------------------
// BacktestFill — Fill with per-fill provenance
// ---------------------------------------------------------------------------

/// A single fill produced by the backtest engine, with full provenance.
///
/// Extends [`mqk_portfolio::Fill`] with four provenance fields:
///
/// - `fill_id`: deterministic UUIDv5 — unique per fill, stable across replays
/// - `order_id`: deterministic UUIDv5 — identifies the originating order intent,
///   stable across replays (same bar + symbol + side + intent position → same ID)
/// - `signal_ts`: epoch seconds of the bar on which the strategy decision that
///   produced this order was made (order-creation time, copied from the
///   originating [`BacktestOrder::signal_ts`])
/// - `fill_ts`: epoch seconds of the bar whose market data actually priced
///   this fill (execution time)
///
/// # BKT-FUTURE-EXECUTION-01 — causal execution
///
/// For every ordinary strategy-intent-driven fill, `fill_ts > signal_ts`
/// strictly: the fill is always priced from the first later bar for the
/// order's own `symbol`, never from the bar the signal was generated on.
/// Flatten-all fills (forced risk flatten) are the one deliberate exception —
/// they remain immediate/same-bar (`fill_ts == signal_ts`), see
/// [`BacktestEngine::flatten_all`](crate::engine::BacktestEngine).
///
/// Implements `Deref<Target = Fill>` so all `Fill` field accesses
/// (`symbol`, `side`, `qty`, `price_micros`, `fee_micros`) work transparently
/// on `BacktestFill` values without any code changes in existing call sites.
///
/// # ID generation
///
/// ```text
/// order_id = UUIDv5(BACKTEST_ORDER_NS, "{signal_ts}:{symbol}:{side_char}:{intent_seq}")
/// fill_id  = UUIDv5(BACKTEST_FILL_NS,  order_id.as_bytes())
/// ```
///
/// For flatten-all fills (risk halt / drawdown flatten):
/// ```text
/// order_id = UUIDv5(BACKTEST_ORDER_NS, "flatten:{signal_ts}:{symbol}:{symbol_seq}")
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
    /// Bar end timestamp (epoch seconds) at which the strategy decision that
    /// produced this fill's order was made. Matches the originating
    /// [`BacktestOrder::signal_ts`].
    pub signal_ts: i64,
    /// Bar end timestamp (epoch seconds) of the market data that actually
    /// priced this fill. For ordinary strategy orders, `fill_ts > signal_ts`.
    pub fill_ts: i64,
    /// The underlying fill record used for portfolio accounting.
    pub inner: Fill,
}

impl BacktestFill {
    /// Build a deterministic order ID for a strategy-intent-driven fill.
    ///
    /// `signal_ts` is the bar timestamp at which the order was generated
    /// (decision time), not the (possibly later) fill time.  `intent_seq` is
    /// the 0-based position of this intent among all intents produced for
    /// the signal bar.
    pub fn make_order_id(signal_ts: i64, symbol: &str, is_buy: bool, intent_seq: usize) -> Uuid {
        let side_char = if is_buy { 'B' } else { 'S' };
        let name = format!("{}:{}:{}:{}", signal_ts, symbol, side_char, intent_seq);
        Uuid::new_v5(&BACKTEST_ORDER_NS, name.as_bytes())
    }

    /// Build a deterministic order ID for a flatten-all fill (risk halt).
    ///
    /// `symbol_seq` is the 0-based position of this symbol in the sorted
    /// flatten iteration (BTreeMap order is alphabetical, hence deterministic).
    pub fn make_flatten_order_id(signal_ts: i64, symbol: &str, symbol_seq: usize) -> Uuid {
        let name = format!("flatten:{}:{}:{}", signal_ts, symbol, symbol_seq);
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
    /// A later eligible bar for the order's own symbol was found; a
    /// corresponding fill was produced.
    Filled,
    /// Risk rejected the order at signal time (or the allocation cap
    /// rejected it once the future fill price became known); no fill was
    /// produced.
    Rejected,
    /// This order triggered (or was caught by) a risk halt. No fill for
    /// the intent itself, but a flatten-all sequence may follow.
    HaltTriggered,
    /// BKT-FUTURE-EXECUTION-01: the order was admitted at signal time but no
    /// later eligible bar for its symbol ever arrived before the dataset
    /// was exhausted. No fill was, or ever will be, produced.
    ///
    /// BKT-FUTURE-EXECUTION-01-REPAIR-01: this status is reserved for a run
    /// that actually reached the end of its input data with the order still
    /// pending. A run that instead stopped early because of a risk halt
    /// reports [`OrderStatus::CanceledOnHalt`] for the same still-pending
    /// order instead -- the data that could have filled it may well still
    /// exist, it was simply never reached, which is a materially different
    /// (and more truthful) reason than "the dataset ran out."
    UnfilledEndOfData,
    /// BKT-FUTURE-EXECUTION-01-REPAIR-01: the order was admitted at signal
    /// time but the run halted (see [`BacktestReport::halted`]) before any
    /// later eligible bar for its symbol was reached. Distinct from
    /// [`OrderStatus::UnfilledEndOfData`]: the run terminated early, it did
    /// not exhaust its input data with the order still outstanding.
    CanceledOnHalt,
}

/// An order intent record produced by the backtest engine.
///
/// BKT-04P: emitted for every intent (strategy-driven or flatten-all),
/// whether risk allowed, rejected, or left permanently unfilled. Enables a
/// complete audit trail of what the strategy wanted vs. what was actually
/// executed.
///
/// `order_id` is the same deterministic UUIDv5 used in `BacktestFill.order_id`
/// for fills that correspond to this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestOrder {
    /// Deterministic per-order UUID (same namespace as `BacktestFill.order_id`).
    pub order_id: Uuid,
    /// Bar end timestamp (epoch seconds) at which the strategy decision that
    /// produced this order was made. This is decision/order-creation time,
    /// not fill time — see `BacktestFill::fill_ts` for when (and whether)
    /// this order actually filled.
    pub signal_ts: i64,
    /// Symbol this order targets. Only market data for this exact symbol may
    /// price a fill for this order.
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
// StrategySizingConfig
// ---------------------------------------------------------------------------

/// Strategy position-sizing parameters captured in backtest config identity.
///
/// Two backtests with different sizing settings produce different `config_id`
/// values, closing the reproducibility gap where `MQK_STRATEGY_*` env vars
/// could silently change behavior without affecting artifact identity.
///
/// # Defaults
///
/// `target_qty = 1`, `max_target_qty = None`, `max_position_notional_usd = None`.
/// These match the live-strategy conservative defaults so a zero-config backtest
/// is directly comparable to default live behavior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategySizingConfig {
    /// Absolute target share count on a bullish signal (must be ≥ 1).
    /// Default: 1 (conservative single-share sizing).
    pub target_qty: i64,

    /// Hard cap on target share count (None = no share cap).
    pub max_target_qty: Option<i64>,

    /// Hard cap on position notional in whole USD (None = no notional cap).
    pub max_position_notional_usd: Option<i64>,
}

impl StrategySizingConfig {
    /// Default conservative sizing: 1 share, no caps.
    pub const fn default_sizing() -> Self {
        Self {
            target_qty: 1,
            max_target_qty: None,
            max_position_notional_usd: None,
        }
    }

    /// Canonical string used as part of `BacktestConfig::config_id()`.
    pub fn canonical_str(&self) -> String {
        format!(
            "sz_tgt={sz_tgt}|sz_max_tgt={sz_max_tgt}|sz_max_notional={sz_max_notional}",
            sz_tgt = self.target_qty,
            sz_max_tgt = self
                .max_target_qty
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
            sz_max_notional = self
                .max_position_notional_usd
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
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

    // --- BACKTEST-CONFIG-DETERMINISM-SIZING-01: strategy sizing ---
    /// Strategy position-sizing configuration.
    ///
    /// Captured in `config_id()` so two backtests with different sizing settings
    /// produce different identity hashes.  Defaults to `target_qty=1` with no caps,
    /// which matches live-strategy conservative defaults.
    pub sizing: StrategySizingConfig,
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
            // BACKTEST-CONFIG-DETERMINISM-SIZING-01: default 1 share, no caps
            sizing: StrategySizingConfig::default_sizing(),
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
            // BACKTEST-CONFIG-DETERMINISM-SIZING-01: default 1 share, no caps
            sizing: StrategySizingConfig::default_sizing(),
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
        let sizing_str = self.sizing.canonical_str();
        let canonical = format!(
            "v2|ts={ts}|hist={hist}|cash={cash}|shadow={shadow}|dll={dll}|mdd={mdd}|\
             rs={rs}|pdt={pdt}|ks={ks}|exp={exp}|slip={slip}|vol={vol}|\
             comm_ps={comm_ps}|comm_bps={comm_bps}|\
             int={int}|stale={stale}|gap={gap}|disagree={disagree}|cal={cal}|{ca}|{sz}",
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
            sz = sizing_str,
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

// ---------------------------------------------------------------------------
// Deterministic run-identity namespace
// ---------------------------------------------------------------------------

/// Namespace for MQK backtest run IDs.
/// Bytes: "mqk_bkt_run__ns0" (ASCII, padded to 16 bytes).
const BACKTEST_RUN_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x71, 0x6b, 0x5f, 0x62, 0x6b, 0x74, 0x5f, 0x72, 0x75, 0x6e, 0x5f, 0x5f, 0x6e, 0x73, 0x30,
]);

/// Derive a deterministic hash over the input bar sequence.
///
/// BKT-PROV-01: closes the input-data identity gap by producing a UUIDv5 that
/// encodes every field of every bar in the sequence.  Two runs with different bar
/// data (different prices, timestamps, symbols, volumes, or completeness flags)
/// produce different hashes; identical bar sequences always produce the same hash.
///
/// The canonical form is `"mqk-bkt.input.v1|{bar0}|{bar1}|..."` where each bar
/// is rendered as `"{symbol}:{end_ts}:{open}:{high}:{low}:{close}:{volume}:{is_complete}:{day_id}:{reject_window_id}"`.
/// The `v1|` prefix allows future schema bumps without ambiguity.
///
/// Returns the hyphenated UUID string (e.g. `"xxxxxxxx-xxxx-5xxx-yxxx-xxxxxxxxxxxx"`).
/// For empty bar sequences the hash is the UUIDv5 of `"mqk-bkt.input.v1|"` (stable,
/// non-nil — empty input is a valid deterministic identity).
pub fn derive_input_data_hash(bars: &[BacktestBar]) -> String {
    let parts: Vec<String> = bars
        .iter()
        .map(|b| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
                b.symbol,
                b.end_ts,
                b.open_micros,
                b.high_micros,
                b.low_micros,
                b.close_micros,
                b.volume,
                b.is_complete as u8,
                b.day_id,
                b.reject_window_id,
            )
        })
        .collect();
    let versioned = format!("mqk-bkt.input.v1|{}", parts.join("|"));
    Uuid::new_v5(&BACKTEST_INPUT_NS, versioned.as_bytes()).to_string()
}

/// Derive a deterministic backtest run ID.
///
/// BKT-PROV-01: run identity is a UUIDv5 over
/// `"mqk-bkt.run.v2|{strategy_name}|{config_id}|{input_data_hash}"`.
/// All three inputs must be provided:
/// - `strategy_name`: from [`StrategySpec::name`] (empty string if no strategy registered)
/// - `config_id`: UUIDv5 over all [`BacktestConfig`] parameters
/// - `input_data_hash`: from [`derive_input_data_hash`] — encodes the full bar sequence
///
/// This ID is unique per (strategy × full config × input bar data).  Replays over
/// identical inputs always produce the same `run_id`.  Any change to strategy name,
/// any config parameter, or any bar field produces a different `run_id`.
///
/// # Format note
///
/// The `v2` prefix distinguishes IDs produced after BKT-PROV-01 (which incorporates
/// bar data) from pre-BKT-PROV-01 `v1` IDs (strategy + config only).  Old and new
/// IDs are mutually incomparable by construction.
pub fn derive_run_id(strategy_name: &str, config_id: &Uuid, input_data_hash: &str) -> Uuid {
    let data = format!(
        "mqk-bkt.run.v2|{}|{}|{}",
        strategy_name, config_id, input_data_hash
    );
    Uuid::new_v5(&BACKTEST_RUN_NS, data.as_bytes())
}

/// Derive a deterministic backtest run ID, folding in instrument economics.
///
/// BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: extends [`derive_run_id`] so that two
/// runs with identical strategy/config/input-data identity but different
/// [`BacktestInstrumentEconomics`] (multiplier or margin) cannot collide on
/// `run_id`.
///
/// When `economics.is_default_equity()` is true (multiplier=1, no margin
/// scaffold), this returns **exactly** the same UUID as [`derive_run_id`] --
/// every pre-existing equity backtest's `run_id` is unchanged. Any other
/// economics value is hashed under a distinct `v3` prefix, which can never
/// collide with a `v2` (legacy/equity) digest because the version prefix
/// itself differs.
pub fn derive_run_id_with_economics(
    strategy_name: &str,
    config_id: &Uuid,
    input_data_hash: &str,
    economics: &BacktestInstrumentEconomics,
) -> Uuid {
    if economics.is_default_equity() {
        return derive_run_id(strategy_name, config_id, input_data_hash);
    }
    let data = format!(
        "mqk-bkt.run.v3|{}|{}|{}|mult={}|im={}|mm={}",
        strategy_name,
        config_id,
        input_data_hash,
        economics.contract_multiplier,
        economics
            .initial_margin_micros
            .map(|v| v.to_string())
            .unwrap_or_else(|| "none".to_string()),
        economics
            .maintenance_margin_micros
            .map(|v| v.to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
    Uuid::new_v5(&BACKTEST_RUN_NS, data.as_bytes())
}

/// BKT-FUTURE-EXECUTION-01-REPAIR-01 (Blocker 2): identifies the
/// execution/replay semantic used to turn admitted signals into fills.
///
/// Folded into `run_id` via [`derive_run_id_with_execution_model`] so that
/// two engines with different fill semantics (e.g. same-bar vs.
/// future-target-symbol-bar) can never collide on identity for otherwise
/// identical strategy/config/input-data/economics -- see that function's
/// doc comment for why `derive_run_id`/`derive_run_id_with_economics` alone
/// cannot guarantee that.
///
/// `BacktestEngine::run` (BKT-FUTURE-EXECUTION-01: pending order -> first
/// later target-symbol bar) always uses exactly this value today; there is
/// no operator-selectable legacy same-bar mode.
pub const BACKTEST_EXECUTION_MODEL_ID: &str = "future_target_symbol_bar_v1";

/// Derive a deterministic backtest run ID, folding in both instrument
/// economics and the execution-model semantic identity.
///
/// BKT-FUTURE-EXECUTION-01-REPAIR-01 (Blocker 2): [`derive_run_id_with_economics`]
/// folds in strategy, config, input data, and economics -- but not *how*
/// admitted signals were turned into fills. BKT-FUTURE-EXECUTION-01 changed
/// that meaning materially (same-bar fills -> future-target-symbol-bar
/// fills, admission-time cap check -> fill-time cap check) without changing
/// any of `derive_run_id_with_economics`'s inputs, so a run under the old
/// semantics and a run under the new semantics could otherwise collide on
/// `run_id` for identical strategy/config/input/economics even though their
/// fills, equity, and P&L differ. This function closes that gap:
/// `execution_model_id` (e.g. [`BACKTEST_EXECUTION_MODEL_ID`]) is hashed in
/// explicitly, under a new `v4` prefix that can never collide with a
/// `v2`/`v3` (execution-model-unaware) digest, since the version prefix
/// itself always differs.
///
/// Economics folding mirrors [`derive_run_id_with_economics`]'s collapsing
/// rule: `economics.is_default_equity()` always hashes to the same
/// `"equity"` token regardless of which explicit (multiplier=1, no margin)
/// value produced it, so a default-equity run and an explicit-equity run
/// remain identity-equivalent under this function too. Any other economics
/// value is hashed by its exact fields, so non-default economics still
/// produces a distinct `run_id`. Changing `execution_model_id` alone (all
/// other inputs held fixed) always changes the resulting `run_id`, since it
/// is hashed in as an explicit, distinguishing component of the canonical
/// string.
pub fn derive_run_id_with_execution_model(
    strategy_name: &str,
    config_id: &Uuid,
    input_data_hash: &str,
    economics: &BacktestInstrumentEconomics,
    execution_model_id: &str,
) -> Uuid {
    let economics_token = if economics.is_default_equity() {
        "equity".to_string()
    } else {
        format!(
            "mult={}|im={}|mm={}",
            economics.contract_multiplier,
            economics
                .initial_margin_micros
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
            economics
                .maintenance_margin_micros
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
    };
    let data = format!(
        "mqk-bkt.run.v4|{}|{}|{}|{}|exec={}",
        strategy_name, config_id, input_data_hash, economics_token, execution_model_id
    );
    Uuid::new_v5(&BACKTEST_RUN_NS, data.as_bytes())
}

/// Derive a deterministic backtest run ID, folding in the strategy's semantic
/// fingerprint alongside config, input data, economics, and execution model.
///
/// BACKTEST-STRATEGY-SEMANTIC-RUN-IDENTITY-01: [`derive_run_id_with_execution_model`]
/// folds in strategy_name, config, input data, economics, and execution
/// model -- but never [`crate::types::BacktestReport::strategy_semantic_fingerprint`]
/// (i.e. `Strategy::semantic_fingerprint()`). Two strategy instances can
/// share a `strategy_name` and `config_id` while differing in semantic
/// implementation (thresholds, internal version, any parameter not captured
/// by `BacktestConfig`), so without this, two materially different strategy
/// semantics could collide on `run_id`. This function closes that gap:
/// `strategy_semantic_fingerprint` is hashed in explicitly, under a new `v5`
/// prefix that can never collide with a `v2`/`v3`/`v4` (semantic-unaware)
/// digest, since the version prefix itself always differs.
///
/// Result metrics (P&L, order count, fills, equity, etc.) never participate
/// in this derivation -- only inputs that are fully known before the run
/// executes.
///
/// `BacktestEngine::run` always uses exactly this function today. Historical
/// `v2`/`v3`/`v4` artifacts remain readable and are never rewritten or
/// backfilled to `v5`.
pub fn derive_run_id_with_semantic_identity(
    strategy_name: &str,
    config_id: &Uuid,
    input_data_hash: &str,
    economics: &BacktestInstrumentEconomics,
    execution_model_id: &str,
    strategy_semantic_fingerprint: &str,
) -> Uuid {
    let economics_token = if economics.is_default_equity() {
        "equity".to_string()
    } else {
        format!(
            "mult={}|im={}|mm={}",
            economics.contract_multiplier,
            economics
                .initial_margin_micros
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
            economics
                .maintenance_margin_micros
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
    };
    let data = format!(
        "mqk-bkt.run.v5|{}|{}|{}|{}|exec={}|sem={}",
        strategy_name,
        config_id,
        input_data_hash,
        economics_token,
        execution_model_id,
        strategy_semantic_fingerprint
    );
    Uuid::new_v5(&BACKTEST_RUN_NS, data.as_bytes())
}

// ---------------------------------------------------------------------------
// BacktestReport
// ---------------------------------------------------------------------------

/// Backtest report produced after a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestReport {
    /// BKT-05P: Name of the strategy that drove this run.
    /// Populated from `StrategySpec::name`; empty string if no strategy was registered.
    pub strategy_name: String,
    /// PROMOTION-EVIDENCE-SEMANTIC-BINDING-01: `Strategy::semantic_fingerprint()`
    /// of the EXACT boxed instance that ran this backtest, captured directly
    /// from the engine's own `StrategyHost` at report-build time -- never
    /// reconstructed afterward from `strategy_name`/config/env. Empty string
    /// if no strategy was registered (mirrors `strategy_name`'s own default).
    /// This is the only point in the entire Backtest/Research evidence chain
    /// where a real, instantiated `Strategy` object's semantic identity is
    /// captured; promotion binds fresh evidence to this value, never to
    /// `strategy_name`/`config_id` alone (see
    /// `mqk-daemon::backtest_evidence_gate`).
    pub strategy_semantic_fingerprint: String,
    /// BKT-PROV-01: Deterministic run identity UUID.
    ///
    /// Derived via `derive_run_id_with_semantic_identity(strategy_name, config_id,
    /// input_data_hash, economics, execution_model_id, strategy_semantic_fingerprint)`.
    /// Encodes strategy name, every config parameter, the full input bar
    /// sequence, economics, execution model, and the strategy's semantic
    /// fingerprint. Stable across replays that use identical inputs. Any
    /// change to bar data, config, strategy name, economics, execution
    /// model, or strategy semantic fingerprint produces a different `run_id`.
    pub run_id: Uuid,
    /// Deterministic config identity UUID (UUIDv5 over canonical config string).
    /// Suitable as the `config_hash` in artifact manifests.
    pub config_id: Uuid,
    /// BKT-PROV-01: Deterministic input-data identity hash.
    ///
    /// A UUID-formatted string derived by [`derive_input_data_hash`] over the full bar
    /// sequence passed to [`BacktestEngine::run`].  Different bar sequences produce
    /// different hashes; identical sequences always produce the same hash.
    ///
    /// Consumers can use this to verify that two artifacts were produced from the same
    /// underlying market data, independent of strategy or config identity.
    pub input_data_hash: String,
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
    /// BKT-01P: each fill carries `fill_id`, `order_id`, `signal_ts`, and `fill_ts`.
    /// Implements `Deref<Target = Fill>` for transparent field access.
    pub fills: Vec<BacktestFill>,
    /// Last known price per symbol.
    pub last_prices: BTreeMap<String, i64>,
    /// PATCH 22: Whether integrity disarmed (stale feed / gap blocked execution).
    pub execution_blocked: bool,
    /// Open price of the first complete bar processed (micros). None if no bars ran.
    /// Used by the artifact writer to compute buy-and-hold benchmark return.
    pub first_bar_open_micros: Option<i64>,
    /// Close price of the last complete bar processed (micros). None if no bars ran.
    /// Used by the artifact writer to compute buy-and-hold benchmark return.
    pub last_bar_close_micros: Option<i64>,
    /// BACKTEST-CONFIG-DETERMINISM-SIZING-01: resolved sizing config for this run.
    ///
    /// Copied from `BacktestConfig.sizing` at run time. Carried in the report so
    /// artifact writers can surface sizing values in metrics.json and report.md
    /// without needing access to the original config struct.
    pub sizing: StrategySizingConfig,
    /// BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: truthful instrument economics
    /// for this run (multiplier, margin scaffold, multiplier-aware realized P&L).
    ///
    /// Defaults to [`BacktestEconomicsReport::equity`] for every backtest that
    /// never calls `BacktestEngine::with_economics` -- multiplier=1, no margin,
    /// `margin_enforced=false`, identical to today's implicit equity behavior.
    pub economics: BacktestEconomicsReport,
    /// BKT-FUTURE-EXECUTION-01-REPAIR-01 (Blocker 2): the execution/replay
    /// semantic that actually produced this run's fills -- see
    /// [`BACKTEST_EXECUTION_MODEL_ID`]. Folded into `run_id` via
    /// [`derive_run_id_with_execution_model`] so artifacts can state
    /// truthfully what execution semantics generated them, and so this
    /// value's identity contribution is independently verifiable from the
    /// report alone.
    pub execution_model_id: String,
}

impl BacktestReport {
    /// Minimal, explicit test fixture **for unit tests only**.
    ///
    /// # ⚠ Not for real evaluation
    ///
    /// Mirrors [`BacktestConfig::test_defaults`]: every field is a zero/empty
    /// placeholder so call sites can override only the fields a given test
    /// cares about via functional-update syntax (`..BacktestReport::test_fixture()`).
    /// `BacktestEngine::run` never calls this — production reports are always
    /// built field-by-field from real run state (see `engine.rs`).
    pub fn test_fixture() -> Self {
        Self {
            strategy_name: String::new(),
            strategy_semantic_fingerprint: String::new(),
            run_id: Uuid::nil(),
            config_id: Uuid::nil(),
            input_data_hash: String::new(),
            halted: false,
            halt_reason: None,
            equity_curve: Vec::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            last_prices: BTreeMap::new(),
            execution_blocked: false,
            first_bar_open_micros: None,
            last_bar_close_micros: None,
            sizing: StrategySizingConfig::default_sizing(),
            economics: BacktestEconomicsReport::equity(),
            execution_model_id: String::new(),
        }
    }
}
