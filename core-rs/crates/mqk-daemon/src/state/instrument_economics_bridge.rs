//! ASSET-CORE-04B: pure, read-only bridge from registry-v2 instrument
//! definitions to the ASSET-CORE-04A portfolio economics model.
//!
//! Converts `mqk_md::instrument_registry_v2::InstrumentDefinitionV2` into
//! `mqk_portfolio::InstrumentEconomics`. This is a model/bridge seam, not a
//! production cutover:
//!
//! - Nothing in this module is called by any runtime, risk, order, broker,
//!   OMS, or accounting path. It exists so a future ASSET-CORE-04 slice can
//!   aggregate portfolio NAV / value positions by registry-v2 metadata
//!   without fabricating multipliers, currencies, or asset-class rules.
//! - [`InstrumentEconomicsBridgeResult::trading_enabled_by_bridge`] is always
//!   `false`. `enabled`/`paper_trading_enabled`/`live_trading_enabled` are
//!   carried through as metadata only -- this module never infers trading
//!   permission from them.
//! - Lives in `mqk-daemon` rather than `mqk-portfolio` or `mqk-md`:
//!   `mqk-daemon` already depends on both crates (it is the only crate that
//!   does), so bridging here adds zero new Cargo dependency edges. Neither
//!   `mqk-portfolio` (currently zero dependencies) nor `mqk-md` gains a new
//!   edge to the other.
//!
//! # Multiplier convention
//!
//! Registry-v2 `ContractDefinitionV2::{Future,Option}::multiplier` is a raw
//! integer (e.g. `50`). [`InstrumentEconomics::contract_multiplier_micros`]
//! uses `mqk_portfolio::MICROS_SCALE` (1e-6) fixed-point scaling, so this
//! module multiplies the raw integer by `MICROS_SCALE` to convert "50" into
//! `50_000_000`. Equity/ETF/crypto/forex have no contract-level multiplier
//! field, so they default to a raw multiplier of `1` (i.e. `MICROS_SCALE`,
//! "1.0x") unless overridden -- see below.
//!
//! An instrument's optional `economics.contract_multiplier`
//! (`InstrumentEconomicsMetadataV2`, any asset class) takes precedence over
//! both the contract-level multiplier and the default when present and
//! positive, mirroring the existing precedence rule in
//! `backtest_economics_suggestion_for_instrument`.
//!
//! # Currency safety
//!
//! [`InstrumentEconomics`] carries exactly one `quote_currency` field -- it
//! has no separate "base currency" concept, so it cannot faithfully
//! represent a genuinely cross-currency instrument (e.g. a forex pair settled
//! in a currency different from its quote leg). Rather than fabricate or
//! silently drop one side, this module refuses
//! (`currency_conversion_unsupported`) whenever a registry row's
//! `quote_currency` is present and differs from its `currency` (settlement)
//! field. This is a pure, self-contained check on the instrument's own two
//! currency fields -- it does not require, and this module does not invent,
//! any external "account currency" concept. In practice every real
//! committed registry row is equity/ETF (`quote_currency: None`, vacuously
//! safe); crypto/forex fixtures that quote in their own settlement currency
//! (e.g. `currency: "USD"`, `quote_currency: Some("USD")`) bridge cleanly,
//! while a fixture with mismatched fields refuses truthfully.
//!
//! # Quantity scale
//!
//! Registry-v2 has no `quantity_scale` field today. Equity/ETF/future/option
//! use whole-unit granularity (`MICROS_SCALE`, matching
//! [`InstrumentEconomics::equity`]). Crypto defaults to `1` (the finest
//! representable micros-grain) purely as descriptive model metadata
//! demonstrating fractional-quantity capability -- [`value_position_economics`]
//! never reads this field, so the choice has no effect on any computed
//! valuation. Forex retains whole-unit granularity since it is not
//! demonstrated as fractional-only by any fixture this patch proves.
//!
//! [`value_position_economics`]: mqk_portfolio::value_position_economics

use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentRegistryV2,
};
use mqk_portfolio::{InstrumentEconomics, MICROS_SCALE};

