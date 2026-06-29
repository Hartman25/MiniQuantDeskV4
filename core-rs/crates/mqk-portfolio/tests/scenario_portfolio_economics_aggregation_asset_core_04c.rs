//! Scenario: ASSET-CORE-04C — pure multi-asset portfolio NAV/exposure
//! aggregation model.
//!
//! Proves `aggregate_portfolio_economics` composes already-valued
//! `PositionEconomicsValue` rows (from `ASSET-CORE-04A`'s
//! `value_position_economics`) into a portfolio-level NAV, gross/net
//! exposure, per-position weight, and asset-class/currency exposure
//! breakdown — without wiring anything into live/paper trading, broker,
//! OMS, risk, runtime, or DB. This module has zero production callers.
//!
//! # Proof matrix
//!
//! | Test      | What it proves                                                          |
//! |-----------|--------------------------------------------------------------------------|
//! | EQ-01     | Equity-only: cash + two long equity positions -> NAV/gross/net/weights |
//! | SIGN-01   | Long + short signs preserved in net; gross uses absolute value         |
//! | EMPTY-01  | Empty positions -> deterministic `NoPositions`, NAV == cash            |
//! | EMPTY-02  | Empty positions with non-positive cash -> `NavUnavailable`, not bypassed |
//! | FLAT-01   | Flat position contributes zero and needs no mark                       |
//! | FLAT-02   | Flat position with a mark present still contributes zero               |
//! | UNAVAIL-01| Missing mark on a non-flat position fails the whole snapshot closed    |
//! | NAV-01    | Negative NAV fails closed; gross/net still reported, weights are not   |
//! | NAV-02    | Exact-zero NAV is treated the same as negative (gate is `<=`)          |
//! | FX-01     | Position priced in a foreign currency fails closed, even though it     |
//! |           | individually valued `Active` in its own currency                       |
//! | FX-02     | Empty `account_currency` fails closed before any position is examined |
//! | MIX-01    | Future + option + crypto rows aggregate together, model-only           |
//! | ASSET-01  | Asset-class exposure rows isolate sums per class (equity vs future)    |
//! | CCY-01    | Currency exposure rows sum multiple same-currency rows into one bucket |
//! | UNSUP-01  | Empty `asset_class` position fails closed as value-unavailable         |
//! | UNSUP-02  | Arbitrary non-empty `asset_class` is explicitly surfaced, not refused  |
//! | OVF-01    | Near-`i128::MAX` notionals report `Overflow`, never panic              |
//! | DET-01    | Identical input yields identical output, including row/exposure order |
//! | REGR-01   | `value_position_economics` (`ASSET-CORE-04A`) is untouched              |
//! | REGR-02   | `compute_portfolio_weights` (`PORTFOLIO-LIVE-WEIGHTS-01`) is untouched  |
//! | REGR-03   | `evaluate_sector_risk` (`ETF-RISK-CLOSURE-01`) is untouched             |

use std::collections::BTreeMap;

use mqk_portfolio::{
    aggregate_portfolio_economics, compute_portfolio_weights, evaluate_sector_risk,
    value_position_economics, InstrumentEconomics, InstrumentEconomicsTruthState,
    PortfolioEconomicsInput, PortfolioEconomicsTruthState, PositionEconomicsInput,
    PositionEconomicsValue, PositionMark, PositionWeightInput,
};

const M: i64 = 1_000_000; // micro-dollar / micro-unit scale factor

fn equity_value(
    symbol: &str,
    qty_micros: i64,
    mark: Option<i64>,
    currency: &str,
) -> PositionEconomicsValue {
    value_position_economics(PositionEconomicsInput {
        instrument: InstrumentEconomics::equity(format!("equity:US:{symbol}"), symbol, currency),
        signed_qty_micros: qty_micros,
        mark_price_micros: mark,
        account_currency: currency.to_string(),
    })
}

