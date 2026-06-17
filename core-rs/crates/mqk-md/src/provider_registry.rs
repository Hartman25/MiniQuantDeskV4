//! Canonical provider registry for market-data ingestion.
//!
//! The registry file at `config/providers/providers.json` is the single source of truth
//! for which data providers are supported, their asset class coverage, free-tier availability,
//! and current implementation status.
//!
//! # Multi-asset design notes
//! - Each provider entry declares which `asset_classes` it supports.
//! - Only providers with `enabled=true` should be used in production.
//! - Providers with `enabled=false` are listed as candidates for future implementation.
//! - All provider capability claims must be verified against official docs before use.
//! - `verification_status="requires_external_verification"` means the entry is based on
//!   public knowledge only; no repo-level test proves the claimed capabilities.

use crate::{
    AlpacaHistoricalProvider, FakeMarketDataProvider, HistoricalProviderMarketDataAdapter,
    MarketDataProvider, MarketDataProviderCapabilities, MarketDataProviderHealth,
    MarketDataProviderRateLimits, ProviderAssetClass, Timeframe, TwelveDataHistoricalProvider,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

/// One provider entry in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Unique stable identifier (e.g. `"twelvedata"`).
    pub provider_id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Asset classes this provider supports: `"equity"` | `"etf"` | `"crypto"` | `"futures"` | `"options"` | `"forex"`.
    pub asset_classes: Vec<String>,
    /// Whether a free tier is known to be available.
    pub free_tier_available: bool,
    /// Whether an API key is required.
    pub api_key_required: bool,
    /// Environment variable names used to load credentials. Values are never stored here.
    #[serde(default)]
    pub credential_env_vars: Vec<String>,
    /// Rate-limit notes for the operator; may require external verification.
    pub rate_limit_notes: String,
    /// Timeframes known to be supported (e.g. `["1D", "1m", "5m"]`).
    pub supported_timeframes: Vec<String>,
    /// Historical depth notes; requires external verification.
    pub historical_depth_notes: String,
    /// Real-time support notes; requires external verification.
    pub realtime_support_notes: String,
    /// Licensing notes; requires external verification.
    pub licensing_notes: String,
    /// Implementation status: `"implemented_equity_provider"` | `"candidate_unverified"`.
    pub implementation_status: String,
    /// Whether this provider is enabled for use in this version.
    pub enabled: bool,
    /// Verification status: `"repo_implemented_official_limits_unverified"` | `"requires_external_verification"`.
    pub verification_status: String,
    /// Official documentation URL. Empty string if unknown.
    pub docs_url: String,
}

impl ProviderConfig {
    /// Returns `true` if this provider declares support for the given asset class (case-insensitive).
    pub fn supports_asset_class(&self, asset_class: &str) -> bool {
        self.asset_classes
            .iter()
            .any(|ac| ac.eq_ignore_ascii_case(asset_class))
    }

    /// Returns `true` if this provider declares support for the given timeframe (case-insensitive).
    pub fn supports_timeframe(&self, timeframe: &str) -> bool {
        self.supported_timeframes
            .iter()
            .any(|tf| tf.eq_ignore_ascii_case(timeframe))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderFactoryError {
    UnknownProvider {
        provider_id: String,
    },
    DisabledProvider {
        provider_id: String,
    },
    MissingCredential {
        provider_id: String,
        credential: String,
    },
    UnsupportedProvider {
        provider_id: String,
    },
    InvalidProviderConfig {
        provider_id: String,
        message: String,
    },
}

impl ProviderFactoryError {
    pub fn code(&self) -> &'static str {
        match self {
            ProviderFactoryError::UnknownProvider { .. } => "unknown_provider",
            ProviderFactoryError::DisabledProvider { .. } => "disabled_provider",
            ProviderFactoryError::MissingCredential { .. } => "missing_credential",
            ProviderFactoryError::UnsupportedProvider { .. } => "unsupported_provider",
            ProviderFactoryError::InvalidProviderConfig { .. } => "invalid_provider_config",
        }
    }
}

impl fmt::Display for ProviderFactoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderFactoryError::UnknownProvider { provider_id } => {
                write!(
                    f,
                    "unknown_provider: provider '{provider_id}' is not registered"
                )
            }
            ProviderFactoryError::DisabledProvider { provider_id } => {
                write!(f, "disabled_provider: provider '{provider_id}' is disabled")
            }
            ProviderFactoryError::MissingCredential {
                provider_id,
                credential,
            } => write!(
                f,
                "missing_credential: provider '{provider_id}' requires credential '{credential}'"
            ),
            ProviderFactoryError::UnsupportedProvider { provider_id } => write!(
                f,
                "unsupported_provider: provider '{provider_id}' has no factory implementation"
            ),
            ProviderFactoryError::InvalidProviderConfig {
                provider_id,
                message,
            } => write!(
                f,
                "invalid_provider_config: provider '{provider_id}' config invalid: {message}"
            ),
        }
    }
}

