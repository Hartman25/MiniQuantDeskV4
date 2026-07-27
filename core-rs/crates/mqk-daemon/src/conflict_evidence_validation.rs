//! MULTI-STRATEGY-CONFLICT-POLICY-01-AUTHORITY-AND-EVIDENCE-REPAIR Defect 6.
//!
//! One shared read-side validator for durable
//! `sys_runtime_strategy_conflict_plans` / `_candidates` evidence, used by
//! every route in `routes/strategy_conflict.rs` (status, list, detail) so a
//! malformed persisted row can never be silently projected as active
//! current truth. Read-only: this module never writes, and never trusts
//! the DB's own CHECK constraints alone — it re-derives every invariant
//! independently at read time.
//!
//! Two entry points:
//! - [`validate_plan_shape`]: cheap, candidate-free structural checks. Safe
//!   to run on every row of a list without an extra DB round trip per row.
//! - [`validate_plan_with_candidates`]: the full check, only callable once
//!   the exact candidates for that `plan_id` have been fetched (status and
//!   detail always fetch them for the one plan they inspect).

use std::collections::{HashMap, HashSet};

use mqk_db::{RuntimeStrategyConflictCandidateRecord, RuntimeStrategyConflictPlanRecord};

/// Closed set of reason codes ever produced by
/// `mqk_portfolio::conflict_policy` — a persisted `reason_code` outside
/// this set is itself evidence corruption, not a policy outcome.
const KNOWN_REASON_CODES: &[&str] = &[
    mqk_portfolio::REASON_SINGLE_CANDIDATE_PASSTHROUGH,
    mqk_portfolio::REASON_TARGET_CONSENSUS_PASSTHROUGH,
    mqk_portfolio::REASON_RISK_REDUCING_CANDIDATE_SELECTED,
    mqk_portfolio::REASON_CONFLICTING_INCREASE_TARGETS_REFUSED,
    mqk_portfolio::REASON_INVALID_CANDIDATE_REFUSED,
    mqk_portfolio::REASON_MISSING_OR_MISMATCHED_BAR_FACTS,
    mqk_portfolio::REASON_DUPLICATE_ECONOMIC_CANDIDATE,
    mqk_portfolio::REASON_WOULD_CREATE_SHORT,
    mqk_portfolio::REASON_ARITHMETIC_OVERFLOW,
    mqk_portfolio::REASON_NO_VALID_CANDIDATE,
    mqk_portfolio::REASON_NOT_SELECTED,
    mqk_portfolio::REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION,
    mqk_portfolio::REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED,
];

const KNOWN_DISPOSITIONS: &[&str] = &[
    "passthrough",
    "selected",
    "not_selected",
    "refused_invalid",
    "refused_conflict",
];

const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &[mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION];

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceValidation {
    pub valid: bool,
    /// Bounded, human-readable blockers. Never echoes untrusted external
    /// input — every value interpolated here originated from this
    /// daemon's own prior writes.
    pub blockers: Vec<String>,
}

