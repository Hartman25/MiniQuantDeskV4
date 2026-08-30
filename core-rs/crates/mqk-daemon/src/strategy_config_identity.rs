//! PROMOTION-CONFIG-IDENTITY-01 (C1) / RUNTIME-PROMOTION-EVIDENCE-BINDING-01
//! (C2): single authoritative derivation of "what semantic configuration
//! would THIS daemon, right now, produce for `(strategy_id, symbol,
//! timeframe_secs)`" — reused by the promotion transition route (C1, write
//! path) and the external signal route (C2, which has no already-running
//! host to query).
//!
//! Never trusts a caller-supplied fingerprint. Always re-derives through
//! the exact same construction convention the live daemon itself uses to
//! build its own registry
//! (`mqk_runtime::native_strategy::build_daemon_plugin_registry_for_symbol`,
//! the identical per-symbol seam Bundle 7's dynamic-selection host pool
//! already uses — see `dynamic_selection_host_pool.rs`), then reads
//! [`mqk_strategy::Strategy::semantic_fingerprint`] (STRATEGY-SEMANTIC-
//! IDENTITY-SEAM-01) off the freshly-instantiated instance.
//!
//! This is intentionally environment-sensitive: the built-in
//! `intraday_scalper`/`intraday_short_scalper` engines read
//! `MQK_STRATEGY_TARGET_QTY` et al. at construction time, identically to how
//! the real runtime bootstrap constructs them
//! (`NativeStrategyBootstrap::bootstrap` via
//! `build_daemon_plugin_registry_and_symbol`). A fingerprint that changes
//! because an operator changed ambient sizing config without re-approving is
//! the correct, desired failure mode this seam exists to catch — not a
//! defect in this derivation.

use mqk_runtime::native_strategy::build_daemon_plugin_registry_for_symbol;

/// Bounded, truthful `config_identity_status` vocabulary persisted on
/// `sys_strategy_promotion_transitions.config_identity_status`.
pub const CONFIG_IDENTITY_STATUS_VERIFIED_V1: &str = "verified_v1";
/// Legacy/failure value — kept identical to the column's pre-C1 default
/// (migration 0046) so historical rows and any row this daemon could not
/// resolve an identity for share one honest, fail-closed label. Never a
/// wildcard: `PromotionConfigMismatch`/continuity checks treat a `None`
/// `config_fingerprint` (always paired with this status) as never matching
/// anything.
pub const CONFIG_IDENTITY_STATUS_UNAVAILABLE: &str = "unavailable_in_current_runtime";

/// Distinct, bounded failure reasons a derivation attempt can produce.
/// Never folded into one generic "unavailable" bucket — each has its own
/// deterministic seed token (see [`ConfigIdentityError::seed_token`]) so two
/// different failure causes for the same identity never collide in the
/// promotion-transition idempotency seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigIdentityError {
    /// `strategy_id` is not registered under this exact per-symbol registry
    /// construction (unknown name, or the engine's own registered metadata
    /// disagrees with its `spec()` — `PluginRegistry::instantiate_verified`
    /// folds both into one `RegistryError`; either way this identity cannot
    /// be resolved).
    UnsupportedStrategyPlugin,
    /// The instantiated strategy's own `spec().timeframe_secs` does not
    /// equal the caller-claimed `timeframe_secs` — the requested identity
    /// does not correspond to what this daemon would actually run.
    TimeframeMismatch { registered_secs: i64 },
}

impl ConfigIdentityError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedStrategyPlugin => "unsupported_strategy_plugin",
            Self::TimeframeMismatch { .. } => "timeframe_mismatch",
        }
    }

    /// Deterministic, human-readable token distinct from any possible
    /// SHA-256 hex fingerprint (which is always exactly 64 lowercase hex
    /// characters) — safe to fold directly into the promotion-transition
    /// idempotency seed alongside a successful fingerprint.
    pub fn seed_token(&self) -> String {
        match self {
            Self::UnsupportedStrategyPlugin => "unsupported_strategy_plugin".to_string(),
            Self::TimeframeMismatch { registered_secs } => {
                format!("timeframe_mismatch:{registered_secs}")
            }
        }
    }

    pub fn message(&self, strategy_id: &str, requested_timeframe_secs: i64) -> String {
        match self {
            Self::UnsupportedStrategyPlugin => format!(
                "strategy '{strategy_id}' could not be resolved to a semantic configuration by \
                 this daemon's authoritative strategy registry -- unknown strategy_id, or an \
                 internal registration inconsistency"
            ),
            Self::TimeframeMismatch { registered_secs } => format!(
                "strategy '{strategy_id}' is registered with timeframe_secs={registered_secs}, \
                 which does not match the requested timeframe_secs={requested_timeframe_secs}"
            ),
        }
    }
}

