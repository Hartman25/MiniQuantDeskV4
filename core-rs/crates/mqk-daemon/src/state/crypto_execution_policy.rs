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
//! - Authority: the parsed policy is bound ONCE into the deployment
//!   configuration authority (`RuntimeSelection::crypto_time_in_force`,
//!   resolved from the environment at daemon start, never re-read). The
//!   strategy->decision construction seam
//!   (`decision::bar_result_to_decisions_with_tif`, driven by
//!   [`strategy_time_in_force`]) emits `gtc`/`ioc` directly into the
//!   `InternalStrategyDecision`. `decision.rs` at admission only VALIDATES
//!   (`admit_time_in_force`); it never reads this policy or the environment
//!   and never rewrites a value. `Unconfigured`/`Invalid` refuses before any
//!   crypto economic intent is constructed.
//! - Crypto capability remains default off (`D2/B4`) at the higher
//!   dispatch/arm gates.

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

    /// Strict parse of an already-emitted decision value (trim + ASCII case
    /// only). `day`, `fok`, `opg`, `cls`, empty and unknown are `None`.
    pub fn parse_admitted(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "gtc" => Some(Self::Gtc),
            "ioc" => Some(Self::Ioc),
            _ => None,
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

/// Construction-seam TIF for a strategy decision of `asset_class`.
///
/// Non-crypto classes keep the generic strategy value (`day`) byte-for-byte.
/// Crypto emits the configured `gtc`/`ioc` directly; `Unconfigured`/`Invalid`
/// refuse so no crypto economic intent is ever constructed without an
/// explicit policy. Pure: the policy is a parameter.
pub fn strategy_time_in_force(
    asset_class: &str,
    config: CryptoTimeInForceConfig,
) -> Result<&'static str, String> {
    if asset_class != "crypto" {
        return Ok("day");
    }
    match config {
        CryptoTimeInForceConfig::Explicit(tif) => Ok(tif.as_str()),
        CryptoTimeInForceConfig::Unconfigured => Err(format!(
            "crypto time_in_force policy is not configured ({CRYPTO_TIME_IN_FORCE_ENV} is unset);              an explicit gtc or ioc policy is required before any crypto economic intent is              constructed"
        )),
        CryptoTimeInForceConfig::Invalid => Err(format!(
            "crypto time_in_force policy ({CRYPTO_TIME_IN_FORCE_ENV}) is not a recognized gtc/ioc              value; no default or rewritten time_in_force is ever emitted"
        )),
    }
}

/// Fingerprint of the TIF actually carried by a constructed crypto decision.
/// `None` when the carried value is not `gtc`/`ioc` (nothing to attest).
pub fn crypto_execution_policy_fingerprint_for_decision_tif(tif: &str) -> Option<String> {
    CryptoTimeInForce::parse_admitted(tif)
        .map(|t| crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Explicit(t)))
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

    #[test]
    fn strategy_time_in_force_emits_configured_value_and_refuses_otherwise() {
        let gtc = CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Gtc);
        let ioc = CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Ioc);
        assert_eq!(strategy_time_in_force("crypto", gtc), Ok("gtc"));
        assert_eq!(strategy_time_in_force("crypto", ioc), Ok("ioc"));
        for cfg in [
            CryptoTimeInForceConfig::Unconfigured,
            CryptoTimeInForceConfig::Invalid,
        ] {
            assert!(strategy_time_in_force("crypto", cfg).is_err());
        }
        // Equity is byte-for-byte the generic strategy value under any config.
        for cfg in [
            CryptoTimeInForceConfig::Unconfigured,
            CryptoTimeInForceConfig::Invalid,
            gtc,
            ioc,
        ] {
            assert_eq!(strategy_time_in_force("equity", cfg), Ok("day"));
        }
    }

    #[test]
    fn parse_admitted_accepts_only_gtc_ioc() {
        assert_eq!(
            CryptoTimeInForce::parse_admitted(" GTC "),
            Some(CryptoTimeInForce::Gtc)
        );
        assert_eq!(
            CryptoTimeInForce::parse_admitted("ioc"),
            Some(CryptoTimeInForce::Ioc)
        );
        for raw in ["day", "fok", "opg", "cls", "", "unknown"] {
            assert_eq!(CryptoTimeInForce::parse_admitted(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn decision_tif_fingerprint_matches_config_fingerprint_and_is_canonical() {
        let gtc = crypto_execution_policy_fingerprint(CryptoTimeInForceConfig::Explicit(
            CryptoTimeInForce::Gtc,
        ));
        assert_eq!(
            crypto_execution_policy_fingerprint_for_decision_tif(" GTC "),
            Some(gtc.clone())
        );
        assert_eq!(
            crypto_execution_policy_fingerprint_for_decision_tif("gtc"),
            Some(gtc)
        );
        assert_ne!(
            crypto_execution_policy_fingerprint_for_decision_tif("gtc"),
            crypto_execution_policy_fingerprint_for_decision_tif("ioc")
        );
        assert_eq!(
            crypto_execution_policy_fingerprint_for_decision_tif("day"),
            None
        );
    }
}
