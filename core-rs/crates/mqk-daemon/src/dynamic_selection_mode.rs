//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 2 — closed runtime mode
//! switch.
//!
//! `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE` controls whether Bundle 7's
//! dynamic strategy-symbol selection has any influence on the runtime
//! dispatch loop. Mirrors `runtime_strategy_conflict_mode.rs`'s closed-enum,
//! fail-closed-on-unknown style exactly — an unrecognized value never
//! panics and never silently picks an arbitrary mode; it resolves to `Off`
//! with an explicit, operator-visible `invalid_configuration` flag.
//!
//! Reuses `mqk_portfolio::DynamicSelectionMode` (the pure crate's own closed
//! enum) rather than duplicating it — one canonical parser
//! ([`DynamicSelectionMode::parse`]) is shared between this env-resolution
//! entry point and any future read-side evidence validator, so the two can
//! never drift into accepting different vocabularies.
//!
//! Pure: no I/O beyond the single env-var read in the `_from_env` entry
//! point; every other function takes already-read values.

use crate::state::{BrokerKind, DeploymentMode};
use mqk_portfolio::DynamicSelectionMode;

pub const DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV: &str =
    "MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE";

/// Result of parsing the raw env value, before any deployment-mode live-lock
/// is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicSelectionModeResolution {
    /// The mode as configured (or `Off` on absence/unrecognized value).
    pub configured_mode: DynamicSelectionMode,
    /// `Some(raw_value)` when the configured value was non-empty but did
    /// not match any recognized mode string. The daemon still runs as
    /// `Off` in this case, but the status route must surface this
    /// distinctly from an intentional, valid `Off`.
    pub invalid_configuration: Option<String>,
}

/// Parse a raw env-var value into a [`DynamicSelectionModeResolution`].
///
/// Absent/blank → `Off`, no invalid-configuration flag (this is the honest
/// default, not a misconfiguration). Any non-empty value that
/// [`DynamicSelectionMode::parse`] does not recognize (exact, trimmed,
/// lowercase `off` / `shadow` / `paper_enforced`) → `Off` with
/// `invalid_configuration = Some(<value>)`. Never guesses a mode for an
/// unrecognized value.
pub fn resolve_dynamic_selection_mode(raw: Option<&str>) -> DynamicSelectionModeResolution {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(value) = trimmed else {
        return DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Off,
            invalid_configuration: None,
        };
    };
    match DynamicSelectionMode::parse(value) {
        Some(mode) => DynamicSelectionModeResolution {
            configured_mode: mode,
            invalid_configuration: None,
        },
        None => DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Off,
            invalid_configuration: Some(value.to_string()),
        },
    }
}

/// [`resolve_dynamic_selection_mode`], reading
/// [`DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV`] from the environment.
pub fn resolve_dynamic_selection_mode_from_env() -> DynamicSelectionModeResolution {
    let raw = std::env::var(DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV).ok();
    resolve_dynamic_selection_mode(raw.as_deref())
}

/// The fully-resolved mode, after applying the deployment-mode live-lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveDynamicSelectionMode {
    pub configured_mode: DynamicSelectionMode,
    pub effective_mode: DynamicSelectionMode,
    pub invalid_configuration: Option<String>,
    /// `true` when the deployment context was not `paper` + `alpaca` and the
    /// live-lock forced `effective_mode` down to `Off` regardless of what
    /// was configured.
    pub live_lock_applied: bool,
}

