//! CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED
//! scenario proof: fixture parser -> completion semantics -> volume
//! semantics -> disabled-by-default provider config/factory, all exercised
//! through `mqk-md`'s public API and committed fixture files (no live
//! network, no DB).

use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("tests/fixtures")
        .join(name)
}

fn providers_registry_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../../config/providers/providers.json")
}

fn instruments_registry_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

const KRAKEN_1D_INTERVAL_SECONDS: i64 = 86_400;

// SC-01: the committed BTC fixture parses end-to-end from disk into
// completed ProviderBars, with the forming candle excluded and the exact
// live-verified values from CRYPTO-DATA-01S-T preserved.
#[test]
fn sc01_btc_fixture_end_to_end_from_disk() {
    let body = std::fs::read_to_string(fixture_path("kraken_ohlc_xbtusd_1d.json"))
        .expect("BTC fixture must exist and be readable");

    let parsed = mqk_md::parse_kraken_ohlc_response(&body, "XBTUSD", KRAKEN_1D_INTERVAL_SECONDS)
        .expect("BTC fixture must parse");

    assert_eq!(parsed.provider_pair_key, "XXBTZUSD");
    assert_eq!(parsed.last, 1_783_123_200);
    assert_eq!(parsed.bars.len(), 3, "2 committed + 1 forming");
    assert_eq!(parsed.completed_bars().len(), 2);
    assert_eq!(parsed.forming_bars().len(), 1);

    let bars = parsed
        .completed_provider_bars("BTC/USD", "1D")
        .expect("completed bars must convert");
    assert_eq!(bars.len(), 2);
    assert!(bars.iter().all(|b| b.is_complete));
    assert!(bars.iter().all(|b| b.symbol == "BTC/USD"));
    assert!(bars.iter().all(|b| b.timeframe == "1D"));

    let latest = bars.iter().max_by_key(|b| b.end_ts).unwrap();
    assert_eq!(
        latest.end_ts, 1_783_209_600,
        "end_ts = row.time + interval_seconds"
    );
    assert_eq!(latest.close, "63085.8");
    assert_eq!(
        latest.volume, 131_715_941_434,
        "BTC volume 1317.15941434 scaled by 1e8"
    );
}

// SC-02: the committed ETH fixture parses end-to-end from disk with the
// same semantics, proving the adapter is not hardcoded to one symbol.
#[test]
fn sc02_eth_fixture_end_to_end_from_disk() {
    let body = std::fs::read_to_string(fixture_path("kraken_ohlc_ethusd_1d.json"))
        .expect("ETH fixture must exist and be readable");

    let parsed = mqk_md::parse_kraken_ohlc_response(&body, "ETHUSD", KRAKEN_1D_INTERVAL_SECONDS)
        .expect("ETH fixture must parse");

    assert_eq!(parsed.provider_pair_key, "XETHZUSD");
    assert_eq!(parsed.completed_bars().len(), 2);
    assert_eq!(parsed.forming_bars().len(), 1);

    let bars = parsed
        .completed_provider_bars("ETH/USD", "1D")
        .expect("completed bars must convert");
    let latest = bars.iter().max_by_key(|b| b.end_ts).unwrap();
    assert_eq!(latest.end_ts, 1_783_209_600);
    assert_eq!(latest.close, "1778.72");
    assert_eq!(
        latest.volume, 1_558_801_759_742,
        "ETH volume 15588.01759742 scaled by 1e8"
    );
}

// SC-03: the forming candle's end_ts never appears among completed
// ProviderBars for either fixture -- no forming-candle ingestion.
#[test]
fn sc03_forming_candle_never_appears_in_completed_bars() {
    for (file, query_pair, symbol) in [
        ("kraken_ohlc_xbtusd_1d.json", "XBTUSD", "BTC/USD"),
        ("kraken_ohlc_ethusd_1d.json", "ETHUSD", "ETH/USD"),
    ] {
        let body = std::fs::read_to_string(fixture_path(file)).unwrap();
        let parsed =
            mqk_md::parse_kraken_ohlc_response(&body, query_pair, KRAKEN_1D_INTERVAL_SECONDS)
                .unwrap();
        let forming_end_ts: Vec<i64> = parsed.forming_bars().iter().map(|b| b.end_ts).collect();
        assert_eq!(
            forming_end_ts.len(),
            1,
            "fixture must carry exactly 1 forming row"
        );

        let bars = parsed.completed_provider_bars(symbol, "1D").unwrap();
        for bar in &bars {
            assert!(
                !forming_end_ts.contains(&bar.end_ts),
                "forming candle end_ts={:?} must never appear in completed ProviderBars for {symbol}",
                forming_end_ts
            );
        }
    }
}

