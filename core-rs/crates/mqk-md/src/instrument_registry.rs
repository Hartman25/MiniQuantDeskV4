//! Canonical tracked-instrument registry (read-only).
//!
//! The registry file at `config/instruments/equities.json` is the single source of truth
//! for which instruments are tracked for market-data ingestion, backtesting, and GUI coverage.
//!
//! # Multi-asset design notes (not yet implemented — fields are present for future expansion)
//! - **equities**: ticker symbols, exchange/venue, regular sessions.
//! - **crypto**: 24/7, pairs or spot symbols, venue/provider mapping.
//! - **futures**: root, contract, expiry, continuous contract mapping, roll logic.
//! - **options**: underlying, expiry, strike, right/call-put, chain snapshots.
//! - **forex**: currency pair, pip precision, 24/5 sessions.
//!
//! The registry file uses `asset_class = "equity"` for all currently seeded entries.
//! Other asset classes will be added in DATA-MULTI-ASSET-MODEL-01.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One tracked instrument entry. Core fields are required; no optional fields on the
/// canonical schema so parsers fail closed on malformed files. `instrument_kind`,
/// `sector`, and `category` (ETF-REGISTRY-01) are the sole exception: they are
/// `None` for every instrument that predates ETF-REGISTRY-01 and for any plain
/// equity going forward — absent in JSON deserializes to `None` via
/// `#[serde(default)]`, so existing entries did not need to change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedInstrument {
    /// Globally unique identifier: `"{asset_class}:{region}:{symbol}"` (e.g. `"equity:US:AAPL"`).
    pub instrument_id: String,
    /// Primary market symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Asset class: `"equity"` | future values: `"crypto"`, `"futures"`, `"options"`, `"forex"`.
    ///
    /// ETFs are NOT a distinct `asset_class` value — they remain `"equity"` so
    /// `enabled_equities()` / `validate_registry()` and every existing ingestion,
    /// backtest, and GUI consumer continue to treat them exactly as before
    /// (ETF-REGISTRY-01). `instrument_kind` below carries the ETF tag instead.
    pub asset_class: String,
    /// Data provider name (e.g. `"twelvedata"`).
    pub provider: String,
    /// Symbol as expected by the provider API (may differ from `symbol` for some assets).
    pub provider_symbol: String,
    /// Primary exchange or venue (e.g. `"NASDAQ"`, `"NYSEARCA"`, `"NYSE"`).
    pub venue: String,
    /// Settlement currency ISO code (e.g. `"USD"`).
    pub currency: String,
    /// Whether this instrument is active for ingestion and backtesting.
    pub enabled: bool,
    /// Timeframe(s) to ingest/maintain (e.g. `["1D"]`).
    pub timeframes: Vec<String>,
    /// Free-form note (source, caveats, seeding origin).
    pub notes: String,
    /// Optional instrument-kind tag (ETF-REGISTRY-01), e.g. `"etf"`. `None` for
    /// plain equities. Informational only — does not affect `asset_class`,
    /// `enabled_equities()`, ingestion, or backtest behavior.
    #[serde(default)]
    pub instrument_kind: Option<String>,
    /// Optional sector/exposure-bucket tag (ETF-REGISTRY-01), e.g.
    /// `"broad_market"`, `"sector_technology"`, `"rates_duration"`. `None` for
    /// untagged instruments. This is the key consumed by
    /// `mqk_portfolio::constraints::check_sector_limits` via [`sector_map`].
    #[serde(default)]
    pub sector: Option<String>,
    /// Optional coarse category tag (ETF-REGISTRY-01), e.g. `"index_equity"`,
    /// `"sector_equity"`, `"fixed_income"`, `"commodity"`. `None` for untagged
    /// instruments. Descriptive only; no consumer reads this field yet.
    #[serde(default)]
    pub category: Option<String>,
}

/// Parse the instrument registry JSON file at `path`.
///
/// Returns all entries, both enabled and disabled, in file order.
/// The file must be a JSON array of `TrackedInstrument` objects.
pub fn load_instrument_registry(path: &Path) -> Result<Vec<TrackedInstrument>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("instrument registry read failed: {}", path.display()))?;
    let instruments: Vec<TrackedInstrument> = serde_json::from_str(&content)
        .with_context(|| format!("instrument registry parse failed: {}", path.display()))?;
    Ok(instruments)
}

