//! Instrument registry v2 — additive schema, loader, and validator (ASSET-CORE-01B).
//!
//! This module is a **model + loader seam, not a production cutover**. It exists
//! so a future unified instrument registry (futures/options/crypto/forex) has a
//! real schema to build on, without changing any current trading behavior:
//!
//! - The canonical v1 registry (`super::instrument_registry`, `TrackedInstrument`,
//!   `config/instruments/equities.json`) remains the only registry any daemon,
//!   CLI, ingestion, backtest, or GUI code path reads.
//! - Nothing in this module is wired into any consumer. `InstrumentRegistryV2` is
//!   parsed and validated only by the tests in this file.
//! - Non-equity asset classes are representable here but [`validate_registry_v2`]
//!   fail-closed rejects `enabled = true` for any non-equity instrument unless the
//!   explicit, test-only [`InstrumentDefinitionV2::allow_enabled_non_equity_for_testing`]
//!   flag is set — there is no production enablement path through this schema.
//!
//! # Relationship to other asset-class types (ASSET-CORE-01A carry-forward)
//!
//! This module does **not** depend on `mqk_schemas::AssetClass`/`Instrument`/
//! `ContractSpec` (the live execution-path types checked by
//! `mqk_execution::gateway::BrokerGateway::submit_with_context`) and `mqk-md`'s
//! `Cargo.toml` gains no new dependency edge in this patch — consistent with
//! ASSET-CORE-01A's Option B precedent. `asset_class` here is a plain `String`
//! using the same canonical singular vocabulary
//! (`mqk_md::provider::provider_asset_class_trading_class`,
//! `mqk_schemas::AssetClass`, `mqk-runtime`'s `validated_asset_class`) already
//! agree on: `"equity"`, `"option"`, `"future"`, `"crypto"`, `"forex"`. ETF is,
//! again deliberately, not a distinct `asset_class` — it stays `"equity"` plus
//! `instrument_kind = Some("etf")`, mirroring `TrackedInstrument` exactly.
//!
//! # V1 -> V2 compatibility
//!
//! [`convert_v1_registry_to_v2`] / [`convert_tracked_instrument_to_v2`] are pure
//! functions that convert the existing `Vec<TrackedInstrument>` into this shape
//! in memory. They do not read or write `config/instruments/equities.json`, do
//! not change `enabled_equities()`, and are not called by any production path.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::instrument_registry::TrackedInstrument;

/// Schema versions this loader/validator currently accepts.
pub const SUPPORTED_SCHEMA_VERSIONS_V2: &[u32] = &[1];

/// Current schema version produced by [`convert_v1_registry_to_v2`].
pub const SCHEMA_VERSION_V2: u32 = 1;

/// Canonical asset-class vocabulary. Matches
/// `mqk_md::provider::provider_asset_class_trading_class`'s output strings and
/// `mqk_schemas::AssetClass`'s variant names, lower-cased and singular.
pub const CANONICAL_ASSET_CLASSES_V2: &[&str] = &["equity", "option", "future", "crypto", "forex"];

/// Top-level v2 registry document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentRegistryV2 {
    pub schema_version: u32,
    pub instruments: Vec<InstrumentDefinitionV2>,
}

/// One v2 instrument definition. Additive sibling of `TrackedInstrument` — not a
/// replacement. See module docs for the production-cutover boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentDefinitionV2 {
    /// Globally unique identifier, e.g. `"equity:US:AAPL"`.
    pub instrument_id: String,
    /// Primary human-facing symbol, e.g. `"AAPL"`, `"BTC/USD"`.
    pub symbol: String,
    /// Canonical singular asset class: one of [`CANONICAL_ASSET_CLASSES_V2`].
    pub asset_class: String,
    /// Optional instrument-kind tag, e.g. `Some("etf")`. `None` for plain instruments.
    #[serde(default)]
    pub instrument_kind: Option<String>,
    /// Primary exchange/venue. Optional — not every derivative class requires one.
    #[serde(default)]
    pub venue: Option<String>,
    /// Settlement currency ISO code, e.g. `"USD"`.
    pub currency: String,
    /// Quote currency for pair-quoted instruments (crypto/forex). `None` otherwise.
    #[serde(default)]
    pub quote_currency: Option<String>,
    /// Provider name -> provider-specific symbol, e.g. `{"twelvedata": "AAPL"}`.
    #[serde(default)]
    pub provider_symbols: BTreeMap<String, String>,
    /// Whether this instrument is tracked at all (ingestion/backtest/GUI scope).
    pub enabled: bool,
    /// Whether this instrument may be paper-traded. Independent of `enabled`.
    #[serde(default)]
    pub paper_trading_enabled: bool,
    /// Whether this instrument may be live-traded. Independent of `enabled`.
    #[serde(default)]
    pub live_trading_enabled: bool,
    /// Timeframe(s) to track, e.g. `["1D"]`.
    #[serde(default)]
    pub timeframes: Vec<String>,
    /// Contract details. Required for option/future/crypto/forex; optional
    /// (implied `Equity`/`Etf`) for equity.
    #[serde(default)]
    pub contract: Option<ContractDefinitionV2>,
    /// Descriptive metadata. Required (sector + category) for ETF-tagged equities.
    #[serde(default)]
    pub metadata: InstrumentMetadataV2,
    /// Free-form note.
    #[serde(default)]
    pub notes: Option<String>,
    /// Test/fixture-only escape hatch: when `false` (the default), the
    /// `enabled = true` + non-equity combination always fails
    /// [`validate_registry_v2`]. Setting this `true` only changes what the
    /// *validator* accepts in this schema/loader module — nothing in any
    /// production daemon, CLI, ingestion, backtest, or GUI path reads
    /// `InstrumentRegistryV2` at all, so this flag has no trading effect. It
    /// exists solely so a test fixture can prove the fail-closed rule is an
    /// explicit, deliberate gate rather than an accidental omission.
    #[serde(default)]
    pub allow_enabled_non_equity_for_testing: bool,
}