fn is_market_date_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Cheap, candidate-free structural checks: `plan_id == cycle_id`,
/// supported schema version, mode/configured-mode coherence, run/
/// market-date shape, and nonnegative/internally-consistent counts.
pub fn validate_plan_shape(plan: &RuntimeStrategyConflictPlanRecord) -> EvidenceValidation {
    let mut blockers = Vec::new();

    if plan.plan_id != plan.cycle_id {
        blockers.push(format!(
            "plan_id {} does not equal cycle_id {}",
            plan.plan_id, plan.cycle_id
        ));
    }
    if !SUPPORTED_SCHEMA_VERSIONS.contains(&plan.policy_schema_version.as_str()) {
        blockers.push(format!(
            "unsupported policy_schema_version '{}'",
            plan.policy_schema_version
        ));
    }
    if !matches!(plan.mode.as_str(), "shadow" | "paper_enforced") {
        blockers.push(format!(
            "mode '{}' is not a valid persisted mode (must be shadow or paper_enforced)",
            plan.mode
        ));
    }
    match plan.configured_mode.as_deref() {
        None => blockers.push(
            "configured_mode is not recorded (legacy evidence written before schema \
             migration 0057; cannot be validated against the current evidence contract)"
                .to_string(),
        ),
        Some(cm) if !matches!(cm, "off" | "shadow" | "paper_enforced") => {
            blockers.push(format!("configured_mode '{cm}' is not a recognized mode"));
        }
        Some(_) => {}
    }
    if plan.run_id.is_nil() {
        blockers.push("run_id is nil".to_string());
    }
    if !is_market_date_shaped(&plan.market_date) {
        blockers.push(format!(
            "market_date '{}' is not YYYY-MM-DD shaped",
            plan.market_date
        ));
    }
    if plan.symbol_group_count < 0
        || plan.candidate_count < 0
        || plan.selected_count < 0
        || plan.refused_count < 0
    {
        blockers.push("a plan count field is negative".to_string());
    }
    if plan.selected_count > plan.symbol_group_count {
        blockers.push("selected_count exceeds symbol_group_count".to_string());
    }
    if plan.selected_count > plan.candidate_count {
        blockers.push("selected_count exceeds candidate_count".to_string());
    }

    EvidenceValidation {
        valid: blockers.is_empty(),
        blockers,
    }
}

