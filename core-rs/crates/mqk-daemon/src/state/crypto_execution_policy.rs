//! B5: explicit Crypto time-in-force execution policy.
//!
//! `IR-2` (operator policy, commit `1f1c9961`) made Crypto time-in-force
//! EXPLICIT-ONLY at the admission gate (`decision::admit_time_in_force`):
//! an incoming crypto order must itself carry `gtc` or `ioc`; `day` and
//! everything else is refused, never rewritten. That gate proves refusal —
//! it does not create a configured source that could ever supply the
//! admitted `gtc`/`ioc` value in the first place, since the generic
//! strategy-to-decision translator (`decision::bar_result_to_decisions`)
//! always emits `"day"` regardless of asset class.
//!
//! This module is that configured source: a pure, deterministic,
//! deployment-level policy read from `MQK_CRYPTO_TIME_IN_FORCE`, plus a
//! canonical, versioned fingerprint of the resolved policy so a `gtc`<->`ioc`
//! change (an economic behavior change) is distinguishable from a
//! spelling/case/whitespace variant of the same choice (which is not).
//!
//! # Honesty contract (same shape as `market_calendar`'s ASSET-CORE-05A
//! model-only seam)
//!
//! - Pure: no DB, no network, no broker/OMS access. `_from_env_value` takes
//!   an already-read `Option<&str>`; `_from_env` is the one env-reading
//!   wrapper, mirroring `env.rs`'s `operator_auth_mode_from_env_values`/
//!   `_from_env` pair.
//! - This module does not by itself put a crypto order on the wire, does not
//!   rewrite or default any order's `time_in_force` field, and is not called
//!   from `decision.rs`. Wiring a configured value into a real order remains
//!   explicit future work for whichever caller eventually constructs a
//!   crypto `InternalStrategyDecision` — it must pass the resolved,
//!   already-admissible value in directly, never let `decision.rs` reach
//!   into this config and silently rewrite an existing value (that
//!   "silent rewrite" pattern is the exact defect IR-2 exists to forbid).
//! - Crypto capability remains default off (`D2/B4`); this module has zero
//!   production callers today, matching this repo's established precedent
//!   for asset-class capability work (`ASSET-CORE-01`..`05`: build the
//!   model/config layer and prove it before a concrete consumer requires
//!   wiring).

use sha2::{Digest, Sha256};

/// Env var carrying the operator's explicit Crypto time-in-force choice.
pub const CRYPTO_TIME_IN_FORCE_ENV: &str = "MQK_CRYPTO_TIME_IN_FORCE";

/// The only two time-in-force values Crypto ever admits (IR-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CryptoTimeInForce {
    Gtc,
    Ioc,
}

impl CryptoTimeInForce {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "gtc",
            Self::Ioc => "ioc",
        }
    }
}

/// Three-way configured-policy truth, mirroring
/// `autonomous_daily_operation::FixedWindowOverrideConfig`'s
/// absent/valid/invalid shape rather than collapsing "not configured" and
/// "configured wrong" into one bare `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoTimeInForceConfig {
    /// `MQK_CRYPTO_TIME_IN_FORCE` is unset or blank.
    Unconfigured,
    /// A valid, explicit `gtc` or `ioc` selection (any case/whitespace).
    Explicit(CryptoTimeInForce),
    /// The env var is set but is not `gtc`/`ioc` — e.g. `day`, `fok`, empty
    /// after trim of a non-blank value, or an unrecognized string. Distinct
    /// from `Unconfigured` so an operator typo is never silently treated as
    /// "no policy configured".
    Invalid,
}

impl CryptoTimeInForceConfig {
    /// The explicit selection, or `None` for `Unconfigured`/`Invalid`.
    pub fn explicit_tif(self) -> Option<CryptoTimeInForce> {
        match self {
            Self::Explicit(tif) => Some(tif),
            Self::Unconfigured | Self::Invalid => None,
        }
    }

    /// Stable label for this config state, used only by
    /// [`crypto_execution_policy_fingerprint`] and diagnostics — never
    /// parsed back, never compared against a raw operator-supplied string.
    fn fingerprint_label(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Invalid => "invalid",
            Self::Explicit(CryptoTimeInForce::Gtc) => "gtc",
            Self::Explicit(CryptoTimeInForce::Ioc) => "ioc",
        }
    }
}

/// Pure resolution: classify an already-read env value into
/// [`CryptoTimeInForceConfig`]. Normalizes only whitespace and ASCII case —
/// `" GTC "`, `"gtc"`, and `"Gtc"` all resolve to the identical
/// `Explicit(Gtc)`, so [`crypto_execution_policy_fingerprint`] (which is
/// computed from this resolved enum, never the raw string) cannot be
/// perturbed by spelling/case alone.
pub fn crypto_time_in_force_config_from_env_value(raw: Option<&str>) -> CryptoTimeInForceConfig {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => CryptoTimeInForceConfig::Unconfigured,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "gtc" => CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Gtc),
            "ioc" => CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Ioc),
            _ => CryptoTimeInForceConfig::Invalid,
        },
    }
}