/// Contract details for non-spot / derivative instruments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContractDefinitionV2 {
    /// Spot equity.
    Equity,
    /// Exchange-traded fund. Trades as equity; this variant is purely descriptive.
    Etf,
    /// Spot crypto pair.
    CryptoPair { base: String, quote: String },
    /// Futures contract.
    Future {
        root: String,
        expiry: String,
        multiplier: i64,
        tick_size_micros: i64,
    },
    /// Listed option contract.
    Option {
        underlying: String,
        expiry: String,
        strike_micros: i64,
        right: String,
        multiplier: i64,
    },
    /// Spot/currency-future forex pair.
    ForexPair { base: String, quote: String },
}

/// Descriptive metadata, separate from trading/contract semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentMetadataV2 {
    #[serde(default)]
    pub sector: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Parse a v2 instrument registry JSON file at `path`. Pure parse — does not
/// call [`validate_registry_v2`]. No production file exists for this schema
/// today; this exists for symmetry with `load_instrument_registry` (v1) and
/// for future use once a real multi-provider registry file is introduced.
pub fn load_instrument_registry_v2(path: &Path) -> Result<InstrumentRegistryV2> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("instrument registry v2 read failed: {}", path.display()))?;
    let registry: InstrumentRegistryV2 = serde_json::from_str(&content)
        .with_context(|| format!("instrument registry v2 parse failed: {}", path.display()))?;
    Ok(registry)
}

/// Validate v2 registry invariants. Returns `Ok(())` if all pass, or the first
/// violation as `Err`. See module docs for the fail-closed non-equity-enablement
/// rule.
pub fn validate_registry_v2(registry: &InstrumentRegistryV2) -> Result<()> {
    if !SUPPORTED_SCHEMA_VERSIONS_V2.contains(&registry.schema_version) {
        anyhow::bail!(
            "instrument_registry_v2: unsupported schema_version={} (supported={:?})",
            registry.schema_version,
            SUPPORTED_SCHEMA_VERSIONS_V2
        );
    }

    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut seen_symbols: HashSet<&str> = HashSet::new();

    for inst in &registry.instruments {
        if inst.instrument_id.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry_v2: empty instrument_id for symbol={}",
                inst.symbol
            );
        }
        if inst.symbol.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry_v2: empty symbol for instrument_id={}",
                inst.instrument_id
            );
        }
        if inst.asset_class.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry_v2: empty asset_class for symbol={}",
                inst.symbol
            );
        }
        if inst.currency.trim().is_empty() {
            anyhow::bail!(
                "instrument_registry_v2: empty currency for symbol={}",
                inst.symbol
            );
        }
        if !CANONICAL_ASSET_CLASSES_V2.contains(&inst.asset_class.as_str()) {
            anyhow::bail!(
                "instrument_registry_v2: unknown asset_class={} for symbol={} (expected one of {:?})",
                inst.asset_class,
                inst.symbol,
                CANONICAL_ASSET_CLASSES_V2
            );
        }

        if !seen_ids.insert(inst.instrument_id.as_str()) {
            anyhow::bail!(
                "instrument_registry_v2: duplicate instrument_id={}",
                inst.instrument_id
            );
        }
        if !seen_symbols.insert(inst.symbol.as_str()) {
            anyhow::bail!("instrument_registry_v2: duplicate symbol={}", inst.symbol);
        }

        if let Some(kind) = &inst.instrument_kind {
            if kind.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry_v2: empty instrument_kind for symbol={}",
                    inst.symbol
                );
            }
        }
        if let Some(sector) = &inst.metadata.sector {
            if sector.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry_v2: empty sector for symbol={}",
                    inst.symbol
                );
            }
        }
        if let Some(category) = &inst.metadata.category {
            if category.trim().is_empty() {
                anyhow::bail!(
                    "instrument_registry_v2: empty category for symbol={}",
                    inst.symbol
                );
            }
        }

        let instrument_kind = inst.instrument_kind.as_deref();
        if instrument_kind == Some("etf") {
            let sector_ok = inst
                .metadata
                .sector
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            let category_ok = inst
                .metadata
                .category
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty());
            if !sector_ok || !category_ok {
                anyhow::bail!(
                    "instrument_registry_v2: etf instrument symbol={} missing sector and/or category metadata",
                    inst.symbol
                );
            }
        }

        validate_contract_v2(
            &inst.asset_class,
            instrument_kind,
            &inst.contract,
            &inst.symbol,
        )?;

        if inst.enabled
            && inst.asset_class != "equity"
            && !inst.allow_enabled_non_equity_for_testing
        {
            anyhow::bail!(
                "instrument_registry_v2: enabled non-equity instrument symbol={} asset_class={} requires allow_enabled_non_equity_for_testing=true (test/fixture only; no production path reads this schema)",
                inst.symbol,
                inst.asset_class
            );
        }

        if inst.enabled && inst.provider_symbols.is_empty() {
            anyhow::bail!(
                "instrument_registry_v2: enabled instrument symbol={} has empty provider_symbols",
                inst.symbol
            );
        }
    }

    Ok(())
}

