//! TV-04D — Deployment economics outcome and evaluator.

use std::path::Path;

// ---------------------------------------------------------------------------
// TV-04D — Deployment economics outcome
// ---------------------------------------------------------------------------

/// Result of evaluating deployment economics constraints at the runtime
/// **start boundary**.
///
/// TV-04D adds a second gate on top of TV-04A:
///
/// - TV-04A asks: is the capital policy `enabled`?
/// - TV-04D asks: does the enabled policy specify a valid portfolio-level
///   economics bound (`max_portfolio_notional_usd`)?
///
/// An operator cannot deploy with an enabled policy that carries no economics
/// bound.  `EconomicsNotSpecified` is fail-closed.
///
/// # Start-safe variants
///
/// | Variant              | Meaning                                                      |
/// |----------------------|--------------------------------------------------------------|
/// | `NotConfigured`      | No policy path configured; gate not applicable.              |
/// | `PolicyDisabled`     | Policy `enabled=false`; TV-04A handles this; passed through. |
/// | `EconomicsSpecified` | Policy enabled + `max_portfolio_notional_usd` > 0.           |
///
/// # Fail-closed variants
///
/// | Variant                | Meaning                                                    |
/// |------------------------|------------------------------------------------------------|
/// | `EconomicsNotSpecified`| Policy enabled but portfolio economics bound absent/invalid.|
/// | `PolicyInvalid`        | Policy configured but structurally invalid.                |
/// | `Unavailable`          | Evaluator could not run (reserved).                        |
#[derive(Debug, Clone, PartialEq)]
pub enum DeploymentEconomicsOutcome {
    /// No policy path was configured (env var absent or empty).
    NotConfigured,

    /// Policy file is valid but `enabled = false`.
    ///
    /// TV-04A already blocks on this variant.  TV-04D passes through here
    /// because it only concerns itself with *enabled* policies.
    PolicyDisabled,

    /// Policy enabled and `max_portfolio_notional_usd` is present and positive.
    EconomicsSpecified {
        /// The `policy_id` from the policy file.
        policy_id: String,
        /// The stated portfolio notional cap in USD.
        max_portfolio_notional_usd: f64,
    },

    /// Policy enabled but `max_portfolio_notional_usd` is absent or not a
    /// positive number.  Fail-closed.
    EconomicsNotSpecified {
        /// Human-readable reason.
        reason: String,
    },

    /// Policy path is configured but the file is unreadable or invalid.
    /// Fail-closed.
    PolicyInvalid {
        /// Human-readable reason.
        reason: String,
    },

    /// Evaluator could not run.  Reserved.  Fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl DeploymentEconomicsOutcome {
    /// Truth-state label for operator-visible surfaces.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::PolicyDisabled => "policy_disabled",
            Self::EconomicsSpecified { .. } => "economics_specified",
            Self::EconomicsNotSpecified { .. } => "economics_not_specified",
            Self::PolicyInvalid { .. } => "policy_invalid",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome allows runtime start to proceed.
    ///
    /// `true` for `NotConfigured`, `PolicyDisabled`, `EconomicsSpecified`.
    /// `false` for all others (fail-closed).
    pub fn is_start_safe(&self) -> bool {
        matches!(
            self,
            Self::NotConfigured | Self::PolicyDisabled | Self::EconomicsSpecified { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator — deployment economics (TV-04D)
// ---------------------------------------------------------------------------

/// Evaluate deployment economics constraints against the policy at `path`.
///
/// Pure: no env reads, no network, no DB.  Pass `None` to represent an
/// unconfigured path.
///
/// # Validation contract
///
/// 1. `path` must be `Some` and non-empty — otherwise `NotConfigured`.
/// 2. File must be readable and valid JSON — otherwise `PolicyInvalid`.
/// 3. `schema_version` must equal `"policy-v1"` — otherwise `PolicyInvalid`.
/// 4. `enabled = false` → `PolicyDisabled` (TV-04A handles refusal; pass through).
/// 5. `enabled = true` + `max_portfolio_notional_usd` present and > 0 → `EconomicsSpecified`.
/// 6. `enabled = true` + `max_portfolio_notional_usd` absent or ≤ 0 → `EconomicsNotSpecified`.
pub fn evaluate_deployment_economics(path: Option<&Path>) -> DeploymentEconomicsOutcome {
    let path = match path {
        None => return DeploymentEconomicsOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return DeploymentEconomicsOutcome::NotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return DeploymentEconomicsOutcome::PolicyInvalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return DeploymentEconomicsOutcome::PolicyInvalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == super::CAPITAL_POLICY_SCHEMA_VERSION => {}
        _ => {
            return DeploymentEconomicsOutcome::PolicyInvalid {
                reason: "missing or unsupported schema_version".to_string(),
            }
        }
    }

    let policy_id = j
        .get("policy_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let enabled = j.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        return DeploymentEconomicsOutcome::PolicyDisabled;
    }

    match j.get("max_portfolio_notional_usd") {
        None => DeploymentEconomicsOutcome::EconomicsNotSpecified {
            reason: format!(
                "policy '{}' has enabled=true but max_portfolio_notional_usd is absent; \
                 the operator must specify a portfolio-level economics bound to deploy",
                policy_id
            ),
        },
        Some(v) => match v.as_f64() {
            Some(n) if n > 0.0 => DeploymentEconomicsOutcome::EconomicsSpecified {
                policy_id,
                max_portfolio_notional_usd: n,
            },
            _ => DeploymentEconomicsOutcome::EconomicsNotSpecified {
                reason: format!(
                    "policy '{}' max_portfolio_notional_usd must be a positive number; \
                     got: {}",
                    policy_id, v
                ),
            },
        },
    }
}

/// Read [`super::ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate
/// deployment economics constraints.
///
/// Returns `NotConfigured` when the env var is absent or empty.
pub fn evaluate_deployment_economics_from_env() -> DeploymentEconomicsOutcome {
    let raw = std::env::var(super::ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_deployment_economics(path.as_deref())
}
