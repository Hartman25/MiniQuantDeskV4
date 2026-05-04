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
use std::collections::HashSet;
use std::path::Path;

/// One tracked instrument entry. All fields are required; no optional fields on the
/// canonical schema so parsers fail closed on malformed files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedInstrument {
    /// Globally unique identifier: `"{asset_class}:{region}:{symbol}"` (e.g. `"equity:US:AAPL"`).
    pub instrument_id: String,
    /// Primary market symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Asset class: `"equity"` | future values: `"crypto"`, `"futures"`, `"options"`, `"forex"`.
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
pub fn validate_registry(instruments: &[TrackedInstrument]) -> Result<()> {
    let mut seen_ids: HashSet<&str> = HashSet::new();
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
    }

    Ok(())
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
}
