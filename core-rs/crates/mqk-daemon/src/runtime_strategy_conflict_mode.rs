//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase B — closed runtime mode switch.
//!
//! `MQK_STRATEGY_CONFLICT_POLICY_MODE` controls whether Bundle 6's
//! conflict-resolution layer has any influence on the runtime dispatch
//! loop. Mirrors `runtime_opportunity_mode.rs`'s closed-enum, fail-closed-on-
//! unknown style exactly — an unrecognized value never panics and never
//! silently picks an arbitrary mode; it resolves to `Off` with an explicit,
//! operator-visible `invalid_configuration` flag.
//!
//! Pure: no I/O beyond the single env-var read in the `_from_env` entry
//! point; every other function takes already-read values.

use crate::state::{BrokerKind, DeploymentMode};

pub const STRATEGY_CONFLICT_POLICY_MODE_ENV: &str = "MQK_STRATEGY_CONFLICT_POLICY_MODE";

/// Closed set of runtime modes (task-frozen vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicyMode {
    /// Exact pre-Bundle-6 behavior. The conflict resolver is never
    /// consulted; no conflict-plan DB write; no new API mutation; no
    /// broker/outbox call; no extra runtime I/O.
    Off,
    /// Evaluate and record what conflict enforcement would do; every
    /// original input decision is returned unchanged and in original order.
    Shadow,
    /// Replace each same-symbol group with zero or one exact original
    /// decision per the frozen policy. Only reachable when
    /// [`effective_mode`] confirms the paper+Alpaca live-lock.
    PaperEnforced,
}

impl ConflictPolicyMode {
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
pub struct ConflictPolicyModeResolution {
    /// The mode as configured (or `Off` on absence/unrecognized value).
    pub configured_mode: ConflictPolicyMode,
    /// `Some(raw_value)` when the configured value was non-empty but did
    /// not match any recognized mode string. The daemon still runs as
    /// `Off` in this case, but the status route must surface this
    /// distinctly from an intentional, valid `Off`.
    pub invalid_configuration: Option<String>,
}

/// Parse a raw env-var value into a [`ConflictPolicyModeResolution`].
///
/// Absent/blank → `Off`, no invalid-configuration flag (this is the honest
/// default, not a misconfiguration). Any non-empty value that is not one of
/// `off` / `shadow` / `paper_enforced` (case-insensitive, trimmed) → `Off`
/// with `invalid_configuration = Some(<value>)`.
pub fn resolve_conflict_policy_mode(raw: Option<&str>) -> ConflictPolicyModeResolution {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(value) = trimmed else {
        return ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Off,
            invalid_configuration: None,
        };
    };
    match value.to_ascii_lowercase().as_str() {
        "off" => ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Off,
            invalid_configuration: None,
        },
        "shadow" => ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Shadow,
            invalid_configuration: None,
        },
        "paper_enforced" => ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::PaperEnforced,
            invalid_configuration: None,
        },
        _ => ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Off,
            invalid_configuration: Some(value.to_string()),
        },
    }
}

/// [`resolve_conflict_policy_mode`], reading [`STRATEGY_CONFLICT_POLICY_MODE_ENV`]
/// from the environment.
pub fn resolve_conflict_policy_mode_from_env() -> ConflictPolicyModeResolution {
    let raw = std::env::var(STRATEGY_CONFLICT_POLICY_MODE_ENV).ok();
    resolve_conflict_policy_mode(raw.as_deref())
}

/// The fully-resolved mode, after applying the deployment-mode live-lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConflictPolicyMode {
    pub configured_mode: ConflictPolicyMode,
    pub effective_mode: ConflictPolicyMode,
    pub invalid_configuration: Option<String>,
    /// `true` when the deployment context was not `paper` + `alpaca` and the
    /// live-lock forced `effective_mode` down to `Off` regardless of what
    /// was configured.
    pub live_lock_applied: bool,
}

