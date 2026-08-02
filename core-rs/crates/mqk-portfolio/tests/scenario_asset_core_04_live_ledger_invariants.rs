//! Scenario: ASSET-CORE-04 — live ledger boundary invariants.
//!
//! Phase B of `ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED`.
//!
//! The `ASSET-CORE-04A`/`04C` economics scaffold's own module docs already
//! *claim* that valuing an equity position through
//! `value_position_economics` with `multiplier = 1x` reproduces the live
//! `accounting.rs`/`metrics.rs`/`valuation.rs` un-multiplied `qty * price`
//! math "exactly". That claim was previously backed only by the scaffold's
//! own isolated unit tests (which never call the live accounting path) and
//! by a doc comment. This file is the one cross-module test that did not
//! already exist: it drives real fills through the live FIFO ledger
//! (`apply_entry`/`accounting.rs`), independently recomputes the same
//! position's value through the `ASSET-CORE-04A`/`04C` scaffold, and
//! asserts the two numbers agree exactly — for both a single position and
//! a multi-position, multi-symbol book, including the portfolio-level NAV
//! seam (`compute_portfolio_weights`) against its scaffold counterpart
//! (`aggregate_portfolio_economics`).
//!
//! It does **not** change `accounting.rs`, `metrics.rs`, `valuation.rs`,
//! `instrument_economics.rs`, or `portfolio_economics.rs`. It only proves,
//! at the numeric level, what `docs/specs/asset_core_04_live_ledger_boundary_audit.md`
//! already proved by caller-map grep: the two paths are separate call
//! graphs that happen to agree on equity-shaped input, and only the live
//! path is ever reachable from production code.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                                 |
//! |---------|---------------------------------------------------------------------------------|
//! | EQ-01   | Live FIFO market value (single long position) == scaffold notional             |
//! | EQ-02   | Live FIFO market value (single short position) == scaffold notional (negative) |
//! | EQ-03   | Live exposure (gross) == scaffold absolute notional after partial-fill FIFO    |
//! | EQ-04   | Live equity (cash + mv) == scaffold NAV for a multi-symbol book                |
//! | EQ-05   | `compute_portfolio_weights` NAV == `aggregate_portfolio_economics` NAV         |
//! | EQ-06   | Flat position values to zero identically on both paths, no mark required       |
//! | SEP-01  | Live path never needs an `InstrumentEconomics`/currency/multiplier value       |
//! | SEP-02  | Same fills replayed through `recompute_from_ledger` still agree with scaffold  |

use std::collections::BTreeMap;

use mqk_portfolio::{
    aggregate_portfolio_economics, apply_entry, compute_equity_micros, compute_exposure_micros,
    compute_portfolio_weights, marks, recompute_from_ledger, value_position_economics, Fill,
    InstrumentEconomics, InstrumentEconomicsTruthState, LedgerEntry, PortfolioEconomicsInput,
    PortfolioEconomicsTruthState, PortfolioState, PositionMark, PositionWeightInput, Side,
};

const M: i64 = 1_000_000; // MICROS_SCALE

fn equity_notional(symbol: &str, signed_qty: i64, mark_price_micros: i64) -> i128 {
    let value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity(format!("equity:US:{symbol}"), symbol, "USD"),
        signed_qty_micros: signed_qty * M,
        mark_price_micros: Some(mark_price_micros),
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    value
        .notional_micros
        .expect("active value must carry a notional")
}

// ---------------------------------------------------------------------------
// EQ-01 / EQ-02: single-position market value agreement
// ---------------------------------------------------------------------------

#[test]
fn eq01_live_long_market_value_matches_scaffold_notional() {
    let mut pf = PortfolioState::new(100_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0)),
    );

    let pos = &pf.positions["AAPL"];
    let signed_qty = pos.qty_signed();
    assert_eq!(signed_qty, 10);

    let mark_price = 110 * M; // marked away from entry, proves this isn't a cost-basis coincidence
    let mm = marks([("AAPL", mark_price)]);
    let live_mv = compute_equity_micros(pf.cash_micros, &pf.positions, &mm) - pf.cash_micros;

    let scaffold_notional = equity_notional("AAPL", signed_qty, mark_price);

    assert_eq!(live_mv as i128, scaffold_notional);
}