fn future_value(
    symbol: &str,
    qty_micros: i64,
    mark: i64,
    multiplier_x: i64,
) -> PositionEconomicsValue {
    let instrument = InstrumentEconomics {
        instrument_id: format!("future:CME:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "future".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: multiplier_x * M,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: Some(250_000),
    };
    value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: qty_micros,
        mark_price_micros: Some(mark),
        account_currency: "USD".to_string(),
    })
}

fn option_value(
    symbol: &str,
    qty_micros: i64,
    mark: i64,
    multiplier_x: i64,
) -> PositionEconomicsValue {
    let instrument = InstrumentEconomics {
        instrument_id: format!("option:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "option".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: multiplier_x * M,
        quantity_scale: M,
        min_trade_qty_micros: None,
        tick_size_micros: None,
    };
    value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: qty_micros,
        mark_price_micros: Some(mark),
        account_currency: "USD".to_string(),
    })
}

fn crypto_value(symbol: &str, qty_micros: i64, mark: i64) -> PositionEconomicsValue {
    let instrument = InstrumentEconomics {
        instrument_id: format!("crypto:GLOBAL:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "crypto".to_string(),
        quote_currency: "USD".to_string(),
        contract_multiplier_micros: M,
        quantity_scale: 1,
        min_trade_qty_micros: Some(1),
        tick_size_micros: None,
    };
    value_position_economics(PositionEconomicsInput {
        instrument,
        signed_qty_micros: qty_micros,
        mark_price_micros: Some(mark),
        account_currency: "USD".to_string(),
    })
}

/// Hand-constructed `PositionEconomicsValue` for tests that need to drive
/// the *aggregator's own* overflow path directly, independent of
/// `ASSET-CORE-04A`'s already-proven per-position overflow path.
fn raw_active_position(
    asset_class: &str,
    currency: &str,
    notional: i128,
) -> PositionEconomicsValue {
    PositionEconomicsValue {
        instrument_id: format!("{asset_class}:raw"),
        symbol: "RAW".to_string(),
        asset_class: asset_class.to_string(),
        quote_currency: currency.to_string(),
        account_currency: currency.to_string(),
        signed_qty_micros: M,
        mark_price_micros: Some(M),
        contract_multiplier_micros: M,
        notional_micros: Some(notional),
        absolute_notional_micros: Some(notional.unsigned_abs() as i128),
        truth_state: InstrumentEconomicsTruthState::Active,
        reason_code: "active".to_string(),
    }
}

fn input(
    cash_micros: i128,
    currency: &str,
    positions: Vec<PositionEconomicsValue>,
) -> PortfolioEconomicsInput {
    PortfolioEconomicsInput {
        cash_micros,
        account_currency: currency.to_string(),
        positions,
    }
}

// ---------------------------------------------------------------------------
// EQ: equity-only aggregation reproduces current multiplier=1 behavior,
// composed at the portfolio level.
// ---------------------------------------------------------------------------

#[test]
fn eq01_equity_only_two_long_positions_nav_gross_net_weights() {
    let positions = vec![
        equity_value("AAPL", 10 * M, Some(50 * M), "USD"), // +500
        equity_value("MSFT", 5 * M, Some(20 * M), "USD"),  // +100
    ];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snap.nav_micros, Some(1_600 * M as i128));
    assert_eq!(snap.gross_exposure_micros, Some(600 * M as i128));
    assert_eq!(snap.net_exposure_micros, Some(600 * M as i128));

    let aapl = snap.positions.iter().find(|r| r.symbol == "AAPL").unwrap();
    assert_eq!(aapl.notional_micros, Some(500 * M as i128));
    assert_eq!(aapl.weight_bps, Some(3125)); // 500/1600 * 10_000

    let msft = snap.positions.iter().find(|r| r.symbol == "MSFT").unwrap();
    assert_eq!(msft.weight_bps, Some(625)); // 100/1600 * 10_000
}

// ---------------------------------------------------------------------------
// SIGN: long/short signs preserved in net exposure; gross uses absolute value
// ---------------------------------------------------------------------------

