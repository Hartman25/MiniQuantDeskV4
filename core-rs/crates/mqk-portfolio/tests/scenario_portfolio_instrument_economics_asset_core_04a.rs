//! Scenario: ASSET-CORE-04A — pure instrument economics model.
//!
//! Proves `value_position_economics` makes the current equity assumption
//! (multiplier=1, single currency, whole shares) an explicit special case of
//! a more general multiplier/currency/quantity-scale-aware single-position
//! valuation, without wiring anything into live/paper trading, broker, OMS,
//! risk, runtime, or DB. This module has zero production callers.
//!
//! # Proof matrix
//!
//! | Test      | What it proves                                                          |
//! |-----------|--------------------------------------------------------------------------|
//! | EQ-01     | Equity constructor is multiplier=1, whole-share quantity scale          |
//! | EQ-02     | 10 shares @ $100, multiplier=1 -> $1,000 notional                       |
//! | SIGN-01   | Short position -> negative notional, positive absolute notional        |
//! | SIGN-02   | Long position -> notional == absolute_notional                         |
//! | FLAT-01   | Zero qty values to zero without requiring a mark                       |
//! | FLAT-02   | Zero qty values to zero even when a mark is supplied                   |
//! | MARK-01   | Missing mark for non-flat long position fails closed                   |
//! | MARK-02   | Missing mark for non-flat short position fails closed                  |
//! | MULT-01   | Zero multiplier fails closed                                            |
//! | MULT-02   | Negative multiplier fails closed                                       |
//! | CUR-01    | Empty quote currency fails closed                                      |
//! | CUR-02    | Empty account currency fails closed                                    |
//! | CUR-03    | Whitespace-only currency fails closed                                  |
//! | FX-01     | quote_currency != account_currency fails closed (no fake conversion)   |
//! | FX-02     | Matching currencies never trigger conversion-unsupported               |
//! | FUT-01    | 1 contract, $5,000, 50x multiplier -> $250,000 notional                 |
//! | FUT-02    | 2 contracts scales linearly                                             |
//! | OPT-01    | 1 contract, $2.50, 100x multiplier -> $250 notional                    |
//! | FRAC-01   | Fractional (0.5 unit) quantity values proportionally, model-only       |
//! | FRAC-02   | Fractional representation carries no enablement signal                 |
//! | UNSUP-01  | Empty asset_class fails closed                                         |
//! | UNSUP-02  | Arbitrary non-empty asset_class is not itself a failure                |
//! | OVF-01    | Extreme magnitudes report Overflow, never panic                        |
//! | OVF-02    | Large-but-representable magnitudes compute without overflow            |
//! | OVF-03    | Extreme short-side magnitudes also report Overflow, never panic        |
//! | DET-01    | Identical input yields identical (`PartialEq`) output                  |
//! | DET-02    | Identical failure input yields identical output                       |
//! | ORDER-01  | Flat position with empty currency still fails closed (no bypass)       |
//! | ORDER-02  | Flat position with invalid multiplier still fails closed (no bypass)   |
//! | REGR-01   | `compute_portfolio_weights` (PORTFOLIO-LIVE-WEIGHTS-01) is untouched    |
//! | REGR-02   | `evaluate_sector_risk` (ETF-RISK-CLOSURE-01) is untouched               |

use std::collections::BTreeMap;

use mqk_portfolio::{
    compute_portfolio_weights, value_position_economics, InstrumentEconomics,
    InstrumentEconomicsTruthState, PositionEconomicsInput, PositionMark, PositionWeightInput,
};

const M: i64 = 1_000_000; // micro-dollar / micro-unit scale factor

fn equity_input(qty_micros: i64, mark: Option<i64>) -> PositionEconomicsInput {
    PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD"),
        signed_qty_micros: qty_micros,
        mark_price_micros: mark,
        account_currency: "USD".to_string(),
    }
}

// ---------------------------------------------------------------------------
// EQ: equity multiplier=1 reproduces plain qty*price
// ---------------------------------------------------------------------------

#[test]
fn eq01_equity_constructor_is_multiplier_one_whole_share_scale() {
    let econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    assert_eq!(econ.contract_multiplier_micros, M);
    assert_eq!(econ.quantity_scale, M);
    assert_eq!(econ.min_trade_qty_micros, None);
    assert_eq!(econ.tick_size_micros, None);
    assert_eq!(econ.asset_class, "equity");
}

