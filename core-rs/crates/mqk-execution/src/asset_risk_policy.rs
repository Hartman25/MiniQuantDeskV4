use mqk_schemas::AssetClass;

use crate::types::{IntentV2Contract, IntentV2Routability, IntentV2Validation, OrderIntentV2};

/// ASSET-CORE-03 model-only policy state.
///
/// This is not a production enforcement verdict. Current production order
/// submission is still guarded by the existing equity-only signal and broker
/// submit gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetRiskPolicyState {
    Enabled,
    Disabled,
    ResearchOnly,
    Unsupported,
}

/// Static per-asset risk-policy truth used by the model-only router foundation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRiskPolicy {
    pub asset_class: &'static str,
    pub state: AssetRiskPolicyState,
    pub paper_trading_enabled: bool,
    pub live_trading_enabled: bool,
    pub requires_margin_model: bool,
    pub requires_contract_multiplier: bool,
    pub requires_session_profile: bool,
    pub requires_currency_conversion: bool,
    pub reason_code: &'static str,
    pub message: &'static str,
}

/// Model-only route decision. `AllowedEquity` means the intent is only an
/// equity candidate for future policy layers; it does not submit or route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetRiskRouteDecision {
    AllowedEquity,
    DisabledAssetClass,
    ResearchOnly,
    Unsupported,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRiskRouteEvaluation {
    pub decision: AssetRiskRouteDecision,
    pub validation: IntentV2Validation,
    pub policy: AssetRiskPolicy,
    pub reason_code: &'static str,
    pub message: &'static str,
}

pub const ASSET_RISK_POLICY_SOURCE: &str = "static_default_foundation";
pub const ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED: bool = false;
pub const ASSET_RISK_NON_EQUITY_ROUTING_ENABLED: bool = false;

pub fn default_asset_risk_policies() -> Vec<AssetRiskPolicy> {
    vec![
        equity_policy(),
        etf_as_equity_policy(),
        crypto_policy(),
        future_policy(),
        option_policy(),
        forex_policy(),
        rates_fixed_income_policy(),
    ]
}

pub fn asset_risk_policy_for_asset_class(asset_class: &str) -> AssetRiskPolicy {
    match normalize_asset_class(asset_class).as_str() {
        "equity" | "us_equities" => equity_policy(),
        "etf_as_equity" | "etf" => etf_as_equity_policy(),
        "crypto" => crypto_policy(),
        "future" | "futures" => future_policy(),
        "option" | "options" => option_policy(),
        "forex" | "fx" => forex_policy(),
        "rates_fixed_income" | "rates" | "fixed_income" => rates_fixed_income_policy(),
        _ => unsupported_policy(),
    }
}

pub fn evaluate_asset_risk_for_order_intent_v2(intent: &OrderIntentV2) -> AssetRiskRouteEvaluation {
    let validation = intent.validate_model();
    if !validation.valid || validation.routability == IntentV2Routability::Invalid {
        return AssetRiskRouteEvaluation {
            decision: AssetRiskRouteDecision::Invalid,
            validation,
            policy: unsupported_policy(),
            reason_code: "intent_invalid",
            message: "OrderIntentV2 is invalid; asset-risk policy routing is not evaluated.",
        };
    }

    let policy = policy_for_intent(intent);
    let decision = match validation.routability {
        IntentV2Routability::EquityRoutableCandidate => match policy.state {
            AssetRiskPolicyState::Enabled => AssetRiskRouteDecision::AllowedEquity,
            AssetRiskPolicyState::ResearchOnly => AssetRiskRouteDecision::ResearchOnly,
            AssetRiskPolicyState::Disabled => AssetRiskRouteDecision::DisabledAssetClass,
            AssetRiskPolicyState::Unsupported => AssetRiskRouteDecision::Unsupported,
        },
        IntentV2Routability::ResearchOnly => AssetRiskRouteDecision::ResearchOnly,
        IntentV2Routability::DisabledAssetClass => AssetRiskRouteDecision::DisabledAssetClass,
        IntentV2Routability::Invalid => AssetRiskRouteDecision::Invalid,
    };

    AssetRiskRouteEvaluation {
        decision,
        validation,
        policy,
        reason_code: match decision {
            AssetRiskRouteDecision::AllowedEquity => "equity_model_candidate",
            AssetRiskRouteDecision::DisabledAssetClass => "asset_class_disabled",
            AssetRiskRouteDecision::ResearchOnly => "research_only",
            AssetRiskRouteDecision::Unsupported => "unsupported_asset_class",
            AssetRiskRouteDecision::Invalid => "intent_invalid",
        },
        message: match decision {
            AssetRiskRouteDecision::AllowedEquity => {
                "Asset risk router foundation classifies this as an equity model-level candidate only; production routing remains unchanged."
            }
            AssetRiskRouteDecision::DisabledAssetClass => {
                "Asset class is disabled for production routing by policy; existing equity-only gates remain the active enforcement boundary."
            }
            AssetRiskRouteDecision::ResearchOnly => {
                "Asset class is research-only in this model and is not production routable."
            }
            AssetRiskRouteDecision::Unsupported => {
                "Asset class is unsupported by the asset risk router foundation."
            }
            AssetRiskRouteDecision::Invalid => {
                "OrderIntentV2 is invalid before asset-risk policy routing."
            }
        },
    }
}

