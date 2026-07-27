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
//!
//! # FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 (Defects 3, 4, 8)
//!
//! The previous validator checked only `plan_id == cycle_id` and a handful
//! of candidate-shape invariants — it never recomputed the deterministic
//! cycle id from persisted evidence, never re-ran the pure policy resolver,
//! and never reconciled `symbol_group_count` against the candidates' actual
//! canonical symbol groups. A corrupted row could set `plan_id` and
//! `cycle_id` to the same arbitrary (but matching) UUID and still pass.
//!
//! [`validate_plan_with_candidates`] now, whenever the persisted row carries
//! full current-schema provenance (every candidate has `order_type`,
//! `time_in_force`, and `bar_present` recorded — never true for a
//! pre-0057-repair legacy row):
//! 1. Reconstructs the exact `ConflictCandidateInput` batch from the
//!    persisted candidate rows.
//! 2. Recomputes the cycle id via the single shared
//!    [`crate::runtime_strategy_conflict::compute_conflict_cycle_id`] and
//!    requires it to equal both `plan_id` and `cycle_id`.
//! 3. Re-runs the pure `mqk_portfolio::resolve_conflict_cycle` resolver
//!    against the reconstructed batch and compares every recomputed
//!    candidate outcome (selected / disposition / reason_code /
//!    proposed_target_qty) against the persisted evidence, keyed by
//!    `ordinal`.
//! 4. Reconciles `symbol_group_count` against the candidates' actual
//!    distinct canonical symbol groups.
//!
//! Only when every recomputed fact matches exactly does the row pass. A
//! superseded schema version (see `mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION`)
//! is legacy/incomplete evidence, never current active truth —
//! `SUPPORTED_SCHEMA_VERSIONS` intentionally admits only the current
//! version.
//!
//! Defect 8: total blockers and per-plan candidate count are both bounded
//! (see [`MAX_BLOCKERS`], [`MAX_BLOCKER_LEN`], [`MAX_CANDIDATES_PER_PLAN`])
//! so a pathological row can never make a read-side validation pass
//! unbounded work or return an unbounded response body.

use std::collections::{HashMap, HashSet};

use mqk_db::{RuntimeStrategyConflictCandidateRecord, RuntimeStrategyConflictPlanRecord};
use mqk_portfolio::{ConflictCandidateInput, ConflictCycleContext};

use crate::runtime_strategy_conflict::{compute_conflict_cycle_id, disposition_str};
use crate::runtime_strategy_conflict_mode::ConflictPolicyMode;

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

/// Only the current schema version is ever "active current truth" — a
/// row persisted under a superseded version (e.g. the pre-FINAL-IDENTITY-
/// AND-READ-AUTHORITY-REPAIR-01 `-v1` schema) is legacy/incomplete evidence
/// by definition, never silently treated as equivalent to the current
/// contract.
const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &[mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION];

/// Defect 8: maximum blockers ever returned in one [`EvidenceValidation`].
/// When exceeded, the list is truncated and a final bounded truth message
/// (never echoing raw values) states that additional blockers were omitted.
const MAX_BLOCKERS: usize = 25;

/// Defect 8: maximum length (bytes) of any one blocker string. Every
/// blocker is built from this daemon's own prior writes (never raw
/// caller-supplied input), but a corrupted row could still contain an
/// arbitrarily long stored string (e.g. `market_date`), so every blocker is
/// truncated defensively before being returned.
const MAX_BLOCKER_LEN: usize = 200;

/// Defect 8: maximum candidates validated/projected for one plan. Normal
/// plans carry a handful of candidates (bounded by
/// `watchlist_intake::MULTI_SYMBOL_HARD_CEILING`, currently 5, times at
/// most a few competing strategies per symbol); this bound is generous
/// headroom while still refusing to perform unbounded validation work or
/// return an unbounded response body for a pathological row.
const MAX_CANDIDATES_PER_PLAN: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceValidation {
    pub valid: bool,
    /// Bounded, human-readable blockers. Never echoes untrusted external
    /// input — every value interpolated here originated from this
    /// daemon's own prior writes.
    pub blockers: Vec<String>,
}

