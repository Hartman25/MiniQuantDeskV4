use crate::sizing::TargetSizing;
use crate::{PluginRegistry, RegistryError, Strategy};
use mqk_execution::AssetClass;

pub mod absolute_momentum_252;
pub mod close_channel_100_50_trend_v1;
mod daily_math;
pub mod delayed_overnight_gap_reversal_v1;
pub mod dual_sma_50_200_trend;
pub mod halloween_nov_apr;
pub mod intraday_scalper;
pub mod mean_reversion;
mod monthly;
pub mod monthly_10month_trend_timing_v1;
pub mod monthly_12_minus_1_abs_momentum_v1;
pub mod monthly_52week_high_proximity_v1;
pub mod monthly_multihorizon_abs_momentum_consensus_v1;
pub mod near_high_momentum_252_3pct;
pub mod pre_holiday_two_session_long_v1;
pub mod pullback_mean_reversion_20_2;
mod session_calendar;
pub mod swing_momentum;
pub mod trading_range_breakout_50d_hold10;
pub mod trend_filtered_extreme_3d_atr_reversal_v1;
pub mod trend_filtered_rsi5_reversion_v1;
pub mod trend_filtered_zscore20_reversion_v1;
pub mod trend_pullback_5d_4pct_hold5;
pub mod trend_sma50;
pub mod turn_of_month_last1_first3;
pub mod volatility_breakout;
pub mod volatility_contraction_breakout_v1;
mod window;

pub use absolute_momentum_252::AbsoluteMomentum252Strategy;
pub use close_channel_100_50_trend_v1::CloseChannel10050TrendV1Strategy;
pub use delayed_overnight_gap_reversal_v1::DelayedOvernightGapReversalV1Strategy;
pub use dual_sma_50_200_trend::DualSma50200TrendStrategy;
pub use halloween_nov_apr::HalloweenNovAprStrategy;
pub use intraday_scalper::{
    compute_diagnostics as intraday_scalper_compute_diagnostics, IntradayScalperDiagnostics,
    IntradayScalperStrategy,
};
pub use mean_reversion::MeanReversionStrategy;
pub use monthly_10month_trend_timing_v1::Monthly10MonthTrendTimingV1Strategy;
pub use monthly_12_minus_1_abs_momentum_v1::Monthly12Minus1AbsMomentumV1Strategy;
pub use monthly_52week_high_proximity_v1::Monthly52WeekHighProximityV1Strategy;
pub use monthly_multihorizon_abs_momentum_consensus_v1::MonthlyMultihorizonAbsMomentumConsensusV1Strategy;
pub use near_high_momentum_252_3pct::NearHighMomentum2523PctStrategy;
pub use pre_holiday_two_session_long_v1::PreHolidayTwoSessionLongV1Strategy;
pub use pullback_mean_reversion_20_2::PullbackMeanReversion202Strategy;
pub use swing_momentum::SwingMomentumStrategy;
pub use trading_range_breakout_50d_hold10::TradingRangeBreakout50dHold10Strategy;
pub use trend_filtered_extreme_3d_atr_reversal_v1::TrendFilteredExtreme3dAtrReversalV1Strategy;
pub use trend_filtered_rsi5_reversion_v1::TrendFilteredRsi5ReversionV1Strategy;
pub use trend_filtered_zscore20_reversion_v1::TrendFilteredZscore20ReversionV1Strategy;
pub use trend_pullback_5d_4pct_hold5::TrendPullback5d4pctHold5Strategy;
pub use trend_sma50::TrendSma50Strategy;
pub use turn_of_month_last1_first3::TurnOfMonthLast1First3Strategy;
pub use volatility_breakout::VolatilityBreakoutStrategy;
pub use volatility_contraction_breakout_v1::VolatilityContractionBreakoutV1Strategy;