#[test]
fn sign01_long_and_short_signs_preserved_in_net_gross_uses_absolute() {
    let positions = vec![
        equity_value("AAPL", 10 * M, Some(50 * M), "USD"), // +500
        equity_value("TLT", -5 * M, Some(20 * M), "USD"),  // -100
    ];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snap.nav_micros, Some(1_400 * M as i128)); // 1000 + 500 - 100
    assert_eq!(snap.net_exposure_micros, Some(400 * M as i128)); // signed: 500 - 100
    assert_eq!(snap.gross_exposure_micros, Some(600 * M as i128)); // absolute: 500 + 100

    let tlt = snap.positions.iter().find(|r| r.symbol == "TLT").unwrap();
    assert_eq!(tlt.notional_micros, Some(-100 * M as i128));
    assert_eq!(tlt.absolute_notional_micros, Some(100 * M as i128));
}

// ---------------------------------------------------------------------------
// EMPTY: empty positions list
// ---------------------------------------------------------------------------

#[test]
fn empty01_no_positions_with_positive_cash_is_deterministic_no_position_result() {
    let snap1 = aggregate_portfolio_economics(input(500 * M as i128, "USD", vec![]));
    let snap2 = aggregate_portfolio_economics(input(500 * M as i128, "USD", vec![]));

    assert_eq!(snap1, snap2);
    assert_eq!(snap1.truth_state, PortfolioEconomicsTruthState::NoPositions);
    assert_eq!(snap1.nav_micros, Some(500 * M as i128));
    assert_eq!(snap1.gross_exposure_micros, Some(0));
    assert_eq!(snap1.net_exposure_micros, Some(0));
    assert!(snap1.positions.is_empty());
    assert!(snap1.asset_class_exposures.is_empty());
    assert!(snap1.currency_exposures.is_empty());
}

#[test]
fn empty02_no_positions_with_non_positive_cash_is_nav_unavailable_not_bypassed() {
    let snap = aggregate_portfolio_economics(input(0, "USD", vec![]));
    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::NavUnavailable,
        "an empty position list must not bypass the NAV <= 0 gate"
    );
    assert_eq!(snap.nav_micros, Some(0));
}

// ---------------------------------------------------------------------------
// FLAT: flat positions contribute zero and never require a mark
// ---------------------------------------------------------------------------

#[test]
fn flat01_flat_position_contributes_zero_without_a_mark() {
    let positions = vec![equity_value("FLAT", 0, None, "USD")];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snap.nav_micros, Some(1_000 * M as i128));
    let row = &snap.positions[0];
    assert_eq!(row.notional_micros, Some(0));
    assert_eq!(row.weight_bps, Some(0));
    assert_eq!(row.truth_state, "active");
}

#[test]
fn flat02_flat_position_with_mark_present_still_contributes_zero() {
    let positions = vec![equity_value("FLAT2", 0, Some(9_999 * M), "USD")];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snap.positions[0].notional_micros, Some(0));
}

// ---------------------------------------------------------------------------
// UNAVAIL: missing/unavailable position value fails the whole snapshot closed
// ---------------------------------------------------------------------------

#[test]
fn unavail01_missing_mark_on_non_flat_position_fails_whole_snapshot_closed() {
    let positions = vec![
        equity_value("AAPL", 10 * M, Some(50 * M), "USD"),
        equity_value("MSFT", 5 * M, None, "USD"), // missing mark
    ];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::PositionValueUnavailable
    );
    assert_eq!(snap.nav_micros, None);
    assert_eq!(snap.gross_exposure_micros, None);
    assert_eq!(snap.net_exposure_micros, None);
    assert!(snap.asset_class_exposures.is_empty());
    assert!(snap.currency_exposures.is_empty());

    let aapl = snap.positions.iter().find(|r| r.symbol == "AAPL").unwrap();
    assert_eq!(
        aapl.notional_micros,
        Some(500 * M as i128),
        "a position's own known value is still surfaced even though the snapshot-wide NAV is blocked"
    );
    assert_eq!(
        aapl.weight_bps, None,
        "no weight is ever computed without a confirmed NAV"
    );

    let msft = snap.positions.iter().find(|r| r.symbol == "MSFT").unwrap();
    assert_eq!(msft.notional_micros, None);
    assert_eq!(msft.truth_state, "position_value_unavailable");
    assert_eq!(msft.reason_code, "missing_mark_for_non_flat_position");
}