impl std::error::Error for ProviderFactoryError {}

pub type MarketDataProviderBox = Box<dyn MarketDataProvider>;

fn asset_class_from_config(value: &str) -> ProviderAssetClass {
    match value.trim().to_ascii_lowercase().as_str() {
        "equity" => ProviderAssetClass::Equity,
        "etf" => ProviderAssetClass::Etf,
        "crypto" => ProviderAssetClass::Crypto,
        "futures" => ProviderAssetClass::Futures,
        "options" => ProviderAssetClass::Options,
        "forex" => ProviderAssetClass::Forex,
        other => ProviderAssetClass::Other(other.to_string()),
    }
}

fn timeframes_from_config(
    provider_id: &str,
    config: &ProviderConfig,
) -> Result<Vec<Timeframe>, ProviderFactoryError> {
    config
        .supported_timeframes
        .iter()
        .map(|tf| {
            Timeframe::parse(tf).map_err(|err| ProviderFactoryError::InvalidProviderConfig {
                provider_id: provider_id.to_string(),
                message: format!("unsupported timeframe '{tf}': {err}"),
            })
        })
        .collect()
}

pub fn capabilities_from_provider_config(
    config: &ProviderConfig,
) -> Result<MarketDataProviderCapabilities, ProviderFactoryError> {
    let provider_id = config.provider_id.trim();
    let supported_timeframes = timeframes_from_config(provider_id, config)?;
    Ok(MarketDataProviderCapabilities {
        historical_bars: !supported_timeframes.is_empty(),
        // Alpaca supports latest_closed_bar via a bounded historical window fetch.
        // Other providers are not advertised as capable until their implementation exists.
        latest_closed_bar: provider_id.eq_ignore_ascii_case("alpaca"),
        completed_bar_stream: false,
        supported_asset_classes: config
            .asset_classes
            .iter()
            .map(|asset_class| asset_class_from_config(asset_class))
            .collect(),
        supported_timeframes,
    })
}

pub fn provider_supports_historical_bars(
    config: &ProviderConfig,
) -> Result<bool, ProviderFactoryError> {
    capabilities_from_provider_config(config).map(|capabilities| capabilities.historical_bars)
}

pub fn build_market_data_provider_from_env(
    provider_id: &str,
    providers: &[ProviderConfig],
) -> Result<MarketDataProviderBox, ProviderFactoryError> {
    build_market_data_provider(provider_id, providers, |name| std::env::var(name).ok())
}

pub fn build_market_data_provider<F>(
    provider_id: &str,
    providers: &[ProviderConfig],
    env_lookup: F,
) -> Result<MarketDataProviderBox, ProviderFactoryError>
where
    F: Fn(&str) -> Option<String>,
{
    let config = find_provider(providers, provider_id).ok_or_else(|| {
        ProviderFactoryError::UnknownProvider {
            provider_id: provider_id.trim().to_ascii_lowercase(),
        }
    })?;
    build_market_data_provider_from_config(config, env_lookup)
}