/// Contract-shape validation for one instrument. Called only from
/// [`validate_registry_v2`]; `asset_class` membership in
/// [`CANONICAL_ASSET_CLASSES_V2`] is already checked by the caller, so the
/// fallback arm here is defensive, not reachable in practice.
fn validate_contract_v2(
    asset_class: &str,
    instrument_kind: Option<&str>,
    contract: &Option<ContractDefinitionV2>,
    symbol: &str,
) -> Result<()> {
    let is_etf = instrument_kind == Some("etf");

    match asset_class {
        "equity" if is_etf => match contract {
            None | Some(ContractDefinitionV2::Etf) => Ok(()),
            Some(_) => anyhow::bail!(
                "instrument_registry_v2: etf-tagged instrument symbol={symbol} must use contract=Etf or omit contract"
            ),
        },
        "equity" => match contract {
            None | Some(ContractDefinitionV2::Equity) => Ok(()),
            Some(ContractDefinitionV2::Etf) => anyhow::bail!(
                "instrument_registry_v2: non-etf equity symbol={symbol} cannot have contract=Etf"
            ),
            Some(_) => anyhow::bail!(
                "instrument_registry_v2: equity symbol={symbol} has a non-equity contract"
            ),
        },
        "future" => match contract {
            Some(ContractDefinitionV2::Future {
                root,
                expiry,
                multiplier,
                tick_size_micros,
            }) => {
                if root.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: future symbol={symbol} missing root");
                }
                if expiry.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: future symbol={symbol} missing expiry");
                }
                if *multiplier <= 0 {
                    anyhow::bail!(
                        "instrument_registry_v2: future symbol={symbol} multiplier must be positive"
                    );
                }
                if *tick_size_micros <= 0 {
                    anyhow::bail!(
                        "instrument_registry_v2: future symbol={symbol} tick_size_micros must be positive"
                    );
                }
                Ok(())
            }
            _ => anyhow::bail!(
                "instrument_registry_v2: future symbol={symbol} requires contract=Future with root/expiry/multiplier/tick_size_micros"
            ),
        },
        "option" => match contract {
            Some(ContractDefinitionV2::Option {
                underlying,
                expiry,
                strike_micros,
                right,
                multiplier,
            }) => {
                if underlying.trim().is_empty() {
                    anyhow::bail!(
                        "instrument_registry_v2: option symbol={symbol} missing underlying"
                    );
                }
                if expiry.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: option symbol={symbol} missing expiry");
                }
                if *strike_micros <= 0 {
                    anyhow::bail!(
                        "instrument_registry_v2: option symbol={symbol} strike_micros must be positive"
                    );
                }
                if *multiplier <= 0 {
                    anyhow::bail!(
                        "instrument_registry_v2: option symbol={symbol} multiplier must be positive"
                    );
                }
                if right != "call" && right != "put" {
                    anyhow::bail!(
                        "instrument_registry_v2: option symbol={symbol} right must be 'call' or 'put'"
                    );
                }
                Ok(())
            }
            _ => anyhow::bail!(
                "instrument_registry_v2: option symbol={symbol} requires contract=Option with underlying/expiry/strike_micros/right/multiplier"
            ),
        },
        "crypto" => match contract {
            Some(ContractDefinitionV2::CryptoPair { base, quote }) => {
                if base.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: crypto symbol={symbol} missing base");
                }
                if quote.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: crypto symbol={symbol} missing quote");
                }
                Ok(())
            }
            _ => anyhow::bail!(
                "instrument_registry_v2: crypto symbol={symbol} requires contract=CryptoPair with base/quote"
            ),
        },
        "forex" => match contract {
            Some(ContractDefinitionV2::ForexPair { base, quote }) => {
                if base.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: forex symbol={symbol} missing base");
                }
                if quote.trim().is_empty() {
                    anyhow::bail!("instrument_registry_v2: forex symbol={symbol} missing quote");
                }
                Ok(())
            }
            _ => anyhow::bail!(
                "instrument_registry_v2: forex symbol={symbol} requires contract=ForexPair with base/quote"
            ),
        },
        other => anyhow::bail!(
            "instrument_registry_v2: unknown asset_class={other} for symbol={symbol}"
        ),
    }
}

