//! CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED: second-symbol
//! mark-to-model-chain proof.
//!
//! Continues `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`'s pure in-memory chain
//! proof (`scenario_crypto_portfolio_economics_local_marks_01a.rs`) by
//! proving the same unmodified ASSET-CORE-04A/04B/04C model chain is not
//! hardcoded to `BTC/USD`: a real mark, parsed from the newly-committed
//! `crypto_ethusd_1d_local.csv` fixture, reaches the chain for the disabled
//! `ETH/USD` registry-v2 row this patch adds beside `BTC/USD`:
//!
//! ```text
//! crypto_ethusd_1d_local.csv --(mqk_md::ingest_csv)--> RawBar (latest close)
//! instruments_v2.crypto_local_marks.example.json --(mqk_md::instrument_registry_v2)--> InstrumentDefinitionV2 (ETH/USD row)
//!     --(mqk_daemon::state::bridge_instrument_registry_v2_to_economics, ASSET-CORE-04B)--> InstrumentEconomics
//!     --(mqk_portfolio::value_position_economics, ASSET-CORE-04A)--> PositionEconomicsValue
//!     --(mqk_portfolio::aggregate_portfolio_economics, ASSET-CORE-04C)--> PortfolioEconomicsSnapshot
//! ```
//!
//! This is a pure, in-memory, no-DB, no-router, no-network proof, mirroring
//! `scenario_crypto_portfolio_economics_local_marks_01a.rs` exactly except
//! for the symbol. It does not start a daemon, does not build an
//! `axum::Router`, does not open a DB pool, and does not touch
//! `/api/v1/portfolio/economics/status` (ASSET-CORE-04D) for the same
//! structural reason that file's module docs already record.
//!
//! # Proof matrix
//!
//! | Test       | What it proves                                                          |
//! |------------|--------------------------------------------------------------------------|
//! | CHAIN-01   | ETH/USD registry-v2 row bridges via ASSET-CORE-04B (bridged, not failed)|
//! | CHAIN-02   | Bridged economics: crypto, USD, multiplier=1.0x, model_only, never trading-enabled |
//! | CHAIN-03   | ETH/USD CSV latest completed close feeds ASSET-CORE-04A -> Active, real notional |
//! | CHAIN-04   | ASSET-CORE-04C aggregation over that position -> Active, NAV computed  |
//! | CHAIN-05   | Aggregation has a `"crypto"` asset-class exposure bucket                |
//! | CHAIN-06   | Aggregation has a `"USD"` currency exposure bucket                      |
//! | CHAIN-07   | No field anywhere in the chain indicates trading enablement             |
//! | CHAIN-08   | BTC/USD row in the same fixture is unaffected by ETH/USD's presence    |

use std::path::PathBuf;

use mqk_daemon::state::bridge_instrument_registry_v2_to_economics;
use mqk_md::ingest_csv::parse_csv_file;
use mqk_md::instrument_registry_v2::{load_instrument_registry_v2, validate_registry_v2};
use mqk_portfolio::{
    aggregate_portfolio_economics, value_position_economics, InstrumentEconomicsTruthState,
    PortfolioEconomicsInput, PortfolioEconomicsTruthState, PositionEconomicsInput, MICROS_SCALE,
};

const ACCOUNT_CURRENCY: &str = "USD";
const CSV_TIMEFRAME: &str = "1D";
const ETH_SYMBOL: &str = "ETH/USD";

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn eth_csv_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/crypto_ethusd_1d_local.csv")
}

/// Decimal-string -> `*_micros` (1e-6) conversion, identical to the
/// test-local helper in `scenario_crypto_portfolio_economics_local_marks_01a.rs`.
/// Test-local only -- not a production helper.
fn decimal_str_to_micros(s: &str) -> i64 {
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    let int_val: i64 = int_part.parse().expect("integer part must parse");
    let mut frac = frac_part.to_string();
    while frac.len() < 6 {
        frac.push('0');
    }
    frac.truncate(6);
    let frac_val: i64 = if frac.is_empty() {
        0
    } else {
        frac.parse().expect("fractional part must parse")
    };
    int_val * MICROS_SCALE + frac_val
}

