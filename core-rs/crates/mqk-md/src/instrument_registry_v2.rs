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
    /// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: optional backtest-economics
    /// metadata (contract multiplier + margin scaffold). Metadata only — see
    /// [`InstrumentEconomicsMetadataV2`]. Never read by any trading/execution
    /// path; never enables or implies enablement of any asset class.
    #[serde(default)]
    pub economics: Option<InstrumentEconomicsMetadataV2>,
}

/// Backtest-economics metadata for a registry-v2 instrument: contract
/// multiplier + optional margin scaffold.
///
/// Mirrors `mqk_backtest::BacktestInstrumentEconomics`'s shape without
/// introducing a dependency on `mqk-backtest` from `mqk-md` (consistent with
/// this module's existing no-new-Cargo-dependency precedent). Metadata only:
/// nothing in this module enforces margin or reads this to gate, block, or
/// route any order. `contract_multiplier` is `Option` (rather than the
/// always-positive `i64` on the backtest-side type) so an instrument can
/// omit it entirely rather than a caller fabricating a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentEconomicsMetadataV2 {
    #[serde(default)]
    pub contract_multiplier: Option<i64>,
    #[serde(default)]
    pub initial_margin_micros: Option<i64>,
    #[serde(default)]
    pub maintenance_margin_micros: Option<i64>,
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

        validate_economics_v2(&inst.economics, &inst.symbol)?;

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

