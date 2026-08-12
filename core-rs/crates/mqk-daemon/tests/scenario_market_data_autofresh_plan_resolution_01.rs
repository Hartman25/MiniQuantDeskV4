//! MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: pure required-universe plan
//! resolution proofs.
//!
//! These tests call
//! `mqk_daemon::state::required_market_data_autofresh::build_required_market_data_refresh_plan`
//! directly against in-memory registry fixtures — no DB, no provider/network
//! call, no daemon process. They prove the resolver/grouping/blocker logic
//! itself, independent of the HTTP surface (covered separately in
//! `scenario_market_data_autofresh_required_universe_01.rs`).

use mqk_daemon::market_data_freshness::{
    RequiredSymbolTimeframe, RequiredSymbolsResolution, SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL,
};
use mqk_daemon::state::required_market_data_autofresh::{
    build_required_market_data_refresh_plan, RequirementConfigBlocker, RequirementResolution,
};
use mqk_daemon::watchlist_intake::WatchlistIntakeOutcome;
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_md::provider_registry::ProviderConfig;

fn instrument(
    symbol: &str,
    provider: &str,
    provider_symbol: &str,
    timeframes: &[&str],
) -> TrackedInstrument {
    TrackedInstrument {
        instrument_id: format!("equity:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        provider: provider.to_string(),
        provider_symbol: provider_symbol.to_string(),
        venue: "TEST".to_string(),
        currency: "USD".to_string(),
        enabled: true,
        timeframes: timeframes.iter().map(|s| s.to_string()).collect(),
        notes: "fixture".to_string(),
        instrument_kind: None,
        sector: None,
        category: None,
    }
}

fn provider(
    id: &str,
    enabled: bool,
    asset_classes: &[&str],
    timeframes: &[&str],
) -> ProviderConfig {
    ProviderConfig {
        provider_id: id.to_string(),
        display_name: id.to_string(),
        asset_classes: asset_classes.iter().map(|s| s.to_string()).collect(),
        free_tier_available: true,
        api_key_required: false,
        credential_env_vars: Vec::new(),
        rate_limit_notes: "test".to_string(),
        supported_timeframes: timeframes.iter().map(|s| s.to_string()).collect(),
        historical_depth_notes: "test".to_string(),
        realtime_support_notes: "test".to_string(),
        licensing_notes: "test".to_string(),
        implementation_status: "implemented_equity_provider".to_string(),
        enabled,
        verification_status: "repo_implemented_official_limits_unverified".to_string(),
        docs_url: String::new(),
    }
}

fn resolution(required: Vec<RequiredSymbolTimeframe>) -> RequiredSymbolsResolution {
    RequiredSymbolsResolution {
        required,
        source: SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL,
        watchlist_outcome: WatchlistIntakeOutcome::NotConfigured,
    }
}

fn req(symbol: &str, timeframe: &str) -> RequiredSymbolTimeframe {
    RequiredSymbolTimeframe {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
    }
}

const MARKET_DATE: (i64, i64, i64) = (2026, 8, 12);

/// §11: the current accepted lane (AAPL/5m, registry provider=alpaca) must
/// produce exactly one requirement and one provider group -- no TwelveData
/// group, no SPY fallback, no full-registry expansion.
#[test]
fn plan_current_lane_resolves_aapl_5m_alpaca_exactly() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["5m"])];
    let providers = vec![
        provider("alpaca", true, &["equity", "etf"], &["1D", "1m", "5m"]),
        provider("twelvedata", true, &["equity", "etf"], &["1D", "1m", "5m"]),
    ];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );

    assert_eq!(plan.resolutions.len(), 1);
    assert!(plan.resolutions[0].is_resolved());
    let resolved: Vec<_> = plan.resolved().collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].symbol, "AAPL");
    assert_eq!(resolved[0].timeframe, "5m");
    assert_eq!(resolved[0].provider_id, "alpaca");
    assert_eq!(resolved[0].provider_symbol, "AAPL");

    assert_eq!(plan.groups.len(), 1, "exactly one provider/timeframe group");
    assert_eq!(plan.groups[0].provider_id, "alpaca");
    assert_eq!(plan.groups[0].timeframe, "5m");
    assert_eq!(plan.groups[0].symbols, vec!["AAPL".to_string()]);
    assert!(
        !plan.groups.iter().any(|g| g.provider_id == "twelvedata"),
        "no TwelveData group for the current AAPL-only lane"
    );
}