/// Return all enabled equity instruments in deterministic alphabetical order by `symbol`.
///
/// This is the primary consumer entry point for ingestion and backtesting symbol lists.
pub fn enabled_equities(instruments: &[TrackedInstrument]) -> Vec<&TrackedInstrument> {
    let mut result: Vec<&TrackedInstrument> = instruments
        .iter()
        .filter(|i| i.enabled && i.asset_class == "equity")
        .collect();
    result.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    result
}

/// Return the symbols of all enabled equities in deterministic alphabetical order.
pub fn enabled_equity_symbols(instruments: &[TrackedInstrument]) -> Vec<String> {
    enabled_equities(instruments)
        .into_iter()
        .map(|i| i.symbol.clone())
        .collect()
}

/// Validate registry invariants. Returns `Ok(())` if all pass, or the first violation as `Err`.
///
/// Checks:
/// 1. All `instrument_id` values are unique.
/// 2. All `symbol` values are non-empty.
/// 3. All `asset_class` values are non-empty.
/// 4. All `provider_symbol` values are non-empty.
/// 5. No enabled equity has duplicate `provider_symbol` within the same provider.
/// 6. No duplicate `symbol` across the whole registry.
/// 7. ETF-REGISTRY-01: if present, `instrument_kind`/`sector`/`category` must not
///    be empty/whitespace-only strings; an instrument tagged `instrument_kind ==
///    "etf"` must carry both a non-empty `sector` and a non-empty `category`.
pub fn validate_registry(instruments: &[TrackedInstrument]) -> Result<()> {
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut seen_symbols: HashSet<&str> = HashSet::new();
    let mut seen_provider_symbols: HashSet<(&str, &str)> = HashSet::new();

    for inst in instruments {
        if inst.symbol.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry: empty symbol for instrument_id={}",
                inst.instrument_id
            );
        }
        if inst.asset_class.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry: empty asset_class for symbol={}",
                inst.symbol
            );
        }
        if inst.provider_symbol.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry: empty provider_symbol for symbol={}",
                inst.symbol
            );
        }

        if !seen_ids.insert(inst.instrument_id.as_str()) {
            anyhow::bail!(
                "instrument_registry: duplicate instrument_id={}",
                inst.instrument_id
            );
        }

        if !seen_symbols.insert(inst.symbol.as_str()) {
            anyhow::bail!("instrument_registry: duplicate symbol={}", inst.symbol);
        }

        if inst.enabled && inst.asset_class == "equity" {
            let key = (inst.provider.as_str(), inst.provider_symbol.as_str());
            if !seen_provider_symbols.insert(key) {
                anyhow::bail!(
                    "instrument_registry: duplicate enabled equity provider_symbol={} for provider={}",
                    inst.provider_symbol,
                    inst.provider
                );
            }
        }

        // ETF-REGISTRY-01: optional tags must not be empty-string when present.
        if let Some(kind) = &inst.instrument_kind {
            if kind.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry: empty instrument_kind for symbol={}",
                    inst.symbol
                );
            }
        }
        if let Some(sector) = &inst.sector {
            if sector.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry: empty sector for symbol={}",
                    inst.symbol
                );
            }
        }
        if let Some(category) = &inst.category {
            if category.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry: empty category for symbol={}",
                    inst.symbol
                );
            }
        }
        if inst.instrument_kind.as_deref() == Some("etf") {
            let sector_ok = inst.sector.as_deref().is_some_and(|s| !s.trim().is_empty());
            let category_ok = inst
                .category
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty());
            if !sector_ok || !category_ok {
                anyhow::bail!(
                    "instrument_registry: etf instrument symbol={} missing sector and/or category metadata",
                    inst.symbol
                );
            }
        }
    }

    Ok(())
}