/// Result of bridging one [`InstrumentDefinitionV2`] into the ASSET-CORE-04A
/// economics model. `economics` is `None` whenever bridging fails closed --
/// never a fabricated default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentEconomicsBridgeResult {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub instrument_kind: Option<String>,
    /// Metadata only, carried through from the registry row. Never read by
    /// this module to decide whether to bridge.
    pub enabled: bool,
    pub paper_trading_enabled: bool,
    pub live_trading_enabled: bool,
    pub economics: Option<InstrumentEconomics>,
    /// `"active"` | `"missing_currency"` | `"currency_conversion_unsupported"`
    /// | `"missing_contract"` | `"missing_multiplier"` | `"invalid_multiplier"`
    /// | `"unsupported_instrument"`.
    pub truth_state: String,
    pub reason_code: String,
    /// `true` for every non-`"equity"` asset class (independent of
    /// success/failure) -- equity/ETF economics mirror this crate's existing
    /// live accounting assumptions; everything else is illustrative only.
    pub model_only: bool,
    /// Always `false`. This bridge never enables, implies, or decides
    /// trading permission for any instrument or asset class.
    pub trading_enabled_by_bridge: bool,
}

fn bridge_failure(
    instrument: &InstrumentDefinitionV2,
    model_only: bool,
    truth_state: &str,
    reason_code: &str,
) -> InstrumentEconomicsBridgeResult {
    InstrumentEconomicsBridgeResult {
        instrument_id: instrument.instrument_id.clone(),
        symbol: instrument.symbol.clone(),
        asset_class: instrument.asset_class.clone(),
        instrument_kind: instrument.instrument_kind.clone(),
        enabled: instrument.enabled,
        paper_trading_enabled: instrument.paper_trading_enabled,
        live_trading_enabled: instrument.live_trading_enabled,
        economics: None,
        truth_state: truth_state.to_string(),
        reason_code: reason_code.to_string(),
        model_only,
        trading_enabled_by_bridge: false,
    }
}

/// Bridge one registry-v2 instrument definition into the ASSET-CORE-04A
/// economics model. Pure; no IO; never panics on malformed input -- every
/// refusal returns `economics: None` with an explicit `truth_state` and
/// `reason_code` rather than fabricating a value. See module docs for the
/// currency-safety and multiplier-precedence rules.
pub fn instrument_v2_to_economics(
    instrument: &InstrumentDefinitionV2,
) -> InstrumentEconomicsBridgeResult {
    let model_only = instrument.asset_class != "equity";

    if instrument.currency.trim().is_empty() {
        return bridge_failure(
            instrument,
            model_only,
            "missing_currency",
            "instrument_currency_is_empty",
        );
    }

    if let Some(quote) = instrument.quote_currency.as_deref() {
        let quote = quote.trim();
        if !quote.is_empty() && quote != instrument.currency {
            return bridge_failure(
                instrument,
                model_only,
                "currency_conversion_unsupported",
                "registry_quote_currency_does_not_match_settlement_currency",
            );
        }
    }

    let explicit_multiplier_override = instrument.economics.and_then(|e| e.contract_multiplier);
    if let Some(m) = explicit_multiplier_override {
        if m <= 0 {
            return bridge_failure(
                instrument,
                model_only,
                "invalid_multiplier",
                "economics_contract_multiplier_must_be_positive",
            );
        }
    }

    let is_etf = instrument.instrument_kind.as_deref() == Some("etf");

    let (raw_multiplier, tick_size_micros): (i64, Option<i64>) =
        match instrument.asset_class.as_str() {
            "equity" if is_etf => match &instrument.contract {
                None | Some(ContractDefinitionV2::Etf) => {
                    (explicit_multiplier_override.unwrap_or(1), None)
                }
                Some(_) => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_contract",
                        "etf_contract_shape_invalid",
                    )
                }
            },
            "equity" => match &instrument.contract {
                None | Some(ContractDefinitionV2::Equity) => {
                    (explicit_multiplier_override.unwrap_or(1), None)
                }
                Some(_) => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_contract",
                        "equity_contract_shape_invalid",
                    )
                }
            },
            "future" => match &instrument.contract {
                Some(ContractDefinitionV2::Future {
                    multiplier,
                    tick_size_micros,
                    ..
                }) => {
                    let resolved = explicit_multiplier_override.unwrap_or(*multiplier);
                    if resolved <= 0 {
                        return bridge_failure(
                            instrument,
                            model_only,
                            "invalid_multiplier",
                            "future_contract_multiplier_must_be_positive",
                        );
                    }
                    (resolved, Some(*tick_size_micros))
                }
                _ => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_multiplier",
                        "future_contract_missing_or_wrong_variant",
                    )
                }
            },
            "option" => match &instrument.contract {
                Some(ContractDefinitionV2::Option { multiplier, .. }) => {
                    let resolved = explicit_multiplier_override.unwrap_or(*multiplier);
                    if resolved <= 0 {
                        return bridge_failure(
                            instrument,
                            model_only,
                            "invalid_multiplier",
                            "option_contract_multiplier_must_be_positive",
                        );
                    }
                    (resolved, None)
                }
                _ => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_multiplier",
                        "option_contract_missing_or_wrong_variant",
                    )
                }
            },
            "crypto" => match &instrument.contract {
                Some(ContractDefinitionV2::CryptoPair { .. }) => {
                    let resolved = explicit_multiplier_override.unwrap_or(1);
                    if resolved <= 0 {
                        return bridge_failure(
                            instrument,
                            model_only,
                            "invalid_multiplier",
                            "crypto_contract_multiplier_must_be_positive",
                        );
                    }
                    (resolved, None)
                }
                _ => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_contract",
                        "crypto_contract_missing_or_wrong_variant",
                    )
                }
            },
            "forex" => match &instrument.contract {
                Some(ContractDefinitionV2::ForexPair { .. }) => {
                    let resolved = explicit_multiplier_override.unwrap_or(1);
                    if resolved <= 0 {
                        return bridge_failure(
                            instrument,
                            model_only,
                            "invalid_multiplier",
                            "forex_contract_multiplier_must_be_positive",
                        );
                    }
                    (resolved, None)
                }
                _ => {
                    return bridge_failure(
                        instrument,
                        model_only,
                        "missing_contract",
                        "forex_contract_missing_or_wrong_variant",
                    )
                }
            },
            _ => {
                return bridge_failure(
                    instrument,
                    model_only,
                    "unsupported_instrument",
                    "unknown_asset_class",
                )
            }
        };

    let Some(contract_multiplier_micros) = raw_multiplier.checked_mul(MICROS_SCALE) else {
        return bridge_failure(
            instrument,
            model_only,
            "invalid_multiplier",
            "contract_multiplier_overflowed_i64",
        );
    };

    let quantity_scale = if instrument.asset_class == "crypto" {
        1
    } else {
        MICROS_SCALE
    };

    let economics = InstrumentEconomics {
        instrument_id: instrument.instrument_id.clone(),
        symbol: instrument.symbol.clone(),
        asset_class: instrument.asset_class.clone(),
        quote_currency: instrument.currency.clone(),
        contract_multiplier_micros,
        quantity_scale,
        min_trade_qty_micros: None,
        tick_size_micros,
    };

    InstrumentEconomicsBridgeResult {
        instrument_id: instrument.instrument_id.clone(),
        symbol: instrument.symbol.clone(),
        asset_class: instrument.asset_class.clone(),
        instrument_kind: instrument.instrument_kind.clone(),
        enabled: instrument.enabled,
        paper_trading_enabled: instrument.paper_trading_enabled,
        live_trading_enabled: instrument.live_trading_enabled,
        economics: Some(economics),
        truth_state: "active".to_string(),
        reason_code: "active".to_string(),
        model_only,
        trading_enabled_by_bridge: false,
    }
}