pub fn build_market_data_provider_from_config<F>(
    config: &ProviderConfig,
    env_lookup: F,
) -> Result<MarketDataProviderBox, ProviderFactoryError>
where
    F: Fn(&str) -> Option<String>,
{
    let provider_id = config.provider_id.trim().to_ascii_lowercase();
    if !config.enabled {
        return Err(ProviderFactoryError::DisabledProvider { provider_id });
    }

    let capabilities = capabilities_from_provider_config(config)?;
    let rate_limits = MarketDataProviderRateLimits {
        calls_per_minute: None,
        calls_per_day: None,
        remaining_calls: None,
        notes: (!config.rate_limit_notes.trim().is_empty())
            .then(|| config.rate_limit_notes.clone()),
    };

    match provider_id.as_str() {
        "fake" => Ok(Box::new(
            FakeMarketDataProvider::new(vec![], None)
                .with_capabilities(capabilities)
                .with_rate_limits(Some(rate_limits)),
        )),
        "twelvedata" => {
            let key = credential_value(config, &env_lookup, 0)?;
            Ok(Box::new(
                HistoricalProviderMarketDataAdapter::new(
                    TwelveDataHistoricalProvider::new(key),
                    config.provider_id.clone(),
                    config.display_name.clone(),
                    capabilities.supported_timeframes,
                )
                .with_health(MarketDataProviderHealth::unknown())
                .with_rate_limits(rate_limits),
            ))
        }
        "alpaca" => {
            let key = credential_value(config, &env_lookup, 0)?;
            let secret = credential_value(config, &env_lookup, 1)?;
            Ok(Box::new(
                HistoricalProviderMarketDataAdapter::new(
                    AlpacaHistoricalProvider::new(key, secret),
                    config.provider_id.clone(),
                    config.display_name.clone(),
                    capabilities.supported_timeframes,
                )
                .with_latest_closed_bar_enabled()
                .with_health(MarketDataProviderHealth::unknown())
                .with_rate_limits(rate_limits),
            ))
        }
        _ => Err(ProviderFactoryError::UnsupportedProvider { provider_id }),
    }
}

fn credential_value<F>(
    config: &ProviderConfig,
    env_lookup: &F,
    index: usize,
) -> Result<String, ProviderFactoryError>
where
    F: Fn(&str) -> Option<String>,
{
    let provider_id = config.provider_id.trim().to_ascii_lowercase();
    let credential = config.credential_env_vars.get(index).ok_or_else(|| {
        ProviderFactoryError::InvalidProviderConfig {
            provider_id: provider_id.clone(),
            message: format!("missing credential_env_vars[{index}]"),
        }
    })?;
    env_lookup(credential)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ProviderFactoryError::MissingCredential {
            provider_id,
            credential: credential.clone(),
        })
}

/// Parse the provider registry JSON file at `path`.
///
/// Returns all entries in file order. The file must be a JSON array of `ProviderConfig` objects.
pub fn load_provider_registry(path: &Path) -> Result<Vec<ProviderConfig>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("provider registry read failed: {}", path.display()))?;
    let providers: Vec<ProviderConfig> = serde_json::from_str(&content)
        .with_context(|| format!("provider registry parse failed: {}", path.display()))?;
    Ok(providers)
}

/// Find a provider by `provider_id` (case-insensitive). Returns `None` if not found.
pub fn find_provider<'a>(
    providers: &'a [ProviderConfig],
    provider_id: &str,
) -> Option<&'a ProviderConfig> {
    providers
        .iter()
        .find(|p| p.provider_id.eq_ignore_ascii_case(provider_id))
}