/// Convert one v1 `TrackedInstrument` into its v2 shape. Pure; no IO; does not
/// mutate or read from `input` beyond field access. `provider_symbols` carries
/// exactly the one provider identity v1 already proves (`provider` ->
/// `provider_symbol`). `paper_trading_enabled`/`live_trading_enabled` default
/// `false` — v1 has no such fields, so there is no "current registry already
/// has a field proving otherwise" case to preserve.
pub fn convert_tracked_instrument_to_v2(input: &TrackedInstrument) -> InstrumentDefinitionV2 {
    let contract = if input.is_etf() {
        Some(ContractDefinitionV2::Etf)
    } else {
        Some(ContractDefinitionV2::Equity)
    };

    let mut provider_symbols = BTreeMap::new();
    provider_symbols.insert(input.provider.clone(), input.provider_symbol.clone());

    InstrumentDefinitionV2 {
        instrument_id: input.instrument_id.clone(),
        symbol: input.symbol.clone(),
        asset_class: input.asset_class.clone(),
        instrument_kind: input.normalized_instrument_kind().map(str::to_string),
        venue: Some(input.venue.clone()),
        currency: input.currency.clone(),
        quote_currency: None,
        provider_symbols,
        enabled: input.enabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: input.timeframes.clone(),
        contract,
        metadata: InstrumentMetadataV2 {
            sector: input.normalized_sector().map(str::to_string),
            category: input.normalized_category().map(str::to_string),
            tags: Vec::new(),
        },
        notes: if input.notes.is_empty() {
            None
        } else {
            Some(input.notes.clone())
        },
        allow_enabled_non_equity_for_testing: false,
    }
}