/// Read-only summary of bridging every instrument in a registry. Pure; no
/// IO. Order mirrors `registry.instruments` (deterministic, not sorted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentEconomicsBridgeSummary {
    pub total_instruments: usize,
    pub bridged_count: usize,
    pub failed_count: usize,
    pub model_only_count: usize,
    /// Count of registry rows that are both `enabled` and non-`"equity"`.
    /// Always `0` for any registry that passed
    /// `mqk_md::instrument_registry_v2::validate_registry_v2` without the
    /// test-only `allow_enabled_non_equity_for_testing` escape hatch.
    pub non_equity_enabled_count: usize,
    pub rows: Vec<InstrumentEconomicsBridgeResult>,
}

/// Bridge every instrument in `registry`. Pure; no IO; never mutates or
/// reads anything outside `registry`.
pub fn bridge_instrument_registry_v2_to_economics(
    registry: &InstrumentRegistryV2,
) -> InstrumentEconomicsBridgeSummary {
    let mut rows = Vec::with_capacity(registry.instruments.len());
    let mut bridged_count = 0usize;
    let mut failed_count = 0usize;
    let mut model_only_count = 0usize;
    let mut non_equity_enabled_count = 0usize;

    for inst in &registry.instruments {
        let result = instrument_v2_to_economics(inst);
        if result.economics.is_some() {
            bridged_count += 1;
        } else {
            failed_count += 1;
        }
        if result.model_only {
            model_only_count += 1;
        }
        if inst.enabled && inst.asset_class != "equity" {
            non_equity_enabled_count += 1;
        }
        rows.push(result);
    }

    InstrumentEconomicsBridgeSummary {
        total_instruments: registry.instruments.len(),
        bridged_count,
        failed_count,
        model_only_count,
        non_equity_enabled_count,
        rows,
    }
}
