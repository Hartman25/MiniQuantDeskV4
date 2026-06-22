//! ETF-RISK-CLOSURE-01 — optional per-sector live gross exposure cap.
//!
//! Parses `MQK_SECTOR_EXPOSURE_LIMITS_BPS`, the env-var configuration for the
//! sector-risk gate (Gate 1h in `decision.rs`). Pure parsing only — no IO, no
//! DB, no live weights — consistent with every other module in this package
//! (`portfolio_risk`, `position_sizing`, `short_entry_policy`): "Pure: no env
//! reads, no network, no DB" for the evaluator itself, with a `_from_env`
//! wrapper that reads the environment and nothing else.
//!
//! The actual evaluation against real positions/marks is
//! `mqk_portfolio::evaluate_sector_risk`; the DB/snapshot/registry glue that
//! builds its inputs lives in `decision.rs`'s Gate 1h. That glue is
//! deliberately NOT in this module: every `capital_policy` evaluator today is
//! synchronous and DB-free, while fetching live positions/marks is async and
//! DB-touching, the same shape Gates 2-7 in `decision.rs` already use for
//! direct `mqk_db::*` calls rather than going through this package.
//!
//! # Configuration
//!
//! ```text
//! MQK_SECTOR_EXPOSURE_LIMITS_BPS="sector_technology=2500,broad_market=5000"
//! ```
//!
//! - Unset or empty (after trimming) -> sector risk disabled (empty map).
//!   This is the default: existing equity/ETF order behavior is unchanged
//!   and the order path never needs DB marks or the instrument registry.
//! - Each entry is `sector=max_gross_weight_bps`. `max_gross_weight_bps` must
//!   parse as a non-negative `i64`. Whitespace around entries/keys/values is
//!   trimmed; a stray trailing/empty entry (e.g. a trailing comma) is
//!   tolerated and skipped.
//! - An entry with no `=`, an empty sector name, a non-integer or negative
//!   value, or a duplicate sector key is rejected — the whole map fails to
//!   parse. Malformed config fails closed for the sector-risk gate only
//!   (the decision being evaluated when the gate runs), not daemon startup
//!   and not any other gate.

use std::collections::HashMap;

/// Env var the operator sets to configure per-sector gross exposure caps, in
/// basis points of NAV. See module docs for the exact format.
pub const ENV_SECTOR_EXPOSURE_LIMITS_BPS: &str = "MQK_SECTOR_EXPOSURE_LIMITS_BPS";

/// Parse `raw` into a `sector -> max_gross_weight_bps` map.
///
/// Pure: no env reads. An empty or whitespace-only `raw` returns an empty
/// map (disabled), not an error.
///
/// # Format
///
/// Comma-separated `sector=bps` pairs, e.g.
/// `"sector_technology=2500,broad_market=5000"`. Returns `Err` (naming the
/// offending entry) for:
/// - an entry with no `=`
/// - an empty sector name (after trimming)
/// - a value that does not parse as an `i64`
/// - a negative value
/// - a duplicate sector key
pub fn parse_sector_exposure_limits_bps(raw: &str) -> Result<HashMap<String, i64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(HashMap::new());
    }

    let mut limits: HashMap<String, i64> = HashMap::new();
    for entry in trimmed.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue; // tolerate a trailing/stray comma
        }
        let (sector, bps_str) = entry.split_once('=').ok_or_else(|| {
            format!("entry '{entry}' is missing '=' (expected sector=max_gross_weight_bps)")
        })?;
        let sector = sector.trim();
        if sector.is_empty() {
            return Err(format!("entry '{entry}' has an empty sector name"));
        }
        let bps_str = bps_str.trim();
        let bps: i64 = bps_str.parse().map_err(|_| {
            format!("entry '{entry}' has a non-integer max_gross_weight_bps value '{bps_str}'")
        })?;
        if bps < 0 {
            return Err(format!(
                "entry '{entry}' has a negative max_gross_weight_bps value {bps}; \
                 limits must be >= 0"
            ));
        }
        if limits.insert(sector.to_string(), bps).is_some() {
            return Err(format!("duplicate sector key '{sector}'"));
        }
    }
    Ok(limits)
}