#[test]
fn eq02_ten_shares_at_100_dollars_multiplier_one_is_1000_notional() {
    let value = value_position_economics(equity_input(10 * M, Some(100 * M)));
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(1_000 * M as i128));
    assert_eq!(value.absolute_notional_micros, Some(1_000 * M as i128));
    assert_eq!(value.reason_code, "active");
}

// ---------------------------------------------------------------------------
// SIGN: long/short signs preserved, absolute notional always positive
// ---------------------------------------------------------------------------

#[test]
fn sign01_short_position_has_negative_notional_positive_absolute() {
    let value = value_position_economics(equity_input(-10 * M, Some(100 * M)));
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(-1_000 * M as i128));
    assert_eq!(value.absolute_notional_micros, Some(1_000 * M as i128));
}

#[test]
fn sign02_long_position_notional_equals_absolute_notional() {
    let value = value_position_economics(equity_input(10 * M, Some(100 * M)));
    assert_eq!(value.notional_micros, value.absolute_notional_micros);
}

// ---------------------------------------------------------------------------
// FLAT: zero qty values to zero without requiring a mark
// ---------------------------------------------------------------------------

#[test]
fn flat01_zero_qty_with_no_mark_values_to_zero_active() {
    let value = value_position_economics(equity_input(0, None));
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(0));
    assert_eq!(value.absolute_notional_micros, Some(0));
    assert_eq!(value.reason_code, "flat_position");
}

#[test]
fn flat02_zero_qty_with_mark_present_still_values_to_zero() {
    let value = value_position_economics(equity_input(0, Some(9_999 * M)));
    assert_eq!(value.notional_micros, Some(0));
}

// ---------------------------------------------------------------------------
// MARK: missing mark for a non-flat position fails closed
// ---------------------------------------------------------------------------

#[test]
fn mark01_missing_mark_for_non_flat_long_position_fails_closed() {
    let value = value_position_economics(equity_input(10 * M, None));
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingMark
    );
    assert_eq!(value.notional_micros, None);
    assert_eq!(value.absolute_notional_micros, None);
    assert_eq!(value.reason_code, "missing_mark_for_non_flat_position");
}

#[test]
fn mark02_missing_mark_for_non_flat_short_position_fails_closed() {
    let value = value_position_economics(equity_input(-5 * M, None));
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingMark
    );
}

// ---------------------------------------------------------------------------
// MULT: missing/invalid multiplier fails closed
// ---------------------------------------------------------------------------

#[test]
fn mult01_zero_multiplier_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.contract_multiplier_micros = 0;
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingMultiplier
    );
    assert_eq!(value.notional_micros, None);
}

#[test]
fn mult02_negative_multiplier_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.contract_multiplier_micros = -50 * M;
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingMultiplier
    );
}

// ---------------------------------------------------------------------------
// CUR: empty currency fails closed
// ---------------------------------------------------------------------------

#[test]
fn cur01_empty_quote_currency_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.quote_currency = String::new();
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingCurrency
    );
    assert_eq!(value.notional_micros, None);
}

#[test]
fn cur02_empty_account_currency_fails_closed() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD"),
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: String::new(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingCurrency
    );
}

#[test]
fn cur03_whitespace_only_currency_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.quote_currency = "   ".to_string();
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingCurrency
    );
}

// ---------------------------------------------------------------------------
// FX: cross-currency fails closed as conversion-unsupported (no fake FX)
// ---------------------------------------------------------------------------

#[test]
fn fx01_quote_currency_mismatch_fails_closed_as_conversion_unsupported() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:EU:ASML", "ASML", "EUR"),
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(700 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::CurrencyConversionUnsupported
    );
    assert_eq!(value.notional_micros, None);
    assert_eq!(
        value.reason_code,
        "quote_currency_does_not_match_account_currency"
    );
}

#[test]
fn fx02_matching_currency_does_not_trigger_conversion_unsupported() {
    let value = value_position_economics(equity_input(10 * M, Some(100 * M)));
    assert_ne!(
        value.truth_state,
        InstrumentEconomicsTruthState::CurrencyConversionUnsupported
    );
}