#[test]
fn eq02_live_short_market_value_matches_scaffold_notional_and_is_negative() {
    let mut pf = PortfolioState::new(100_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("TSLA", Side::Sell, 5, 200 * M, 0)),
    );

    let pos = &pf.positions["TSLA"];
    let signed_qty = pos.qty_signed();
    assert_eq!(signed_qty, -5);

    let mark_price = 210 * M;
    let mm = marks([("TSLA", mark_price)]);
    let live_mv = compute_equity_micros(pf.cash_micros, &pf.positions, &mm) - pf.cash_micros;

    let scaffold_notional = equity_notional("TSLA", signed_qty, mark_price);

    assert!(scaffold_notional < 0, "short position must value negative");
    assert_eq!(live_mv as i128, scaffold_notional);
}

// ---------------------------------------------------------------------------
// EQ-03: FIFO partial-fill sequence still agrees after multiple fills
// ---------------------------------------------------------------------------

#[test]
fn eq03_partial_fill_fifo_sequence_gross_exposure_matches_scaffold() {
    let mut pf = PortfolioState::new(1_000_000 * M);
    for (side, qty, px) in [
        (Side::Buy, 10, 50 * M),
        (Side::Buy, 5, 60 * M),
        (Side::Sell, 8, 70 * M),
    ] {
        apply_entry(
            &mut pf,
            LedgerEntry::Fill(Fill::new("MSFT", side, qty, px, 0)),
        );
    }

    let pos = &pf.positions["MSFT"];
    let signed_qty = pos.qty_signed();
    assert_eq!(signed_qty, 7, "10 + 5 - 8 = 7 remaining long");

    let mark_price = 65 * M;
    let mm = marks([("MSFT", mark_price)]);
    let exposure = compute_exposure_micros(&pf.positions, &mm);

    let scaffold_notional = equity_notional("MSFT", signed_qty, mark_price);

    assert_eq!(
        exposure.gross_exposure_micros as i128,
        scaffold_notional.abs()
    );
    assert_eq!(exposure.net_exposure_micros as i128, scaffold_notional);
}

// ---------------------------------------------------------------------------
// EQ-04 / EQ-05: portfolio-level NAV agreement across a multi-symbol book
// ---------------------------------------------------------------------------

#[test]
fn eq04_live_equity_matches_scaffold_nav_for_multi_symbol_book() {
    let mut pf = PortfolioState::new(50_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0)),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("TSLA", Side::Sell, 5, 200 * M, 0)),
    );

    let mm = marks([("AAPL", 110 * M), ("TSLA", 190 * M)]);
    let live_equity = compute_equity_micros(pf.cash_micros, &pf.positions, &mm);

    let aapl_value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD"),
        signed_qty_micros: pf.positions["AAPL"].qty_signed() * M,
        mark_price_micros: Some(110 * M),
        account_currency: "USD".to_string(),
    });
    let tsla_value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:TSLA", "TSLA", "USD"),
        signed_qty_micros: pf.positions["TSLA"].qty_signed() * M,
        mark_price_micros: Some(190 * M),
        account_currency: "USD".to_string(),
    });

    let snapshot = aggregate_portfolio_economics(PortfolioEconomicsInput {
        cash_micros: pf.cash_micros as i128,
        account_currency: "USD".to_string(),
        positions: vec![aapl_value, tsla_value],
    });

    assert_eq!(snapshot.truth_state, PortfolioEconomicsTruthState::Active);
    assert_eq!(snapshot.nav_micros, Some(live_equity as i128));
}

#[test]
fn eq05_compute_portfolio_weights_nav_matches_aggregate_portfolio_economics_nav() {
    let mut pf = PortfolioState::new(50_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 10, 100 * M, 0)),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("TSLA", Side::Sell, 5, 200 * M, 0)),
    );

    let aapl_qty = pf.positions["AAPL"].qty_signed();
    let tsla_qty = pf.positions["TSLA"].qty_signed();

    // Live weights seam (PORTFOLIO-LIVE-WEIGHTS-01).
    let inputs = vec![
        PositionWeightInput {
            symbol: "AAPL".to_string(),
            signed_qty: aapl_qty,
        },
        PositionWeightInput {
            symbol: "TSLA".to_string(),
            signed_qty: tsla_qty,
        },
    ];
    let mut live_marks: BTreeMap<String, PositionMark> = BTreeMap::new();
    live_marks.insert(
        "AAPL".to_string(),
        PositionMark {
            symbol: "AAPL".to_string(),
            mark_price_micros: 110 * M,
            mark_ts_utc: None,
            source: "test".to_string(),
        },
    );
    live_marks.insert(
        "TSLA".to_string(),
        PositionMark {
            symbol: "TSLA".to_string(),
            mark_price_micros: 190 * M,
            mark_ts_utc: None,
            source: "test".to_string(),
        },
    );
    let weights_snapshot = compute_portfolio_weights(pf.cash_micros, &inputs, &live_marks);
    assert_eq!(weights_snapshot.truth_state, "active");

    // Scaffold seam (ASSET-CORE-04A/04C).
    let aapl_value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:AAPL", "AAPL", "USD"),
        signed_qty_micros: aapl_qty * M,
        mark_price_micros: Some(110 * M),
        account_currency: "USD".to_string(),
    });
    let tsla_value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:TSLA", "TSLA", "USD"),
        signed_qty_micros: tsla_qty * M,
        mark_price_micros: Some(190 * M),
        account_currency: "USD".to_string(),
    });
    let economics_snapshot = aggregate_portfolio_economics(PortfolioEconomicsInput {
        cash_micros: pf.cash_micros as i128,
        account_currency: "USD".to_string(),
        positions: vec![aapl_value, tsla_value],
    });

    assert_eq!(weights_snapshot.nav_micros, economics_snapshot.nav_micros);
}