/// Convert a full v1 registry slice into a v2 [`InstrumentRegistryV2`]. Pure;
/// no IO; preserves input order and count. Does not read or write
/// `config/instruments/equities.json` and is not called by any production path.
pub fn convert_v1_registry_to_v2(input: &[TrackedInstrument]) -> InstrumentRegistryV2 {
    InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments: input.iter().map(convert_tracked_instrument_to_v2).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument_registry::{
        enabled_equities, load_instrument_registry, validate_registry,
    };
    use std::path::PathBuf;

    // ── Construction helpers (test-only fixtures; not production data) ─────

    fn base_equity(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("equity:US:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "equity".to_string(),
            instrument_kind: None,
            venue: Some("NASDAQ".to_string()),
            currency: "USD".to_string(),
            quote_currency: None,
            provider_symbols: BTreeMap::from([("twelvedata".to_string(), symbol.to_string())]),
            enabled: true,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::Equity),
            metadata: InstrumentMetadataV2::default(),
            notes: None,
            allow_enabled_non_equity_for_testing: false,
        }
    }

    fn base_etf(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_kind: Some("etf".to_string()),
            contract: Some(ContractDefinitionV2::Etf),
            metadata: InstrumentMetadataV2 {
                sector: Some("broad_market".to_string()),
                category: Some("index_equity".to_string()),
                tags: Vec::new(),
            },
            venue: Some("NYSEARCA".to_string()),
            ..base_equity(symbol)
        }
    }

    fn base_future(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("future:CME:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "future".to_string(),
            instrument_kind: None,
            venue: Some("CME".to_string()),
            currency: "USD".to_string(),
            quote_currency: None,
            provider_symbols: BTreeMap::new(),
            enabled: false,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::Future {
                root: "ES".to_string(),
                expiry: "2026-09".to_string(),
                multiplier: 50,
                tick_size_micros: 250_000,
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: Some("backlog fixture; not enabled".to_string()),
            allow_enabled_non_equity_for_testing: false,
        }
    }

    fn base_option(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("option:US:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "option".to_string(),
            instrument_kind: None,
            venue: Some("OPRA".to_string()),
            currency: "USD".to_string(),
            quote_currency: None,
            provider_symbols: BTreeMap::new(),
            enabled: false,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::Option {
                underlying: "AAPL".to_string(),
                expiry: "2026-09-18".to_string(),
                strike_micros: 150_000_000,
                right: "call".to_string(),
                multiplier: 100,
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: Some("backlog fixture; not enabled".to_string()),
            allow_enabled_non_equity_for_testing: false,
        }
    }

    fn base_crypto(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("crypto:GLOBAL:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "crypto".to_string(),
            instrument_kind: None,
            venue: None,
            currency: "USD".to_string(),
            quote_currency: Some("USD".to_string()),
            provider_symbols: BTreeMap::new(),
            enabled: false,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::CryptoPair {
                base: "BTC".to_string(),
                quote: "USD".to_string(),
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: Some("backlog fixture; not enabled".to_string()),
            allow_enabled_non_equity_for_testing: false,
        }
    }

    fn base_forex(symbol: &str) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("forex:GLOBAL:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "forex".to_string(),
            instrument_kind: None,
            venue: None,
            currency: "USD".to_string(),
            quote_currency: Some("USD".to_string()),
            provider_symbols: BTreeMap::new(),
            enabled: false,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::ForexPair {
                base: "EUR".to_string(),
                quote: "USD".to_string(),
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: Some("backlog fixture; not enabled".to_string()),
            allow_enabled_non_equity_for_testing: false,
        }
    }

    fn registry_of(instruments: Vec<InstrumentDefinitionV2>) -> InstrumentRegistryV2 {
        InstrumentRegistryV2 {
            schema_version: SCHEMA_VERSION_V2,
            instruments,
        }
    }

    // ── V2-01/02: schema/loader parses real JSON text ───────────────────────

    // V2-01: a hand-written JSON document containing one equity, one ETF, and
    // one disabled instance each of future/option/crypto/forex parses through
    // serde and validates cleanly. This is the literal "registry v2 parses X"
    // proof via the wire format, not just Rust struct construction.
    #[test]
    fn v2_01_parses_mixed_registry_with_all_asset_kinds_and_validates() {
        let json = r#"
        {
            "schema_version": 1,
            "instruments": [
                {
                    "instrument_id": "equity:US:AAPL",
                    "symbol": "AAPL",
                    "asset_class": "equity",
                    "venue": "NASDAQ",
                    "currency": "USD",
                    "provider_symbols": {"twelvedata": "AAPL"},
                    "enabled": true,
                    "timeframes": ["1D"],
                    "contract": {"kind": "equity"}
                },
                {
                    "instrument_id": "equity:US:SPY",
                    "symbol": "SPY",
                    "asset_class": "equity",
                    "instrument_kind": "etf",
                    "venue": "NYSEARCA",
                    "currency": "USD",
                    "provider_symbols": {"twelvedata": "SPY"},
                    "enabled": true,
                    "timeframes": ["1D"],
                    "contract": {"kind": "etf"},
                    "metadata": {"sector": "broad_market", "category": "index_equity"}
                },
                {
                    "instrument_id": "future:CME:ES2026U",
                    "symbol": "ES2026U",
                    "asset_class": "future",
                    "venue": "CME",
                    "currency": "USD",
                    "enabled": false,
                    "contract": {"kind": "future", "root": "ES", "expiry": "2026-09", "multiplier": 50, "tick_size_micros": 250000}
                },
                {
                    "instrument_id": "option:US:AAPL20260918C150",
                    "symbol": "AAPL20260918C150",
                    "asset_class": "option",
                    "currency": "USD",
                    "enabled": false,
                    "contract": {"kind": "option", "underlying": "AAPL", "expiry": "2026-09-18", "strike_micros": 150000000, "right": "call", "multiplier": 100}
                },
                {
                    "instrument_id": "crypto:GLOBAL:BTCUSD",
                    "symbol": "BTC/USD",
                    "asset_class": "crypto",
                    "currency": "USD",
                    "enabled": false,
                    "contract": {"kind": "crypto_pair", "base": "BTC", "quote": "USD"}
                },
                {
                    "instrument_id": "forex:GLOBAL:EURUSD",
                    "symbol": "EUR/USD",
                    "asset_class": "forex",
                    "currency": "USD",
                    "enabled": false,
                    "contract": {"kind": "forex_pair", "base": "EUR", "quote": "USD"}
                }
            ]
        }
        "#;

        let registry: InstrumentRegistryV2 =
            serde_json::from_str(json).expect("v2 registry json must parse");
        assert_eq!(registry.instruments.len(), 6);
        validate_registry_v2(&registry).expect("mixed fixture registry must validate");

        let spy = registry
            .instruments
            .iter()
            .find(|i| i.symbol == "SPY")
            .unwrap();
        assert_eq!(spy.asset_class, "equity");
        assert_eq!(spy.instrument_kind.as_deref(), Some("etf"));
        assert_eq!(spy.contract, Some(ContractDefinitionV2::Etf));

        for symbol in ["ES2026U", "AAPL20260918C150", "BTC/USD", "EUR/USD"] {
            let inst = registry
                .instruments
                .iter()
                .find(|i| i.symbol == symbol)
                .unwrap();
            assert!(!inst.enabled, "{symbol} must be disabled/backlog");
        }
    }

    // V2-02: load_instrument_registry_v2 round-trips a registry through a real
    // file on disk (proves the loader function itself, not just serde derive).
    #[test]
    fn v2_02_loader_round_trips_through_temp_file() {
        let registry = registry_of(vec![base_equity("AAPL"), base_etf("SPY")]);
        let json = serde_json::to_string_pretty(&registry).unwrap();

        let path = std::env::temp_dir().join(format!(
            "mqk_test_instrument_registry_v2_{}.json",
            std::process::id()
        ));
        std::fs::write(&path, json).expect("write temp v2 fixture file");

        let loaded =
            load_instrument_registry_v2(&path).expect("load_instrument_registry_v2 must succeed");
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded, registry);
    }

    // ── Validation failures (V2-03..V2-17) ──────────────────────────────────

    #[test]
    fn v2_03_invalid_asset_class_fails() {
        let mut inst = base_equity("AAPL");
        inst.asset_class = "stock".to_string();
        let err = validate_registry_v2(&registry_of(vec![inst])).unwrap_err();
        assert!(err.to_string().contains("unknown asset_class"));
    }

    #[test]
    fn v2_04_duplicate_instrument_id_fails() {
        let mut spy = base_equity("SPY");
        spy.instrument_id = "equity:US:AAPL".to_string();
        let aapl = base_equity("AAPL");
        let err = validate_registry_v2(&registry_of(vec![aapl, spy])).unwrap_err();
        assert!(err.to_string().contains("duplicate instrument_id"));
    }

    #[test]
    fn v2_05_duplicate_symbol_fails() {
        let mut dup = base_equity("AAPL");
        dup.instrument_id = "equity:US:AAPL2".to_string();
        let original = base_equity("AAPL");
        let err = validate_registry_v2(&registry_of(vec![original, dup])).unwrap_err();
        assert!(err.to_string().contains("duplicate symbol"));
    }

    #[test]
    fn v2_06_unsupported_schema_version_fails() {
        let mut registry = registry_of(vec![base_equity("AAPL")]);
        registry.schema_version = 99;
        let err = validate_registry_v2(&registry).unwrap_err();
        assert!(err.to_string().contains("unsupported schema_version"));
    }

    #[test]
    fn v2_07_missing_contract_entirely_fails_for_each_derivative_class() {
        for mut inst in [
            base_future("ES2026U"),
            base_option("AAPL20260918C150"),
            base_crypto("BTC/USD"),
            base_forex("EUR/USD"),
        ] {
            inst.contract = None;
            let err = validate_registry_v2(&registry_of(vec![inst.clone()])).unwrap_err();
            assert!(
                err.to_string().contains("requires contract="),
                "symbol={} message={}",
                inst.symbol,
                err
            );
        }
    }

    #[test]
    fn v2_08_future_contract_field_violations_fail() {
        let cases = [
            ContractDefinitionV2::Future {
                root: "".to_string(),
                expiry: "2026-09".to_string(),
                multiplier: 50,
                tick_size_micros: 250_000,
            },
            ContractDefinitionV2::Future {
                root: "ES".to_string(),
                expiry: "".to_string(),
                multiplier: 50,
                tick_size_micros: 250_000,
            },
            ContractDefinitionV2::Future {
                root: "ES".to_string(),
                expiry: "2026-09".to_string(),
                multiplier: 0,
                tick_size_micros: 250_000,
            },
            ContractDefinitionV2::Future {
                root: "ES".to_string(),
                expiry: "2026-09".to_string(),
                multiplier: 50,
                tick_size_micros: 0,
            },
        ];
        for contract in cases {
            let mut inst = base_future("ES2026U");
            inst.contract = Some(contract);
            assert!(validate_registry_v2(&registry_of(vec![inst])).is_err());
        }
    }

    #[test]
    fn v2_09_option_contract_field_violations_fail() {
        let cases = [
            ContractDefinitionV2::Option {
                underlying: "".to_string(),
                expiry: "2026-09-18".to_string(),
                strike_micros: 150_000_000,
                right: "call".to_string(),
                multiplier: 100,
            },
            ContractDefinitionV2::Option {
                underlying: "AAPL".to_string(),
                expiry: "".to_string(),
                strike_micros: 150_000_000,
                right: "call".to_string(),
                multiplier: 100,
            },
            ContractDefinitionV2::Option {
                underlying: "AAPL".to_string(),
                expiry: "2026-09-18".to_string(),
                strike_micros: 0,
                right: "call".to_string(),
                multiplier: 100,
            },
            ContractDefinitionV2::Option {
                underlying: "AAPL".to_string(),
                expiry: "2026-09-18".to_string(),
                strike_micros: 150_000_000,
                right: "straddle".to_string(),
                multiplier: 100,
            },
            ContractDefinitionV2::Option {
                underlying: "AAPL".to_string(),
                expiry: "2026-09-18".to_string(),
                strike_micros: 150_000_000,
                right: "call".to_string(),
                multiplier: 0,
            },
        ];
        for contract in cases {
            let mut inst = base_option("AAPL20260918C150");
            inst.contract = Some(contract);
            assert!(validate_registry_v2(&registry_of(vec![inst])).is_err());
        }
    }

    #[test]
    fn v2_10_crypto_contract_field_violations_fail() {
        let cases = [
            ContractDefinitionV2::CryptoPair {
                base: "".to_string(),
                quote: "USD".to_string(),
            },
            ContractDefinitionV2::CryptoPair {
                base: "BTC".to_string(),
                quote: "".to_string(),
            },
        ];
        for contract in cases {
            let mut inst = base_crypto("BTC/USD");
            inst.contract = Some(contract);
            assert!(validate_registry_v2(&registry_of(vec![inst])).is_err());
        }
    }

    #[test]
    fn v2_11_forex_contract_field_violations_fail() {
        let cases = [
            ContractDefinitionV2::ForexPair {
                base: "".to_string(),
                quote: "USD".to_string(),
            },
            ContractDefinitionV2::ForexPair {
                base: "EUR".to_string(),
                quote: "".to_string(),
            },
        ];
        for contract in cases {
            let mut inst = base_forex("EUR/USD");
            inst.contract = Some(contract);
            assert!(validate_registry_v2(&registry_of(vec![inst])).is_err());
        }
    }

    #[test]
    fn v2_12_etf_missing_sector_or_category_fails() {
        let mut missing_sector = base_etf("SPY");
        missing_sector.metadata.sector = None;
        assert!(validate_registry_v2(&registry_of(vec![missing_sector]))
            .unwrap_err()
            .to_string()
            .contains("missing sector and/or category"));

        let mut missing_category = base_etf("QQQ");
        missing_category.instrument_id = "equity:US:QQQ".to_string();
        missing_category.metadata.category = None;
        assert!(validate_registry_v2(&registry_of(vec![missing_category]))
            .unwrap_err()
            .to_string()
            .contains("missing sector and/or category"));
    }

    #[test]
    fn v2_13_non_etf_equity_cannot_have_etf_contract_fails() {
        let mut inst = base_equity("AAPL");
        inst.contract = Some(ContractDefinitionV2::Etf);
        let err = validate_registry_v2(&registry_of(vec![inst])).unwrap_err();
        assert!(err.to_string().contains("cannot have contract=Etf"));
    }

    #[test]
    fn v2_14_enabled_non_equity_fails_without_allow_flag() {
        let mut inst = base_future("ES2026U");
        inst.enabled = true;
        inst.provider_symbols
            .insert("synthetic".to_string(), "ES".to_string());
        let err = validate_registry_v2(&registry_of(vec![inst])).unwrap_err();
        assert!(err
            .to_string()
            .contains("allow_enabled_non_equity_for_testing"));
    }

    #[test]
    fn v2_15_enabled_non_equity_passes_with_explicit_allow_flag_set() {
        let mut inst = base_future("ES2026U");
        inst.enabled = true;
        inst.allow_enabled_non_equity_for_testing = true;
        inst.provider_symbols
            .insert("synthetic".to_string(), "ES".to_string());
        validate_registry_v2(&registry_of(vec![inst]))
            .expect("explicit test-only allow flag must permit enabled non-equity");
    }

    #[test]
    fn v2_16_enabled_instrument_requires_provider_symbols_disabled_does_not() {
        let mut enabled_empty = base_equity("AAPL");
        enabled_empty.provider_symbols.clear();
        let err = validate_registry_v2(&registry_of(vec![enabled_empty])).unwrap_err();
        assert!(err.to_string().contains("empty provider_symbols"));

        let mut disabled_empty = base_future("ES2026U");
        disabled_empty.provider_symbols.clear();
        assert!(disabled_empty.provider_symbols.is_empty());
        validate_registry_v2(&registry_of(vec![disabled_empty]))
            .expect("disabled instrument may have empty provider_symbols");
    }

    #[test]
    fn v2_17_disabled_derivative_fixtures_validate_cleanly() {
        let registry = registry_of(vec![
            base_future("ES2026U"),
            base_option("AAPL20260918C150"),
            base_crypto("BTC/USD"),
            base_forex("EUR/USD"),
        ]);
        validate_registry_v2(&registry).expect("disabled backlog fixtures must validate");
    }

    // ── V1 -> V2 compatibility (compat_01..compat_09) ───────────────────────

    fn v1_registry_path() -> PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest_dir).join("../../../config/instruments/equities.json")
    }

    const V1_ETF_SYMBOLS: &[&str] = &[
        "SPY", "QQQ", "IWM", "DIA", "XLK", "XLF", "XLE", "XLI", "XLP", "XLU", "TLT", "IEF", "SHY",
        "GLD",
    ];

    // compat_01: the existing v1 loader/validator are unaffected by this
    // module's existence in the same crate.
    #[test]
    fn compat_01_v1_registry_still_loads_and_validates_unchanged() {
        let v1 = load_instrument_registry(&v1_registry_path()).expect("v1 registry must load");
        validate_registry(&v1).expect("v1 registry must still validate");
        assert_eq!(v1.len(), 88);
    }

    // compat_02: conversion preserves the exact instrument count.
    #[test]
    fn compat_02_v1_to_v2_conversion_preserves_count() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        assert_eq!(v2.instruments.len(), v1.len());
        assert_eq!(v2.schema_version, SCHEMA_VERSION_V2);
    }

    // compat_03: the v1 enabled_equities() symbol order is preserved when the
    // same filter+sort is applied to the v2-converted instruments.
    #[test]
    fn compat_03_v1_to_v2_conversion_preserves_enabled_equity_symbol_order() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v1_symbols: Vec<&str> = enabled_equities(&v1)
            .into_iter()
            .map(|i| i.symbol.as_str())
            .collect();
        assert_eq!(v1_symbols.len(), 88);

        let v2 = convert_v1_registry_to_v2(&v1);
        let mut v2_symbols: Vec<&str> = v2
            .instruments
            .iter()
            .filter(|i| i.enabled && i.asset_class == "equity")
            .map(|i| i.symbol.as_str())
            .collect();
        v2_symbols.sort();

        assert_eq!(v2_symbols, v1_symbols);
    }

    // compat_04: all 14 tagged ETFs convert with contract=Etf and
    // instrument_kind="etf", asset_class staying "equity".
    #[test]
    fn compat_04_all_tagged_etfs_convert_with_etf_contract_and_kind() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        for symbol in V1_ETF_SYMBOLS {
            let inst = v2.instruments.iter().find(|i| i.symbol == *symbol).unwrap();
            assert_eq!(inst.asset_class, "equity");
            assert_eq!(inst.instrument_kind.as_deref(), Some("etf"));
            assert_eq!(inst.contract, Some(ContractDefinitionV2::Etf));
        }
    }

    // compat_05: a non-ETF stock converts to plain equity with contract=Equity
    // and no instrument_kind.
    #[test]
    fn compat_05_non_etf_stocks_convert_to_plain_equity_with_equity_contract() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        let aapl = v2.instruments.iter().find(|i| i.symbol == "AAPL").unwrap();
        assert_eq!(aapl.asset_class, "equity");
        assert_eq!(aapl.instrument_kind, None);
        assert_eq!(aapl.contract, Some(ContractDefinitionV2::Equity));
    }

    // compat_06: provider_symbol/timeframes/venue/currency carry through.
    #[test]
    fn compat_06_provider_symbol_timeframes_venue_currency_carry_through() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v1_aapl = v1.iter().find(|i| i.symbol == "AAPL").unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        let v2_aapl = v2.instruments.iter().find(|i| i.symbol == "AAPL").unwrap();

        assert_eq!(
            v2_aapl.provider_symbols.get(&v1_aapl.provider),
            Some(&v1_aapl.provider_symbol)
        );
        assert_eq!(v2_aapl.timeframes, v1_aapl.timeframes);
        assert_eq!(v2_aapl.venue.as_deref(), Some(v1_aapl.venue.as_str()));
        assert_eq!(v2_aapl.currency, v1_aapl.currency);
    }

    // compat_07: sector/category metadata carries through for ETF entries.
    #[test]
    fn compat_07_sector_and_category_carry_through_for_etf_metadata() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        let spy = v2.instruments.iter().find(|i| i.symbol == "SPY").unwrap();
        assert_eq!(spy.metadata.sector.as_deref(), Some("broad_market"));
        assert_eq!(spy.metadata.category.as_deref(), Some("index_equity"));
    }

    // compat_08: the entire real production registry, once converted,
    // validates cleanly under v2 rules — the core "no behavior break" proof.
    #[test]
    fn compat_08_validate_registry_v2_passes_for_full_converted_production_registry() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        validate_registry_v2(&v2)
            .expect("converted production registry must validate under v2 rules");
    }

    // compat_09: paper/live trading flags default false for every converted
    // instrument, equity or ETF alike.
    #[test]
    fn compat_09_paper_and_live_trading_flags_default_false_for_all_converted() {
        let v1 = load_instrument_registry(&v1_registry_path()).unwrap();
        let v2 = convert_v1_registry_to_v2(&v1);
        for inst in &v2.instruments {
            assert!(!inst.paper_trading_enabled, "symbol={}", inst.symbol);
            assert!(!inst.live_trading_enabled, "symbol={}", inst.symbol);
        }
    }
}