pub fn evaluate_asset_risk_for_order_intent_v2_with_caller_routing_request(
    intent: &OrderIntentV2,
    _caller_routing_requested: bool,
) -> AssetRiskRouteEvaluation {
    evaluate_asset_risk_for_order_intent_v2(intent)
}

fn policy_for_intent(intent: &OrderIntentV2) -> AssetRiskPolicy {
    match intent.instrument.asset_class {
        AssetClass::Equity => match &intent.contract {
            IntentV2Contract::Equity {
                instrument_kind: Some(kind),
            } if kind.eq_ignore_ascii_case("etf") => etf_as_equity_policy(),
            _ => equity_policy(),
        },
        AssetClass::Crypto => crypto_policy(),
        AssetClass::Future => future_policy(),
        AssetClass::Option => option_policy(),
        AssetClass::Forex => forex_policy(),
    }
}

fn normalize_asset_class(asset_class: &str) -> String {
    asset_class.trim().to_ascii_lowercase().replace('-', "_")
}

fn equity_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "equity",
        state: AssetRiskPolicyState::Enabled,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        requires_margin_model: false,
        requires_contract_multiplier: false,
        requires_session_profile: true,
        requires_currency_conversion: false,
        reason_code: "equity_enabled_model_candidate",
        message: "US equity is the only enabled executable asset class in the current model; this does not bypass production gates.",
    }
}

fn etf_as_equity_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "etf_as_equity",
        state: AssetRiskPolicyState::Enabled,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        requires_margin_model: false,
        requires_contract_multiplier: false,
        requires_session_profile: true,
        requires_currency_conversion: false,
        reason_code: "etf_inherits_equity_policy",
        message: "ETF is modeled as equity plus instrument_kind=etf; existing ETF sector-risk metadata remains separate.",
    }
}

fn crypto_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "crypto",
        state: AssetRiskPolicyState::Disabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: false,
        requires_contract_multiplier: false,
        requires_session_profile: true,
        requires_currency_conversion: true,
        reason_code: "crypto_disabled_missing_pair_registry_and_risk_model",
        message: "Crypto requires pair registry, fractional quantity policy, 24/7 session profile, and crypto risk model before routing.",
    }
}

fn future_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "future",
        state: AssetRiskPolicyState::Disabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: true,
        requires_contract_multiplier: true,
        requires_session_profile: true,
        requires_currency_conversion: false,
        reason_code: "future_disabled_missing_margin_multiplier_expiry_session_model",
        message: "Futures require margin model, contract multiplier, expiry handling, and futures session calendar before routing.",
    }
}

fn option_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "option",
        state: AssetRiskPolicyState::Disabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: true,
        requires_contract_multiplier: true,
        requires_session_profile: true,
        requires_currency_conversion: false,
        reason_code: "option_disabled_missing_chain_greeks_assignment_model",
        message: "Options require chain metadata, contract multiplier, Greeks, assignment, and margin risk model before routing.",
    }
}

fn forex_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "forex",
        state: AssetRiskPolicyState::Disabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: true,
        requires_contract_multiplier: false,
        requires_session_profile: true,
        requires_currency_conversion: true,
        reason_code: "forex_disabled_missing_pair_pip_lot_leverage_session_model",
        message: "Forex requires pair registry, pip/lot sizing, leverage, currency conversion, and 24x5 session model before routing.",
    }
}

fn rates_fixed_income_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "rates_fixed_income",
        state: AssetRiskPolicyState::ResearchOnly,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: true,
        requires_contract_multiplier: false,
        requires_session_profile: true,
        requires_currency_conversion: false,
        reason_code: "rates_fixed_income_research_only",
        message: "Rates and fixed-income exposure is represented only through current equity/ETF metadata unless a future product-specific policy enables it.",
    }
}

fn unsupported_policy() -> AssetRiskPolicy {
    AssetRiskPolicy {
        asset_class: "unsupported",
        state: AssetRiskPolicyState::Unsupported,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        requires_margin_model: true,
        requires_contract_multiplier: true,
        requires_session_profile: true,
        requires_currency_conversion: true,
        reason_code: "unsupported_asset_class",
        message: "Asset class is not represented in the static asset risk router foundation.",
    }
}