// ---------------------------------------------------------------------------
// NAV: NAV <= 0 fails closed; gross/net still reported, weights are not
// ---------------------------------------------------------------------------

#[test]
fn nav01_negative_nav_fails_closed_but_reports_gross_net() {
    let positions = vec![equity_value("AAPL", 10 * M, Some(50 * M), "USD")]; // +500
    let snap = aggregate_portfolio_economics(input(-2_000 * M as i128, "USD", positions));

    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::NavUnavailable
    );
    assert_eq!(snap.nav_micros, Some(-1_500 * M as i128));
    assert_eq!(
        snap.gross_exposure_micros,
        Some(500 * M as i128),
        "gross exposure is still well-defined; only weights require NAV > 0"
    );
    assert_eq!(snap.net_exposure_micros, Some(500 * M as i128));
    assert_eq!(snap.positions[0].weight_bps, None);
    assert_eq!(snap.asset_class_exposures.len(), 1);
    assert_eq!(snap.asset_class_exposures[0].weight_bps, None);
}

#[test]
fn nav02_exact_zero_nav_is_treated_same_as_negative() {
    let positions = vec![equity_value("AAPL", M, Some(10 * M), "USD")]; // +10
    let snap = aggregate_portfolio_economics(input(-10 * M as i128, "USD", positions));

    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::NavUnavailable,
        "NAV == 0 must be treated the same as NAV < 0 (gate is <=)"
    );
    assert_eq!(snap.nav_micros, Some(0));
    assert_eq!(snap.positions[0].weight_bps, None);
}

// ---------------------------------------------------------------------------
// FX: cross-currency / missing-currency inputs fail closed
// ---------------------------------------------------------------------------

#[test]
fn fx01_position_valued_in_foreign_currency_fails_whole_snapshot_closed() {
    // ASML individually values `Active` -- it is internally consistent in its
    // own (EUR) currency. The *portfolio* is USD, so this must still refuse,
    // proving the aggregator re-checks currency against its own
    // account_currency rather than blindly trusting an upstream `Active`.
    let asml = equity_value("ASML", 10 * M, Some(700 * M), "EUR");
    assert_eq!(asml.truth_state, InstrumentEconomicsTruthState::Active);

    let positions = vec![equity_value("AAPL", 10 * M, Some(50 * M), "USD"), asml];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::CurrencyConversionUnsupported
    );
    assert_eq!(snap.nav_micros, None);
    assert!(snap.asset_class_exposures.is_empty());
    assert!(snap.currency_exposures.is_empty());

    let asml_row = snap.positions.iter().find(|r| r.symbol == "ASML").unwrap();
    assert_eq!(asml_row.truth_state, "currency_conversion_unsupported");
    assert_eq!(
        asml_row.reason_code,
        "quote_currency_does_not_match_portfolio_account_currency"
    );

    let aapl_row = snap.positions.iter().find(|r| r.symbol == "AAPL").unwrap();
    assert_eq!(
        aapl_row.truth_state, "active",
        "an unrelated USD row is not itself refused"
    );
}

#[test]
fn fx02_empty_account_currency_fails_closed_before_examining_positions() {
    let positions = vec![equity_value("AAPL", 10 * M, Some(50 * M), "USD")];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "", positions));

    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::CurrencyConversionUnsupported
    );
    assert_eq!(snap.reason_code, "empty_account_currency");
    assert_eq!(snap.nav_micros, None);
    assert!(snap.positions.is_empty());
}

// ---------------------------------------------------------------------------
// MIX: futures/options/crypto model-only rows aggregate without enabling
// trading -- `PortfolioEconomicsPositionRow`/`PortfolioEconomicsSnapshot`
// carry no field of any kind that could be mistaken for an enablement
// signal (same precedent as ASSET-CORE-04A's `frac02`).
// ---------------------------------------------------------------------------