// ---------------------------------------------------------------------------
// EQ-06: flat position agreement without a mark on either path
// ---------------------------------------------------------------------------

#[test]
fn eq06_flat_position_values_to_zero_on_both_paths_without_a_mark() {
    let mut pf = PortfolioState::new(10_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("NFLX", Side::Buy, 3, 400 * M, 0)),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("NFLX", Side::Sell, 3, 420 * M, 0)),
    );

    // Live path drops flat positions entirely (accounting.rs: "if flat, drop
    // the position to keep state minimal/deterministic").
    assert!(!pf.positions.contains_key("NFLX"));

    // Scaffold path: a flat position values to zero without requiring a mark.
    let value = value_position_economics(mqk_portfolio::PositionEconomicsInput {
        instrument: InstrumentEconomics::equity("equity:US:NFLX", "NFLX", "USD"),
        signed_qty_micros: 0,
        mark_price_micros: None,
        account_currency: "USD".to_string(),
    });
    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active);
    assert_eq!(value.notional_micros, Some(0));
}

// ---------------------------------------------------------------------------
// SEP-01 / SEP-02: the two paths are genuinely separate call graphs
// ---------------------------------------------------------------------------

#[test]
fn sep01_live_accounting_produces_a_correct_position_with_no_economics_types_involved() {
    // This test constructs and applies fills using only the live-path types
    // (`Fill`, `Side`, `PortfolioState`, `apply_entry`) and never references
    // `InstrumentEconomics`/`PositionEconomicsInput`/`value_position_economics`
    // anywhere in its own body — demonstrating the live path is fully
    // self-sufficient and does not require the scaffold to produce a correct
    // answer. (`docs/specs/asset_core_04_live_ledger_boundary_audit.md` §2/§4
    // independently proves this at the workspace-caller-graph level via grep;
    // this test proves it behaviorally, for this one scenario, at the
    // type-usage level.)
    let mut pf = PortfolioState::new(100_000 * M);
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("GOOG", Side::Buy, 4, 150 * M, 0)),
    );
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new("GOOG", Side::Sell, 1, 160 * M, 0)),
    );

    assert_eq!(pf.positions["GOOG"].qty_signed(), 3);
    // realized pnl on the 1-share sale: (160 - 150) * 1 = 10 * M
    assert_eq!(pf.realized_pnl_micros, 10 * M);
}

#[test]
fn sep02_recompute_from_ledger_replay_still_agrees_with_scaffold() {
    let initial_cash = 25_000 * M;
    let ledger = vec![
        LedgerEntry::Fill(Fill::new("AMZN", Side::Buy, 6, 130 * M, 0)),
        LedgerEntry::Fill(Fill::new("AMZN", Side::Buy, 2, 140 * M, 0)),
        LedgerEntry::Fill(Fill::new("AMZN", Side::Sell, 3, 150 * M, 0)),
    ];

    let (cash, _realized, positions) = recompute_from_ledger(initial_cash, &ledger);
    let signed_qty = positions["AMZN"].qty_signed();
    assert_eq!(signed_qty, 5, "6 + 2 - 3 = 5 remaining long");

    let mark_price = 155 * M;
    let mm = marks([("AMZN", mark_price)]);
    let live_mv = compute_equity_micros(cash, &positions, &mm) - cash;

    let scaffold_notional = equity_notional("AMZN", signed_qty, mark_price);

    assert_eq!(live_mv as i128, scaffold_notional);
}
