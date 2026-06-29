//! CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01: registry-v2 fixture + local CSV proof.
//!
//! Proves two independent, no-network facts about the committed fixtures this
//! patch adds:
//!
//! 1. `config/instruments/instruments_v2.crypto_local_marks.example.json` is a
//!    real (non-`_TEST`-suffixed) registry-v2 entry for `BTC/USD` that loads,
//!    validates, and stays disabled/non-tradable in every honesty flag.
//! 2. `core-rs/crates/mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv` parses
//!    through the *existing* `mqk_md::ingest_csv` parser (case-insensitive
//!    columns, decimal-string prices, no floats) and preserves the chosen
//!    `BTC/USD` symbol exactly.
//!
//! Neither proof touches the network, a broker, a provider, the DB, or any
//! trading/runtime/risk path. This module has zero production callers and
//! enables nothing.
//!
//! # Proof matrix
//!
//! | Test       | What it proves                                                          |
//! |------------|--------------------------------------------------------------------------|
//! | REG-01     | Fixture file loads via `load_instrument_registry_v2`                   |
//! | REG-02     | Fixture validates via `validate_registry_v2`                           |
//! | REG-03     | Exactly one instrument, and it is `BTC/USD`                            |
//! | REG-04     | `asset_class == "crypto"`                                              |
//! | REG-05     | `contract == CryptoPair { base: "BTC", quote: "USD" }`                 |
//! | REG-06     | `enabled`/`paper_trading_enabled`/`live_trading_enabled` all `false`    |
//! | REG-07     | `allow_enabled_non_equity_for_testing` is `false` (absent in JSON)      |
//! | REG-08     | `provider_symbols.local_csv == "BTC/USD"`                              |
//! | REG-09     | `quote_currency == Some("USD")`, `currency == "USD"` (no FX needed)    |
//! | REG-10     | No future/option/forex rows are introduced by this fixture              |
//! | CSV-01     | CSV fixture parses with zero errors via `ingest_csv::parse_csv_file`   |
//! | CSV-02     | All rows carry the exact symbol `"BTC/USD"` (slash survives intact)    |
//! | CSV-03     | All rows carry timeframe `"1D"`                                        |
//! | CSV-04     | All rows are `is_complete == true`                                     |
//! | CSV-05     | Row count and `end_ts` ordering are deterministic                      |
//! | CSV-06     | Latest completed close is deterministic (`"44100.00"`)                 |
//! | CSV-07     | Decimal price strings are preserved exactly (no float rounding)        |
//! | NET-01     | Nothing in this file performs any provider/network/broker call         |

use std::path::PathBuf;

use mqk_md::ingest_csv::parse_csv_file;
use mqk_md::instrument_registry_v2::{
    load_instrument_registry_v2, validate_registry_v2, ContractDefinitionV2,
};

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn csv_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("tests/fixtures/crypto_btcusd_1d_local.csv")
}

const CSV_TIMEFRAME: &str = "1D";

// ---------------------------------------------------------------------------
// REG: registry-v2 fixture loads, validates, and stays disabled/non-tradable
// ---------------------------------------------------------------------------

#[test]
fn reg01_fixture_loads_via_load_instrument_registry_v2() {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks fixture must load");
    assert_eq!(registry.schema_version, 1);
}

#[test]
fn reg02_fixture_validates_via_validate_registry_v2() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    validate_registry_v2(&registry).expect("crypto local-marks fixture must validate");
}

#[test]
fn reg03_fixture_has_exactly_one_btc_usd_instrument() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    assert_eq!(registry.instruments.len(), 1);
    assert_eq!(registry.instruments[0].symbol, "BTC/USD");
}

#[test]
fn reg04_btc_usd_asset_class_is_crypto() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert_eq!(btc.asset_class, "crypto");
}

#[test]
fn reg05_btc_usd_contract_is_crypto_pair_btc_usd() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert_eq!(
        btc.contract,
        Some(ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        })
    );
}

