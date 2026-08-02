//! CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01: mark-to-model-chain proof.
//!
//! Proves that a real (non-fixture-economics) mark, parsed from the
//! committed local CSV fixture, reaches the unmodified ASSET-CORE-04A/04B/04C
//! model chain for the disabled `BTC/USD` registry-v2 fixture this patch
//! adds:
//!
//! ```text
//! crypto_btcusd_1d_local.csv --(mqk_md::ingest_csv)--> RawBar (latest close)
//! instruments_v2.crypto_local_marks.example.json --(mqk_md::instrument_registry_v2)--> InstrumentDefinitionV2
//!     --(mqk_daemon::state::bridge_instrument_registry_v2_to_economics, ASSET-CORE-04B)--> InstrumentEconomics
//!     --(mqk_portfolio::value_position_economics, ASSET-CORE-04A)--> PositionEconomicsValue
//!     --(mqk_portfolio::aggregate_portfolio_economics, ASSET-CORE-04C)--> PortfolioEconomicsSnapshot
//! ```
//!
//! This is a pure, in-memory, no-DB, no-router, no-network proof: it calls
//! only pure functions already proven independently by
//! `scenario_instrument_economics_registry_bridge_asset_core_04b.rs`,
//! `scenario_portfolio_instrument_economics_asset_core_04a.rs`, and
//! `scenario_portfolio_economics_aggregation_asset_core_04c.rs`. It does not
//! start a daemon, does not build an `axum::Router`, does not open a DB pool,
//! and does not touch `/api/v1/portfolio/economics/status`
//! (ASSET-CORE-04D) -- that route loads the v1 equity-only registry
//! (`config/instruments/equities.json`) and converts it to v2
//! (`convert_v1_registry_to_v2`), which always stamps a plain
//! `Equity`/`Etf` contract regardless of the source row's `asset_class`
//! string, so a v1-sourced row can never carry a `CryptoPair` contract. A
//! real crypto position can only reach that specific route once a
//! registry-v2-shaped route-input seam exists; building that seam is out of
//! this patch's scope (see module docs on
//! `mqk_daemon::routes::portfolio::portfolio_economics_status` and
//! `mqk_md::instrument_registry_v2::convert_tracked_instrument_to_v2`).
//!
//! # Proof matrix
//!
//! | Test       | What it proves                                                          |
//! |------------|--------------------------------------------------------------------------|
//! | CHAIN-01   | Registry-v2 fixture bridges via ASSET-CORE-04B (bridged, not failed)   |
//! | CHAIN-02   | Bridged economics: crypto, USD, multiplier=1.0x, model_only, never trading-enabled |
//! | CHAIN-03   | CSV latest completed close feeds ASSET-CORE-04A -> Active, real notional |
//! | CHAIN-04   | ASSET-CORE-04C aggregation over that position -> Active, NAV computed  |
//! | CHAIN-05   | Aggregation has a `"crypto"` asset-class exposure bucket                |
//! | CHAIN-06   | Aggregation has a `"USD"` currency exposure bucket                      |
//! | CHAIN-07   | No field anywhere in the chain indicates trading enablement             |

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

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn csv_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv")
}

/// Decimal-string -> `*_micros` (1e-6) conversion for a fixed-format price
/// string with at most 6 fractional digits (the CSV fixture always emits
/// exactly 2). No floats, matching this crate's existing fixed-point
/// convention. Test-local only -- not a production helper.
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

/// Load + validate the registry-v2 fixture and parse the CSV fixture's
/// latest completed close, in micros. Shared setup for every CHAIN test.
fn latest_close_micros_and_bridged_economics() -> (i64, mqk_portfolio::InstrumentEconomics) {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks registry fixture must load");
    validate_registry_v2(&registry).expect("crypto local-marks registry fixture must validate");

    let bridge_summary = bridge_instrument_registry_v2_to_economics(&registry);
    let btc_bridge = bridge_summary
        .rows
        .iter()
        .find(|r| r.symbol == "BTC/USD")
        .expect("BTC/USD row must be present in bridge summary");
    let economics = btc_bridge
        .economics
        .clone()
        .expect("BTC/USD must bridge to InstrumentEconomics");

    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME)
        .expect("crypto CSV fixture must parse via the existing ingest_csv parser");
    let latest = bars
        .iter()
        .max_by_key(|b| b.end_ts)
        .expect("CSV fixture must have at least one bar");
    assert_eq!(
        latest.close, "44100.00",
        "fixture close must be deterministic"
    );

    (decimal_str_to_micros(&latest.close), economics)
}

// ---------------------------------------------------------------------------
// CHAIN-01/02: ASSET-CORE-04B bridge of the real registry-v2 fixture
// ---------------------------------------------------------------------------