#[test]
fn mix01_future_option_crypto_rows_aggregate_together_model_only() {
    let positions = vec![
        future_value("ES2026U", M, 5_000 * M, 50), // 1 contract, 50x, $5,000 -> $250,000
        option_value("AAPL20260918C150", M, 2 * M + M / 2, 100), // 100x, $2.50 -> $250
        crypto_value("BTCUSD", M / 2, 60_000 * M), // 0.5 BTC @ $60,000 -> $30,000
    ];
    let snap = aggregate_portfolio_economics(input(10_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    // 10,000 cash + 250,000 + 250 + 30,000 = 290,250
    assert_eq!(snap.nav_micros, Some(290_250 * M as i128));
    assert_eq!(snap.gross_exposure_micros, Some(280_250 * M as i128));

    let classes: Vec<&str> = snap
        .asset_class_exposures
        .iter()
        .map(|r| r.key.as_str())
        .collect();
    assert!(classes.contains(&"future"));
    assert!(classes.contains(&"option"));
    assert!(classes.contains(&"crypto"));
}

// ---------------------------------------------------------------------------
// ASSET: asset-class exposure rows isolate sums per class
// ---------------------------------------------------------------------------

#[test]
fn asset01_asset_class_exposure_rows_isolate_sums_per_class() {
    let positions = vec![
        equity_value("AAPL", 10 * M, Some(50 * M), "USD"), // equity +500
        equity_value("MSFT", -2 * M, Some(30 * M), "USD"), // equity -60
        future_value("ES2026U", M, 1_000 * M, 50),         // future +50,000
    ];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snap.asset_class_exposures.len(), 2);

    let equity = snap
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "equity")
        .unwrap();
    assert_eq!(equity.signed_notional_micros, 440 * M as i128); // 500 - 60
    assert_eq!(equity.absolute_notional_micros, 560 * M as i128); // 500 + 60

    let future = snap
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "future")
        .unwrap();
    assert_eq!(future.signed_notional_micros, 50_000 * M as i128);
    assert_eq!(future.absolute_notional_micros, 50_000 * M as i128);
}

// ---------------------------------------------------------------------------
// CCY: currency exposure rows sum multiple same-currency rows into one bucket
// ---------------------------------------------------------------------------

#[test]
fn ccy01_currency_exposure_rows_sum_same_currency_rows_into_one_bucket() {
    let positions = vec![
        equity_value("AAPL", 10 * M, Some(50 * M), "USD"), // +500
        equity_value("MSFT", 5 * M, Some(20 * M), "USD"),  // +100
    ];
    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(
        snap.currency_exposures.len(),
        1,
        "every Active snapshot is necessarily single-currency (cross-currency fails closed), \
         so two same-currency rows must collapse into exactly one bucket"
    );
    let usd = &snap.currency_exposures[0];
    assert_eq!(usd.key, "USD");
    assert_eq!(usd.signed_notional_micros, 600 * M as i128);
    assert_eq!(usd.absolute_notional_micros, 600 * M as i128);
    assert_eq!(usd.weight_bps, Some(3750)); // 600/1600 * 10_000
}

// ---------------------------------------------------------------------------
// UNSUP: empty asset_class fails closed; an arbitrary non-empty asset_class
// is explicitly surfaced rather than refused (mirrors ASSET-CORE-04A's
// unsup01/unsup02 precedent at the portfolio-aggregation layer).
// ---------------------------------------------------------------------------

#[test]
fn unsup01_empty_asset_class_position_fails_closed_as_value_unavailable() {
    let mut econ = InstrumentEconomics::equity("equity:US:X", "X", "USD");
    econ.asset_class = String::new();
    let bad = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        bad.truth_state,
        InstrumentEconomicsTruthState::UnsupportedInstrument
    );

    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", vec![bad]));
    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::PositionValueUnavailable
    );
    assert_eq!(snap.positions[0].reason_code, "empty_asset_class");
}