/// Load + validate the registry-v2 fixture and parse the ETH/USD CSV
/// fixture's latest completed close, in micros. Shared setup for every
/// CHAIN test.
fn latest_close_micros_and_bridged_economics() -> (i64, mqk_portfolio::InstrumentEconomics) {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks registry fixture must load");
    validate_registry_v2(&registry).expect("crypto local-marks registry fixture must validate");

    let bridge_summary = bridge_instrument_registry_v2_to_economics(&registry);
    let eth_bridge = bridge_summary
        .rows
        .iter()
        .find(|r| r.symbol == ETH_SYMBOL)
        .expect("ETH/USD row must be present in bridge summary");
    let economics = eth_bridge
        .economics
        .clone()
        .expect("ETH/USD must bridge to InstrumentEconomics");

    let bars = parse_csv_file(&eth_csv_fixture_path(), CSV_TIMEFRAME)
        .expect("ETH/USD CSV fixture must parse via the existing ingest_csv parser");
    let latest = bars
        .iter()
        .max_by_key(|b| b.end_ts)
        .expect("ETH/USD CSV fixture must have at least one bar");
    assert_eq!(latest.close, "3200.00", "fixture close must be deterministic");

    (decimal_str_to_micros(&latest.close), economics)
}

// ---------------------------------------------------------------------------
// CHAIN-01/02: ASSET-CORE-04B bridge of the real ETH/USD registry-v2 row
// ---------------------------------------------------------------------------

#[test]
fn chain01_eth_registry_row_bridges_successfully_via_asset_core_04b() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);

    let eth_bridge = summary
        .rows
        .iter()
        .find(|r| r.symbol == ETH_SYMBOL)
        .expect("ETH/USD row must be present");
    assert_eq!(eth_bridge.truth_state, "active", "result={eth_bridge:?}");
    assert!(eth_bridge.economics.is_some());
}

#[test]
fn chain02_eth_bridged_economics_is_crypto_usd_unit_multiplier_model_only_never_trading_enabled() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);
    let eth_bridge = summary
        .rows
        .iter()
        .find(|r| r.symbol == ETH_SYMBOL)
        .expect("ETH/USD row must be present");

    let economics = eth_bridge.economics.as_ref().expect("economics must be present");
    assert_eq!(economics.asset_class, "crypto");
    assert_eq!(economics.quote_currency, "USD");
    assert_eq!(economics.contract_multiplier_micros, MICROS_SCALE);
    assert!(eth_bridge.model_only, "crypto must be flagged model_only");
    assert!(!eth_bridge.trading_enabled_by_bridge);
    assert!(!eth_bridge.enabled);
    assert!(!eth_bridge.paper_trading_enabled);
    assert!(!eth_bridge.live_trading_enabled);
}

// ---------------------------------------------------------------------------
// CHAIN-03: ASSET-CORE-04A valuation using the ETH/USD CSV-derived mark
// ---------------------------------------------------------------------------

#[test]
fn chain03_eth_csv_latest_close_feeds_asset_core_04a_valuation_active_with_real_notional() {
    let (mark_price_micros, economics) = latest_close_micros_and_bridged_economics();
    assert_eq!(mark_price_micros, 3_200 * MICROS_SCALE);

    // A whole 1 ETH position, matching this patch's suggested deterministic
    // shape (as opposed to CRYPTO-DATA-01A/01B's fractional 0.01 BTC).
    let signed_qty_micros = MICROS_SCALE;

    let value = value_position_economics(PositionEconomicsInput {
        instrument: economics,
        signed_qty_micros,
        mark_price_micros: Some(mark_price_micros),
        account_currency: ACCOUNT_CURRENCY.to_string(),
    });

    assert_eq!(value.truth_state, InstrumentEconomicsTruthState::Active, "value={value:?}");
    assert_eq!(value.asset_class, "crypto");
    assert_eq!(value.quote_currency, "USD");
    // 1 ETH * $3,200.00 * 1.0x multiplier = $3,200.00.
    assert_eq!(value.notional_micros, Some(3_200 * MICROS_SCALE as i128));
    assert_eq!(value.absolute_notional_micros, Some(3_200 * MICROS_SCALE as i128));
}