/// Re-derive the exact semantic fingerprint this daemon would currently
/// produce for `(strategy_id, symbol, timeframe_secs)`.
pub fn resolve_server_semantic_fingerprint(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> Result<String, ConfigIdentityError> {
    let registry = build_daemon_plugin_registry_for_symbol(symbol);
    let instance = registry
        .instantiate_verified(strategy_id)
        .map_err(|_| ConfigIdentityError::UnsupportedStrategyPlugin)?;
    let registered_secs = instance.spec().timeframe_secs;
    if registered_secs != timeframe_secs {
        return Err(ConfigIdentityError::TimeframeMismatch { registered_secs });
    }
    Ok(instance.semantic_fingerprint())
}

/// Seed-safe token for [`resolve_server_semantic_fingerprint`]'s outcome —
/// the fingerprint itself on success, or a distinct failure token on error.
/// Used to fold config identity into a deterministic idempotency seed
/// without needing a second derivation call.
pub fn seed_token(result: &Result<String, ConfigIdentityError>) -> String {
    match result {
        Ok(fp) => fp.clone(),
        Err(e) => e.seed_token(),
    }
}

// ---------------------------------------------------------------------------
// R7: canonical config-identity verification predicate
// ---------------------------------------------------------------------------
//
// CANONICAL-CONFIG-IDENTITY-VERIFIER-01: before this patch, the same narrow
// rule -- status == verified_v1 AND both a durable/promoted fingerprint AND
// a currently-resolved fingerprint are structurally valid AND the two are
// byte-equal -- was independently reimplemented at three call sites (the
// promotion-transition continuity check, the runtime dispatch gate, and the
// dynamic-selection plan-build-time eligibility check). Independently
// reimplemented copies of a security-critical predicate can silently drift
// out of sync; verified_fingerprint and config_identity_is_verified are now
// the ONE place this rule is defined. Every site that decides whether a
// config identity is verified must call one of these, never reimplement the
// check inline.

/// Returns `fingerprint` iff `status` is exactly [`CONFIG_IDENTITY_STATUS_VERIFIED_V1`]
/// AND `fingerprint` is `Some` and structurally valid
/// (`mqk_db::is_valid_evidence_fingerprint_v2_hex`) -- `None` in every other
/// case (wrong status, missing fingerprint, or malformed fingerprint).
///
/// This is the "one side" of [`config_identity_is_verified`]'s rule, exposed
/// standalone because some callers need to know a record's own verified
/// fingerprint (e.g. for a diagnostic message) independent of comparing it
/// against a second fingerprint.
pub fn verified_fingerprint<'a>(status: &str, fingerprint: Option<&'a str>) -> Option<&'a str> {
    if status != CONFIG_IDENTITY_STATUS_VERIFIED_V1 {
        return None;
    }
    fingerprint.filter(|fp| mqk_db::is_valid_evidence_fingerprint_v2_hex(fp))
}