/// IR9: the single authoritative list of every strategy identity
/// [`register_builtin_strategies`] registers, in registration order. Each
/// engine implementation backs one registered strategy *identity*, except
/// `intraday_scalper`, whose short-only variant (`intraday_short_scalper`) is
/// a distinct identity sharing the same implementation, so the identity count
/// is one more than the implementation count. This constant is the only place
/// the membership is stated; do not copy a count into prose. Any bound or guard elsewhere in the
/// workspace that needs to know the size or membership of the built-in
/// strategy universe (e.g. `mqk_portfolio::MAX_STRATEGY_UNIVERSE`) must
/// consume or cross-check against this list, not a hand-maintained count —
/// see `registered_strategy_ids_match_this_constant` below and the
/// cross-crate bound test in `mqk-daemon`. Adding a new registration to
/// [`register_builtin_strategies`] without updating this constant fails
/// that structural test.
pub const REGISTERED_STRATEGY_IDS: &[&str] = &[
    swing_momentum::NAME,
    mean_reversion::NAME,
    volatility_breakout::NAME,
    intraday_scalper::NAME,
    trend_sma50::NAME,
    dual_sma_50_200_trend::NAME,
    pullback_mean_reversion_20_2::NAME,
    absolute_momentum_252::NAME,
    near_high_momentum_252_3pct::NAME,
    trend_pullback_5d_4pct_hold5::NAME,
    turn_of_month_last1_first3::NAME,
    halloween_nov_apr::NAME,
    trading_range_breakout_50d_hold10::NAME,
    monthly_multihorizon_abs_momentum_consensus_v1::NAME,
    trend_filtered_rsi5_reversion_v1::NAME,
    trend_filtered_extreme_3d_atr_reversal_v1::NAME,
    close_channel_100_50_trend_v1::NAME,
    monthly_10month_trend_timing_v1::NAME,
    trend_filtered_zscore20_reversion_v1::NAME,
    volatility_contraction_breakout_v1::NAME,
    monthly_12_minus_1_abs_momentum_v1::NAME,
    delayed_overnight_gap_reversal_v1::NAME,
    monthly_52week_high_proximity_v1::NAME,
    pre_holiday_two_session_long_v1::NAME,
    intraday_scalper::SHORT_NAME,
];