/// The full check: [`validate_plan_shape`] plus every candidate-level
/// invariant (candidate_count/selected_count/refused_count reconciliation,
/// at most one selected candidate per canonical symbol, unique ordinals,
/// side/qty/timeframe/current/target invariants, no negative target, known
/// disposition/reason_code, selected/disposition coherence, and full bar
/// provenance presence coherence). Only callable once the exact candidates
/// for `plan.plan_id` have been fetched.
pub fn validate_plan_with_candidates(
    plan: &RuntimeStrategyConflictPlanRecord,
    candidates: &[RuntimeStrategyConflictCandidateRecord],
) -> EvidenceValidation {
    let mut result = validate_plan_shape(plan);

    if plan.candidate_count as usize != candidates.len() {
        result.blockers.push(format!(
            "candidate_count {} does not match {} actual candidate rows",
            plan.candidate_count,
            candidates.len()
        ));
    }

    let mut seen_ordinals: HashSet<i32> = HashSet::new();
    let mut duplicate_ordinal = false;
    let mut selected_count = 0i32;
    let mut refused_count = 0i32;
    let mut selected_by_symbol: HashMap<String, i32> = HashMap::new();

    for c in candidates {
        if !seen_ordinals.insert(c.ordinal) {
            duplicate_ordinal = true;
        }

        if c.side != "buy" && c.side != "sell" {
            result.blockers.push(format!(
                "candidate ordinal {} has invalid side '{}'",
                c.ordinal, c.side
            ));
        }
        if c.qty <= 0 {
            result
                .blockers
                .push(format!("candidate ordinal {} has non-positive qty", c.ordinal));
        }
        if c.current_qty < 0 {
            result.blockers.push(format!(
                "candidate ordinal {} has negative current_qty",
                c.ordinal
            ));
        }
        if c.timeframe_secs <= 0 {
            result.blockers.push(format!(
                "candidate ordinal {} has non-positive timeframe_secs",
                c.ordinal
            ));
        }
        if let Some(target) = c.proposed_target_qty {
            if target < 0 {
                result.blockers.push(format!(
                    "candidate ordinal {} has a negative proposed_target_qty (oversell)",
                    c.ordinal
                ));
            }
            let expected = match c.side.as_str() {
                "buy" => c.current_qty.checked_add(c.qty),
                "sell" => c.current_qty.checked_sub(c.qty),
                _ => None,
            };
            if expected != Some(target) {
                result.blockers.push(format!(
                    "candidate ordinal {} proposed_target_qty does not match current_qty/qty/side",
                    c.ordinal
                ));
            }
        }
        if !KNOWN_DISPOSITIONS.contains(&c.disposition.as_str()) {
            result.blockers.push(format!(
                "candidate ordinal {} has unknown disposition '{}'",
                c.ordinal, c.disposition
            ));
        }
        if !KNOWN_REASON_CODES.contains(&c.reason_code.as_str()) {
            result.blockers.push(format!(
                "candidate ordinal {} has unknown reason_code '{}'",
                c.ordinal, c.reason_code
            ));
        }
        let selected_disposition = matches!(c.disposition.as_str(), "selected" | "passthrough");
        if c.selected && !selected_disposition {
            result.blockers.push(format!(
                "candidate ordinal {} is selected but disposition is '{}'",
                c.ordinal, c.disposition
            ));
        }
        if !c.selected && c.disposition == "selected" {
            result.blockers.push(format!(
                "candidate ordinal {} has disposition 'selected' but selected=false",
                c.ordinal
            ));
        }
        if c.selected {
            selected_count += 1;
            *selected_by_symbol
                .entry(mqk_portfolio::canonical_symbol(&c.symbol))
                .or_insert(0) += 1;
        }
        if matches!(c.disposition.as_str(), "refused_invalid" | "refused_conflict") {
            refused_count += 1;
        }

        // Full bar provenance presence coherence. A mismatched-but-present
        // bar (e.g. wrong symbol) is legitimate refusal *evidence*, not
        // corruption -- only outright incoherence between `bar_present`
        // and the actual field presence is flagged here.
        match c.bar_present {
            Some(true) => {
                if c.bar_symbol.is_none()
                    || c.bar_strategy_id.is_none()
                    || c.bar_timeframe.is_none()
                    || c.bar_end_ts.is_none()
                    || c.close_micros.is_none()
                {
                    result.blockers.push(format!(
                        "candidate ordinal {} has bar_present=true but an incomplete bar field",
                        c.ordinal
                    ));
                }
            }
            Some(false) => {
                if c.bar_symbol.is_some()
                    || c.bar_strategy_id.is_some()
                    || c.bar_timeframe.is_some()
                    || c.bar_end_ts.is_some()
                    || c.close_micros.is_some()
                {
                    result.blockers.push(format!(
                        "candidate ordinal {} has bar_present=false but a bar field is populated",
                        c.ordinal
                    ));
                }
            }
            None => {
                // Legacy (pre-0057) row -- already flagged once at the plan
                // level via configured_mode being absent.
            }
        }
    }

    if duplicate_ordinal {
        result
            .blockers
            .push("duplicate candidate ordinal within this plan".to_string());
    }
    for (symbol, count) in &selected_by_symbol {
        if *count > 1 {
            result
                .blockers
                .push(format!("symbol {symbol} has more than one selected candidate"));
        }
    }
    if selected_count != plan.selected_count {
        result.blockers.push(format!(
            "selected_count {} does not match {} actually-selected candidate rows",
            plan.selected_count, selected_count
        ));
    }
    if refused_count != plan.refused_count {
        result.blockers.push(format!(
            "refused_count {} does not match {} actually-refused candidate rows",
            plan.refused_count, refused_count
        ));
    }

    result.valid = result.blockers.is_empty();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn plan(overrides: impl FnOnce(&mut RuntimeStrategyConflictPlanRecord)) -> RuntimeStrategyConflictPlanRecord {
        let plan_id = Uuid::new_v4();
        let mut p = RuntimeStrategyConflictPlanRecord {
            plan_id,
            cycle_id: plan_id,
            run_id: Uuid::new_v4(),
            mode: "shadow".to_string(),
            configured_mode: Some("shadow".to_string()),
            market_date: "2026-07-26".to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
            symbol_group_count: 1,
            candidate_count: 1,
            selected_count: 1,
            refused_count: 0,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc: Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        };
        overrides(&mut p);
        p
    }

    fn candidate(
        overrides: impl FnOnce(&mut RuntimeStrategyConflictCandidateRecord),
    ) -> RuntimeStrategyConflictCandidateRecord {
        let mut c = RuntimeStrategyConflictCandidateRecord {
            plan_id: Uuid::new_v4(),
            ordinal: 0,
            symbol: "AAPL".to_string(),
            strategy_id: "s1".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: 10,
            current_qty: 0,
            order_type: Some("market".to_string()),
            time_in_force: Some("day".to_string()),
            limit_price: None,
            proposed_target_qty: Some(10),
            bar_present: Some(true),
            bar_symbol: Some("AAPL".to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
            selected: true,
            disposition: "selected".to_string(),
            reason_code: mqk_portfolio::REASON_TARGET_CONSENSUS_PASSTHROUGH.to_string(),
        };
        overrides(&mut c);
        c
    }

    #[test]
    fn well_formed_plan_and_candidates_are_valid() {
        let p = plan(|_| {});
        let c = candidate(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(v.valid, "unexpected blockers: {:?}", v.blockers);
    }

    #[test]
    fn plan_id_cycle_id_mismatch_is_invalid() {
        let p = plan(|p| p.cycle_id = Uuid::new_v4());
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn legacy_row_with_no_configured_mode_is_invalid() {
        let p = plan(|p| p.configured_mode = None);
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("configured_mode")));
    }

    #[test]
    fn off_mode_persisted_is_invalid() {
        let p = plan(|p| p.mode = "off".to_string());
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn negative_count_is_invalid() {
        let p = plan(|p| p.refused_count = -1);
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn selected_count_exceeding_candidate_count_is_invalid() {
        let p = plan(|p| {
            p.selected_count = 5;
            p.candidate_count = 1;
        });
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn candidate_count_mismatch_is_invalid() {
        let p = plan(|p| p.candidate_count = 2);
        let c = candidate(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn two_selected_candidates_same_symbol_is_invalid() {
        let p = plan(|p| {
            p.candidate_count = 2;
            p.selected_count = 2;
        });
        let c1 = candidate(|c| c.ordinal = 0);
        let mut c2 = candidate(|c| c.ordinal = 1);
        c2.strategy_id = "s2".to_string();
        let v = validate_plan_with_candidates(&p, &[c1, c2]);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("more than one selected")));
    }

    #[test]
    fn selected_true_with_not_selected_disposition_is_invalid() {
        let c = candidate(|c| c.disposition = "not_selected".to_string());
        let p = plan(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn unknown_reason_code_is_invalid() {
        let c = candidate(|c| c.reason_code = "totally_made_up_reason".to_string());
        let p = plan(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn negative_proposed_target_is_invalid() {
        let c = candidate(|c| c.proposed_target_qty = Some(-5));
        let p = plan(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn proposed_target_not_matching_qty_current_side_is_invalid() {
        let c = candidate(|c| c.proposed_target_qty = Some(999));
        let p = plan(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn bar_present_true_with_missing_field_is_invalid() {
        let c = candidate(|c| c.close_micros = None);
        let p = plan(|_| {});
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(!v.valid);
    }

    #[test]
    fn mismatched_but_fully_present_bar_facts_are_not_flagged_as_corruption() {
        // A candidate refused for a mismatched (not missing) bar symbol is
        // legitimate evidence, not evidence corruption.
        let c = candidate(|c| {
            c.bar_symbol = Some("MSFT".to_string());
            c.disposition = "refused_invalid".to_string();
            c.selected = false;
            c.reason_code = mqk_portfolio::REASON_MISSING_OR_MISMATCHED_BAR_FACTS.to_string();
        });
        let p = plan(|p| {
            p.selected_count = 0;
            p.refused_count = 1;
        });
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        assert!(v.valid, "unexpected blockers: {:?}", v.blockers);
    }

    #[test]
    fn legacy_candidate_row_with_none_provenance_does_not_double_flag() {
        let c = candidate(|c| {
            c.order_type = None;
            c.time_in_force = None;
            c.bar_present = None;
            c.bar_symbol = None;
            c.bar_strategy_id = None;
            c.bar_timeframe = None;
            c.close_micros = None;
        });
        let p = plan(|p| p.configured_mode = None);
        let v = validate_plan_with_candidates(&p, std::slice::from_ref(&c));
        // Still invalid overall (legacy plan-level flag), but must not
        // additionally blow up or double-report the candidate-level absence.
        assert!(!v.valid);
        let legacy_blockers = v
            .blockers
            .iter()
            .filter(|b| b.contains("configured_mode"))
            .count();
        assert_eq!(legacy_blockers, 1);
    }
}