/// The ONE canonical config-identity verification rule, reused by every
/// site that decides whether a durable/promoted config fingerprint remains
/// verified against a currently-resolved config fingerprint.
///
/// Narrow rule: `status == verified_v1` AND both `durable_config_fingerprint`
/// and `current_config_fingerprint` are structurally valid AND the two are
/// byte-equal. A caller claim of `status == verified_v1` alongside a
/// mismatched, missing, or malformed fingerprint pair fails closed -- this
/// function never trusts `status` alone.
pub fn config_identity_is_verified(
    status: &str,
    durable_config_fingerprint: Option<&str>,
    current_config_fingerprint: Option<&str>,
) -> bool {
    let Some(durable) = verified_fingerprint(status, durable_config_fingerprint) else {
        return false;
    };
    let Some(current) =
        current_config_fingerprint.filter(|fp| mqk_db::is_valid_evidence_fingerprint_v2_hex(fp))
    else {
        return false;
    };
    durable == current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_identity_resolves_and_matches_direct_instantiation() {
        let result = resolve_server_semantic_fingerprint("swing_momentum", "AAPL", 86_400);
        let registry = build_daemon_plugin_registry_for_symbol("AAPL");
        let expected = registry
            .instantiate("swing_momentum")
            .unwrap()
            .semantic_fingerprint();
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn unknown_strategy_id_fails_closed() {
        let result = resolve_server_semantic_fingerprint("does_not_exist", "AAPL", 300);
        assert_eq!(result, Err(ConfigIdentityError::UnsupportedStrategyPlugin));
    }

    #[test]
    fn wrong_timeframe_fails_closed_with_registered_value() {
        // swing_momentum is registered at 86_400s; claiming 300s must fail.
        let result = resolve_server_semantic_fingerprint("swing_momentum", "AAPL", 300);
        assert_eq!(
            result,
            Err(ConfigIdentityError::TimeframeMismatch {
                registered_secs: 86_400
            })
        );
    }

    #[test]
    fn symbol_change_changes_the_resolved_fingerprint() {
        let aapl = resolve_server_semantic_fingerprint("swing_momentum", "AAPL", 86_400).unwrap();
        let msft = resolve_server_semantic_fingerprint("swing_momentum", "MSFT", 86_400).unwrap();
        assert_ne!(aapl, msft);
    }

    #[test]
    fn seed_token_distinguishes_success_from_every_failure_kind() {
        let ok_a = Ok("a".repeat(64));
        let ok_b = Ok("b".repeat(64));
        let err1 = Err(ConfigIdentityError::UnsupportedStrategyPlugin);
        let err2 = Err(ConfigIdentityError::TimeframeMismatch {
            registered_secs: 300,
        });
        let tokens = [
            seed_token(&ok_a),
            seed_token(&ok_b),
            seed_token(&err1),
            seed_token(&err2),
        ];
        for i in 0..tokens.len() {
            for j in 0..tokens.len() {
                if i != j {
                    assert_ne!(tokens[i], tokens[j], "tokens at {i} and {j} must differ");
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // R7: config_identity_is_verified / verified_fingerprint negative
    // controls -- CANONICAL-CONFIG-IDENTITY-VERIFIER-01.
    // -----------------------------------------------------------------

    fn hex(tag: char) -> String {
        std::iter::repeat_n(tag, 64).collect()
    }

    #[test]
    fn verified_status_with_exact_valid_pair_passes() {
        let fp = hex('a');
        assert!(config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            Some(&fp),
            Some(&fp),
        ));
    }

    #[test]
    fn verified_status_with_mismatched_fingerprints_refuses() {
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            Some(&hex('a')),
            Some(&hex('b')),
        ));
    }

    #[test]
    fn verified_status_with_malformed_durable_fingerprint_refuses() {
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            Some("not-a-valid-hex-fingerprint"),
            Some(&hex('a')),
        ));
    }

    #[test]
    fn verified_status_with_malformed_current_fingerprint_refuses() {
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            Some(&hex('a')),
            Some("not-a-valid-hex-fingerprint"),
        ));
    }

    #[test]
    fn verified_status_with_missing_durable_fingerprint_refuses() {
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            None,
            Some(&hex('a')),
        ));
    }

    #[test]
    fn verified_status_with_missing_current_fingerprint_refuses() {
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_VERIFIED_V1,
            Some(&hex('a')),
            None,
        ));
    }

    #[test]
    fn non_verified_status_with_matching_fingerprints_refuses() {
        // A record whose fingerprints agree but whose status is NOT
        // verified_v1 must still refuse -- status alone can never be
        // bypassed by a coincidentally-matching fingerprint pair.
        let fp = hex('a');
        assert!(!config_identity_is_verified(
            CONFIG_IDENTITY_STATUS_UNAVAILABLE,
            Some(&fp),
            Some(&fp),
        ));
    }

    #[test]
    fn verified_fingerprint_returns_none_for_non_verified_status() {
        assert_eq!(
            verified_fingerprint(CONFIG_IDENTITY_STATUS_UNAVAILABLE, Some(&hex('a'))),
            None
        );
    }

    #[test]
    fn verified_fingerprint_returns_none_for_malformed_fingerprint() {
        assert_eq!(
            verified_fingerprint(CONFIG_IDENTITY_STATUS_VERIFIED_V1, Some("short")),
            None
        );
    }

    #[test]
    fn verified_fingerprint_returns_fingerprint_when_coherent() {
        let fp = hex('a');
        assert_eq!(
            verified_fingerprint(CONFIG_IDENTITY_STATUS_VERIFIED_V1, Some(&fp)),
            Some(fp.as_str())
        );
    }
}