#[test]
fn unsup02_arbitrary_non_empty_asset_class_is_explicitly_surfaced_not_refused() {
    let mut econ = InstrumentEconomics::equity("x:1", "X", "USD");
    econ.asset_class = "exotic_derivative".to_string();
    let exotic = value_position_economics(PositionEconomicsInput {
        instrument: econ,
        signed_qty_micros: M,
        mark_price_micros: Some(10 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(exotic.truth_state, InstrumentEconomicsTruthState::Active);

    let snap = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", vec![exotic]));
    assert_eq!(
        snap.truth_state,
        PortfolioEconomicsTruthState::Active,
        "a merely unusual non-empty asset_class is not itself a failure -- only emptiness is"
    );
    assert_eq!(snap.asset_class_exposures.len(), 1);
    assert_eq!(snap.asset_class_exposures[0].key, "exotic_derivative");
}

// ---------------------------------------------------------------------------
// OVF: overflow is reported explicitly -- never panics, never silently wraps
// ---------------------------------------------------------------------------

#[test]
fn ovf01_near_i128_max_notionals_report_overflow_not_panic() {
    let positions = vec![
        raw_active_position("equity", "USD", i128::MAX - 10),
        raw_active_position("equity", "USD", 20),
    ];
    let snap = aggregate_portfolio_economics(input(0, "USD", positions));

    assert_eq!(snap.truth_state, PortfolioEconomicsTruthState::Overflow);
    assert_eq!(snap.reason_code, "nav_or_exposure_sum_overflowed_i128");
    assert_eq!(snap.nav_micros, None);
    assert_eq!(snap.gross_exposure_micros, None);
    assert_eq!(snap.net_exposure_micros, None);
    assert!(snap.asset_class_exposures.is_empty());
    assert!(snap.currency_exposures.is_empty());
}

// ---------------------------------------------------------------------------
// DET: determinism
// ---------------------------------------------------------------------------

#[test]
fn det01_identical_input_yields_identical_output_including_ordering() {
    let build = || {
        vec![
            equity_value("MSFT", 5 * M, Some(20 * M), "USD"),
            equity_value("AAPL", 10 * M, Some(50 * M), "USD"),
            future_value("ES2026U", M, 1_000 * M, 50),
        ]
    };
    let snap1 = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", build()));
    let snap2 = aggregate_portfolio_economics(input(1_000 * M as i128, "USD", build()));

    assert_eq!(snap1, snap2);
    // Row order mirrors input order (MSFT before AAPL); exposure-row order
    // is sorted by key regardless of input order (BTreeMap-backed).
    assert_eq!(snap1.positions[0].symbol, "MSFT");
    assert_eq!(snap1.positions[1].symbol, "AAPL");
    let keys: Vec<&str> = snap1
        .asset_class_exposures
        .iter()
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(keys, vec!["equity", "future"]);
}

// ---------------------------------------------------------------------------
// REGR: existing upstream/sibling seams are untouched by this patch. These
// call the pre-existing, unmodified public API directly (not a re-run of
// their own dedicated scenario files) as an in-file regression smoke proof.
// ---------------------------------------------------------------------------

#[test]
fn regr01_value_position_economics_asset_core_04a_is_unchanged() {
    let value = value_position_economics(PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD"),
        signed_qty_micros: 10 * M,
        mark_price_micros: Some(100 * M),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(1_000 * M as i128));
}

#[test]
fn regr02_compute_portfolio_weights_live_weights_01_is_unchanged() {
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
fn regr03_evaluate_sector_risk_etf_risk_closure_01_is_unchanged() {
    use std::collections::HashMap;

    let positions = vec![PositionWeightInput {
        symbol: "SPY".to_string(),
        signed_qty: 0,
    }];
    let marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    let sector_map: HashMap<String, String> = HashMap::new();
    let sector_limits_bps: HashMap<String, i64> = HashMap::new();

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