/// Read [`ENV_SECTOR_EXPOSURE_LIMITS_BPS`] from the environment and parse it.
///
/// Returns `Ok(HashMap::new())` (disabled) when the env var is absent or
/// empty — the default. Returns `Err` when the env var is set but malformed;
/// callers must fail closed for the decision being evaluated only, not
/// daemon startup.
pub fn sector_exposure_limits_bps_from_env() -> Result<HashMap<String, i64>, String> {
    let raw = std::env::var(ENV_SECTOR_EXPOSURE_LIMITS_BPS).unwrap_or_default();
    parse_sector_exposure_limits_bps(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_disabled() {
        assert_eq!(
            parse_sector_exposure_limits_bps("").unwrap(),
            HashMap::new()
        );
    }

    #[test]
    fn whitespace_only_is_disabled() {
        assert_eq!(
            parse_sector_exposure_limits_bps("   ").unwrap(),
            HashMap::new()
        );
    }

    #[test]
    fn single_entry_parses() {
        let m = parse_sector_exposure_limits_bps("sector_technology=2500").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.get("sector_technology"), Some(&2500));
    }

    #[test]
    fn multiple_entries_parse() {
        let m = parse_sector_exposure_limits_bps(
            "sector_technology=2500,broad_market=5000,rates_duration=3000",
        )
        .unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("sector_technology"), Some(&2500));
        assert_eq!(m.get("broad_market"), Some(&5000));
        assert_eq!(m.get("rates_duration"), Some(&3000));
    }

    #[test]
    fn whitespace_around_entries_is_tolerated() {
        let m = parse_sector_exposure_limits_bps(" sector_technology = 2500 , broad_market=5000 ")
            .unwrap();
        assert_eq!(m.get("sector_technology"), Some(&2500));
        assert_eq!(m.get("broad_market"), Some(&5000));
    }

    #[test]
    fn trailing_comma_is_tolerated() {
        let m = parse_sector_exposure_limits_bps("sector_technology=2500,").unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn zero_bps_is_a_valid_value() {
        // A configured zero-tolerance lockout for a sector is a legitimate
        // operator choice, not a disabled check.
        let m = parse_sector_exposure_limits_bps("sector_technology=0").unwrap();
        assert_eq!(m.get("sector_technology"), Some(&0));
    }

    #[test]
    fn missing_equals_is_rejected() {
        let err = parse_sector_exposure_limits_bps("sector_technology2500").unwrap_err();
        assert!(err.contains("sector_technology2500"));
    }

    #[test]
    fn empty_sector_name_is_rejected() {
        let err = parse_sector_exposure_limits_bps("=2500").unwrap_err();
        assert!(err.to_lowercase().contains("empty sector name"));
    }

    #[test]
    fn non_integer_value_is_rejected() {
        let err = parse_sector_exposure_limits_bps("sector_technology=abc").unwrap_err();
        assert!(err.contains("non-integer"));
    }

    #[test]
    fn float_value_is_rejected() {
        // bps must be an integer, not a float — even a "round" one.
        let err = parse_sector_exposure_limits_bps("sector_technology=2500.0").unwrap_err();
        assert!(err.contains("non-integer"));
    }

    #[test]
    fn negative_value_is_rejected() {
        let err = parse_sector_exposure_limits_bps("sector_technology=-100").unwrap_err();
        assert!(err.contains("negative"));
    }

    #[test]
    fn duplicate_sector_key_is_rejected() {
        let err =
            parse_sector_exposure_limits_bps("sector_technology=2500,sector_technology=3000")
                .unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn one_malformed_entry_rejects_the_whole_config() {
        // A good entry alongside a bad one must not partially succeed —
        // fail closed for the whole config, not a silent partial parse.
        let err = parse_sector_exposure_limits_bps("sector_technology=2500,broad_market=bad")
            .unwrap_err();
        assert!(err.contains("broad_market=bad"));
    }

    #[test]
    fn env_wrapper_defaults_to_disabled_when_unset() {
        // Use a key guaranteed not to collide with any real env var.
        std::env::remove_var(ENV_SECTOR_EXPOSURE_LIMITS_BPS);
        assert_eq!(
            sector_exposure_limits_bps_from_env().unwrap(),
            HashMap::new()
        );
    }
}