/// Validate one instrument's optional backtest-economics metadata. Called
/// only from [`validate_registry_v2`]. Metadata-only: this never checks
/// `enabled`/asset class -- any instrument, equity or not, may carry
/// `economics`.
fn validate_economics_v2(
    economics: &Option<InstrumentEconomicsMetadataV2>,
    symbol: &str,
) -> Result<()> {
    let Some(econ) = economics else {
        return Ok(());
    };
    if let Some(multiplier) = econ.contract_multiplier {
        if multiplier <= 0 {
            anyhow::bail!(
                "instrument_registry_v2: economics.contract_multiplier must be positive for symbol={symbol}, got {multiplier}"
            );
        }
    }
    if let Some(margin) = econ.initial_margin_micros {
        if margin < 0 {
            anyhow::bail!(
                "instrument_registry_v2: economics.initial_margin_micros must be non-negative for symbol={symbol}, got {margin}"
            );
        }
    }
    if let Some(margin) = econ.maintenance_margin_micros {
        if margin < 0 {
            anyhow::bail!(
                "instrument_registry_v2: economics.maintenance_margin_micros must be non-negative for symbol={symbol}, got {margin}"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: read-only backtest economics
// suggestion helper
// ---------------------------------------------------------------------------

/// Read-only backtest-economics suggestion derived from one v2 instrument
/// definition. Pure; no IO; never reads `enabled`, `paper_trading_enabled`,
/// or `live_trading_enabled` -- this has no bearing on, and never implies,
/// trading enablement for any asset class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacktestEconomicsSuggestionV2 {
    /// `"active"` or `"no_contract_economics"`.
    pub truth_state: &'static str,
    pub contract_multiplier: Option<i64>,
    pub initial_margin_micros: Option<i64>,
    pub maintenance_margin_micros: Option<i64>,
    pub reason: &'static str,
}

/// Suggest backtest economics for `inst`:
///
/// 1. Explicit `inst.economics.contract_multiplier` wins when present --
///    `"active"`, `reason="registry_v2_explicit"`, regardless of asset class.
/// 2. Otherwise, equity/ETF instruments (no contract, or `Equity`/`Etf`) fall
///    back to the implicit default -- `"active"`, multiplier=1, no margin,
///    `reason="equity_default"` -- matching
///    `mqk_backtest::BacktestInstrumentEconomics::equity()`.
/// 3. Otherwise, `"no_contract_economics"` -- truthfully reports that no
///    economics metadata is available rather than fabricating a multiplier.
pub fn backtest_economics_suggestion_for_instrument(
    inst: &InstrumentDefinitionV2,
) -> BacktestEconomicsSuggestionV2 {
    if let Some(econ) = &inst.economics {
        if let Some(multiplier) = econ.contract_multiplier {
            return BacktestEconomicsSuggestionV2 {
                truth_state: "active",
                contract_multiplier: Some(multiplier),
                initial_margin_micros: econ.initial_margin_micros,
                maintenance_margin_micros: econ.maintenance_margin_micros,
                reason: "registry_v2_explicit",
            };
        }
    }

    let is_equity_like = inst.asset_class == "equity"
        && matches!(
            inst.contract,
            None | Some(ContractDefinitionV2::Equity) | Some(ContractDefinitionV2::Etf)
        );
    if is_equity_like {
        return BacktestEconomicsSuggestionV2 {
            truth_state: "active",
            contract_multiplier: Some(1),
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            reason: "equity_default",
        };
    }

    BacktestEconomicsSuggestionV2 {
        truth_state: "no_contract_economics",
        contract_multiplier: None,
        initial_margin_micros: None,
        maintenance_margin_micros: None,
        reason:
            "registry has no supported equity contract economics and no explicit economics metadata",
    }
}

// ---------------------------------------------------------------------------
// ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED: read-only operator-visibility
// summary for a configured v2 registry SOURCE (MQK_INSTRUMENT_REGISTRY_V2_PATH).
// ---------------------------------------------------------------------------

/// Cap on [`InstrumentRegistryV2Summary::sample_symbols`] length, so an
/// arbitrarily large configured source still produces a bounded operator
/// status payload.
pub const SAMPLE_SYMBOLS_LIMIT: usize = 10;

/// Pure, read-only summary of one [`InstrumentRegistryV2`] document.
///
/// Used only by operator-visibility surfaces (the daemon's
/// registry-v2-source status route). Nothing here is read by, or implies
/// enablement for, any trading/execution/ingestion/backtest path -- counting
/// `enabled`/`paper_trading_enabled`/`live_trading_enabled` is observational
/// only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentRegistryV2Summary {
    pub total_instruments: usize,
    /// Instrument count by `asset_class`. Counts all entries, not just enabled.
    pub asset_class_counts: BTreeMap<String, usize>,
    pub enabled_count: usize,
    pub paper_trading_enabled_count: usize,
    pub live_trading_enabled_count: usize,
    pub non_equity_count: usize,
    pub non_equity_present: bool,
    /// `true` when every non-equity instrument has `enabled == false`.
    /// Vacuously `true` when `non_equity_present` is `false`.
    pub non_equity_all_disabled: bool,
    /// `true` when at least one instrument carries `economics` metadata.
    pub has_economics_metadata: bool,
    /// First [`SAMPLE_SYMBOLS_LIMIT`] symbols in registry order (deterministic;
    /// not sorted or randomized).
    pub sample_symbols: Vec<String>,
}

/// Summarize `registry` for operator visibility. Pure; no IO. The daemon
/// route that calls this never uses the result to gate, block, or route any
/// order -- see module docs and [`InstrumentRegistryV2Summary`].
pub fn summarize_instrument_registry_v2_status(
    registry: &InstrumentRegistryV2,
) -> InstrumentRegistryV2Summary {
    let mut asset_class_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut enabled_count = 0usize;
    let mut paper_trading_enabled_count = 0usize;
    let mut live_trading_enabled_count = 0usize;
    let mut non_equity_count = 0usize;
    let mut non_equity_all_disabled = true;
    let mut has_economics_metadata = false;

    for inst in &registry.instruments {
        *asset_class_counts
            .entry(inst.asset_class.clone())
            .or_insert(0) += 1;
        if inst.enabled {
            enabled_count += 1;
        }
        if inst.paper_trading_enabled {
            paper_trading_enabled_count += 1;
        }
        if inst.live_trading_enabled {
            live_trading_enabled_count += 1;
        }
        if inst.asset_class != "equity" {
            non_equity_count += 1;
            if inst.enabled {
                non_equity_all_disabled = false;
            }
        }
        if inst.economics.is_some() {
            has_economics_metadata = true;
        }
    }

    let sample_symbols = registry
        .instruments
        .iter()
        .take(SAMPLE_SYMBOLS_LIMIT)
        .map(|inst| inst.symbol.clone())
        .collect();

    InstrumentRegistryV2Summary {
        total_instruments: registry.instruments.len(),
        asset_class_counts,
        enabled_count,
        paper_trading_enabled_count,
        live_trading_enabled_count,
        non_equity_count,
        non_equity_present: non_equity_count > 0,
        non_equity_all_disabled,
        has_economics_metadata,
        sample_symbols,
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
        // v1 has no multiplier/margin data — leave economics unset rather
        // than fabricating a value. The equity-default fallback in
        // `backtest_economics_suggestion_for_instrument` covers this case.
        economics: None,
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
            economics: None,
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
            economics: None,
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
            economics: None,
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
            economics: None,
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
            economics: None,
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

    // ── BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: economics metadata ────────

    fn with_economics(
        mut inst: InstrumentDefinitionV2,
        economics: InstrumentEconomicsMetadataV2,
    ) -> InstrumentDefinitionV2 {
        inst.economics = Some(economics);
        inst
    }

    // ECON-01: a valid positive contract_multiplier validates cleanly.
    #[test]
    fn econ01_valid_contract_multiplier_validates() {
        let inst = with_economics(
            base_equity("AAPL"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(1),
                initial_margin_micros: None,
                maintenance_margin_micros: None,
            },
        );
        validate_registry_v2(&registry_of(vec![inst]))
            .expect("positive contract_multiplier must validate");
    }

    // ECON-02: contract_multiplier <= 0 fails validation, for both zero and negative.
    #[test]
    fn econ02_non_positive_contract_multiplier_fails() {
        for bad in [0i64, -1, -50] {
            let inst = with_economics(
                base_equity("AAPL"),
                InstrumentEconomicsMetadataV2 {
                    contract_multiplier: Some(bad),
                    initial_margin_micros: None,
                    maintenance_margin_micros: None,
                },
            );
            let err = validate_registry_v2(&registry_of(vec![inst])).unwrap_err();
            assert!(
                err.to_string()
                    .contains("economics.contract_multiplier must be positive"),
                "multiplier={bad} message={err}"
            );
        }
    }

    // ECON-03: negative margin metadata fails validation (both fields, independently).
    #[test]
    fn econ03_negative_margin_metadata_fails() {
        let with_initial = with_economics(
            base_equity("AAPL"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(1),
                initial_margin_micros: Some(-1),
                maintenance_margin_micros: None,
            },
        );
        let err = validate_registry_v2(&registry_of(vec![with_initial])).unwrap_err();
        assert!(err
            .to_string()
            .contains("economics.initial_margin_micros must be non-negative"));

        let mut with_maintenance = base_equity("MSFT");
        with_maintenance.instrument_id = "equity:US:MSFT".to_string();
        let with_maintenance = with_economics(
            with_maintenance,
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(1),
                initial_margin_micros: None,
                maintenance_margin_micros: Some(-1),
            },
        );
        let err = validate_registry_v2(&registry_of(vec![with_maintenance])).unwrap_err();
        assert!(err
            .to_string()
            .contains("economics.maintenance_margin_micros must be non-negative"));
    }

    // ECON-04: zero-valued margins are non-negative and validate cleanly.
    #[test]
    fn econ04_zero_margins_validate() {
        let inst = with_economics(
            base_equity("AAPL"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(1),
                initial_margin_micros: Some(0),
                maintenance_margin_micros: Some(0),
            },
        );
        validate_registry_v2(&registry_of(vec![inst])).expect("zero margins must validate");
    }

    // ECON-05: absent economics (None) is always valid -- metadata is opt-in.
    #[test]
    fn econ05_absent_economics_is_valid() {
        for inst in [
            base_equity("AAPL"),
            base_future("ES2026U"),
            base_option("AAPL20260918C150"),
            base_crypto("BTC/USD"),
            base_forex("EUR/USD"),
        ] {
            assert_eq!(inst.economics, None);
        }
        validate_registry_v2(&registry_of(vec![base_equity("AAPL")]))
            .expect("absent economics must validate");
    }

    // ── BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: suggestion helper ──────────

    // SUG-01: explicit registry-v2 economics on a non-equity fixture wins
    // truth_state="active" with the explicit multiplier/margins, even though
    // no production non-equity registry data exists today (this proves the
    // helper, not a wired production path -- see module docs).
    #[test]
    fn sug01_explicit_economics_on_non_equity_fixture_returns_active() {
        let inst = with_economics(
            base_future("ES2026U"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(50),
                initial_margin_micros: Some(500_000_000),
                maintenance_margin_micros: Some(400_000_000),
            },
        );
        let suggestion = backtest_economics_suggestion_for_instrument(&inst);
        assert_eq!(suggestion.truth_state, "active");
        assert_eq!(suggestion.contract_multiplier, Some(50));
        assert_eq!(suggestion.initial_margin_micros, Some(500_000_000));
        assert_eq!(suggestion.maintenance_margin_micros, Some(400_000_000));
        assert_eq!(suggestion.reason, "registry_v2_explicit");
    }

    // SUG-02: explicit economics on an equity instrument also wins over the
    // equity-default fallback (explicit always takes precedence).
    #[test]
    fn sug02_explicit_economics_on_equity_overrides_default() {
        let inst = with_economics(
            base_equity("AAPL"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(5),
                initial_margin_micros: None,
                maintenance_margin_micros: None,
            },
        );
        let suggestion = backtest_economics_suggestion_for_instrument(&inst);
        assert_eq!(suggestion.truth_state, "active");
        assert_eq!(suggestion.contract_multiplier, Some(5));
        assert_eq!(suggestion.reason, "registry_v2_explicit");
    }

    // SUG-03: equity/ETF instruments with no explicit economics fall back to
    // the truthful equity default (multiplier=1, no margin).
    #[test]
    fn sug03_equity_and_etf_without_explicit_economics_default_to_multiplier_one() {
        for inst in [base_equity("AAPL"), base_etf("SPY")] {
            let suggestion = backtest_economics_suggestion_for_instrument(&inst);
            assert_eq!(suggestion.truth_state, "active", "symbol={}", inst.symbol);
            assert_eq!(
                suggestion.contract_multiplier,
                Some(1),
                "symbol={}",
                inst.symbol
            );
            assert_eq!(suggestion.initial_margin_micros, None);
            assert_eq!(suggestion.maintenance_margin_micros, None);
            assert_eq!(suggestion.reason, "equity_default");
        }
    }

    // SUG-04: non-equity instruments without explicit economics truthfully
    // report no_contract_economics -- never a fabricated multiplier.
    #[test]
    fn sug04_non_equity_without_explicit_economics_reports_no_contract_economics() {
        for inst in [
            base_future("ES2026U"),
            base_option("AAPL20260918C150"),
            base_crypto("BTC/USD"),
            base_forex("EUR/USD"),
        ] {
            let suggestion = backtest_economics_suggestion_for_instrument(&inst);
            assert_eq!(
                suggestion.truth_state, "no_contract_economics",
                "symbol={}",
                inst.symbol
            );
            assert_eq!(suggestion.contract_multiplier, None);
        }
    }

    // SUG-05: the suggestion helper never reads enabled/paper/live trading
    // flags -- proven by flipping them and observing no change in output.
    #[test]
    fn sug05_suggestion_ignores_enablement_flags() {
        let mut inst = base_equity("AAPL");
        let baseline = backtest_economics_suggestion_for_instrument(&inst);

        inst.enabled = false;
        inst.paper_trading_enabled = true;
        inst.live_trading_enabled = true;
        let flipped = backtest_economics_suggestion_for_instrument(&inst);

        assert_eq!(baseline, flipped);
    }

    // ── INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED: committed example fixture ──

    fn instruments_v2_example_fixture_path() -> PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest_dir)
            .join("../../../config/instruments/instruments_v2.backtest_suggestions.example.json")
    }

    // EX-01: the committed example v2 fixture loads and validates cleanly, and
    // every entry stays disabled/non-tradable while still carrying explicit
    // economics metadata -- proving the file can demonstrate non-equity
    // backtest economics suggestions without enabling any trading.
    #[test]
    fn ex01_committed_example_v2_fixture_loads_validates_and_stays_disabled() {
        let registry = load_instrument_registry_v2(&instruments_v2_example_fixture_path())
            .expect("committed example v2 fixture must load");
        validate_registry_v2(&registry).expect("committed example v2 fixture must validate");
        assert!(!registry.instruments.is_empty());
        for inst in &registry.instruments {
            assert!(
                !inst.enabled,
                "example fixture symbol={} must stay disabled",
                inst.symbol
            );
            assert!(!inst.paper_trading_enabled, "symbol={}", inst.symbol);
            assert!(!inst.live_trading_enabled, "symbol={}", inst.symbol);
            assert_ne!(
                inst.asset_class, "equity",
                "example fixture symbol={} is meant to demonstrate non-equity economics",
                inst.symbol
            );
            assert!(
                inst.economics
                    .is_some_and(|e| e.contract_multiplier.is_some()),
                "example fixture symbol={} should demonstrate an explicit contract_multiplier",
                inst.symbol
            );
        }
    }

    // EX-02: the suggestion helper reports "active"/"registry_v2_explicit" for
    // every committed example fixture instrument (end-to-end through the same
    // pure helper the daemon route calls), proving the file is wired
    // correctly rather than merely well-formed JSON.
    #[test]
    fn ex02_committed_example_v2_fixture_instruments_yield_explicit_active_suggestions() {
        let registry = load_instrument_registry_v2(&instruments_v2_example_fixture_path())
            .expect("committed example v2 fixture must load");
        for inst in &registry.instruments {
            let suggestion = backtest_economics_suggestion_for_instrument(inst);
            assert_eq!(suggestion.truth_state, "active", "symbol={}", inst.symbol);
            assert_eq!(
                suggestion.reason, "registry_v2_explicit",
                "symbol={}",
                inst.symbol
            );
            assert!(
                suggestion.contract_multiplier.is_some(),
                "symbol={}",
                inst.symbol
            );
        }
    }

    // ── ASSET-CORE-01D: summarize_instrument_registry_v2_status ────────────

    // V2STATUS-01: asset_class_counts tallies every instrument, mixed classes.
    #[test]
    fn v2status_01_summary_counts_asset_classes_correctly() {
        let registry = registry_of(vec![
            base_equity("AAPL"),
            base_etf("SPY"),
            base_future("ES2026U"),
            base_crypto("BTC/USD"),
        ]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert_eq!(summary.total_instruments, 4);
        assert_eq!(summary.asset_class_counts["equity"], 2);
        assert_eq!(summary.asset_class_counts["future"], 1);
        assert_eq!(summary.asset_class_counts["crypto"], 1);
        assert_eq!(summary.asset_class_counts.len(), 3);
    }

    // V2STATUS-02: enabled/paper/live counts tally independently of each other.
    #[test]
    fn v2status_02_summary_counts_enabled_paper_live_flags_correctly() {
        let mut paper_only = base_equity("MSFT");
        paper_only.instrument_id = "equity:US:MSFT".to_string();
        paper_only.paper_trading_enabled = true;

        let mut live_only = base_equity("GOOG");
        live_only.instrument_id = "equity:US:GOOG".to_string();
        live_only.enabled = false;
        live_only.live_trading_enabled = true;

        let registry = registry_of(vec![base_equity("AAPL"), paper_only, live_only]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert_eq!(summary.enabled_count, 2, "AAPL + MSFT are enabled");
        assert_eq!(summary.paper_trading_enabled_count, 1);
        assert_eq!(summary.live_trading_enabled_count, 1);
    }

    // V2STATUS-03: non_equity_all_disabled is true when every non-equity
    // instrument is disabled (the only production-safe configuration).
    #[test]
    fn v2status_03_summary_non_equity_all_disabled_true_when_all_disabled() {
        let registry = registry_of(vec![
            base_equity("AAPL"),
            base_future("ES2026U"),
            base_crypto("BTC/USD"),
        ]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert!(summary.non_equity_present);
        assert_eq!(summary.non_equity_count, 2);
        assert!(summary.non_equity_all_disabled);
    }

    // V2STATUS-03b: an enabled non-equity instrument (test-only escape hatch)
    // flips non_equity_all_disabled to false -- the summary must not hide it.
    #[test]
    fn v2status_03b_summary_non_equity_all_disabled_false_when_any_enabled() {
        let mut enabled_future = base_future("ES2026U");
        enabled_future.enabled = true;
        enabled_future.allow_enabled_non_equity_for_testing = true;
        enabled_future
            .provider_symbols
            .insert("synthetic".to_string(), "ES".to_string());

        let registry = registry_of(vec![base_equity("AAPL"), enabled_future]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert!(!summary.non_equity_all_disabled);
    }

    // V2STATUS-03c: with zero non-equity instruments, non_equity_present is
    // false and non_equity_all_disabled is vacuously true.
    #[test]
    fn v2status_03c_summary_non_equity_all_disabled_vacuously_true_when_no_non_equity() {
        let registry = registry_of(vec![base_equity("AAPL"), base_etf("SPY")]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert!(!summary.non_equity_present);
        assert_eq!(summary.non_equity_count, 0);
        assert!(summary.non_equity_all_disabled);
    }

    // V2STATUS-04: has_economics_metadata is true iff at least one instrument
    // carries `economics`.
    #[test]
    fn v2status_04_summary_has_economics_metadata_true_when_any_present() {
        let with_econ = with_economics(
            base_future("ES2026U"),
            InstrumentEconomicsMetadataV2 {
                contract_multiplier: Some(50),
                initial_margin_micros: None,
                maintenance_margin_micros: None,
            },
        );
        let registry = registry_of(vec![base_equity("AAPL"), with_econ]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert!(summary.has_economics_metadata);
    }

    // V2STATUS-04b: has_economics_metadata is false when no instrument carries
    // `economics` -- must not default to true.
    #[test]
    fn v2status_04b_summary_has_economics_metadata_false_when_none_present() {
        let registry = registry_of(vec![base_equity("AAPL"), base_future("ES2026U")]);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert!(!summary.has_economics_metadata);
    }

    // V2STATUS-05: sample_symbols preserves registry order and is capped at
    // SAMPLE_SYMBOLS_LIMIT for a larger registry.
    #[test]
    fn v2status_05_summary_sample_symbols_capped_and_ordered() {
        let instruments: Vec<InstrumentDefinitionV2> = (0..(SAMPLE_SYMBOLS_LIMIT + 5))
            .map(|i| {
                let mut inst = base_equity(&format!("SYM{i}"));
                inst.instrument_id = format!("equity:US:SYM{i}");
                inst
            })
            .collect();
        let registry = registry_of(instruments);
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert_eq!(summary.sample_symbols.len(), SAMPLE_SYMBOLS_LIMIT);
        assert_eq!(summary.sample_symbols[0], "SYM0");
        assert_eq!(
            summary.sample_symbols[SAMPLE_SYMBOLS_LIMIT - 1],
            format!("SYM{}", SAMPLE_SYMBOLS_LIMIT - 1)
        );
    }

    // V2STATUS-06: the committed example v2 fixture summarizes to the exact
    // shape ASSET-CORE-01D's daemon route response documents: 3 instruments
    // (2 future + 1 crypto), all disabled, all carrying economics metadata.
    #[test]
    fn v2status_06_committed_example_fixture_summary_matches_expected_shape() {
        let registry = load_instrument_registry_v2(&instruments_v2_example_fixture_path())
            .expect("committed example v2 fixture must load");
        let summary = summarize_instrument_registry_v2_status(&registry);

        assert_eq!(summary.total_instruments, 3);
        assert_eq!(summary.asset_class_counts["future"], 2);
        assert_eq!(summary.asset_class_counts["crypto"], 1);
        assert_eq!(summary.enabled_count, 0);
        assert_eq!(summary.paper_trading_enabled_count, 0);
        assert_eq!(summary.live_trading_enabled_count, 0);
        assert!(summary.non_equity_present);
        assert!(summary.non_equity_all_disabled);
        assert!(summary.has_economics_metadata);
        assert_eq!(
            summary.sample_symbols,
            vec![
                "ES_TEST".to_string(),
                "MES_TEST".to_string(),
                "BTCUSD_TEST".to_string(),
            ]
        );
    }
}