/// Apply the hard live-lock: any configured mode other than `Off` has zero
/// effect outside `deployment_mode == paper` with `adapter == alpaca`. This
/// runs *before* any selection-plan construction — Phase 4's insertion
/// point must consult this, not the raw resolution, before treating a
/// non-`Off` mode as active.
///
/// The live-lock demotes all the way to `Off` (not `Shadow`) for a
/// non-paper/non-Alpaca deployment — mirrors
/// `runtime_strategy_conflict_mode::effective_mode` exactly: even Shadow
/// mode is a paper-only evidence path, and Bundle 7 must have zero
/// footprint of any kind (no promotion query, no evidence persistence, no
/// host construction) outside the frozen paper+Alpaca operating lane. This
/// is a strict superset of the task's literal requirement ("`paper_enforced`
/// effective only for deployment Paper + adapter Alpaca") — extending the
/// same protection to `Shadow` is the fail-closed choice, never a
/// violation, and matches the precedent this codebase already established
/// for Bundle 6.
pub fn effective_mode(
    resolution: &DynamicSelectionModeResolution,
    deployment_mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> EffectiveDynamicSelectionMode {
    let is_paper_alpaca =
        deployment_mode == DeploymentMode::Paper && broker_kind == Some(BrokerKind::Alpaca);

    if resolution.configured_mode != DynamicSelectionMode::Off && !is_paper_alpaca {
        return EffectiveDynamicSelectionMode {
            configured_mode: resolution.configured_mode,
            effective_mode: DynamicSelectionMode::Off,
            invalid_configuration: resolution.invalid_configuration.clone(),
            live_lock_applied: true,
        };
    }

    EffectiveDynamicSelectionMode {
        configured_mode: resolution.configured_mode,
        effective_mode: resolution.configured_mode,
        invalid_configuration: resolution.invalid_configuration.clone(),
        live_lock_applied: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_env_resolves_to_off_no_invalid_flag() {
        let r = resolve_dynamic_selection_mode(None);
        assert_eq!(r.configured_mode, DynamicSelectionMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn blank_env_resolves_to_off_no_invalid_flag() {
        let r = resolve_dynamic_selection_mode(Some("   "));
        assert_eq!(r.configured_mode, DynamicSelectionMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn recognizes_all_three_modes_exactly() {
        assert_eq!(
            resolve_dynamic_selection_mode(Some("off")).configured_mode,
            DynamicSelectionMode::Off
        );
        assert_eq!(
            resolve_dynamic_selection_mode(Some("shadow")).configured_mode,
            DynamicSelectionMode::Shadow
        );
        assert_eq!(
            resolve_dynamic_selection_mode(Some(" paper_enforced ")).configured_mode,
            DynamicSelectionMode::PaperEnforced
        );
    }

    #[test]
    fn miscased_value_fails_closed_to_off_with_invalid_flag() {
        // DynamicSelectionMode::parse is exact-lowercase-only -- "OFF" is
        // not a valid spelling of "off" and must never guess.
        let r = resolve_dynamic_selection_mode(Some("OFF"));
        assert_eq!(r.configured_mode, DynamicSelectionMode::Off);
        assert_eq!(r.invalid_configuration, Some("OFF".to_string()));
    }

    #[test]
    fn unknown_value_fails_closed_to_off_with_invalid_flag() {
        let r = resolve_dynamic_selection_mode(Some("enforced"));
        assert_eq!(r.configured_mode, DynamicSelectionMode::Off);
        assert_eq!(r.invalid_configuration, Some("enforced".to_string()));
    }

    #[test]
    fn paper_alpaca_passes_through_unchanged() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.effective_mode, DynamicSelectionMode::PaperEnforced);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn shadow_paper_alpaca_passes_through_unchanged() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Shadow,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Shadow);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn live_capital_forces_off_even_when_paper_enforced_configured() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveCapital,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn live_shadow_forces_off_even_for_shadow_mode() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Shadow,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveShadow,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn non_alpaca_adapter_forces_off_even_in_paper_mode() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Paper));
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn missing_broker_kind_forces_off_when_mode_configured() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, None);
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn off_configured_mode_never_triggers_live_lock() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Off,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::LiveCapital, None);
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn invalid_configuration_flag_survives_live_lock() {
        let resolution = DynamicSelectionModeResolution {
            configured_mode: DynamicSelectionMode::Off,
            invalid_configuration: Some("bogus".to_string()),
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.invalid_configuration, Some("bogus".to_string()));
    }

    #[test]
    fn invalid_configuration_flag_survives_off_effective_mode() {
        // An unrecognized configured value is Off by construction, so it
        // never triggers the live-lock branch -- but the invalid flag must
        // still surface through effective_mode for the status route.
        let resolution = resolve_dynamic_selection_mode(Some("bogus_mode"));
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveCapital,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, DynamicSelectionMode::Off);
        assert!(!eff.live_lock_applied);
        assert_eq!(eff.invalid_configuration, Some("bogus_mode".to_string()));
    }
}