/// §13: a hardcoded/default-provider implementation would resolve AAPL to
/// twelvedata (first in the registry, or a fixed default). This fixture
/// deliberately lists twelvedata first and alpaca second to structurally
/// prove the resolver reads the *instrument's own* registered provider, not
/// registry order or a fixed default.
#[test]
fn plan_never_defaults_to_first_registered_or_hardcoded_provider() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["5m"])];
    let providers = vec![
        // twelvedata listed FIRST -- a first-provider-wins bug would pick this.
        provider("twelvedata", true, &["equity", "etf"], &["1D", "1m", "5m"]),
        provider("alpaca", true, &["equity", "etf"], &["1D", "1m", "5m"]),
    ];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );

    let resolved: Vec<_> = plan.resolved().collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        resolved[0].provider_id, "alpaca",
        "must resolve the instrument's own registered provider, never registry order"
    );
}

/// §12: mixed-provider proof -- AAPL(alpaca)/MSFT(twelvedata), same
/// timeframe, must produce two distinct provider groups. Never one provider
/// picked for both.
#[test]
fn plan_mixed_provider_produces_two_distinct_groups() {
    let instruments = vec![
        instrument("AAPL", "alpaca", "AAPL", &["5m"]),
        instrument("MSFT", "twelvedata", "MSFT", &["5m"]),
    ];
    let providers = vec![
        provider("alpaca", true, &["equity", "etf"], &["1D", "1m", "5m"]),
        provider("twelvedata", true, &["equity", "etf"], &["1D", "1m", "5m"]),
    ];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m"), req("MSFT", "5m")]),
        MARKET_DATE,
    );

    assert_eq!(plan.resolutions.len(), 2);
    assert_eq!(
        plan.groups.len(),
        2,
        "two distinct provider/timeframe groups"
    );
    let alpaca_group = plan
        .groups
        .iter()
        .find(|g| g.provider_id == "alpaca")
        .expect("alpaca group present");
    let twelvedata_group = plan
        .groups
        .iter()
        .find(|g| g.provider_id == "twelvedata")
        .expect("twelvedata group present");
    assert_eq!(alpaca_group.symbols, vec!["AAPL".to_string()]);
    assert_eq!(twelvedata_group.symbols, vec!["MSFT".to_string()]);
}

/// §36 test 2 / §41 test 12: multi-symbol same-timeframe/same-provider proof
/// -- every required symbol must appear in the plan; a structurally
/// AAPL-only implementation would fail this (only 1 requirement / 1 group
/// member instead of 3).
#[test]
fn plan_multi_symbol_same_provider_includes_every_required_symbol() {
    let instruments = vec![
        instrument("AAPL", "alpaca", "AAPL", &["5m"]),
        instrument("MSFT", "alpaca", "MSFT", &["5m"]),
        instrument("NVDA", "alpaca", "NVDA", &["5m"]),
    ];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![
            req("AAPL", "5m"),
            req("MSFT", "5m"),
            req("NVDA", "5m"),
        ]),
        MARKET_DATE,
    );

    assert_eq!(plan.resolutions.len(), 3);
    assert!(plan.resolutions.iter().all(|r| r.is_resolved()));
    assert_eq!(plan.groups.len(), 1);
    let mut symbols = plan.groups[0].symbols.clone();
    symbols.sort();
    assert_eq!(
        symbols,
        vec!["AAPL".to_string(), "MSFT".to_string(), "NVDA".to_string()],
        "no required symbol may silently disappear from the group"
    );
}

/// §36 test 5: missing-symbol negative control -- MSFT absent from the
/// instrument registry must block only MSFT, never shrink the required
/// universe down to AAPL alone.
#[test]
fn plan_missing_instrument_blocks_only_that_symbol() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["5m"])];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m"), req("MSFT", "5m")]),
        MARKET_DATE,
    );

    assert_eq!(
        plan.resolutions.len(),
        2,
        "MSFT must still appear as a blocked requirement"
    );
    let aapl = plan
        .resolutions
        .iter()
        .find(|r| r.symbol() == "AAPL")
        .expect("AAPL present");
    assert!(aapl.is_resolved());
    let msft = plan
        .resolutions
        .iter()
        .find(|r| r.symbol() == "MSFT")
        .expect("MSFT present as a blocked requirement, not silently dropped");
    match msft {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "instrument_registry_invalid");
        }
        RequirementResolution::Resolved(_) => {
            panic!("MSFT must not resolve without a registry entry")
        }
    }
}

/// §36 test 6: provider-disabled negative control -- the authoritative
/// provider is disabled; the requirement must block with `provider_disabled`
/// and never appear in any group (no provider call would ever be attempted).
#[test]
fn plan_disabled_provider_blocks_with_typed_reason() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["5m"])];
    let providers = vec![provider(
        "alpaca",
        false,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );

    assert_eq!(
        plan.groups.len(),
        0,
        "a disabled provider must never form a poll group"
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "provider_disabled");
            assert!(matches!(
                b.blocker,
                RequirementConfigBlocker::ProviderDisabled { .. }
            ));
        }
        RequirementResolution::Resolved(_) => {
            panic!("must not resolve against a disabled provider")
        }
    }
}