/// Production entry point: reads [`CRYPTO_TIME_IN_FORCE_ENV`] and delegates
/// to [`crypto_time_in_force_config_from_env_value`].
pub fn crypto_time_in_force_config_from_env() -> CryptoTimeInForceConfig {
    crypto_time_in_force_config_from_env_value(
        std::env::var(CRYPTO_TIME_IN_FORCE_ENV).ok().as_deref(),
    )
}

/// Deterministic, versioned identity for the resolved Crypto execution-policy
/// config — the "relevant semantic/config fingerprint" the economic TIF
/// choice must participate in. `Explicit(Gtc)` and `Explicit(Ioc)` produce
/// different identities (an economic behavior change); every spelling/case/
/// whitespace variant of the same choice normalizes to the same
/// [`CryptoTimeInForceConfig`] before this function ever runs, so it cannot
/// manufacture a distinct identity for a non-economic difference.
pub fn crypto_execution_policy_fingerprint(config: CryptoTimeInForceConfig) -> String {
    let canonical = format!(
        "mqk.crypto-execution-policy.v1|time_in_force={}",
        config.fingerprint_label()
    );
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Parse table (mirrors decision.rs's rcm6b_crypto_time_in_force_table
    // shape: exhaustive admit/refuse cases plus normalization proof).
    // -----------------------------------------------------------------

    #[test]
    fn unset_is_unconfigured() {
        assert_eq!(
            crypto_time_in_force_config_from_env_value(None),
            CryptoTimeInForceConfig::Unconfigured
        );
    }

    #[test]
    fn blank_after_trim_is_unconfigured() {
        assert_eq!(
            crypto_time_in_force_config_from_env_value(Some("   ")),
            CryptoTimeInForceConfig::Unconfigured
        );
    }

    #[test]
    fn gtc_and_ioc_resolve_explicit() {
        assert_eq!(
            crypto_time_in_force_config_from_env_value(Some("gtc")),
            CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Gtc)
        );
        assert_eq!(
            crypto_time_in_force_config_from_env_value(Some("ioc")),
            CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Ioc)
        );
    }

    #[test]
    fn case_and_whitespace_are_normalized() {
        for raw in [" GTC ", "Gtc", "gTc", "\tgtc\n"] {
            assert_eq!(
                crypto_time_in_force_config_from_env_value(Some(raw)),
                CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Gtc),
                "raw={raw:?} must normalize to Explicit(Gtc)"
            );
        }
    }

    #[test]
    fn day_and_other_tifs_are_invalid_never_defaulted() {
        for raw in ["day", "fok", "opg", "cls", "unknown", "gtc-ish"] {
            assert_eq!(
                crypto_time_in_force_config_from_env_value(Some(raw)),
                CryptoTimeInForceConfig::Invalid,
                "raw={raw:?} must be Invalid, never silently admitted or defaulted"
            );
        }
    }

    #[test]
    fn invalid_is_distinct_from_unconfigured() {
        assert_ne!(
            crypto_time_in_force_config_from_env_value(Some("day")),
            crypto_time_in_force_config_from_env_value(None),
            "an operator typo must never be indistinguishable from no config at all"
        );
    }

    // -----------------------------------------------------------------
    // Fingerprint: economic identity proof.
    // -----------------------------------------------------------------

    #[test]
    fn gtc_and_ioc_fingerprints_differ() {
        let gtc = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Explicit(
            CryptoTimeInForce::Gtc,
        ));
        let ioc = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Explicit(
            CryptoTimeInForce::Ioc,
        ));
        assert_ne!(
            gtc, ioc,
            "gtc<->ioc is an economic behavior change and must change the fingerprint"
        );
    }

    #[test]
    fn spelling_and_case_variants_of_the_same_choice_share_one_fingerprint() {
        let canonical = crypto_time_in_force_config_from_env_value(Some("gtc"));
        let via_case = crypto_time_in_force_config_from_env_value(Some(" GTC "));
        assert_eq!(canonical, via_case);
        assert_eq!(
            crypto_execution_policy_fingerprint(canonical),
            crypto_execution_policy_fingerprint(via_case),
            "spelling/case/whitespace normalization must never change the fingerprint"
        );
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Unconfigured);
        let b = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Unconfigured);
        assert_eq!(a, b);
    }

    #[test]
    fn unconfigured_invalid_and_explicit_all_have_distinct_fingerprints() {
        let unconfigured =
            crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Unconfigured);
        let invalid = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Invalid);
        let gtc = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Explicit(
            CryptoTimeInForce::Gtc,
        ));
        assert_ne!(unconfigured, invalid);
        assert_ne!(unconfigured, gtc);
        assert_ne!(invalid, gtc);
    }
}