// ---------------------------------------------------------------------------
// CHAIN-04/05/06: ASSET-CORE-04C aggregation over the valued ETH/USD position
// ---------------------------------------------------------------------------

#[test]
fn chain04_05_06_eth_aggregation_is_active_with_crypto_and_usd_exposure_buckets() {
    let (mark_price_micros, economics) = latest_close_micros_and_bridged_economics();
    let signed_qty_micros = MICROS_SCALE; // 1 ETH

    let position = value_position_economics(PositionEconomicsInput {
        instrument: economics,
        signed_qty_micros,
        mark_price_micros: Some(mark_price_micros),
        account_currency: ACCOUNT_CURRENCY.to_string(),
    });
    assert_eq!(position.truth_state, InstrumentEconomicsTruthState::Active);

    let cash_micros: i128 = 100_000 * MICROS_SCALE as i128;
    let snapshot = aggregate_portfolio_economics(PortfolioEconomicsInput {
        cash_micros,
        account_currency: ACCOUNT_CURRENCY.to_string(),
        positions: vec![position],
    });

    // CHAIN-04: aggregation is Active with a computed NAV = cash + notional.
    assert_eq!(snapshot.truth_state, PortfolioEconomicsTruthState::Active, "snapshot={snapshot:?}");
    assert_eq!(
        snapshot.nav_micros,
        Some(cash_micros + 3_200 * MICROS_SCALE as i128)
    );
    assert_eq!(snapshot.gross_exposure_micros, Some(3_200 * MICROS_SCALE as i128));

    // CHAIN-05: a "crypto" asset-class exposure bucket exists with the
    // position's full notional.
    let crypto_row = snapshot
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "crypto")
        .expect("crypto asset-class exposure row must exist");
    assert_eq!(crypto_row.signed_notional_micros, 3_200 * MICROS_SCALE as i128);
    assert_eq!(crypto_row.absolute_notional_micros, 3_200 * MICROS_SCALE as i128);
    assert!(crypto_row.weight_bps.is_some_and(|bps| bps > 0));

    // CHAIN-06: a "USD" currency exposure bucket exists with the same totals
    // (the only position is USD-quoted).
    let usd_row = snapshot
        .currency_exposures
        .iter()
        .find(|r| r.key == "USD")
        .expect("USD currency exposure row must exist");
    assert_eq!(usd_row.signed_notional_micros, 3_200 * MICROS_SCALE as i128);
    assert_eq!(usd_row.absolute_notional_micros, 3_200 * MICROS_SCALE as i128);
}

// ---------------------------------------------------------------------------
// CHAIN-07: nothing in the chain indicates trading enablement
// ---------------------------------------------------------------------------

#[test]
fn chain07_nothing_in_the_eth_chain_indicates_trading_enablement() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);

    assert_eq!(summary.non_equity_enabled_count, 0, "summary={summary:?}");
    for row in &summary.rows {
        assert!(!row.enabled, "row={row:?}");
        assert!(!row.paper_trading_enabled, "row={row:?}");
        assert!(!row.live_trading_enabled, "row={row:?}");
        assert!(!row.trading_enabled_by_bridge, "row={row:?}");
    }

    // PositionEconomicsValue / PortfolioEconomicsSnapshot have no
    // enablement-shaped field at all -- their type signatures alone are the
    // proof; this is a compile-time fact, not a runtime assertion.
}

// ---------------------------------------------------------------------------
// CHAIN-08: BTC/USD is unaffected by ETH/USD's presence in the same fixture
// ---------------------------------------------------------------------------

#[test]
fn chain08_btc_usd_row_in_same_fixture_is_unaffected_by_eth_usd_addition() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);

    let btc_bridge = summary
        .rows
        .iter()
        .find(|r| r.symbol == "BTC/USD")
        .expect("BTC/USD row must still be present");
    assert_eq!(btc_bridge.truth_state, "active", "result={btc_bridge:?}");
    let economics = btc_bridge.economics.as_ref().expect("economics must be present");
    assert_eq!(economics.asset_class, "crypto");
    assert_eq!(economics.quote_currency, "USD");
    assert_eq!(economics.contract_multiplier_micros, MICROS_SCALE);
    assert!(!btc_bridge.trading_enabled_by_bridge);
}