// ---------------------------------------------------------------------------
// FUT: futures-style multiplier example
// ---------------------------------------------------------------------------

fn futures_economics() -> InstrumentEconomics {
    InstrumentEconomics {
        instrument_id: "future:CME:ES2026U".to_string(),
        symbol: "ES2026U".to_string(),
        asset_class: "future".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: 50 * M,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: Some(250_000),
    }
}

#[test]
fn fut01_one_contract_5000_price_50x_multiplier_is_250000_notional() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: futures_economics(),
        signed_qty_micros: M,
        mark_price_micros: Some(5_000 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(250_000 * M as i128));
}

#[test]
fn fut02_two_contracts_scales_linearly() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: futures_economics(),
        signed_qty_micros: 2 * M,
        mark_price_micros: Some(4_510 * M),
        account_currency: "USD".to_string(),
    });
    // 2 contracts * 4,510 * 50 = 451,000
    assert_eq!(value.notional_micros, Some(451_000 * M as i128));
}

// ---------------------------------------------------------------------------
// OPT: options-style multiplier example
// ---------------------------------------------------------------------------

#[test]
fn opt01_one_contract_2_50_price_100x_multiplier_is_250_notional() {
    let instrument = InstrumentEconomics {
        instrument_id: "option:US:AAPL20260918C150".to_string(),
        symbol: "AAPL20260918C150".to_string(),
        asset_class: "option".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: 100 * M,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: None,
    };
    let value = value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: M,
        mark_price_micros: Some(2 * M + M / 2), // $2.50
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(250 * M as i128));
}

// ---------------------------------------------------------------------------
// FRAC: fractional/crypto-style quantity is representable, model-only
// ---------------------------------------------------------------------------

fn crypto_economics() -> InstrumentEconomics {
    InstrumentEconomics {
        instrument_id: "crypto:GLOBAL:BTCUSD".to_string(),
        symbol: "BTC/USD".to_string(),
        asset_class: "crypto".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: M,
        quantity_scale: 1,
        min_trade_qty_micros: Some(1),
        tick_size_micros: None,
    }
}

#[test]
fn frac01_half_unit_quantity_values_proportionally() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: crypto_economics(),
        signed_qty_micros: M / 2, // 0.5 BTC
        mark_price_micros: Some(60_000 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    // 0.5 * 60,000 = 30,000
    assert_eq!(value.notional_micros, Some(30_000 * M as i128));
}

#[test]
fn frac02_fractional_quantity_does_not_imply_any_trading_enablement() {
    // This module has no "enabled" concept at all -- it is purely a valuation
    // function. There is no field anywhere on `PositionEconomicsValue` that
    // could be mistaken for a trading-enablement signal.
    let value = value_position_economics(PositionEconomicsInput {
        instrument: crypto_economics(),
        signed_qty_micros: M / 4,
        mark_price_micros: Some(60_000 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.asset_class, "crypto");
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
}

// ---------------------------------------------------------------------------
// UNSUP: unsupported asset class / malformed economics fails closed
// ---------------------------------------------------------------------------

#[test]
fn unsup01_empty_asset_class_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.asset_class = String::new();
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::UnsupportedInstrument
    );
    assert_eq!(value.notional_micros, None);
}

#[test]
fn unsup02_arbitrary_non_empty_asset_class_is_not_itself_a_failure() {
    // Deliberately generic: a non-empty, made-up asset class string is not a
    // failure by itself -- only emptiness is. No asset class is enabled for
    // trading by this function either way; it has no enablement concept.
    let mut econ = InstrumentEconomics::equity("x:1", "X", "USD");
    econ.asset_class = "exotic_derivative".to_string();
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: M,
        mark_price_micros: Some(10 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
}

// ---------------------------------------------------------------------------
// OVF: overflow is reported explicitly -- never panics, never silently wraps
// ---------------------------------------------------------------------------

#[test]
fn ovf01_extreme_magnitudes_report_overflow_not_panic() {
    let instrument = InstrumentEconomics {
        instrument_id: "x:1".to_string(),
        symbol: "X".to_string(),
        asset_class: "equity".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: i64::MAX,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: None,
    };
    let value = value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: i64::MAX,
        mark_price_micros: Some(i64::MAX),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Overflow);
    assert_eq!(value.notional_micros, None);
    assert_eq!(value.absolute_notional_micros, None);
    assert_eq!(value.reason_code, "notional_computation_overflowed_i128");
}