#[test]
fn reg06_btc_usd_enabled_paper_and_live_all_false() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert!(!btc.enabled, "fixture must stay disabled");
    assert!(!btc.paper_trading_enabled);
    assert!(!btc.live_trading_enabled);
}

#[test]
fn reg07_allow_enabled_non_equity_for_testing_is_false() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert!(
        !btc.allow_enabled_non_equity_for_testing,
        "real fixture must never set the test-only escape hatch"
    );
}

#[test]
fn reg08_provider_symbols_local_csv_matches_canonical_symbol() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert_eq!(
        btc.provider_symbols.get("local_csv").map(String::as_str),
        Some("BTC/USD")
    );
}

#[test]
fn reg09_quote_currency_and_currency_are_both_usd_no_fx_needed() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let btc = &registry.instruments[0];
    assert_eq!(btc.currency, "USD");
    assert_eq!(btc.quote_currency.as_deref(), Some("USD"));
}

#[test]
fn reg10_fixture_introduces_no_future_option_or_forex_rows() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    assert!(registry
        .instruments
        .iter()
        .all(|i| i.asset_class == "crypto"));
    assert!(!registry
        .instruments
        .iter()
        .any(|i| matches!(i.asset_class.as_str(), "future" | "option" | "forex")));
}

// ---------------------------------------------------------------------------
// CSV: local CSV fixture parses through the existing ingest_csv parser
// ---------------------------------------------------------------------------

#[test]
fn csv01_fixture_parses_with_zero_errors() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME)
        .expect("crypto CSV fixture must parse via the existing ingest_csv parser");
    assert_eq!(bars.len(), 3, "fixture must contain exactly 3 daily bars");
}

#[test]
fn csv02_all_rows_carry_exact_symbol_with_slash_intact() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    for bar in &bars {
        assert_eq!(
            bar.symbol, "BTC/USD",
            "slash-delimited symbol must survive the parser unmodified"
        );
    }
}

#[test]
fn csv03_all_rows_carry_timeframe_1d() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    for bar in &bars {
        assert_eq!(bar.timeframe, "1D");
    }
}

#[test]
fn csv04_all_rows_are_complete() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    for bar in &bars {
        assert!(bar.is_complete, "every fixture row must be a closed bar");
    }
}

#[test]
fn csv05_row_count_and_end_ts_ordering_are_deterministic() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    let end_ts_values: Vec<i64> = bars.iter().map(|b| b.end_ts).collect();
    assert_eq!(
        end_ts_values,
        vec![1_767_312_000, 1_767_398_400, 1_767_484_800]
    );
}

#[test]
fn csv06_latest_completed_close_is_deterministic() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    let latest = bars
        .iter()
        .max_by_key(|b| b.end_ts)
        .expect("fixture must have at least one bar");
    assert_eq!(latest.end_ts, 1_767_484_800);
    assert_eq!(latest.close, "44100.00");
}

#[test]
fn csv07_decimal_price_strings_preserved_exactly_no_floats() {
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    let first = &bars[0];
    assert_eq!(first.open, "42000.00");
    assert_eq!(first.high, "43000.00");
    assert_eq!(first.low, "41000.00");
    assert_eq!(first.close, "42500.00");
    assert_eq!(first.volume, 1_234_567);
}

// ---------------------------------------------------------------------------
// NET: no provider/network call anywhere in this proof
// ---------------------------------------------------------------------------

#[test]
fn net01_proof_is_purely_local_file_io_no_provider_or_network_call() {
    // This test file calls exactly two functions that touch the filesystem
    // (`load_instrument_registry_v2`, `parse_csv_file`), both reading
    // committed local fixtures. Neither `mqk_md::provider`,
    // `mqk_md::provider_registry`, `mqk_md::alpaca_provider`, nor
    // `mqk_md::TwelveDataHistoricalProvider` is referenced anywhere in this
    // file -- there is no network-capable symbol in scope to call.
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME).unwrap();
    assert!(!registry.instruments.is_empty());
    assert!(!bars.is_empty());
}