/// Apply the hard live-lock: any configured mode other than `Off` has zero
/// effect outside `deployment_mode == paper` with `adapter == alpaca`. This
/// runs *before* any conflict-resolution call — Phase B's insertion point
/// must consult this, not the raw resolution, before treating a non-`Off`
/// mode as active.
///
/// The live-lock demotes all the way to `Off` (not `Shadow`) for a
/// non-paper/non-Alpaca deployment — mirrors
/// `runtime_opportunity_mode::effective_mode` exactly: even Shadow mode is a
/// paper-only evidence path, and Bundle 6 must have zero footprint of any
/// kind outside the frozen paper+Alpaca operating lane.
pub fn effective_mode(
    resolution: &ConflictPolicyModeResolution,
    deployment_mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> EffectiveConflictPolicyMode {
    let is_paper_alpaca =
        deployment_mode == DeploymentMode::Paper && broker_kind == Some(BrokerKind::Alpaca);

    if resolution.configured_mode != ConflictPolicyMode::Off && !is_paper_alpaca {
        return EffectiveConflictPolicyMode {
            configured_mode: resolution.configured_mode,
            effective_mode: ConflictPolicyMode::Off,
            invalid_configuration: resolution.invalid_configuration.clone(),
            live_lock_applied: true,
        };
    }

    EffectiveConflictPolicyMode {
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
        let r = resolve_conflict_policy_mode(None);
        assert_eq!(r.configured_mode, ConflictPolicyMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn blank_env_resolves_to_off_no_invalid_flag() {
        let r = resolve_conflict_policy_mode(Some("   "));
        assert_eq!(r.configured_mode, ConflictPolicyMode::Off);
        assert_eq!(r.invalid_configuration, None);
    }

    #[test]
    fn recognizes_all_three_modes_case_insensitively() {
        assert_eq!(
            resolve_conflict_policy_mode(Some("Off")).configured_mode,
            ConflictPolicyMode::Off
        );
        assert_eq!(
            resolve_conflict_policy_mode(Some("SHADOW")).configured_mode,
            ConflictPolicyMode::Shadow
        );
        assert_eq!(
            resolve_conflict_policy_mode(Some(" paper_enforced ")).configured_mode,
            ConflictPolicyMode::PaperEnforced
        );
    }

    #[test]
    fn unknown_value_fails_closed_to_off_with_invalid_flag() {
        let r = resolve_conflict_policy_mode(Some("enforced"));
        assert_eq!(r.configured_mode, ConflictPolicyMode::Off);
        assert_eq!(r.invalid_configuration, Some("enforced".to_string()));
    }

    #[test]
    fn paper_alpaca_passes_through_unchanged() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.effective_mode, ConflictPolicyMode::PaperEnforced);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn live_capital_forces_off_even_when_paper_enforced_configured() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveCapital,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, ConflictPolicyMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn live_shadow_forces_off_even_for_shadow_mode() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Shadow,
            invalid_configuration: None,
        };
        let eff = effective_mode(
            &resolution,
            DeploymentMode::LiveShadow,
            Some(BrokerKind::Alpaca),
        );
        assert_eq!(eff.effective_mode, ConflictPolicyMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn non_alpaca_adapter_forces_off_even_in_paper_mode() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::PaperEnforced,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Paper));
        assert_eq!(eff.effective_mode, ConflictPolicyMode::Off);
        assert!(eff.live_lock_applied);
    }

    #[test]
    fn off_configured_mode_never_triggers_live_lock() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Off,
            invalid_configuration: None,
        };
        let eff = effective_mode(&resolution, DeploymentMode::LiveCapital, None);
        assert_eq!(eff.effective_mode, ConflictPolicyMode::Off);
        assert!(!eff.live_lock_applied);
    }

    #[test]
    fn invalid_configuration_flag_survives_live_lock() {
        let resolution = ConflictPolicyModeResolution {
            configured_mode: ConflictPolicyMode::Off,
            invalid_configuration: Some("bogus".to_string()),
        };
        let eff = effective_mode(&resolution, DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert_eq!(eff.invalid_configuration, Some("bogus".to_string()));
    }
}
