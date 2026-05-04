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

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
}