/// Validate registry invariants. Returns `Ok(())` if all pass, or the first violation as `Err`.
///
/// Checks:
/// 1. All `provider_id` values are unique (case-insensitive).
/// 2. All `provider_id` values are non-empty.
/// 3. All `display_name` values are non-empty.
/// 4. All `asset_classes` lists are non-empty.
pub fn validate_provider_registry(providers: &[ProviderConfig]) -> Result<()> {
    let mut seen_ids: HashSet<String> = HashSet::new();

    for p in providers {
        if p.provider_id.trim().is_empty() {
            anyhow::bail!("provider_registry: empty provider_id");
        }
        if p.display_name.trim().is_empty() {
            anyhow::bail!(
                "provider_registry: empty display_name for provider_id={}",
                p.provider_id
            );
        }
        if p.asset_classes.is_empty() {
            anyhow::bail!(
                "provider_registry: empty asset_classes for provider_id={}",
                p.provider_id
            );
        }

        let id_lower = p.provider_id.to_ascii_lowercase();
        if !seen_ids.insert(id_lower) {
            anyhow::bail!("provider_registry: duplicate provider_id={}", p.provider_id);
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
        PathBuf::from(manifest_dir).join("../../../config/providers/providers.json")
    }

    // PR-01: registry parses without error
    #[test]
    fn pr_01_registry_parses_successfully() {
        let path = registry_path();
        let providers =
            load_provider_registry(&path).expect("provider registry must parse without error");
        assert!(
            !providers.is_empty(),
            "provider registry must contain at least one entry"
        );
    }

    // PR-02: all provider_id values are unique
    #[test]
    fn pr_02_provider_ids_are_unique() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        let mut seen: HashSet<String> = HashSet::new();
        for p in &providers {
            assert!(
                seen.insert(p.provider_id.to_ascii_lowercase()),
                "duplicate provider_id: {}",
                p.provider_id
            );
        }
    }

    // PR-03: disabled providers have verification_status != repo_implemented
    #[test]
    fn pr_03_disabled_providers_are_unverified_candidates() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        for p in providers.iter().filter(|p| !p.enabled) {
            assert_ne!(
                p.verification_status.as_str(),
                "repo_implemented_official_limits_unverified",
                "disabled provider {} must not have repo_implemented status",
                p.provider_id
            );
        }
    }

    // PR-04: twelvedata entry exists, is enabled, supports equity + 1D
    #[test]
    fn pr_04_twelvedata_exists_and_is_enabled() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        let td = find_provider(&providers, "twelvedata")
            .expect("twelvedata must be in the provider registry");
        assert!(td.enabled, "twelvedata must be enabled");
        assert!(
            td.supports_asset_class("equity"),
            "twelvedata must support asset_class=equity"
        );
        assert!(
            td.supports_timeframe("1D"),
            "twelvedata must support timeframe=1D"
        );
        assert_eq!(
            td.implementation_status.as_str(),
            "implemented_equity_provider",
            "twelvedata implementation_status must be implemented_equity_provider"
        );
    }

    // PR-05: validate_provider_registry passes on canonical file
    #[test]
    fn pr_05_validate_registry_passes() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        validate_provider_registry(&providers)
            .expect("validate_provider_registry must pass on canonical file");
    }

    // PR-06: find_provider returns None for unknown provider
    #[test]
    fn pr_06_unknown_provider_not_found() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        let result = find_provider(&providers, "not_a_real_provider_xyz");
        assert!(result.is_none(), "unknown provider must not be found");
    }

    // PR-07: validate_provider_registry rejects duplicate provider_id
    #[test]
    fn pr_07_validate_rejects_duplicate_provider_id() {
        let td = ProviderConfig {
            provider_id: "twelvedata".into(),
            display_name: "TwelveData".into(),
            asset_classes: vec!["equity".into()],
            free_tier_available: true,
            api_key_required: true,
            credential_env_vars: vec!["TWELVEDATA_API_KEY".into()],
            rate_limit_notes: "test".into(),
            supported_timeframes: vec!["1D".into()],
            historical_depth_notes: "test".into(),
            realtime_support_notes: "test".into(),
            licensing_notes: "test".into(),
            implementation_status: "test".into(),
            enabled: true,
            verification_status: "test".into(),
            docs_url: "".into(),
        };
        let duplicate = td.clone();
        let result = validate_provider_registry(&[td, duplicate]);
        assert!(result.is_err(), "must reject duplicate provider_id");
        assert!(
            result.unwrap_err().to_string().contains("duplicate"),
            "error must mention duplicate"
        );
    }

    // PR-08: supports_asset_class is case-insensitive
    #[test]
    fn pr_08_supports_asset_class_case_insensitive() {
        let p = ProviderConfig {
            provider_id: "test".into(),
            display_name: "Test".into(),
            asset_classes: vec!["equity".into(), "Crypto".into()],
            free_tier_available: true,
            api_key_required: false,
            credential_env_vars: vec![],
            rate_limit_notes: "".into(),
            supported_timeframes: vec!["1D".into()],
            historical_depth_notes: "".into(),
            realtime_support_notes: "".into(),
            licensing_notes: "".into(),
            implementation_status: "test".into(),
            enabled: true,
            verification_status: "test".into(),
            docs_url: "".into(),
        };
        assert!(p.supports_asset_class("equity"));
        assert!(p.supports_asset_class("EQUITY"));
        assert!(p.supports_asset_class("crypto"));
        assert!(p.supports_asset_class("CRYPTO"));
        assert!(!p.supports_asset_class("futures"));
    }

    // PR-09: twelvedata does NOT declare support for futures or options
    #[test]
    fn pr_09_twelvedata_does_not_support_futures_or_options() {
        let providers = load_provider_registry(&registry_path()).unwrap();
        let td = find_provider(&providers, "twelvedata").unwrap();
        assert!(
            !td.supports_asset_class("futures"),
            "twelvedata must not declare futures support"
        );
        assert!(
            !td.supports_asset_class("options"),
            "twelvedata must not declare options support"
        );
    }

    fn provider_config(
        provider_id: &str,
        enabled: bool,
        credential_env_vars: Vec<&str>,
        supported_timeframes: Vec<&str>,
    ) -> ProviderConfig {
        ProviderConfig {
            provider_id: provider_id.to_string(),
            display_name: format!("{provider_id} Display"),
            asset_classes: vec!["equity".into()],
            free_tier_available: true,
            api_key_required: !credential_env_vars.is_empty(),
            credential_env_vars: credential_env_vars
                .into_iter()
                .map(|value| value.to_string())
                .collect(),
            rate_limit_notes: "test limits".into(),
            supported_timeframes: supported_timeframes
                .into_iter()
                .map(|value| value.to_string())
                .collect(),
            historical_depth_notes: "test".into(),
            realtime_support_notes: "test".into(),
            licensing_notes: "test".into(),
            implementation_status: "implemented_equity_provider".into(),
            enabled,
            verification_status: "repo_implemented_official_limits_unverified".into(),
            docs_url: "".into(),
        }
    }

    // PROVIDER-FACTORY-01: fake provider can be built without env lookup.
    #[test]
    fn provider_factory_builds_fake_provider_with_injected_config() {
        let providers = vec![provider_config("fake", true, vec![], vec!["1D", "5m"])];

        let provider = build_market_data_provider("fake", &providers, |_| None)
            .expect("fake provider must build without credentials");

        assert_eq!(provider.provider_id(), "fake");
        assert!(provider.capabilities().historical_bars);
        assert!(provider
            .capabilities()
            .supported_timeframes
            .contains(&Timeframe::M5));
    }

    // PROVIDER-FACTORY-02: TwelveData construction requires injected credential presence.
    #[test]
    fn provider_factory_builds_twelvedata_with_injected_credential() {
        let providers = vec![provider_config(
            "twelvedata",
            true,
            vec!["TWELVEDATA_API_KEY"],
            vec!["1D"],
        )];

        let provider = build_market_data_provider("twelvedata", &providers, |name| {
            (name == "TWELVEDATA_API_KEY").then(|| "test-key".to_string())
        })
        .expect("twelvedata provider must build with injected credential");

        assert_eq!(provider.provider_id(), "twelvedata");
        assert!(provider.capabilities().historical_bars);
    }

    // PROVIDER-FACTORY-03: missing credential is typed and does not contain the secret value.
    #[test]
    fn provider_factory_missing_credential_fails_without_secret_value() {
        let providers = vec![provider_config(
            "twelvedata",
            true,
            vec!["TWELVEDATA_API_KEY"],
            vec!["1D"],
        )];

        let err = match build_market_data_provider("twelvedata", &providers, |_| None) {
            Ok(_) => panic!("twelvedata provider must fail without credential"),
            Err(err) => err,
        };

        assert_eq!(
            err,
            ProviderFactoryError::MissingCredential {
                provider_id: "twelvedata".into(),
                credential: "TWELVEDATA_API_KEY".into()
            }
        );
        assert_eq!(err.code(), "missing_credential");
        assert!(!err.to_string().contains("test-key"));
    }

    // PROVIDER-FACTORY-04: unknown provider id is typed.
    #[test]
    fn provider_factory_unknown_provider_fails_truthfully() {
        let providers = vec![provider_config("fake", true, vec![], vec!["1D"])];

        let err = match build_market_data_provider("not_registered", &providers, |_| None) {
            Ok(_) => panic!("unknown provider must fail"),
            Err(err) => err,
        };

        assert_eq!(
            err,
            ProviderFactoryError::UnknownProvider {
                provider_id: "not_registered".into()
            }
        );
        assert_eq!(err.code(), "unknown_provider");
    }

    // PROVIDER-FACTORY-05: disabled provider is typed and refused before credentials.
    #[test]
    fn provider_factory_disabled_provider_fails_truthfully() {
        let providers = vec![provider_config(
            "twelvedata",
            false,
            vec!["TWELVEDATA_API_KEY"],
            vec!["1D"],
        )];

        let err = match build_market_data_provider("twelvedata", &providers, |_| {
            Some("test-key".to_string())
        }) {
            Ok(_) => panic!("disabled provider must fail"),
            Err(err) => err,
        };

        assert_eq!(
            err,
            ProviderFactoryError::DisabledProvider {
                provider_id: "twelvedata".into()
            }
        );
        assert_eq!(err.code(), "disabled_provider");
    }

    // PROVIDER-FACTORY-06: Alpaca factory path uses two injected credentials.
    #[test]
    fn provider_factory_builds_alpaca_with_injected_credentials() {
        let providers = vec![provider_config(
            "alpaca",
            true,
            vec!["ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER"],
            vec!["1D", "5m"],
        )];

        let provider = build_market_data_provider("alpaca", &providers, |name| match name {
            "ALPACA_API_KEY_PAPER" => Some("paper-key".to_string()),
            "ALPACA_API_SECRET_PAPER" => Some("paper-secret".to_string()),
            _ => None,
        })
        .expect("alpaca provider must build with injected credentials");

        assert_eq!(provider.provider_id(), "alpaca");
        assert!(provider.capabilities().historical_bars);
        assert!(provider
            .capabilities()
            .supported_timeframes
            .contains(&Timeframe::M5));
    }

    // PROVIDER-FACTORY-07: capability metadata is available without constructing credentials.
    #[test]
    fn provider_factory_capabilities_are_exposed_from_config() {
        let config = provider_config("fake", true, vec![], vec!["1D", "1m"]);

        let capabilities = capabilities_from_provider_config(&config)
            .expect("capabilities must derive from config");

        assert!(capabilities.historical_bars);
        assert!(!capabilities.latest_closed_bar);
        assert!(capabilities
            .supported_asset_classes
            .contains(&ProviderAssetClass::Equity));
        assert!(capabilities.supported_timeframes.contains(&Timeframe::M1));
    }

    // ALPACA-CAP-01: Alpaca provider config reports latest_closed_bar=true.
    #[test]
    fn alpaca_provider_config_reports_latest_closed_bar_true() {
        let config = provider_config(
            "alpaca",
            true,
            vec!["ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER"],
            vec!["1D", "5m"],
        );

        let capabilities = capabilities_from_provider_config(&config)
            .expect("alpaca capabilities must derive from config");

        assert!(
            capabilities.latest_closed_bar,
            "alpaca must advertise latest_closed_bar=true"
        );
        assert!(capabilities.historical_bars);
    }

    // ALPACA-CAP-02: Non-Alpaca provider configs do not advertise latest_closed_bar.
    #[test]
    fn non_alpaca_provider_configs_do_not_report_latest_closed_bar() {
        for id in &["twelvedata", "fake", "polygon", "candidate_xyz"] {
            let config = provider_config(id, true, vec![], vec!["1D"]);
            let capabilities = capabilities_from_provider_config(&config)
                .expect("capabilities must derive from config");
            assert!(
                !capabilities.latest_closed_bar,
                "provider '{id}' must not advertise latest_closed_bar"
            );
        }
    }

    // ALPACA-CAP-03: Alpaca factory builds a provider that reports latest_closed_bar=true.
    #[test]
    fn alpaca_factory_provider_reports_latest_closed_bar_capability() {
        let providers = vec![provider_config(
            "alpaca",
            true,
            vec!["ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER"],
            vec!["1D", "5m"],
        )];

        let provider = build_market_data_provider("alpaca", &providers, |name| match name {
            "ALPACA_API_KEY_PAPER" => Some("paper-key".to_string()),
            "ALPACA_API_SECRET_PAPER" => Some("paper-secret".to_string()),
            _ => None,
        })
        .expect("alpaca provider must build with injected credentials");

        assert!(
            provider.capabilities().latest_closed_bar,
            "alpaca factory provider must report latest_closed_bar=true"
        );
        assert_eq!(provider.provider_id(), "alpaca");
    }
}