// SC-04: providers.json registers a kraken entry that is disabled by
// default, declares crypto support, and does not claim
// repo_implemented_official_limits_unverified (matching every other
// disabled candidate's convention).
#[test]
fn sc04_providers_json_kraken_entry_is_disabled_by_default() {
    let providers = mqk_md::provider_registry::load_provider_registry(&providers_registry_path())
        .expect("provider registry must parse");
    let kraken = mqk_md::provider_registry::find_provider(&providers, "kraken")
        .expect("kraken must be registered in providers.json");

    assert!(!kraken.enabled, "kraken must be disabled by default");
    assert!(kraken.supports_asset_class("crypto"));
    assert!(!kraken.api_key_required);
    assert!(kraken.credential_env_vars.is_empty());
    assert_eq!(
        kraken.verification_status,
        "fixture_parser_proven_live_network_not_exercised_by_this_patch"
    );
}

// SC-05: building the kraken provider from the real (disabled) registry
// config fails closed with DisabledProvider -- no network call is possible
// through the default factory path.
#[test]
fn sc05_kraken_factory_disabled_by_default_from_real_registry() {
    let providers = mqk_md::provider_registry::load_provider_registry(&providers_registry_path())
        .expect("provider registry must parse");

    let err = match mqk_md::build_market_data_provider("kraken", &providers, |_| None) {
        Ok(_) => panic!("disabled kraken provider must fail to build"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        mqk_md::ProviderFactoryError::DisabledProvider {
            provider_id: "kraken".to_string()
        }
    );
}

// SC-06: the registry-v2 instrument fixture carries disabled kraken_pair
// aliases for both BTC/USD and ETH/USD, matching the live-verified query
// pairs, without enabling the instruments for trading.
#[test]
fn sc06_registry_v2_kraken_aliases_present_and_disabled() {
    let registry =
        mqk_md::instrument_registry_v2::load_instrument_registry_v2(&instruments_registry_path())
            .expect("registry-v2 fixture must load");

    let aliases = mqk_md::kraken_aliases_from_registry_v2(&registry);
    assert_eq!(aliases.len(), 2);

    let btc = aliases
        .iter()
        .find(|a| a.canonical_symbol == "BTC/USD")
        .expect("BTC/USD kraken alias must be present");
    assert_eq!(btc.kraken_pair, "XBTUSD");

    let eth = aliases
        .iter()
        .find(|a| a.canonical_symbol == "ETH/USD")
        .expect("ETH/USD kraken alias must be present");
    assert_eq!(eth.kraken_pair, "ETHUSD");

    for inst in &registry.instruments {
        if inst.symbol == "BTC/USD" || inst.symbol == "ETH/USD" {
            assert!(!inst.enabled, "{} must remain disabled", inst.symbol);
            assert!(!inst.paper_trading_enabled);
            assert!(!inst.live_trading_enabled);
        }
    }
}

// SC-07: HistoricalProvider::fetch_bars against a mocked Kraken response
// (httpmock, no live network) returns only completed bars with correctly
// scaled volume -- end-to-end adapter proof.
#[tokio::test]
async fn sc07_historical_provider_adapter_end_to_end_via_mock() {
    use httpmock::prelude::*;
    use mqk_md::HistoricalProvider;

    let body = std::fs::read_to_string(fixture_path("kraken_ohlc_xbtusd_1d.json")).unwrap();

    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/0/public/OHLC");
        then.status(200).body(&body);
    });

    let provider = mqk_md::KrakenHistoricalProvider::new_with_base_url(server.base_url());
    let req = mqk_md::FetchBarsRequest {
        symbols: vec!["BTC/USD".to_string()],
        timeframe: mqk_md::Timeframe::D1,
        start: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
        end: chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap(),
    };

    let bars = provider
        .fetch_bars(req)
        .await
        .expect("fetch_bars must succeed against mock");
    assert_eq!(bars.len(), 2);
    assert!(bars.iter().all(|b| b.is_complete));
    let latest = bars.iter().max_by_key(|b| b.end_ts).unwrap();
    assert_eq!(latest.volume, 131_715_941_434);
}