/// Defect 8: bound the final blockers vector -- at most [`MAX_BLOCKERS`]
/// entries, each at most [`MAX_BLOCKER_LEN`] bytes, with a final bounded
/// truth message (never echoing a raw stored value) when truncation
/// occurred. Applied once, at the very end of validation, so every
/// intermediate check above can push freely without its own bound.
fn bound_blockers(mut blockers: Vec<String>) -> Vec<String> {
    for b in &mut blockers {
        if b.len() > MAX_BLOCKER_LEN {
            b.truncate(MAX_BLOCKER_LEN);
            b.push_str("... (truncated)");
        }
    }
    if blockers.len() > MAX_BLOCKERS {
        let omitted = blockers.len() - (MAX_BLOCKERS - 1);
        blockers.truncate(MAX_BLOCKERS - 1);
        blockers.push(format!(
            "{omitted} additional blocker(s) omitted (bounded evidence output)"
        ));
    }
    blockers
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

    let blockers = bound_blockers(blockers);
    EvidenceValidation {
        valid: blockers.is_empty(),
        blockers,
    }
}

/// The full check: [`validate_plan_shape`] plus every candidate-level
/// invariant (candidate_count/selected_count/refused_count reconciliation,
/// at most one selected candidate per canonical symbol, unique ordinals,
/// side/qty/timeframe/current/target invariants, no negative target, known
/// disposition/reason_code, selected/disposition coherence, full bar
/// provenance presence coherence, canonical symbol-group reconciliation,
/// and — whenever the row carries full current-schema provenance — a
/// complete recomputation of cycle identity and the pure policy outcome
/// (FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defects 3/4). Only callable
/// once the exact candidates for `plan.plan_id` have been fetched.
pub fn validate_plan_with_candidates(
    plan: &RuntimeStrategyConflictPlanRecord,
    candidates: &[RuntimeStrategyConflictCandidateRecord],
) -> EvidenceValidation {
    let mut result = validate_plan_shape(plan);

    // Defect 8: refuse to perform unbounded per-candidate validation work
    // (or return an unbounded response body) for a pathological row.
    if candidates.len() > MAX_CANDIDATES_PER_PLAN {
        result.blockers.push(format!(
            "candidate_count {} exceeds the maximum {} permitted for read-side validation \
             (bounded read)",
            candidates.len(),
            MAX_CANDIDATES_PER_PLAN
        ));
        let blockers = bound_blockers(result.blockers);
        return EvidenceValidation {
            valid: false,
            blockers,
        };
    }

    if plan.candidate_count as usize != candidates.len() {
        result.blockers.push(format!(
            "candidate_count {} does not match {} actual candidate rows",
            plan.candidate_count,
            candidates.len()
        ));
    }

    // Defect 4: a persisted non-off `mode` can only ever equal its own
    // `configured_mode` -- `runtime_strategy_conflict_mode::effective_mode`
    // has exactly two outcomes: `effective == configured`, or `effective ==
    // Off` (which is never persisted -- `gather_and_resolve` returns before
    // persistence when the effective mode is `Off`). A persisted row whose
    // `configured_mode` differs from `mode` is a combination the runtime
    // can never actually produce.
    if let Some(cm) = plan.configured_mode.as_deref() {
        if cm != plan.mode.as_str() {
            result.blockers.push(format!(
                "configured_mode '{cm}' and persisted mode '{}' is a combination the runtime \
                 can never actually persist (a persisted non-off mode always equals its own \
                 configured_mode)",
                plan.mode
            ));
        }
    }

    // Defect 4: plan-level truth/blockers coherence -- the pure resolver's
    // contract always produces `truth_state == "computed"` and an empty
    // `blockers` vec (see `mqk_portfolio::conflict_policy::resolve_conflict_cycle`).
    if plan.truth_state != mqk_portfolio::conflict_policy::TRUTH_STATE_COMPUTED {
        result.blockers.push(format!(
            "plan truth_state '{}' is not the pure resolver's contractual value",
            plan.truth_state
        ));
    }
    if !plan.blockers.is_empty() {
        result.blockers.push(
            "plan carries a non-empty blockers list, which the pure resolver's current \
             contract never produces"
                .to_string(),
        );
    }

    let mut seen_ordinals: HashSet<i32> = HashSet::new();
    let mut duplicate_ordinal = false;
    let mut selected_count = 0i32;
    let mut refused_count = 0i32;
    let mut selected_by_symbol: HashMap<String, i32> = HashMap::new();
    let mut canonical_symbol_groups: HashSet<String> = HashSet::new();

    for c in candidates {
        if !seen_ordinals.insert(c.ordinal) {
            duplicate_ordinal = true;
        }

        if c.plan_id != plan.plan_id {
            result.blockers.push(format!(
                "candidate ordinal {} carries plan_id {} which does not match this plan's {}",
                c.ordinal, c.plan_id, plan.plan_id
            ));
        }
        let canonical_symbol = mqk_portfolio::canonical_symbol(&c.symbol);
        if canonical_symbol.is_empty() {
            result.blockers.push(format!(
                "candidate ordinal {} has a blank canonical symbol",
                c.ordinal
            ));
        }
        if c.strategy_id.trim().is_empty() {
            result.blockers.push(format!(
                "candidate ordinal {} has a blank strategy_id",
                c.ordinal
            ));
        }
        canonical_symbol_groups.insert(canonical_symbol);
        if c.side != "buy" && c.side != "sell" {
            result.blockers.push(format!(
                "candidate ordinal {} has invalid side '{}'",
                c.ordinal, c.side
            ));
        }
        if c.qty <= 0 {
            result.blockers.push(format!(
                "candidate ordinal {} has non-positive qty",
                c.ordinal
            ));
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
        // Defect 4 gap: `selected=false` with disposition `passthrough` was
        // not previously rejected -- `passthrough` (by the pure model's own
        // contract) always implies `selected`.
        if !c.selected && c.disposition == "passthrough" {
            result.blockers.push(format!(
                "candidate ordinal {} has disposition 'passthrough' but selected=false",
                c.ordinal
            ));
        }
        // Defect 4 gap: a selected candidate with no proposed_target_qty
        // was not previously rejected -- the pure model only ever selects
        // a candidate whose own delta arithmetic succeeded.
        if c.selected && c.proposed_target_qty.is_none() {
            result.blockers.push(format!(
                "candidate ordinal {} is selected but has no proposed_target_qty",
                c.ordinal
            ));
        }
        if c.selected {
            selected_count += 1;
            *selected_by_symbol
                .entry(mqk_portfolio::canonical_symbol(&c.symbol))
                .or_insert(0) += 1;
        }
        if matches!(
            c.disposition.as_str(),
            "refused_invalid" | "refused_conflict"
        ) {
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

        // Defect 4 gap: a current-schema plan (configured_mode recorded)
        // whose candidate is nonetheless missing order_type/time_in_force/
        // bar_present is internally incoherent, not merely legacy.
        if plan.configured_mode.is_some()
            && (c.order_type.is_none() || c.time_in_force.is_none() || c.bar_present.is_none())
        {
            result.blockers.push(format!(
                "candidate ordinal {} is missing current-schema order_type/time_in_force/\
                 bar_present provenance on a plan that is not legacy",
                c.ordinal
            ));
        }
    }

    if duplicate_ordinal {
        result
            .blockers
            .push("duplicate candidate ordinal within this plan".to_string());
    }
    for (symbol, count) in &selected_by_symbol {
        if *count > 1 {
            result.blockers.push(format!(
                "symbol {symbol} has more than one selected candidate"
            ));
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
    // Defect 4 gap: symbol_group_count was never reconciled against the
    // candidates' actual canonical symbol groups.
    if canonical_symbol_groups.len() as i32 != plan.symbol_group_count {
        result.blockers.push(format!(
            "symbol_group_count {} does not match {} actual canonical symbol group(s)",
            plan.symbol_group_count,
            canonical_symbol_groups.len()
        ));
    }

    // ------------------------------------------------------------------
    // Defects 3/4: full reconstruction, cycle-id recomputation, and a
    // re-run of the pure resolver -- only when every candidate carries
    // full current-schema provenance. A legacy (pre-0057) row already
    // fails above (absent configured_mode / missing per-candidate
    // provenance) and cannot be safely reconstructed into a well-typed
    // `ConflictCandidateInput` batch regardless.
    // ------------------------------------------------------------------
    let can_reconstruct = plan.configured_mode.is_some()
        && candidates.iter().all(|c| {
            c.order_type.is_some() && c.time_in_force.is_some() && c.bar_present.is_some()
        });

    if can_reconstruct {
        let parsed_modes = (
            plan.configured_mode
                .as_deref()
                .and_then(ConflictPolicyMode::parse),
            ConflictPolicyMode::parse(&plan.mode),
        );
        match parsed_modes {
            (Some(configured), Some(effective)) => {
                let inputs: Vec<ConflictCandidateInput> = candidates
                    .iter()
                    .map(|c| ConflictCandidateInput {
                        ordinal: c.ordinal as usize,
                        symbol: c.symbol.clone(),
                        strategy_id: c.strategy_id.clone(),
                        timeframe_secs: c.timeframe_secs,
                        side: c.side.clone(),
                        qty: c.qty,
                        current_qty: c.current_qty,
                        order_type: c.order_type.clone().unwrap_or_default(),
                        time_in_force: c.time_in_force.clone().unwrap_or_default(),
                        limit_price: c.limit_price,
                        bar_symbol: c.bar_symbol.clone(),
                        bar_strategy_id: c.bar_strategy_id.clone(),
                        bar_timeframe: c.bar_timeframe.clone(),
                        bar_end_ts: c.bar_end_ts,
                        close_micros: c.close_micros,
                    })
                    .collect();

                let recomputed_cycle_id = compute_conflict_cycle_id(
                    plan.run_id,
                    &plan.market_date,
                    configured,
                    effective,
                    &inputs,
                );
                if recomputed_cycle_id != plan.cycle_id.to_string() {
                    result.blockers.push(
                        "recomputed cycle_id does not equal the persisted plan_id/cycle_id"
                            .to_string(),
                    );
                }

                let cycle_context = ConflictCycleContext {
                    cycle_id: plan.cycle_id.to_string(),
                    run_id: plan.run_id.to_string(),
                    market_date: plan.market_date.clone(),
                    policy_schema_version: plan.policy_schema_version.clone(),
                };
                let recomputed = mqk_portfolio::resolve_conflict_cycle(cycle_context, &inputs);
                let recomputed_by_ordinal: HashMap<usize, &mqk_portfolio::ConflictCandidateResult> =
                    recomputed
                        .symbol_results
                        .iter()
                        .flat_map(|s| s.candidates.iter())
                        .map(|c| (c.ordinal, c))
                        .collect();

                for c in candidates {
                    match recomputed_by_ordinal.get(&(c.ordinal as usize)) {
                        Some(expected) => {
                            let expected_disposition = disposition_str(expected.disposition);
                            if expected.selected != c.selected
                                || expected_disposition != c.disposition
                                || expected.reason_code != c.reason_code
                                || expected.proposed_target_qty != c.proposed_target_qty
                            {
                                result.blockers.push(format!(
                                    "candidate ordinal {} recomputed policy outcome does not \
                                     match persisted evidence",
                                    c.ordinal
                                ));
                            }
                        }
                        None => result.blockers.push(format!(
                            "candidate ordinal {} is absent from the recomputed policy batch",
                            c.ordinal
                        )),
                    }
                }
            }
            _ => {
                result.blockers.push(
                    "configured_mode/mode cannot be parsed into a recognized policy mode -- \
                     cycle identity cannot be recomputed"
                        .to_string(),
                );
            }
        }
    }

    let blockers = bound_blockers(result.blockers);
    EvidenceValidation {
        valid: blockers.is_empty(),
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn default_run_id() -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"test.conflict-evidence-validation.run",
        )
    }

    const MARKET_DATE: &str = "2026-07-26";

    fn candidate_input(
        ordinal: usize,
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        current_qty: i64,
        bar_symbol: Option<&str>,
    ) -> ConflictCandidateInput {
        ConflictCandidateInput {
            ordinal,
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty,
            current_qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: bar_symbol.map(str::to_string),
            bar_strategy_id: Some(strategy_id.to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
        }
    }

    fn single_well_formed_candidate() -> ConflictCandidateInput {
        candidate_input(0, "AAPL", "s1", 10, 0, Some("AAPL"))
    }

    /// Builds a fully self-consistent `(plan, candidates)` pair by actually
    /// running the single shared identity function
    /// ([`compute_conflict_cycle_id`]) and the real pure resolver
    /// ([`mqk_portfolio::resolve_conflict_cycle`]) against `inputs` — so a
    /// test proving the validator catches *corruption* mutates the
    /// returned records afterward (simulating a tampered/stale row), never
    /// hand-computes plan/candidate fields that could silently drift from
    /// what the real runtime would ever actually persist.
    fn consistent_evidence(
        inputs: Vec<ConflictCandidateInput>,
    ) -> (
        RuntimeStrategyConflictPlanRecord,
        Vec<RuntimeStrategyConflictCandidateRecord>,
    ) {
        let plan_id_str = compute_conflict_cycle_id(
            default_run_id(),
            MARKET_DATE,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &inputs,
        );
        let plan_id: Uuid = plan_id_str.parse().expect("valid uuid");
        let cycle_context = ConflictCycleContext {
            cycle_id: plan_id_str,
            run_id: default_run_id().to_string(),
            market_date: MARKET_DATE.to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
        };
        let result = mqk_portfolio::resolve_conflict_cycle(cycle_context, &inputs);

        let mut candidate_records = Vec::new();
        let mut selected_count = 0i32;
        let mut refused_count = 0i32;
        for sym in &result.symbol_results {
            for c in &sym.candidates {
                if c.selected {
                    selected_count += 1;
                }
                if matches!(
                    c.disposition,
                    mqk_portfolio::ConflictDisposition::RefusedInvalid
                        | mqk_portfolio::ConflictDisposition::RefusedConflict
                ) {
                    refused_count += 1;
                }
                let bar_present = c.bar_symbol.is_some()
                    && c.bar_strategy_id.is_some()
                    && c.bar_timeframe.is_some()
                    && c.bar_end_ts.is_some()
                    && c.close_micros.is_some();
                candidate_records.push(RuntimeStrategyConflictCandidateRecord {
                    plan_id,
                    ordinal: c.ordinal as i32,
                    symbol: sym.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    order_type: Some(c.order_type.clone()),
                    time_in_force: Some(c.time_in_force.clone()),
                    limit_price: c.limit_price,
                    proposed_target_qty: c.proposed_target_qty,
                    bar_present: Some(bar_present),
                    bar_symbol: c.bar_symbol.clone(),
                    bar_strategy_id: c.bar_strategy_id.clone(),
                    bar_timeframe: c.bar_timeframe.clone(),
                    bar_end_ts: c.bar_end_ts,
                    close_micros: c.close_micros,
                    selected: c.selected,
                    disposition: disposition_str(c.disposition).to_string(),
                    reason_code: c.reason_code.clone(),
                });
            }
        }
        candidate_records.sort_by_key(|c| c.ordinal);

        let plan = RuntimeStrategyConflictPlanRecord {
            plan_id,
            cycle_id: plan_id,
            run_id: default_run_id(),
            mode: "shadow".to_string(),
            configured_mode: Some("shadow".to_string()),
            market_date: MARKET_DATE.to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
            symbol_group_count: result.symbol_results.len() as i32,
            candidate_count: candidate_records.len() as i32,
            selected_count,
            refused_count,
            truth_state: result.truth_state.clone(),
            blockers: result.blockers.clone(),
            created_at_utc: Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap(),
        };
        (plan, candidate_records)
    }

    #[test]
    fn well_formed_plan_and_candidates_are_valid() {
        let (p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(v.valid, "unexpected blockers: {:?}", v.blockers);
    }

    #[test]
    fn plan_id_cycle_id_mismatch_is_invalid() {
        let (mut p, _c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.cycle_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"test.conflict-evidence-validation.mismatch",
        );
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn legacy_row_with_no_configured_mode_is_invalid() {
        let (mut p, _c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.configured_mode = None;
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("configured_mode")));
    }

    #[test]
    fn off_mode_persisted_is_invalid() {
        let (mut p, _c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.mode = "off".to_string();
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn negative_count_is_invalid() {
        let (mut p, _c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.refused_count = -1;
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn selected_count_exceeding_candidate_count_is_invalid() {
        let (mut p, _c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.selected_count = 5;
        p.candidate_count = 1;
        let v = validate_plan_shape(&p);
        assert!(!v.valid);
    }

    #[test]
    fn candidate_count_mismatch_is_invalid() {
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.candidate_count = 2;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn two_selected_candidates_same_symbol_is_invalid() {
        // Two genuinely competing buys for AAPL -- the real resolver
        // selects exactly one (equal-target consensus, tie-break by
        // strategy_id ascending -> "s1"). Corrupt the persisted rows
        // afterward to force the loser back to selected=true, simulating
        // a tampered/stale row.
        let inputs = vec![
            candidate_input(0, "AAPL", "s1", 10, 0, Some("AAPL")),
            candidate_input(1, "AAPL", "s2", 10, 0, Some("AAPL")),
        ];
        let (mut p, mut c) = consistent_evidence(inputs);
        for row in c.iter_mut() {
            row.selected = true;
            if row.disposition == "not_selected" {
                row.disposition = "selected".to_string();
            }
        }
        p.selected_count = c.len() as i32;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("more than one selected")));
    }

    #[test]
    fn selected_true_with_not_selected_disposition_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].disposition = "not_selected".to_string();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn unknown_reason_code_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].reason_code = "totally_made_up_reason".to_string();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn negative_proposed_target_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].proposed_target_qty = Some(-5);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn proposed_target_not_matching_qty_current_side_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].proposed_target_qty = Some(999);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn bar_present_true_with_missing_field_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].close_micros = None;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
    }

    #[test]
    fn mismatched_but_fully_present_bar_facts_are_not_flagged_as_corruption() {
        // A candidate genuinely refused by the real resolver for a
        // mismatched (not missing) bar symbol is legitimate evidence, not
        // evidence corruption -- built entirely from the real pipeline, no
        // manual override.
        let input = candidate_input(0, "AAPL", "s1", 10, 0, Some("MSFT"));
        let (p, c) = consistent_evidence(vec![input]);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(v.valid, "unexpected blockers: {:?}", v.blockers);
    }

    #[test]
    fn legacy_candidate_row_with_none_provenance_does_not_double_flag() {
        let (mut p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].order_type = None;
        c[0].time_in_force = None;
        c[0].bar_present = None;
        c[0].bar_symbol = None;
        c[0].bar_strategy_id = None;
        c[0].bar_timeframe = None;
        c[0].close_micros = None;
        p.configured_mode = None;
        let v = validate_plan_with_candidates(&p, &c);
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

    // ── FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defects 3/4/8 ─────────

    #[test]
    fn arbitrary_matching_uuid_that_does_not_recompute_is_invalid() {
        // plan_id == cycle_id (an arbitrary but internally-matching UUID)
        // is no longer sufficient -- it must also equal the recomputed
        // identity for the exact persisted candidate evidence.
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        let arbitrary = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.arbitrary-matching-uuid");
        p.plan_id = arbitrary;
        p.cycle_id = arbitrary;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("recomputed cycle_id")));
    }

    #[test]
    fn changed_mode_after_persist_is_invalid() {
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.mode = "paper_enforced".to_string();
        p.configured_mode = Some("paper_enforced".to_string());
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("recomputed cycle_id")));
    }

    #[test]
    fn recomputed_reason_code_mismatch_is_invalid() {
        // reason_code is changed to a *different but still known* reason
        // code -- individual per-field checks (known-vocabulary, selected/
        // disposition coherence) all pass; only re-running the pure
        // resolver and comparing its actual output catches this.
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        assert_eq!(
            c[0].reason_code,
            mqk_portfolio::REASON_SINGLE_CANDIDATE_PASSTHROUGH
        );
        c[0].reason_code = mqk_portfolio::REASON_TARGET_CONSENSUS_PASSTHROUGH.to_string();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("recomputed policy outcome")));
    }

    #[test]
    fn symbol_group_count_mismatch_is_invalid() {
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.symbol_group_count = 2;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("symbol_group_count")));
    }

    #[test]
    fn configured_mode_mode_incoherent_combination_is_invalid() {
        // The runtime can never persist a non-off mode that differs from
        // its own configured_mode (see `runtime_strategy_conflict_mode::effective_mode`).
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.configured_mode = Some("paper_enforced".to_string());
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("can never actually persist")));
    }

    #[test]
    fn plan_truth_state_incoherent_is_invalid() {
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.truth_state = "something_else".to_string();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("truth_state")));
    }

    #[test]
    fn plan_blockers_nonempty_is_invalid() {
        let (mut p, c) = consistent_evidence(vec![single_well_formed_candidate()]);
        p.blockers = vec!["unexpected".to_string()];
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("non-empty blockers")));
    }

    #[test]
    fn passthrough_disposition_with_selected_false_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        assert_eq!(c[0].disposition, "passthrough");
        c[0].selected = false;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("passthrough")));
    }

    #[test]
    fn selected_candidate_with_none_target_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        assert!(c[0].selected);
        c[0].proposed_target_qty = None;
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("no proposed_target_qty")));
    }

    #[test]
    fn candidate_plan_id_mismatch_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].plan_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.wrong-plan-id");
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("does not match this plan's")));
    }

    #[test]
    fn blank_canonical_symbol_candidate_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].symbol = "   ".to_string();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v
            .blockers
            .iter()
            .any(|b| b.contains("blank canonical symbol")));
    }

    #[test]
    fn blank_strategy_id_candidate_is_invalid() {
        let (p, mut c) = consistent_evidence(vec![single_well_formed_candidate()]);
        c[0].strategy_id = String::new();
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("blank strategy_id")));
    }

    #[test]
    fn candidates_exceeding_bound_is_invalid() {
        let mut inputs = Vec::new();
        for i in 0..(MAX_CANDIDATES_PER_PLAN + 1) {
            inputs.push(candidate_input(
                i,
                &format!("SYM{i}"),
                "s1",
                10,
                0,
                Some(&format!("SYM{i}")),
            ));
        }
        let (p, c) = consistent_evidence(inputs);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert!(v.blockers.iter().any(|b| b.contains("exceeds the maximum")));
    }

    #[test]
    fn blockers_are_bounded_with_truncation_message() {
        // Every candidate below has an invalid side, so each independently
        // contributes at least one blocker -- comfortably exceeding
        // MAX_BLOCKERS while staying within MAX_CANDIDATES_PER_PLAN.
        let mut inputs = Vec::new();
        for i in 0..30 {
            let mut c =
                candidate_input(i, &format!("SYM{i}"), "s1", 10, 0, Some(&format!("SYM{i}")));
            c.side = "bogus".to_string();
            inputs.push(c);
        }
        // consistent_evidence can't be used here (it runs the resolver,
        // which is fine with invalid sides), so build directly.
        let (p, c) = consistent_evidence(inputs);
        let v = validate_plan_with_candidates(&p, &c);
        assert!(!v.valid);
        assert_eq!(v.blockers.len(), MAX_BLOCKERS);
        assert!(v.blockers.last().unwrap().contains("omitted"));
    }
}
