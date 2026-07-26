//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase E — closed runtime mode switch.
//!
//! `MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE` controls whether Bundle 5's
//! opportunity allocator has any influence on the runtime dispatch loop.
//! Mirrors the closed-enum, fail-closed-on-unknown style of
//! `state/env.rs::parse_deployment_mode` — an unrecognized value never
//! panics and never silently picks an arbitrary mode; it resolves to `Off`
//! with an explicit, operator-visible `invalid_configuration` flag.
//!
//! Pure: no I/O beyond the single env-var read in the `_from_env` entry
//! point; every other function takes already-read values.

use crate::state::{BrokerKind, DeploymentMode};

pub const RUNTIME_OPPORTUNITY_ALLOCATION_MODE_ENV: &str = "MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE";

/// Closed set of runtime modes (task-frozen vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOpportunityAllocationMode {
    /// Exact pre-Bundle-5 behavior. Allocator is never consulted; no
    /// allocation DB write; no allocation effect.
    Off,
    /// Build and persist the allocation plan; zero allocator-driven outbox
    /// changes; existing strategy decisions continue unchanged.
    Shadow,
    /// Allocator output clamps/refuses new/increasing buy targets before
    /// they reach `submit_internal_strategy_decision`. Only reachable when
    /// [`effective_mode`] confirms the paper+Alpaca live-lock.
    PaperEnforced,
}

impl RuntimeOpportunityAllocationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::PaperEnforced => "paper_enforced",
        }
    }
}

/// Result of parsing the raw env value, before any deployment-mode live-lock
/// is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOpportunityAllocationModeResolution {
    /// The mode as configured (or `Off` on absence/unrecognized value).
    pub configured_mode: RuntimeOpportunityAllocationMode,
    /// `Some(raw_value)` when the configured value was non-empty but did
    /// not match any recognized mode string. The daemon still runs as
    /// `Off` in this case, but Phase H's status route must surface this
    /// distinctly from an intentional, valid `Off`.
    pub invalid_configuration: Option<String>,
}

/// Parse a raw env-var value into a [`RuntimeOpportunityAllocationModeResolution`].
///
/// Absent/blank → `Off`, no invalid-configuration flag (this is the honest
/// default, not a misconfiguration). Any non-empty value that is not one of
/// `off` / `shadow` / `paper_enforced` (case-insensitive, trimmed) → `Off`
/// with `invalid_configuration = Some(<value>)`.
pub fn resolve_runtime_opportunity_allocation_mode(
    raw: Option<&str>,
) -> RuntimeOpportunityAllocationModeResolution {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(value) = trimmed else {
        return RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: None,
        };
    };
    match value.to_ascii_lowercase().as_str() {
        "off" => RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: None,
        },
        "shadow" => RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Shadow,
            invalid_configuration: None,
        },
        "paper_enforced" => RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::PaperEnforced,
            invalid_configuration: None,
        },
        _ => RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: Some(value.to_string()),
        },
    }
}

/// [`resolve_runtime_opportunity_allocation_mode`], reading
/// [`RUNTIME_OPPORTUNITY_ALLOCATION_MODE_ENV`] from the environment.
pub fn resolve_runtime_opportunity_allocation_mode_from_env(
) -> RuntimeOpportunityAllocationModeResolution {
    let raw = std::env::var(RUNTIME_OPPORTUNITY_ALLOCATION_MODE_ENV).ok();
    resolve_runtime_opportunity_allocation_mode(raw.as_deref())
}

/// The fully-resolved mode, after applying the deployment-mode live-lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveRuntimeOpportunityAllocationMode {
    pub configured_mode: RuntimeOpportunityAllocationMode,
    pub effective_mode: RuntimeOpportunityAllocationMode,
    pub invalid_configuration: Option<String>,
    /// `true` when the deployment context was not `paper` + `alpaca` and the
    /// live-lock forced `effective_mode` down to `Off` regardless of what
    /// was configured.
    pub live_lock_applied: bool,
}

/// Apply the hard live-lock: any configured mode other than `Off` has zero
/// effect outside `deployment_mode == paper` with `adapter == alpaca`. This
/// runs *before* any allocator call — Phase F must consult this, not the raw
/// resolution, before treating a non-`Off` mode as active.
///
/// The live-lock demotes all the way to `Off` (not `Shadow`) for a
/// non-paper/non-Alpaca deployment: even Shadow mode is a paper-only
/// evidence path (it still reads the durable snapshot and opportunity
/// artifact), and Bundle 5 must have zero footprint of any kind outside the
/// frozen paper+Alpaca operating lane.
pub fn effective_mode(
    resolution: &RuntimeOpportunityAllocationModeResolution,
    deployment_mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> EffectiveRuntimeOpportunityAllocationMode {
    let is_paper_alpaca =
        deployment_mode == DeploymentMode::Paper && broker_kind == Some(BrokerKind::Alpaca);

    if resolution.configured_mode != RuntimeOpportunityAllocationMode::Off && !is_paper_alpaca {
        return EffectiveRuntimeOpportunityAllocationMode {
            configured_mode: resolution.configured_mode,
            effective_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: resolution.invalid_configuration.clone(),
            live_lock_applied: true,
        };
    }

    EffectiveRuntimeOpportunityAllocationMode {
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
        let r = resolve_runtime_opportunity_allocation_mode(None);
        assert_eq!(r.configured_mode, RuntimeOpportunityAllocationMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn blank_env_resolves_to_off_no_invalid_flag() {
        let r = resolve_runtime_opportunity_allocation_mode(Some("   "));
        assert_eq!(r.configured_mode, RuntimeOpportunityAllocationMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn recognizes_all_three_modes_case_insensitively() {
        assert_eq!(
            resolve_runtime_opportunity_allocation_mode(Some("Off")).configured_mode,
            RuntimeOpportunityAllocationMode::Off
        );
        assert_eq!(
            resolve_runtime_opportunity_allocation_mode(Some("SHADOW")).configured_mode,
            RuntimeOpportunityAllocationMode::Shadow
        );
        assert_eq!(
            resolve_runtime_opportunity_allocation_mode(Some(" paper_enforced ")).configured_mode,
            RuntimeOpportunityAllocationMode::PaperEnforced
        );
    }

    #[test]
    fn unknown_value_fails_closed_to_off_with_invalid_flag() {
        let r = resolve_runtime_opportunity_allocation_mode(Some("enforced"));
        assert_eq!(r.configured_mode, RuntimeOpportunityAllocationMode::Off);
        assert_eq!(r.invalid_configuration, Some("enforced".to_string()));
    }

    #[test]
    fn paper_alpaca_passes_through_unchanged() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(
            eff.effective_mode,
            RuntimeOpportunityAllocationMode::PaperEnforced
        );
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn live_capital_forces_off_even_when_paper_enforced_configured() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveCapital,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, RuntimeOpportunityAllocationMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn live_shadow_forces_off_even_for_shadow_mode() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Shadow,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveShadow,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, RuntimeOpportunityAllocationMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn non_alpaca_adapter_forces_off_even_in_paper_mode() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Paper));
        assert_eq!(eff.effective_mode, RuntimeOpportunityAllocationMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn off_configured_mode_never_triggers_live_lock() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::LiveCapital, None);
        assert_eq!(eff.effective_mode, RuntimeOpportunityAllocationMode::Off);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn invalid_configuration_flag_survives_live_lock() {
        let resolution = RuntimeOpportunityAllocationModeResolution {
            configured_mode: RuntimeOpportunityAllocationMode::Off,
            invalid_configuration: Some("bogus".to_string()),
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.invalid_configuration, Some("bogus".to_string()));
    }
}