/// Register the built-in deterministic strategy engines.
///
/// Tier A intent:
/// - registry/discovery only
/// - no runtime wiring here
/// - no IO
/// - deterministic factories
pub fn register_builtin_strategies(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
) -> Result<(), RegistryError> {
    let symbol = symbol.into();

    let swing_symbol = symbol.clone();
    registry.register(swing_momentum::meta(), move || {
        Box::new(SwingMomentumStrategy::new(swing_symbol.clone())) as Box<dyn Strategy>
    })?;

    let mr_symbol = symbol.clone();
    registry.register(mean_reversion::meta(), move || {
        Box::new(MeanReversionStrategy::new(mr_symbol.clone())) as Box<dyn Strategy>
    })?;

    let vb_symbol = symbol.clone();
    registry.register(volatility_breakout::meta(), move || {
        Box::new(VolatilityBreakoutStrategy::new(vb_symbol.clone())) as Box<dyn Strategy>
    })?;

    let scalp_symbol = symbol.clone();
    registry.register(intraday_scalper::meta(), move || {
        Box::new(IntradayScalperStrategy::new(scalp_symbol.clone())) as Box<dyn Strategy>
    })?;

    let trend_symbol = symbol.clone();
    registry.register(trend_sma50::meta(), move || {
        Box::new(TrendSma50Strategy::new(trend_symbol.clone())) as Box<dyn Strategy>
    })?;

    let dual_symbol = symbol.clone();
    registry.register(dual_sma_50_200_trend::meta(), move || {
        Box::new(DualSma50200TrendStrategy::new(dual_symbol.clone())) as Box<dyn Strategy>
    })?;

    let pullback_symbol = symbol.clone();
    registry.register(pullback_mean_reversion_20_2::meta(), move || {
        Box::new(PullbackMeanReversion202Strategy::new(
            pullback_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let absmom_symbol = symbol.clone();
    registry.register(absolute_momentum_252::meta(), move || {
        Box::new(AbsoluteMomentum252Strategy::new(absmom_symbol.clone())) as Box<dyn Strategy>
    })?;

    let nearhigh_symbol = symbol.clone();
    registry.register(near_high_momentum_252_3pct::meta(), move || {
        Box::new(NearHighMomentum2523PctStrategy::new(
            nearhigh_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let tpb_symbol = symbol.clone();
    registry.register(trend_pullback_5d_4pct_hold5::meta(), move || {
        Box::new(TrendPullback5d4pctHold5Strategy::new(tpb_symbol.clone())) as Box<dyn Strategy>
    })?;

    let tom_symbol = symbol.clone();
    registry.register(turn_of_month_last1_first3::meta(), move || {
        Box::new(TurnOfMonthLast1First3Strategy::new(tom_symbol.clone())) as Box<dyn Strategy>
    })?;

    let hw_symbol = symbol.clone();
    registry.register(halloween_nov_apr::meta(), move || {
        Box::new(HalloweenNovAprStrategy::new(hw_symbol.clone())) as Box<dyn Strategy>
    })?;

    let trb_symbol = symbol.clone();
    registry.register(trading_range_breakout_50d_hold10::meta(), move || {
        Box::new(TradingRangeBreakout50dHold10Strategy::new(
            trb_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f01_symbol = symbol.clone();
    registry.register(
        monthly_multihorizon_abs_momentum_consensus_v1::meta(),
        move || {
            Box::new(MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new(
                f01_symbol.clone(),
            )) as Box<dyn Strategy>
        },
    )?;

    let f02_symbol = symbol.clone();
    registry.register(trend_filtered_rsi5_reversion_v1::meta(), move || {
        Box::new(TrendFilteredRsi5ReversionV1Strategy::new(
            f02_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f03_symbol = symbol.clone();
    registry.register(
        trend_filtered_extreme_3d_atr_reversal_v1::meta(),
        move || {
            Box::new(TrendFilteredExtreme3dAtrReversalV1Strategy::new(
                f03_symbol.clone(),
            )) as Box<dyn Strategy>
        },
    )?;

    let f04_symbol = symbol.clone();
    registry.register(close_channel_100_50_trend_v1::meta(), move || {
        Box::new(CloseChannel10050TrendV1Strategy::new(f04_symbol.clone())) as Box<dyn Strategy>
    })?;

    let f05_symbol = symbol.clone();
    registry.register(monthly_10month_trend_timing_v1::meta(), move || {
        Box::new(Monthly10MonthTrendTimingV1Strategy::new(f05_symbol.clone())) as Box<dyn Strategy>
    })?;

    let f06_symbol = symbol.clone();
    registry.register(trend_filtered_zscore20_reversion_v1::meta(), move || {
        Box::new(TrendFilteredZscore20ReversionV1Strategy::new(
            f06_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f07_symbol = symbol.clone();
    registry.register(volatility_contraction_breakout_v1::meta(), move || {
        Box::new(VolatilityContractionBreakoutV1Strategy::new(
            f07_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f08_symbol = symbol.clone();
    registry.register(monthly_12_minus_1_abs_momentum_v1::meta(), move || {
        Box::new(Monthly12Minus1AbsMomentumV1Strategy::new(
            f08_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f09_symbol = symbol.clone();
    registry.register(delayed_overnight_gap_reversal_v1::meta(), move || {
        Box::new(DelayedOvernightGapReversalV1Strategy::new(
            f09_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let f10_symbol = symbol.clone();
    registry.register(monthly_52week_high_proximity_v1::meta(), move || {
        Box::new(Monthly52WeekHighProximityV1Strategy::new(
            f10_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    let ext032_symbol = symbol.clone();
    registry.register(pre_holiday_two_session_long_v1::meta(), move || {
        Box::new(PreHolidayTwoSessionLongV1Strategy::new(
            ext032_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    // SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01: register the short-only variant
    // as a distinct strategy identity.  Disabled by default in
    // `sys_strategy_registry`; runtime selects at most one strategy via
    // `MQK_STRATEGY_IDS`.  True concurrent long+short dispatch requires
    // MULTI-STRATEGY-RUNTIME-DISPATCH-01.
    let short_scalp_symbol = symbol;
    registry.register(intraday_scalper::meta_short(), move || {
        Box::new(IntradayScalperStrategy::new_short(
            short_scalp_symbol.clone(),
        )) as Box<dyn Strategy>
    })?;

    Ok(())
}

/// Register built-in strategies with explicit sizing parameters for deterministic backtests.
///
/// BACKTEST-CONFIG-DETERMINISM-SIZING-01: backtest callers must use this variant
/// so the strategy is constructed from `BacktestConfig.sizing` rather than ambient
/// env vars. This ensures `config_id` and strategy behavior are consistent.
///
/// Live/paper runtime bootstrap does not use this: it registers from resolved exact
/// sizing via [`register_builtin_strategies_with_target_sizing`]. The env-constructing
/// [`register_builtin_strategies`] is for registry/discovery, backtest, and
/// validation callers.
pub fn register_builtin_strategies_with_sizing(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_notional_usd: Option<i64>,
) -> Result<(), RegistryError> {
    let symbol = symbol.into();
    let sizing = TargetSizing::equity_whole_units(target_qty, max_target_qty, max_notional_usd)
        .map_err(|e| RegistryError::InvalidSizing(e.to_string()))?;
    // Historical behavior: the short-only identity stays env-sized here.
    register_with_sizing(registry, symbol, sizing, false)
}

/// Register built-in strategies from already-resolved exact [`TargetSizing`].
///
/// Equity registers the full built-in set. Any other supported asset class
/// registers ONLY the sizing-configurable intraday scalper identities: the
/// other engines emit a fixed one-share signal and must never trade a
/// non-Equity instrument, so they are left unregistered
/// (`instantiate` => unknown strategy, fail closed).
pub fn register_builtin_strategies_with_target_sizing(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
    sizing: TargetSizing,
) -> Result<(), RegistryError> {
    register_with_sizing(registry, symbol, sizing, true)
}

fn register_with_sizing(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
    sizing: TargetSizing,
    short_uses_resolved_sizing: bool,
) -> Result<(), RegistryError> {
    let symbol = symbol.into();

    // Non-scalper strategies do not have configurable sizing (fixed one-share
    // signal): Equity only.
    if sizing.asset_class() == AssetClass::Equity {
        let swing_symbol = symbol.clone();
        registry.register(swing_momentum::meta(), move || {
            Box::new(SwingMomentumStrategy::new(swing_symbol.clone())) as Box<dyn Strategy>
        })?;

        let mr_symbol = symbol.clone();
        registry.register(mean_reversion::meta(), move || {
            Box::new(MeanReversionStrategy::new(mr_symbol.clone())) as Box<dyn Strategy>
        })?;

        let vb_symbol = symbol.clone();
        registry.register(volatility_breakout::meta(), move || {
            Box::new(VolatilityBreakoutStrategy::new(vb_symbol.clone())) as Box<dyn Strategy>
        })?;

        let trend_symbol = symbol.clone();
        registry.register(trend_sma50::meta(), move || {
            Box::new(TrendSma50Strategy::new(trend_symbol.clone())) as Box<dyn Strategy>
        })?;

        let dual_symbol = symbol.clone();
        registry.register(dual_sma_50_200_trend::meta(), move || {
            Box::new(DualSma50200TrendStrategy::new(dual_symbol.clone())) as Box<dyn Strategy>
        })?;

        let pullback_symbol = symbol.clone();
        registry.register(pullback_mean_reversion_20_2::meta(), move || {
            Box::new(PullbackMeanReversion202Strategy::new(
                pullback_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let absmom_symbol = symbol.clone();
        registry.register(absolute_momentum_252::meta(), move || {
            Box::new(AbsoluteMomentum252Strategy::new(absmom_symbol.clone())) as Box<dyn Strategy>
        })?;

        let nearhigh_symbol = symbol.clone();
        registry.register(near_high_momentum_252_3pct::meta(), move || {
            Box::new(NearHighMomentum2523PctStrategy::new(
                nearhigh_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let tpb_symbol = symbol.clone();
        registry.register(trend_pullback_5d_4pct_hold5::meta(), move || {
            Box::new(TrendPullback5d4pctHold5Strategy::new(tpb_symbol.clone())) as Box<dyn Strategy>
        })?;

        let tom_symbol = symbol.clone();
        registry.register(turn_of_month_last1_first3::meta(), move || {
            Box::new(TurnOfMonthLast1First3Strategy::new(tom_symbol.clone())) as Box<dyn Strategy>
        })?;

        let hw_symbol = symbol.clone();
        registry.register(halloween_nov_apr::meta(), move || {
            Box::new(HalloweenNovAprStrategy::new(hw_symbol.clone())) as Box<dyn Strategy>
        })?;

        let trb_symbol = symbol.clone();
        registry.register(trading_range_breakout_50d_hold10::meta(), move || {
            Box::new(TradingRangeBreakout50dHold10Strategy::new(
                trb_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f01_symbol = symbol.clone();
        registry.register(
            monthly_multihorizon_abs_momentum_consensus_v1::meta(),
            move || {
                Box::new(MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new(
                    f01_symbol.clone(),
                )) as Box<dyn Strategy>
            },
        )?;

        let f02_symbol = symbol.clone();
        registry.register(trend_filtered_rsi5_reversion_v1::meta(), move || {
            Box::new(TrendFilteredRsi5ReversionV1Strategy::new(
                f02_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f03_symbol = symbol.clone();
        registry.register(
            trend_filtered_extreme_3d_atr_reversal_v1::meta(),
            move || {
                Box::new(TrendFilteredExtreme3dAtrReversalV1Strategy::new(
                    f03_symbol.clone(),
                )) as Box<dyn Strategy>
            },
        )?;

        let f04_symbol = symbol.clone();
        registry.register(close_channel_100_50_trend_v1::meta(), move || {
            Box::new(CloseChannel10050TrendV1Strategy::new(f04_symbol.clone())) as Box<dyn Strategy>
        })?;

        let f05_symbol = symbol.clone();
        registry.register(monthly_10month_trend_timing_v1::meta(), move || {
            Box::new(Monthly10MonthTrendTimingV1Strategy::new(f05_symbol.clone()))
                as Box<dyn Strategy>
        })?;

        let f06_symbol = symbol.clone();
        registry.register(trend_filtered_zscore20_reversion_v1::meta(), move || {
            Box::new(TrendFilteredZscore20ReversionV1Strategy::new(
                f06_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f07_symbol = symbol.clone();
        registry.register(volatility_contraction_breakout_v1::meta(), move || {
            Box::new(VolatilityContractionBreakoutV1Strategy::new(
                f07_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f08_symbol = symbol.clone();
        registry.register(monthly_12_minus_1_abs_momentum_v1::meta(), move || {
            Box::new(Monthly12Minus1AbsMomentumV1Strategy::new(
                f08_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f09_symbol = symbol.clone();
        registry.register(delayed_overnight_gap_reversal_v1::meta(), move || {
            Box::new(DelayedOvernightGapReversalV1Strategy::new(
                f09_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let f10_symbol = symbol.clone();
        registry.register(monthly_52week_high_proximity_v1::meta(), move || {
            Box::new(Monthly52WeekHighProximityV1Strategy::new(
                f10_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;

        let ext032_symbol = symbol.clone();
        registry.register(pre_holiday_two_session_long_v1::meta(), move || {
            Box::new(PreHolidayTwoSessionLongV1Strategy::new(
                ext032_symbol.clone(),
            )) as Box<dyn Strategy>
        })?;
    }

    let scalp_symbol = symbol.clone();
    registry.register(intraday_scalper::meta(), move || {
        Box::new(IntradayScalperStrategy::with_sizing(
            scalp_symbol.clone(),
            sizing,
        )) as Box<dyn Strategy>
    })?;

    // SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01: short-only variant. The i64
    // whole-unit entry point keeps its historical env sizing; the explicit
    // `TargetSizing` entry point sizes it identically to the long identity.
    let short_scalp_symbol = symbol;
    registry.register(intraday_scalper::meta_short(), move || {
        Box::new(if short_uses_resolved_sizing {
            IntradayScalperStrategy::new_short_with_sizing(short_scalp_symbol.clone(), sizing)
        } else {
            IntradayScalperStrategy::new_short(short_scalp_symbol.clone())
        }) as Box<dyn Strategy>
    })?;

    Ok(())
}

#[cfg(test)]
mod registered_strategy_ids_tests {
    use super::*;

    /// IR9: [`REGISTERED_STRATEGY_IDS`] must track [`register_builtin_strategies`]
    /// exactly — same identities, same count, same order. A new registration
    /// added to that function without updating the constant fails this test,
    /// which is the whole point: the bound can never silently drift out from
    /// under the strategies actually registered.
    #[test]
    fn registered_strategy_ids_match_register_builtin_strategies_exactly() {
        let mut registry = PluginRegistry::new();
        register_builtin_strategies(&mut registry, "TEST_SYMBOL".to_string())
            .expect("register_builtin_strategies must succeed for a fresh registry");

        let actual: Vec<&str> = registry.list().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            actual, REGISTERED_STRATEGY_IDS,
            "REGISTERED_STRATEGY_IDS has drifted from register_builtin_strategies' actual \
             registrations -- update the constant to match"
        );
    }

    #[test]
    fn registered_strategy_ids_has_twenty_five_distinct_entries() {
        let mut unique = REGISTERED_STRATEGY_IDS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            REGISTERED_STRATEGY_IDS.len(),
            "every registered strategy identity must be distinct"
        );
        assert_eq!(REGISTERED_STRATEGY_IDS.len(), 25);
    }

    /// IR-2: the production seam `instantiate_verified` must refuse every engine
    /// whose state cannot be reconstructed from the bounded history Paper loads,
    /// while Backtest/Research (`instantiate`) keep using it. Engines whose state
    /// is seeded from the durable held-position record are refused by the same
    /// seam and admitted only through `instantiate_for_identity` (the durable
    /// wrapper). The refused sets are stated explicitly so a classification
    /// change cannot pass silently.
    #[test]
    fn restart_unsafe_engines_are_refused_by_the_verified_production_seam() {
        let mut registry = PluginRegistry::new();
        register_builtin_strategies(&mut registry, "SPY".to_string()).unwrap();
        let not_recoverable = [pullback_mean_reversion_20_2::NAME];
        let durable = [
            trend_filtered_rsi5_reversion_v1::NAME,
            trend_filtered_extreme_3d_atr_reversal_v1::NAME,
            close_channel_100_50_trend_v1::NAME,
            trend_filtered_zscore20_reversion_v1::NAME,
            volatility_contraction_breakout_v1::NAME,
            delayed_overnight_gap_reversal_v1::NAME,
        ];

        let classified = |class: crate::RestartRecovery| -> Vec<&str> {
            registry
                .list()
                .iter()
                .filter(|m| m.restart_recovery == class)
                .map(|m| m.name.as_str())
                .collect()
        };
        assert_eq!(
            classified(crate::RestartRecovery::NotRecoverable),
            not_recoverable,
            "NotRecoverable classification drifted"
        );
        assert_eq!(
            classified(crate::RestartRecovery::DurableStateRequired),
            durable,
            "DurableStateRequired classification drifted"
        );

        for name in not_recoverable {
            assert!(registry.instantiate(name).is_ok(), "research/backtest path");
            let err = registry
                .instantiate_verified(name)
                .err()
                .expect("verified production instantiation must refuse the engine");
            assert!(err.to_string().contains("restart"), "{err}");
            assert!(registry.instantiate_for_identity(name).is_err());
        }
        for name in durable {
            assert!(registry.instantiate(name).is_ok(), "research/backtest path");
            let err = registry
                .instantiate_verified(name)
                .err()
                .expect("the unwrapped production seam must refuse a durable-state engine");
            assert!(err.to_string().to_lowercase().contains("durable"), "{err}");
            assert!(
                registry.instantiate_for_identity(name).is_ok(),
                "{name} must be admitted to the durable wrapper"
            );
        }

        for id in REGISTERED_STRATEGY_IDS
            .iter()
            .filter(|id| !not_recoverable.contains(id) && !durable.contains(id))
        {
            assert!(
                registry.instantiate_verified(id).is_ok(),
                "{id} must stay deployable"
            );
        }
    }

    #[test]
    fn short_scalper_identity_is_present_and_distinct_from_long() {
        assert!(REGISTERED_STRATEGY_IDS.contains(&"intraday_scalper"));
        assert!(REGISTERED_STRATEGY_IDS.contains(&"intraday_short_scalper"));
    }
}

// ---------------------------------------------------------------------------
// STRATEGY-SEMANTIC-IDENTITY-SEAM-01 (S1): registry/host-wide identity checks.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod semantic_identity_tests {
    use super::*;

    /// Every built-in engine explicitly overrides `semantic_fingerprint` —
    /// none of the registered identities silently falls back to the
    /// trait's spec-only default (which would carry forward exactly the
    /// defect S1 exists to fix). Detected by comparing against two
    /// deliberately mismatched-config instances of the SAME registered
    /// name/timeframe: the trait default cannot see the mismatch (it only
    /// sees `spec()`), so if any built-in were still using the default, the
    /// pair below would incorrectly fingerprint identically for at least one
    /// identity that has decision-affecting config beyond name/timeframe
    /// (intraday_scalper / intraday_short_scalper here, since the other three
    /// currently have no per-instance runtime-configurable field beyond
    /// symbol, which this test also covers via a symbol change).
    #[test]
    fn every_registered_builtin_overrides_the_default_fingerprint() {
        for symbol in ["AAPL", "MSFT"] {
            let registry = build_registry_for(symbol);
            for name in REGISTERED_STRATEGY_IDS {
                let instance = registry.instantiate(name).unwrap();
                let other_symbol = if symbol == "AAPL" { "MSFT" } else { "AAPL" };
                let other_registry = build_registry_for(other_symbol);
                let other_instance = other_registry.instantiate(name).unwrap();
                assert_ne!(
                    instance.semantic_fingerprint(),
                    other_instance.semantic_fingerprint(),
                    "identity '{name}': fingerprint did not change when symbol changed \
                     ({symbol} -> {other_symbol}) — suspect it is still using the \
                     spec-only trait default"
                );
            }
        }
        // intraday_scalper/intraday_short_scalper additionally vary target_qty.
        let default_caps = build_registry_for("AAPL");
        let custom_caps = {
            let mut r = PluginRegistry::new();
            register_builtin_strategies_with_sizing(&mut r, "AAPL", 9, Some(2), Some(3000))
                .unwrap();
            r
        };
        let name = "intraday_scalper";
        let a = default_caps.instantiate(name).unwrap();
        let b = custom_caps.instantiate(name).unwrap();
        assert_ne!(
            a.semantic_fingerprint(),
            b.semantic_fingerprint(),
            "identity '{name}': sizing change did not change fingerprint"
        );
    }

    fn build_registry_for(symbol: &str) -> PluginRegistry {
        let mut r = PluginRegistry::new();
        register_builtin_strategies(&mut r, symbol.to_string()).unwrap();
        r
    }

    /// Fixed built-in descriptors (no per-instance runtime config beyond
    /// symbol) are fully deterministic across repeated registry
    /// construction: same symbol in -> same fingerprint out, every time.
    #[test]
    fn fixed_builtin_descriptors_are_deterministic_across_registrations() {
        for name in REGISTERED_STRATEGY_IDS {
            let r1 = build_registry_for("AAPL");
            let r2 = build_registry_for("AAPL");
            let fp1 = r1.instantiate(name).unwrap().semantic_fingerprint();
            let fp2 = r2.instantiate(name).unwrap().semantic_fingerprint();
            assert_eq!(fp1, fp2, "identity '{name}' is not deterministic");
        }
    }

    /// `PluginRegistry::instantiate` produces an instance whose
    /// `semantic_fingerprint()` is exactly what `StrategyHost` observes
    /// after registration — the host neither loses nor reconstructs it.
    #[test]
    fn registry_instance_fingerprint_matches_what_host_observes_after_registration() {
        use crate::{ShadowMode, StrategyHost};

        let registry = build_registry_for("AAPL");
        for name in REGISTERED_STRATEGY_IDS {
            let instance = registry.instantiate(name).unwrap();
            let expected = instance.semantic_fingerprint();

            let mut host = StrategyHost::new(ShadowMode::Off);
            host.register(instance).unwrap();

            assert_eq!(
                host.semantic_fingerprint().unwrap(),
                expected,
                "identity '{name}': host-observed fingerprint diverged from the \
                 registry-instantiated instance's own fingerprint"
            );
        }
    }
}