#[test]
fn chain01_registry_fixture_bridges_successfully_via_asset_core_04b() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);

    // CRYPTO-DATA-01G added a second disabled ETH/USD row beside BTC/USD in
    // the same fixture; both must bridge successfully.
    assert_eq!(summary.total_instruments, 2, "summary={summary:?}");
    assert_eq!(summary.bridged_count, 2, "summary={summary:?}");
    assert_eq!(summary.failed_count, 0, "summary={summary:?}");
}

#[test]
fn chain02_bridged_economics_is_crypto_usd_unit_multiplier_model_only_never_trading_enabled() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let summary = bridge_instrument_registry_v2_to_economics(&registry);
    let btc_bridge = summary
        .rows
        .iter()
        .find(|r| r.symbol == "BTC/USD")
        .expect("BTC/USD row must be present");

    assert_eq!(btc_bridge.truth_state, "active", "result={btc_bridge:?}");
    let economics = btc_bridge
        .economics
        .as_ref()
        .expect("economics must be present");
    assert_eq!(economics.asset_class, "crypto");
    assert_eq!(economics.quote_currency, "USD");
    assert_eq!(economics.contract_multiplier_micros, MICROS_SCALE);
    assert!(btc_bridge.model_only, "crypto must be flagged model_only");
    assert!(!btc_bridge.trading_enabled_by_bridge);
    assert!(!btc_bridge.enabled);
    assert!(!btc_bridge.paper_trading_enabled);
    assert!(!btc_bridge.live_trading_enabled);
}

// ---------------------------------------------------------------------------
// CHAIN-03: ASSET-CORE-04A valuation using the CSV-derived mark
// ---------------------------------------------------------------------------

#[test]
fn chain03_csv_latest_close_feeds_asset_core_04a_valuation_active_with_real_notional() {
    let (mark_price_micros, economics) = latest_close_micros_and_bridged_economics();
    assert_eq!(mark_price_micros, 44_100 * MICROS_SCALE);

    // A tiny BTC quantity: 0.01 BTC.
    let signed_qty_micros = MICROS_SCALE / 100;

    let value = value_position_economics(PositionEconomicsInput {
        instrument: economics,
        signed_qty_micros,
        mark_price_micros: Some(mark_price_micros),
        account_currency: ACCOUNT_CURRENCY.to_string(),
    });

    assert_eq!(
        value.truth_state,
        InstrumentEconomicsTruthState::Active,
        "value={value:?}"
    );
    assert_eq!(value.asset_class, "crypto");
    assert_eq!(value.quote_currency, "USD");
    // 0.01 BTC * $44,100.00 * 1.0x multiplier = $441.00.
    assert_eq!(value.notional_micros, Some(441 * MICROS_SCALE as i128));
    assert_eq!(
        value.absolute_notional_micros,
        Some(441 * MICROS_SCALE as i128)
    );
}

// ---------------------------------------------------------------------------
// CHAIN-04/05/06: ASSET-CORE-04C aggregation over the valued position
// ---------------------------------------------------------------------------

#[test]
fn chain04_05_06_aggregation_is_active_with_crypto_and_usd_exposure_buckets() {
    let (mark_price_micros, economics) = latest_close_micros_and_bridged_economics();
    let signed_qty_micros = MICROS_SCALE / 100;

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
    assert_eq!(
        snapshot.truth_state,
        PortfolioEconomicsTruthState::Active,
        "snapshot={snapshot:?}"
    );
    assert_eq!(
        snapshot.nav_micros,
        Some(cash_micros + 441 * MICROS_SCALE as i128)
    );
    assert_eq!(
        snapshot.gross_exposure_micros,
        Some(441 * MICROS_SCALE as i128)
    );

    // CHAIN-05: a "crypto" asset-class exposure bucket exists with the
    // position's full notional.
    let crypto_row = snapshot
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "crypto")
        .expect("crypto asset-class exposure row must exist");
    assert_eq!(
        crypto_row.signed_notional_micros,
        441 * MICROS_SCALE as i128
    );
    assert_eq!(
        crypto_row.absolute_notional_micros,
        441 * MICROS_SCALE as i128
    );
    assert!(crypto_row.weight_bps.is_some_and(|bps| bps > 0));

    // CHAIN-06: a "USD" currency exposure bucket exists with the same totals
    // (the only position is USD-quoted).
    let usd_row = snapshot
        .currency_exposures
        .iter()
        .find(|r| r.key == "USD")
        .expect("USD currency exposure row must exist");
    assert_eq!(usd_row.signed_notional_micros, 441 * MICROS_SCALE as i128);
    assert_eq!(usd_row.absolute_notional_micros, 441 * MICROS_SCALE as i128);
}

// ---------------------------------------------------------------------------
// CHAIN-07: nothing in the chain indicates trading enablement
// ---------------------------------------------------------------------------

#[test]
fn chain07_nothing_in_the_chain_indicates_trading_enablement() {
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
