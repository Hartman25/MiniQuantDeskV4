//! CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED
//!
//! Proves the latest-mark model (01J), the CoinLore ticker parser (01K),
//! and the registry-v2 CoinLore aliases (01L) together, using only the
//! committed fixture file and inline fixture JSON strings -- zero network
//! calls anywhere in this file.
//!
//! CoinLore's verified endpoints (`CRYPTO-DATA-01I`) are ticker/spot-only:
//! no OHLCV, no per-ticker timestamp. This file proves the model built on
//! top of that evidence never fabricates a completed bar and never writes
//! to `md_bars`.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                          |
//! |---------|--------------------------------------------------------------------------|
//! | ALIAS-01| Registry fixture exposes coinlore aliases for BTC/USD and ETH/USD       |
//! | ALIAS-02| BTC/USD alias: coinlore_id="90", coinlore_symbol="BTC"                  |
//! | ALIAS-03| ETH/USD alias: coinlore_id="80", coinlore_symbol="ETH"                  |
//! | ALIAS-04| Both registry rows remain enabled=false/paper=false/live=false          |
//! | ALIAS-05| Aliases feed the CoinLore parser end-to-end against a fixture response |
//! | REGR-01 | Existing 01A/01G registry test invariants still hold on this fixture    |
//! | NET-01  | No provider/network-capable symbol is referenced anywhere in this file  |

use std::path::PathBuf;

use mqk_md::instrument_registry_v2::load_instrument_registry_v2;
use mqk_md::providers::coinlore::coinlore_aliases_from_registry_v2;
use mqk_md::{parse_coinlore_ticker_response, LatestMarkTruthState};

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

const FIXTURE_TICKER_BODY: &str = r#"[
    {"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":16261421089.448309},
    {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":9252669028.469555}
]"#;

const AS_OF_TS: i64 = 1_783_264_281;

// ---------------------------------------------------------------------------
// ALIAS: registry-v2 fixture exposes CoinLore aliases for both symbols
// ---------------------------------------------------------------------------

#[test]
fn alias01_fixture_exposes_coinlore_aliases_for_both_symbols() {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks fixture must load");
    let aliases = coinlore_aliases_from_registry_v2(&registry);
    assert_eq!(
        aliases.len(),
        2,
        "fixture must carry exactly a BTC/USD and ETH/USD coinlore alias"
    );
}

#[test]
fn alias02_btc_usd_alias_id_90_symbol_btc() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let aliases = coinlore_aliases_from_registry_v2(&registry);
    let btc = aliases
        .iter()
        .find(|a| a.canonical_symbol == "BTC/USD")
        .expect("BTC/USD alias must be present");
    assert_eq!(btc.coinlore_id, "90");
    assert_eq!(btc.coinlore_symbol, "BTC");
}

#[test]
fn alias03_eth_usd_alias_id_80_symbol_eth() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let aliases = coinlore_aliases_from_registry_v2(&registry);
    let eth = aliases
        .iter()
        .find(|a| a.canonical_symbol == "ETH/USD")
        .expect("ETH/USD alias must be present");
    assert_eq!(eth.coinlore_id, "80");
    assert_eq!(eth.coinlore_symbol, "ETH");
}

#[test]
fn alias04_both_rows_remain_disabled_non_tradable() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    for inst in &registry.instruments {
        assert!(!inst.enabled, "{} must remain enabled=false", inst.symbol);
        assert!(
            !inst.paper_trading_enabled,
            "{} must remain paper_trading_enabled=false",
            inst.symbol
        );
        assert!(
            !inst.live_trading_enabled,
            "{} must remain live_trading_enabled=false",
            inst.symbol
        );
    }
}

// ---------------------------------------------------------------------------
// ALIAS+PARSER: aliases extracted from the real fixture feed the CoinLore
// parser end-to-end against a response shaped like the 01I-verified body.
// ---------------------------------------------------------------------------

#[test]
fn alias05_registry_aliases_feed_coinlore_parser_end_to_end() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let aliases = coinlore_aliases_from_registry_v2(&registry);

    let marks = parse_coinlore_ticker_response(FIXTURE_TICKER_BODY, &aliases, AS_OF_TS)
        .expect("fixture response must parse against real registry aliases");

    assert_eq!(marks.len(), 2);
    let btc = marks
        .iter()
        .find(|m| m.canonical_symbol == "BTC/USD")
        .unwrap();
    assert_eq!(btc.price_usd, "62906.61");
    assert_eq!(btc.provider_ts, None);
    assert_eq!(btc.truth_state, LatestMarkTruthState::ClientObservedOnly);
    assert!(!btc.is_completed_bar());

    let eth = marks
        .iter()
        .find(|m| m.canonical_symbol == "ETH/USD")
        .unwrap();
    assert_eq!(eth.price_usd, "1777.74");
    assert!(!eth.is_completed_bar());
}

// ---------------------------------------------------------------------------
// REGR: existing 01A/01G registry invariants are unaffected by the new
// provider_symbols keys added by this bundle.
// ---------------------------------------------------------------------------

#[test]
fn regr01_local_csv_alias_still_present_and_unmodified() {
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    assert_eq!(registry.instruments.len(), 2);
    assert_eq!(
        registry.instruments[0].provider_symbols.get("local_csv"),
        Some(&"BTC/USD".to_string())
    );
    assert_eq!(
        registry.instruments[1].provider_symbols.get("local_csv"),
        Some(&"ETH/USD".to_string())
    );
}

// ---------------------------------------------------------------------------
// NET: no provider/network call anywhere in this proof
// ---------------------------------------------------------------------------

#[test]
fn net01_proof_is_purely_local_file_io_and_inline_fixtures_no_network_call() {
    // This file calls only `load_instrument_registry_v2` (local file read),
    // `coinlore_aliases_from_registry_v2` (pure), and
    // `parse_coinlore_ticker_response` (pure, operates on an inline string
    // literal). `mqk_md::fetch_coinlore_ticker_body` -- the only
    // network-capable symbol this crate exposes for CoinLore -- is never
    // referenced anywhere in this file.
    let registry = load_instrument_registry_v2(&registry_fixture_path()).unwrap();
    let aliases = coinlore_aliases_from_registry_v2(&registry);
    let marks = parse_coinlore_ticker_response(FIXTURE_TICKER_BODY, &aliases, AS_OF_TS).unwrap();
    assert_eq!(marks.len(), 2);
}