/// ETF-RISK-01 bridge: build a `symbol -> sector` map from registry entries that
/// carry a `sector` tag.
///
/// Instruments with `sector == None` (every plain equity today) are omitted —
/// `mqk_portfolio::constraints::check_sector_limits` treats a symbol absent
/// from its `sector_map` argument as belonging to no sector, which is exactly
/// the correct behavior for untagged instruments. Pure; no IO.
///
/// This function only produces the map shape that `check_sector_limits`
/// expects; it does not call into `mqk-portfolio` and does not evaluate any
/// constraint itself. Wiring this map into a live, weight-aware admission gate
/// is tracked separately — see `docs/audits/multi_asset_completion_audit.md`
/// (ETF-RISK-01 closure note) for the exact blocking dependency.
pub fn sector_map(instruments: &[TrackedInstrument]) -> HashMap<String, String> {
    instruments
        .iter()
        .filter_map(|inst| {
            inst.sector
                .as_ref()
                .map(|sector| (inst.symbol.clone(), sector.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn registry_path() -> PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest_dir).join("../../../config/instruments/equities.json")
    }

    // REG-01: registry parses without error
    #[test]
    fn reg_01_registry_parses_successfully() {
        let path = registry_path();
        let instruments =
            load_instrument_registry(&path).expect("registry must parse without error");
        assert!(
            !instruments.is_empty(),
            "registry must contain at least one instrument"
        );
    }

    // REG-02: all instrument_ids are unique
    #[test]
    fn reg_02_instrument_ids_are_unique() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let mut seen = HashSet::new();
        for inst in &instruments {
            assert!(
                seen.insert(inst.instrument_id.as_str()),
                "duplicate instrument_id: {}",
                inst.instrument_id
            );
        }
    }

    // REG-03: enabled equity provider_symbols are unique per provider
    #[test]
    fn reg_03_enabled_equity_provider_symbols_unique_per_provider() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        for inst in instruments
            .iter()
            .filter(|i| i.enabled && i.asset_class == "equity")
        {
            let key = (inst.provider.as_str(), inst.provider_symbol.as_str());
            assert!(
                seen.insert(key),
                "duplicate enabled equity provider_symbol={} provider={}",
                inst.provider_symbol,
                inst.provider
            );
        }
    }

    // REG-04: all entries have non-empty required string fields
    #[test]
    fn reg_04_no_empty_required_fields() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        for inst in &instruments {
            assert!(
                !inst.symbol.trim().is_empty(),
                "empty symbol in {:?}",
                inst.instrument_id
            );
            assert!(
                !inst.asset_class.trim().is_empty(),
                "empty asset_class for {}",
                inst.symbol
            );
            assert!(
                !inst.provider_symbol.trim().is_empty(),
                "empty provider_symbol for {}",
                inst.symbol
            );
            assert!(
                !inst.venue.trim().is_empty(),
                "empty venue for {}",
                inst.symbol
            );
            assert!(
                !inst.currency.trim().is_empty(),
                "empty currency for {}",
                inst.symbol
            );
            assert!(
                !inst.provider.trim().is_empty(),
                "empty provider for {}",
                inst.symbol
            );
        }
    }

    // REG-05: all seeded entries are asset_class="equity"
    #[test]
    fn reg_05_all_seeded_entries_are_equity() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        for inst in &instruments {
            assert_eq!(
                inst.asset_class, "equity",
                "unexpected asset_class for {}: {}",
                inst.symbol, inst.asset_class
            );
        }
    }

    // REG-06: no absolute paths in registry
    #[test]
    fn reg_06_no_absolute_paths_in_registry() {
        let path = registry_path();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains(":\\"),
            "registry contains Windows absolute path"
        );
        assert!(
            !content.contains("/home/"),
            "registry contains Unix home path"
        );
        assert!(
            !content.contains("/Users/"),
            "registry contains macOS home path"
        );
    }

    // REG-07: enabled_equities returns symbols in deterministic alphabetical order
    #[test]
    fn reg_07_enabled_equities_deterministic_order() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let equities = enabled_equities(&instruments);
        for window in equities.windows(2) {
            assert!(
                window[0].symbol <= window[1].symbol,
                "enabled_equities not sorted: {} before {}",
                window[0].symbol,
                window[1].symbol
            );
        }
    }

    // REG-08: enabled equity count matches the known backfill universe (88)
    #[test]
    fn reg_08_enabled_equity_count_matches_backfill_universe() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let equities = enabled_equities(&instruments);
        assert_eq!(
            equities.len(),
            88,
            "enabled equity count must match the backfill script universe (88 symbols)"
        );
    }

    // REG-09: validate_registry returns Ok for the canonical file
    #[test]
    fn reg_09_validate_registry_passes() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        validate_registry(&instruments).expect("validate_registry must pass on canonical file");
    }

    // REG-10: validate_registry catches duplicate instrument_id
    #[test]
    fn reg_10_validate_rejects_duplicate_instrument_id() {
        let aapl = TrackedInstrument {
            instrument_id: "equity:US:AAPL".into(),
            symbol: "AAPL".into(),
            asset_class: "equity".into(),
            provider: "twelvedata".into(),
            provider_symbol: "AAPL".into(),
            venue: "NASDAQ".into(),
            currency: "USD".into(),
            enabled: true,
            timeframes: vec!["1D".into()],
            notes: "test".into(),
            instrument_kind: None,
            sector: None,
            category: None,
        };
        let spy = TrackedInstrument {
            instrument_id: "equity:US:AAPL".into(), // intentional duplicate
            symbol: "SPY".into(),
            asset_class: "equity".into(),
            provider: "twelvedata".into(),
            provider_symbol: "SPY".into(),
            venue: "NYSEARCA".into(),
            currency: "USD".into(),
            enabled: true,
            timeframes: vec!["1D".into()],
            notes: "test".into(),
            instrument_kind: None,
            sector: None,
            category: None,
        };
        let result = validate_registry(&[aapl, spy]);
        assert!(result.is_err(), "must reject duplicate instrument_id");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("duplicate instrument_id"),
            "error message must identify the violation"
        );
    }

    // REG-11: enabled_equity_symbols returns plain string list in sorted order
    #[test]
    fn reg_11_enabled_equity_symbols_sorted() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let syms = enabled_equity_symbols(&instruments);
        for window in syms.windows(2) {
            assert!(
                window[0] <= window[1],
                "symbols not sorted: {} before {}",
                window[0],
                window[1]
            );
        }
        assert!(syms.contains(&"AAPL".to_string()));
        assert!(syms.contains(&"SPY".to_string()));
    }

    // ── ETF-REGISTRY-01 ───────────────────────────────────────────────────────

    const ETF_SYMBOLS: &[&str] = &[
        "SPY", "QQQ", "IWM", "DIA", "XLK", "XLF", "XLE", "XLI", "XLP", "XLU", "TLT", "IEF", "SHY",
        "GLD",
    ];

    // REG-12: every target ETF symbol is tagged instrument_kind="etf" with a
    // non-empty sector and category, and asset_class is unchanged ("equity").
    #[test]
    fn reg_12_target_etfs_are_tagged() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        for symbol in ETF_SYMBOLS {
            let inst = instruments
                .iter()
                .find(|i| i.symbol == *symbol)
                .unwrap_or_else(|| panic!("expected ETF symbol {symbol} in registry"));
            assert_eq!(
                inst.instrument_kind.as_deref(),
                Some("etf"),
                "{symbol} must be tagged instrument_kind=etf"
            );
            assert_eq!(
                inst.asset_class, "equity",
                "{symbol} asset_class must remain 'equity' (ETF-REGISTRY-01 does not change it)"
            );
            assert!(
                inst.sector.as_deref().is_some_and(|s| !s.is_empty()),
                "{symbol} must have a non-empty sector"
            );
            assert!(
                inst.category.as_deref().is_some_and(|c| !c.is_empty()),
                "{symbol} must have a non-empty category"
            );
        }
    }

    // REG-13: legacy (non-ETF) equity symbols still parse with no ETF tags.
    #[test]
    fn reg_13_legacy_equities_have_no_etf_tags() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let aapl = instruments
            .iter()
            .find(|i| i.symbol == "AAPL")
            .expect("AAPL must exist in registry");
        assert_eq!(aapl.instrument_kind, None);
        assert_eq!(aapl.sector, None);
        assert_eq!(aapl.category, None);
        assert_eq!(aapl.asset_class, "equity");
    }

    // REG-14: tagging ETFs did not change provider_symbol/timeframes/enabled.
    #[test]
    fn reg_14_etf_provider_fields_preserved() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        for symbol in ETF_SYMBOLS {
            let inst = instruments.iter().find(|i| i.symbol == *symbol).unwrap();
            assert_eq!(inst.provider_symbol, *symbol);
            assert_eq!(inst.timeframes, vec!["1D".to_string()]);
            assert!(inst.enabled, "{symbol} must remain enabled");
            assert_eq!(inst.provider, "twelvedata");
        }
    }

    // REG-15: no duplicate symbols anywhere in the registry (includes ETFs).
    #[test]
    fn reg_15_no_duplicate_symbols() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let mut seen = HashSet::new();
        for inst in &instruments {
            assert!(
                seen.insert(inst.symbol.as_str()),
                "duplicate symbol: {}",
                inst.symbol
            );
        }
    }

    // REG-16: validate_registry rejects malformed ETF metadata (instrument_kind
    // == "etf" with no sector/category).
    #[test]
    fn reg_16_validate_rejects_incomplete_etf_metadata() {
        let malformed = TrackedInstrument {
            instrument_id: "equity:US:SPY".into(),
            symbol: "SPY".into(),
            asset_class: "equity".into(),
            provider: "twelvedata".into(),
            provider_symbol: "SPY".into(),
            venue: "NYSEARCA".into(),
            currency: "USD".into(),
            enabled: true,
            timeframes: vec!["1D".into()],
            notes: "test".into(),
            instrument_kind: Some("etf".into()),
            sector: None, // missing — must fail validation
            category: Some("index_equity".into()),
        };
        let result = validate_registry(&[malformed]);
        assert!(result.is_err(), "must reject etf with missing sector");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing sector and/or category"),
            "error message must identify the violation"
        );
    }

    // REG-17: validate_registry rejects an empty-string sector tag.
    #[test]
    fn reg_17_validate_rejects_empty_sector_string() {
        let malformed = TrackedInstrument {
            instrument_id: "equity:US:SPY".into(),
            symbol: "SPY".into(),
            asset_class: "equity".into(),
            provider: "twelvedata".into(),
            provider_symbol: "SPY".into(),
            venue: "NYSEARCA".into(),
            currency: "USD".into(),
            enabled: true,
            timeframes: vec!["1D".into()],
            notes: "test".into(),
            instrument_kind: None,
            sector: Some("   ".into()), // whitespace-only — must fail validation
            category: None,
        };
        let result = validate_registry(&[malformed]);
        assert!(result.is_err(), "must reject whitespace-only sector");
        assert!(
            result.unwrap_err().to_string().contains("empty sector"),
            "error message must identify the violation"
        );
    }

    // REG-18 (ETF-RISK-01 bridge): sector_map() produces exactly the expected
    // symbol->sector mapping for the canonical file's ETF universe, and omits
    // every untagged (plain equity) symbol.
    #[test]
    fn reg_18_sector_map_matches_expected_etf_taxonomy() {
        let instruments = load_instrument_registry(&registry_path()).unwrap();
        let map = sector_map(&instruments);

        let expected: &[(&str, &str)] = &[
            ("SPY", "broad_market"),
            ("QQQ", "broad_market"),
            ("IWM", "broad_market"),
            ("DIA", "broad_market"),
            ("XLK", "sector_technology"),
            ("XLF", "sector_financials"),
            ("XLE", "sector_energy"),
            ("XLI", "sector_industrials"),
            ("XLP", "sector_consumer_staples"),
            ("XLU", "sector_utilities"),
            ("TLT", "rates_duration"),
            ("IEF", "rates_duration"),
            ("SHY", "rates_duration"),
            ("GLD", "commodity_gold"),
        ];
        for (symbol, sector) in expected {
            assert_eq!(
                map.get(*symbol).map(|s| s.as_str()),
                Some(*sector),
                "sector_map mismatch for {symbol}"
            );
        }

        // Untagged plain equities must not appear in the map.
        assert_eq!(map.get("AAPL"), None);
        assert_eq!(map.get("MSFT"), None);

        // Map size matches exactly the tagged ETF universe — no stray entries.
        assert_eq!(map.len(), expected.len());
    }

    // REG-19 (ETF-RISK-01 bridge): sector_map() is pure and returns an empty
    // map for instruments with no sector tag.
    #[test]
    fn reg_19_sector_map_empty_when_no_tags_present() {
        let aapl = TrackedInstrument {
            instrument_id: "equity:US:AAPL".into(),
            symbol: "AAPL".into(),
            asset_class: "equity".into(),
            provider: "twelvedata".into(),
            provider_symbol: "AAPL".into(),
            venue: "NASDAQ".into(),
            currency: "USD".into(),
            enabled: true,
            timeframes: vec!["1D".into()],
            notes: "test".into(),
            instrument_kind: None,
            sector: None,
            category: None,
        };
        assert!(sector_map(&[aapl]).is_empty());
    }
}