/// §36 test 7: unsupported-timeframe negative control -- the instrument
/// authorizes 5m only; a 1m requirement must block, never silently
/// substitute 5m/1D for the requested timeframe.
#[test]
fn plan_unauthorized_timeframe_blocks_without_substitution() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["5m"])];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "1m")]),
        MARKET_DATE,
    );

    assert_eq!(plan.groups.len(), 0);
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "unsupported_timeframe");
            assert_eq!(
                b.timeframe, "1m",
                "the requested timeframe is reported, never silently changed"
            );
        }
        RequirementResolution::Resolved(r) => panic!(
            "must not silently substitute an authorized timeframe for the requested one \
             (resolved as {}/{})",
            r.symbol, r.timeframe
        ),
    }
}

/// A provider that also does not declare the requested timeframe as
/// supported (registry-declared capability, independent of the instrument's
/// own `timeframes` list) must block the same way.
#[test]
fn plan_provider_capability_timeframe_mismatch_blocks() {
    let instruments = vec![instrument("AAPL", "alpaca", "AAPL", &["1D"])];
    // alpaca in this fixture only declares 1D support -- AAPL requires 1D,
    // which the instrument authorizes, but this proves the provider-level
    // supported_timeframes check is independently enforced.
    let providers = vec![provider("alpaca", true, &["equity", "etf"], &["5m"])];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "1D")]),
        MARKET_DATE,
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "unsupported_timeframe");
        }
        RequirementResolution::Resolved(_) => {
            panic!("provider registry's own supported_timeframes must be enforced")
        }
    }
}

/// Provider registered but not declaring `equity` asset-class support.
#[test]
fn plan_provider_asset_class_mismatch_blocks() {
    let instruments = vec![instrument("AAPL", "cryptoonly", "AAPL", &["5m"])];
    let providers = vec![provider("cryptoonly", true, &["crypto"], &["5m"])];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "provider_capability_mismatch");
        }
        RequirementResolution::Resolved(_) => {
            panic!("a provider without declared equity support must not be selected")
        }
    }
}

/// Blank canonical `provider_symbol` blocks with `provider_symbol_mismatch`.
#[test]
fn plan_blank_provider_symbol_blocks() {
    let instruments = vec![instrument("AAPL", "alpaca", "", &["5m"])];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "provider_symbol_mismatch");
        }
        RequirementResolution::Resolved(_) => panic!("blank provider_symbol must block"),
    }
}

/// Instrument registered for a non-equity asset class is out of this
/// patch's Paper equity/ETF scope and must block, not silently trade.
#[test]
fn plan_non_equity_asset_class_blocks() {
    let mut inst = instrument("BTCUSD", "alpaca", "BTCUSD", &["5m"]);
    inst.asset_class = "crypto".to_string();
    let instruments = vec![inst];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "crypto"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("BTCUSD", "5m")]),
        MARKET_DATE,
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "instrument_registry_invalid");
        }
        RequirementResolution::Resolved(_) => panic!("non-equity asset_class must not resolve"),
    }
}

/// A disabled instrument blocks even though its provider is enabled.
#[test]
fn plan_disabled_instrument_blocks() {
    let mut inst = instrument("AAPL", "alpaca", "AAPL", &["5m"]);
    inst.enabled = false;
    let instruments = vec![inst];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(vec![req("AAPL", "5m")]),
        MARKET_DATE,
    );
    match &plan.resolutions[0] {
        RequirementResolution::Blocked(b) => {
            assert_eq!(b.blocker.reason_code(), "instrument_registry_invalid");
        }
        RequirementResolution::Resolved(_) => panic!("a disabled instrument must not resolve"),
    }
}

/// Empty required universe (nothing configured) resolves to an empty plan --
/// never expands to the full instrument registry.
#[test]
fn plan_empty_required_universe_never_expands_to_full_registry() {
    let instruments = vec![
        instrument("AAPL", "alpaca", "AAPL", &["5m"]),
        instrument("MSFT", "alpaca", "MSFT", &["5m"]),
        instrument("SPY", "alpaca", "SPY", &["5m"]),
    ];
    let providers = vec![provider(
        "alpaca",
        true,
        &["equity", "etf"],
        &["1D", "1m", "5m"],
    )];
    let plan = build_required_market_data_refresh_plan(
        &instruments,
        &providers,
        &resolution(Vec::new()),
        MARKET_DATE,
    );
    assert!(plan.resolutions.is_empty());
    assert!(plan.groups.is_empty());
}