#[test]
fn ovf02_large_but_representable_values_compute_without_overflow() {
    // 1,000,000 contracts at $1,000,000,000 each, multiplier=1: individual
    // magnitudes are large but the i128 product is well within range.
    let instrument = InstrumentEconomics {
        instrument_id: "x:2".to_string(),
        symbol: "X2".to_string(),
        asset_class: "equity".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: M,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: None,
    };
    let value = value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: 1_000_000 * M,
        mark_price_micros: Some(1_000_000_000 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(
        value.notional_micros,
        Some(1_000_000i128 * 1_000_000_000i128 * M as i128)
    );
}

#[test]
fn ovf03_extreme_short_side_magnitude_also_reports_overflow_not_panic() {
    let instrument = InstrumentEconomics {
        instrument_id: "x:3".to_string(),
        symbol: "X3".to_string(),
        asset_class: "equity".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: i64::MAX,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: None,
    };
    let value = value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: i64::MIN,
        mark_price_micros: Some(i64::MAX),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Overflow);
}

// ---------------------------------------------------------------------------
// DET: determinism
// ---------------------------------------------------------------------------

#[test]
fn det01_identical_input_yields_identical_output() {
    let a = value_position_economics(equity_input(10 * M, Some(100 * M)));
    let b = value_position_economics(equity_input(10 * M, Some(100 * M)));
    assert_eq!(a, b);
}

#[test]
fn det02_identical_failure_input_yields_identical_output() {
    let a = value_position_economics(equity_input(10 * M, None));
    let b = value_position_economics(equity_input(10 * M, None));
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// ORDER: structural checks (currency/multiplier) fire even on a flat
// position -- a flat position must not mask malformed instrument metadata.
// ---------------------------------------------------------------------------

#[test]
fn order01_flat_position_with_empty_currency_still_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.quote_currency = String::new();
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 0,
        mark_price_micros: None,
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingCurrency
    );
}

#[test]
fn order02_flat_position_with_invalid_multiplier_still_fails_closed() {
    let mut econ = InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD");
    econ.contract_multiplier_micros = 0;
    let value = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 0,
        mark_price_micros: None,
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::MissingMultiplier
    );
}

// ---------------------------------------------------------------------------
// REGR: existing live-weights / sector-risk seams are untouched by this
// patch. These call the pre-existing, unmodified public API directly (not a
// re-run of their own dedicated scenario files) as an in-file regression
// smoke proof that this patch did not alter their behavior.
// ---------------------------------------------------------------------------

#[test]
fn regr01_compute_portfolio_weights_equity_behavior_is_unchanged() {
    let positions = vec![PositionWeightInput {
        symbol: "AAPL".to_string(),
        signed_qty: 10,
    }];
    let mut marks = BTreeMap::new();
    marks.insert(
        "AAPL".to_string(),
        PositionMark {
            symbol: "AAPL".to_string(),
            mark_price_micros: 100 * M,
            mark_ts_utc: Some(1_700_000_000),
            source: "test:manual".to_string(),
        },
    );
    let snapshot = compute_portfolio_weights(1_000 * M, &positions, &marks);
    assert_eq!(snapshot.truth_state, "active");
    assert_eq!(snapshot.nav_micros, Some(2_000 * M as i128));
}

#[test]
fn regr02_evaluate_sector_risk_disabled_passthrough_is_unchanged() {
    use mqk_portfolio::evaluate_sector_risk;
    use std::collections::HashMap;

    let positions = vec![PositionWeightInput {
        symbol: "SPY".to_string(),
        signed_qty: 0,
    }];
    let marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    let sector_map: HashMap<String, String> = HashMap::new();
    let sector_limits_bps: HashMap<String, i64> = HashMap::new();

    // Empty sector_limits_bps must still passthrough as disabled, untouched
    // by this patch's addition of an unrelated economics model.
    let evaluation = evaluate_sector_risk(
        1_000 * M,
        &positions,
        &marks,
        &sector_map,
        &sector_limits_bps,
        "SPY",
        0,
    );
    assert!(evaluation.allowed);
    assert_eq!(evaluation.truth_state, "sector_risk_disabled");
}
