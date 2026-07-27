//! mqk-portfolio: multi-strategy conflict policy
//!
//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase A — pure deterministic
//! conflict-resolution model for multiple same-cycle strategy decisions
//! concerning the same symbol.
//!
//! Pure: no IO, no DB, no clock, no env, no randomness, no new dependency.
//! Sits between strategy-decision derivation and Bundle 5's runtime
//! opportunity allocation (`mqk-daemon::runtime_opportunity_allocation`) —
//! this module never talks to that allocator and never duplicates its
//! capital-weighting logic. It only decides, per symbol, which single
//! *exact original* candidate (if any) survives into the batch Bundle 5
//! receives.
//!
//! # Scope (frozen by the operating rules for this bundle)
//! - Every candidate is either a strict increase (`side == "buy"`, which in
//!   this long-only model always implies `proposed_target_qty > current_qty`)
//!   or a strict reduction (`side == "sell"`, always implies
//!   `proposed_target_qty < current_qty`). There is no "hold" candidate —
//!   callers (mirroring `bar_result_to_decisions`) never construct a
//!   zero-delta decision.
//! - This module never synthesizes a target, never averages/nets/sums
//!   quantities, never scores/ranks by alpha, and never selects an
//!   economic "winner" among competing *increases* — differing increase
//!   targets refuse the whole symbol group.
//! - Every candidate this module can select is returned by ordinal
//!   reference into the caller's own input slice — this module never
//!   constructs a new decision, id, quantity, side, symbol, or bar.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Reason codes — bounded, closed vocabulary
// ---------------------------------------------------------------------------

pub const REASON_SINGLE_CANDIDATE_PASSTHROUGH: &str = "single_candidate_passthrough";
pub const REASON_TARGET_CONSENSUS_PASSTHROUGH: &str = "target_consensus_passthrough";
pub const REASON_RISK_REDUCING_CANDIDATE_SELECTED: &str = "risk_reducing_candidate_selected";
pub const REASON_CONFLICTING_INCREASE_TARGETS_REFUSED: &str =
    "conflicting_increase_targets_refused";
pub const REASON_INVALID_CANDIDATE_REFUSED: &str = "invalid_candidate_refused";
pub const REASON_MISSING_OR_MISMATCHED_BAR_FACTS: &str = "missing_or_mismatched_bar_facts";
pub const REASON_DUPLICATE_ECONOMIC_CANDIDATE: &str = "duplicate_economic_candidate";
pub const REASON_WOULD_CREATE_SHORT: &str = "would_create_short";
pub const REASON_ARITHMETIC_OVERFLOW: &str = "arithmetic_overflow";
pub const REASON_NO_VALID_CANDIDATE: &str = "no_valid_candidate";
/// A structurally valid candidate that lost to a selected sibling in its
/// group (risk-reducing selection or target-consensus tie-break). Not one of
/// the task's originally-listed reason codes, but required by "Record every
/// other candidate as not selected" — documented and tested here as an
/// additional, stable member of this closed vocabulary.
pub const REASON_NOT_SELECTED: &str = "not_selected";
/// A structurally valid new/increasing candidate that lost specifically
/// because a valid risk-reducing candidate exists in the same group (safety
/// precedence rule). More specific than [`REASON_NOT_SELECTED`] for
/// operator auditability; never used for any other purpose.
pub const REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION: &str =
    "increase_overridden_by_risk_reduction";

pub const TRUTH_STATE_COMPUTED: &str = "computed";

/// Frozen policy schema version, carried through into durable evidence.
pub const CONFLICT_POLICY_SCHEMA_VERSION: &str = "multi-strategy-conflict-policy-v1";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Immutable facts about one conflict-resolution cycle, supplied by the
/// caller. Every identity/timestamp field is caller-minted — this
/// zero-dependency crate never reads a clock or mints a UUID.
#[derive(Clone, Debug, PartialEq)]
pub struct ConflictCycleContext {
    /// Deterministic per-cycle identity (caller-minted).
    pub cycle_id: String,
    pub run_id: String,
    /// `"YYYY-MM-DD"`.
    pub market_date: String,
    pub policy_schema_version: String,
}

/// One symbol-scoped candidate under conflict-resolution consideration.
///
/// `ordinal` is the candidate's position in the caller's own input batch —
/// used only to identify which exact input candidate is selected/refused
/// (never part of economic identity, never used to break a tie in a way
/// that would make the result depend on input order).
#[derive(Clone, Debug, PartialEq)]
pub struct ConflictCandidateInput {
    pub ordinal: usize,
    pub symbol: String,
    pub strategy_id: String,
    /// Canonical timeframe string this decision's cycle was dispatched
    /// under (matches Bundle 5's `ctx.timeframe` usage) — compared against
    /// `bar_timeframe` for buy candidates.
    pub timeframe: String,
    pub timeframe_secs: i64,
    /// `"buy"` or `"sell"`, case-insensitive; anything else is structurally
    /// invalid.
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    /// Exact evaluated-bar identity this decision was derived from.
    /// Required (and must match `symbol`/`strategy_id`/`timeframe`) for a
    /// `"buy"` candidate; never consulted for a `"sell"` candidate (no price
    /// is needed to reduce or exit a position — mirrors Bundle 5's own
    /// sell/reduce exemption).
    pub bar_symbol: Option<String>,
    pub bar_strategy_id: Option<String>,
    pub bar_timeframe: Option<String>,
    pub bar_end_ts: Option<i64>,
    pub close_micros: Option<i64>,
}

/// Closed disposition vocabulary for one candidate's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictDisposition {
    /// Zero or one structurally valid candidate for this symbol this cycle
    /// — passed through unchanged.
    Passthrough,
    /// Chosen among multiple valid candidates (risk-reducing selection or
    /// target-consensus tie-break).
    Selected,
    /// A structurally valid candidate that lost to a selected sibling.
    NotSelected,
    /// Structurally invalid on its own terms (bad field, overflow, missing
    /// bar facts, would-create-short).
    RefusedInvalid,
    /// The whole symbol group was refused (conflicting increase targets, or
    /// a duplicate economic candidate identity within the group).
    RefusedConflict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConflictCandidateResult {
    pub ordinal: usize,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    /// `None` only when the candidate's own delta arithmetic overflowed.
    pub proposed_target_qty: Option<i64>,
    pub bar_end_ts: Option<i64>,
    pub selected: bool,
    pub disposition: ConflictDisposition,
    pub reason_code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConflictSymbolResult {
    pub symbol: String,
    /// The `ordinal` of the exact original candidate selected to pass
    /// through, if any.
    pub selected_ordinal: Option<usize>,
    pub disposition: ConflictDisposition,
    pub reason_code: String,
    /// One row per input candidate in this symbol's group, in input order.
    pub candidates: Vec<ConflictCandidateResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConflictCycleResult {
    pub context: ConflictCycleContext,
    /// Sorted by symbol ascending — deterministic regardless of the input
    /// candidate slice's order.
    pub symbol_results: Vec<ConflictSymbolResult>,
    pub truth_state: String,
    pub blockers: Vec<String>,
}

impl ConflictCycleResult {
    /// Every `selected_ordinal` across every symbol group, in symbol order.
    /// This is the exact ordinal set the caller should keep from its input
    /// batch — nothing else in the batch's buy/sell candidates survives.
    pub fn selected_ordinals(&self) -> Vec<usize> {
        self.symbol_results
            .iter()
            .filter_map(|s| s.selected_ordinal)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Per-candidate structural validity
// ---------------------------------------------------------------------------

enum Side {
    Buy,
    Sell,
}

fn parse_side(raw: &str) -> Option<Side> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "buy" => Some(Side::Buy),
        "sell" => Some(Side::Sell),
        _ => None,
    }
}

/// Outcome of validating one candidate in isolation (no knowledge of its
/// siblings yet).
struct Validated {
    proposed_target_qty: Option<i64>,
    valid: bool,
    reason_code: &'static str,
}

fn validate_candidate(c: &ConflictCandidateInput) -> Validated {
    let invalid = |reason: &'static str| Validated {
        proposed_target_qty: None,
        valid: false,
        reason_code: reason,
    };

    if c.symbol.trim().is_empty() {
        return invalid(REASON_INVALID_CANDIDATE_REFUSED);
    }
    let Some(side) = parse_side(&c.side) else {
        return invalid(REASON_INVALID_CANDIDATE_REFUSED);
    };
    if c.qty <= 0 {
        return invalid(REASON_INVALID_CANDIDATE_REFUSED);
    }

    let proposed = match side {
        Side::Buy => c.current_qty.checked_add(c.qty),
        Side::Sell => c.current_qty.checked_sub(c.qty),
    };
    let Some(proposed_target_qty) = proposed else {
        return Validated {
            proposed_target_qty: None,
            valid: false,
            reason_code: REASON_ARITHMETIC_OVERFLOW,
        };
    };
    if proposed_target_qty < 0 {
        return Validated {
            proposed_target_qty: Some(proposed_target_qty),
            valid: false,
            reason_code: REASON_WOULD_CREATE_SHORT,
        };
    }

    if let Side::Buy = side {
        let bar_ok = match (
            &c.bar_symbol,
            &c.bar_strategy_id,
            &c.bar_timeframe,
            c.bar_end_ts,
            c.close_micros,
        ) {
            (Some(bs), Some(bsid), Some(btf), Some(bts), Some(close)) => {
                bs.trim() == c.symbol.trim()
                    && bsid.trim() == c.strategy_id.trim()
                    && btf.trim() == c.timeframe.trim()
                    && bts > 0
                    && close > 0
            }
            _ => false,
        };
        if !bar_ok {
            return Validated {
                proposed_target_qty: Some(proposed_target_qty),
                valid: false,
                reason_code: REASON_MISSING_OR_MISMATCHED_BAR_FACTS,
            };
        }
    }

    Validated {
        proposed_target_qty: Some(proposed_target_qty),
        valid: true,
        reason_code: TRUTH_STATE_COMPUTED,
    }
}

// ---------------------------------------------------------------------------
// Deterministic tie-break
// ---------------------------------------------------------------------------

/// Sort key used to pick one canonical representative among candidates tied
/// on `proposed_target_qty` (risk-reducing selection) or all sharing the
/// same target (consensus passthrough): normalized `strategy_id` ascending,
/// then `timeframe_secs` ascending, then exact bar end timestamp ascending
/// (absent bar facts sort first via `i64::MIN`), then a final defensive,
/// wall-clock-free, random-free composite that should be unreachable in
/// practice — any full tie through the first three components already
/// implies a duplicate economic candidate identity, which is refused before
/// this tie-break ever runs (see [`resolve_conflict_cycle`]).
fn tie_break_key(c: &ConflictCandidateInput) -> (String, i64, i64, String, i64, i64) {
    (
        c.strategy_id.trim().to_string(),
        c.timeframe_secs,
        c.bar_end_ts.unwrap_or(i64::MIN),
        c.side.trim().to_ascii_lowercase(),
        c.qty,
        c.current_qty,
    )
}

/// Economic identity used to detect a duplicate candidate within one
/// symbol's group: two structurally valid candidates that claim to be
/// derived from the same strategy, side, and (for buys) the same exact bar.
fn economic_identity(c: &ConflictCandidateInput) -> (String, String, Option<i64>) {
    (
        c.strategy_id.trim().to_string(),
        c.side.trim().to_ascii_lowercase(),
        c.bar_end_ts,
    )
}

// ---------------------------------------------------------------------------
// Group (one symbol) resolution
// ---------------------------------------------------------------------------

fn resolve_symbol_group(symbol: &str, group: &[&ConflictCandidateInput]) -> ConflictSymbolResult {
    let validations: Vec<Validated> = group.iter().map(|c| validate_candidate(c)).collect();

    // Duplicate economic candidate identity among structurally valid
    // candidates fails the whole group closed (Q: never silently
    // deduplicate; never pick one arbitrarily).
    let mut seen: std::collections::HashSet<(String, String, Option<i64>)> =
        std::collections::HashSet::new();
    let mut has_duplicate = false;
    for (c, v) in group.iter().zip(validations.iter()) {
        if v.valid && !seen.insert(economic_identity(c)) {
            has_duplicate = true;
        }
    }

    if has_duplicate {
        let candidates = group
            .iter()
            .zip(validations.iter())
            .map(|(c, v)| ConflictCandidateResult {
                ordinal: c.ordinal,
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                side: c.side.clone(),
                qty: c.qty,
                current_qty: c.current_qty,
                proposed_target_qty: v.proposed_target_qty,
                bar_end_ts: c.bar_end_ts,
                selected: false,
                disposition: ConflictDisposition::RefusedConflict,
                reason_code: REASON_DUPLICATE_ECONOMIC_CANDIDATE.to_string(),
            })
            .collect();
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal: None,
            disposition: ConflictDisposition::RefusedConflict,
            reason_code: REASON_DUPLICATE_ECONOMIC_CANDIDATE.to_string(),
            candidates,
        };
    }

    let valid_indices: Vec<usize> = (0..group.len()).filter(|&i| validations[i].valid).collect();

    // Zero-or-one-candidate section: a group with exactly one input
    // candidate is decided purely by that candidate's own validity,
    // regardless of the multi-candidate machinery below.
    if group.len() == 1 {
        let (c, v) = (group[0], &validations[0]);
        let (disposition, reason_code, selected_ordinal) = if v.valid {
            (
                ConflictDisposition::Passthrough,
                REASON_SINGLE_CANDIDATE_PASSTHROUGH,
                Some(c.ordinal),
            )
        } else {
            (ConflictDisposition::RefusedInvalid, v.reason_code, None)
        };
        let candidates = vec![ConflictCandidateResult {
            ordinal: c.ordinal,
            strategy_id: c.strategy_id.clone(),
            timeframe_secs: c.timeframe_secs,
            side: c.side.clone(),
            qty: c.qty,
            current_qty: c.current_qty,
            proposed_target_qty: v.proposed_target_qty,
            bar_end_ts: c.bar_end_ts,
            selected: v.valid,
            disposition,
            reason_code: reason_code.to_string(),
        }];
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal,
            disposition,
            reason_code: reason_code.to_string(),
            candidates,
        };
    }

    if valid_indices.is_empty() {
        // Multiple candidates, none structurally valid.
        let reason = if group.len() == 1 {
            validations[0].reason_code
        } else {
            REASON_NO_VALID_CANDIDATE
        };
        let candidates = group
            .iter()
            .zip(validations.iter())
            .map(|(c, v)| ConflictCandidateResult {
                ordinal: c.ordinal,
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                side: c.side.clone(),
                qty: c.qty,
                current_qty: c.current_qty,
                proposed_target_qty: v.proposed_target_qty,
                bar_end_ts: c.bar_end_ts,
                selected: false,
                disposition: ConflictDisposition::RefusedInvalid,
                reason_code: v.reason_code.to_string(),
            })
            .collect();
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal: None,
            disposition: ConflictDisposition::RefusedInvalid,
            reason_code: reason.to_string(),
            candidates,
        };
    }

    if valid_indices.len() == 1 {
        let i = valid_indices[0];
        let (winner, _) = (group[i], &validations[i]);
        let candidates = group
            .iter()
            .zip(validations.iter())
            .enumerate()
            .map(|(idx, (c, v))| {
                let is_winner = idx == i;
                ConflictCandidateResult {
                    ordinal: c.ordinal,
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    proposed_target_qty: v.proposed_target_qty,
                    bar_end_ts: c.bar_end_ts,
                    selected: is_winner,
                    disposition: if is_winner {
                        ConflictDisposition::Passthrough
                    } else {
                        ConflictDisposition::RefusedInvalid
                    },
                    reason_code: if is_winner {
                        REASON_SINGLE_CANDIDATE_PASSTHROUGH.to_string()
                    } else {
                        v.reason_code.to_string()
                    },
                }
            })
            .collect();
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal: Some(winner.ordinal),
            disposition: ConflictDisposition::Passthrough,
            reason_code: REASON_SINGLE_CANDIDATE_PASSTHROUGH.to_string(),
            candidates,
        };
    }

    // >= 2 structurally valid candidates. Buy/sell dichotomy in this
    // long-only model means every valid sell is risk-reducing
    // (proposed_target_qty < current_qty, since qty > 0) and every valid
    // buy is a strict increase (proposed_target_qty > current_qty).
    let reducing: Vec<usize> = valid_indices
        .iter()
        .copied()
        .filter(|&i| parse_side(&group[i].side).map(|s| matches!(s, Side::Sell)) == Some(true))
        .collect();
    let increasing: Vec<usize> = valid_indices
        .iter()
        .copied()
        .filter(|&i| parse_side(&group[i].side).map(|s| matches!(s, Side::Buy)) == Some(true))
        .collect();

    let (selected_idx, group_disposition, group_reason) = if !reducing.is_empty() {
        let min_target = reducing
            .iter()
            .map(|&i| validations[i].proposed_target_qty.unwrap())
            .min()
            .unwrap();
        let winner = *reducing
            .iter()
            .filter(|&&i| validations[i].proposed_target_qty.unwrap() == min_target)
            .min_by_key(|&&i| tie_break_key(group[i]))
            .unwrap();
        (
            winner,
            ConflictDisposition::Selected,
            REASON_RISK_REDUCING_CANDIDATE_SELECTED,
        )
    } else {
        // No reduction; every valid candidate is a buy (increasing).
        let first_target = validations[increasing[0]].proposed_target_qty.unwrap();
        let all_same = increasing
            .iter()
            .all(|&i| validations[i].proposed_target_qty.unwrap() == first_target);
        if all_same {
            let winner = *increasing
                .iter()
                .min_by_key(|&&i| tie_break_key(group[i]))
                .unwrap();
            (
                winner,
                ConflictDisposition::Selected,
                REASON_TARGET_CONSENSUS_PASSTHROUGH,
            )
        } else {
            // Ambiguous: differing increase targets. No selection at all.
            let candidates = group
                .iter()
                .zip(validations.iter())
                .map(|(c, v)| {
                    let (disposition, reason_code) = if v.valid {
                        (
                            ConflictDisposition::RefusedConflict,
                            REASON_CONFLICTING_INCREASE_TARGETS_REFUSED.to_string(),
                        )
                    } else {
                        (
                            ConflictDisposition::RefusedInvalid,
                            v.reason_code.to_string(),
                        )
                    };
                    ConflictCandidateResult {
                        ordinal: c.ordinal,
                        strategy_id: c.strategy_id.clone(),
                        timeframe_secs: c.timeframe_secs,
                        side: c.side.clone(),
                        qty: c.qty,
                        current_qty: c.current_qty,
                        proposed_target_qty: v.proposed_target_qty,
                        bar_end_ts: c.bar_end_ts,
                        selected: false,
                        disposition,
                        reason_code,
                    }
                })
                .collect();
            return ConflictSymbolResult {
                symbol: symbol.to_string(),
                selected_ordinal: None,
                disposition: ConflictDisposition::RefusedConflict,
                reason_code: REASON_CONFLICTING_INCREASE_TARGETS_REFUSED.to_string(),
                candidates,
            };
        }
    };

    let selected_via_reduction = !reducing.is_empty();
    let candidates = group
        .iter()
        .zip(validations.iter())
        .enumerate()
        .map(|(idx, (c, v))| {
            if idx == selected_idx {
                return ConflictCandidateResult {
                    ordinal: c.ordinal,
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    proposed_target_qty: v.proposed_target_qty,
                    bar_end_ts: c.bar_end_ts,
                    selected: true,
                    disposition: group_disposition,
                    reason_code: group_reason.to_string(),
                };
            }
            if !v.valid {
                return ConflictCandidateResult {
                    ordinal: c.ordinal,
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    proposed_target_qty: v.proposed_target_qty,
                    bar_end_ts: c.bar_end_ts,
                    selected: false,
                    disposition: ConflictDisposition::RefusedInvalid,
                    reason_code: v.reason_code.to_string(),
                };
            }
            let is_increasing = parse_side(&c.side).map(|s| matches!(s, Side::Buy)) == Some(true);
            let reason = if selected_via_reduction && is_increasing {
                REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION
            } else {
                REASON_NOT_SELECTED
            };
            ConflictCandidateResult {
                ordinal: c.ordinal,
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                side: c.side.clone(),
                qty: c.qty,
                current_qty: c.current_qty,
                proposed_target_qty: v.proposed_target_qty,
                bar_end_ts: c.bar_end_ts,
                selected: false,
                disposition: ConflictDisposition::NotSelected,
                reason_code: reason.to_string(),
            }
        })
        .collect();

    ConflictSymbolResult {
        symbol: symbol.to_string(),
        selected_ordinal: Some(group[selected_idx].ordinal),
        disposition: group_disposition,
        reason_code: group_reason.to_string(),
        candidates,
    }
}

// ---------------------------------------------------------------------------
// Cycle (whole batch) resolution
// ---------------------------------------------------------------------------

/// Resolve one whole same-cycle batch of candidates, grouped independently
/// per normalized symbol (trim only — no case-folding; tickers are already
/// canonical in this codebase). Deterministic: identical inputs, in any
/// order, always produce a bit-identical [`ConflictCycleResult`] (symbol
/// groups sorted by symbol ascending in the output; within-group ordering
/// is `candidates` in original input order for audit readability, but the
/// *decision* of which one is selected never depends on that order).
pub fn resolve_conflict_cycle(
    context: ConflictCycleContext,
    candidates: &[ConflictCandidateInput],
) -> ConflictCycleResult {
    let mut groups: BTreeMap<String, Vec<&ConflictCandidateInput>> = BTreeMap::new();
    for c in candidates {
        groups
            .entry(c.symbol.trim().to_string())
            .or_default()
            .push(c);
    }

    let symbol_results: Vec<ConflictSymbolResult> = groups
        .into_iter()
        .map(|(symbol, group)| resolve_symbol_group(&symbol, &group))
        .collect();

    ConflictCycleResult {
        context,
        symbol_results,
        truth_state: TRUTH_STATE_COMPUTED.to_string(),
        blockers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ConflictCycleContext {
        ConflictCycleContext {
            cycle_id: "cycle-1".to_string(),
            run_id: "run-1".to_string(),
            market_date: "2026-07-26".to_string(),
            policy_schema_version: CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
        }
    }

    fn buy(
        ordinal: usize,
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        current_qty: i64,
        bar_end_ts: i64,
        close_micros: i64,
    ) -> ConflictCandidateInput {
        ConflictCandidateInput {
            ordinal,
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: "5m".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty,
            current_qty,
            bar_symbol: Some(symbol.to_string()),
            bar_strategy_id: Some(strategy_id.to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(bar_end_ts),
            close_micros: Some(close_micros),
        }
    }

    fn sell(
        ordinal: usize,
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        current_qty: i64,
    ) -> ConflictCandidateInput {
        ConflictCandidateInput {
            ordinal,
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: "5m".to_string(),
            timeframe_secs: 300,
            side: "sell".to_string(),
            qty,
            current_qty,
            bar_symbol: None,
            bar_strategy_id: None,
            bar_timeframe: None,
            bar_end_ts: None,
            close_micros: None,
        }
    }

    fn result_for<'a>(res: &'a ConflictCycleResult, symbol: &str) -> &'a ConflictSymbolResult {
        res.symbol_results
            .iter()
            .find(|s| s.symbol == symbol)
            .unwrap_or_else(|| panic!("no result for {symbol}"))
    }

    // ── Basic behavior ────────────────────────────────────────────────────

    #[test]
    fn empty_input_produces_empty_output() {
        let res = resolve_conflict_cycle(ctx(), &[]);
        assert!(res.symbol_results.is_empty());
        assert_eq!(res.truth_state, TRUTH_STATE_COMPUTED);
    }

    #[test]
    fn one_valid_candidate_is_exact_passthrough() {
        let candidates = vec![buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
        assert_eq!(aapl.reason_code, REASON_SINGLE_CANDIDATE_PASSTHROUGH);
        assert_eq!(aapl.selected_ordinal, Some(0));
    }

    #[test]
    fn one_invalid_candidate_is_refused_with_exact_reason() {
        let candidates = vec![buy(0, "AAPL", "s1", -5, 0, 1_000, 100_000_000)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_INVALID_CANDIDATE_REFUSED);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn multiple_candidates_at_most_one_selected_per_symbol() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", 10, 0, 1_000, 100_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        // Same target (equal-target consensus) -> tie-break on strategy_id
        // ascending -> "s1" (ordinal 0) wins.
        assert_eq!(aapl.selected_ordinal, Some(0));
        assert_eq!(
            aapl.candidates.iter().filter(|c| c.selected).count(),
            1,
            "at most one candidate selected per symbol"
        );
    }

    // ── Determinism ────────────────────────────────────────────────────────

    #[test]
    fn input_order_does_not_change_result() {
        let forward = vec![sell(0, "AAPL", "s1", 5, 20), sell(1, "AAPL", "s2", 8, 20)];
        let reversed = vec![sell(0, "AAPL", "s2", 8, 20), sell(1, "AAPL", "s1", 5, 20)];
        let r1 = resolve_conflict_cycle(ctx(), &forward);
        let r2 = resolve_conflict_cycle(ctx(), &reversed);
        // Both must select the strategy proposing the smallest target
        // (20-8=12 < 20-5=15), regardless of which ordinal/position it held.
        let sym1 = result_for(&r1, "AAPL");
        let sym2 = result_for(&r2, "AAPL");
        assert_eq!(sym1.reason_code, sym2.reason_code);
        assert_eq!(sym1.disposition, sym2.disposition);
        let selected1 = sym1.candidates.iter().find(|c| c.selected).unwrap();
        let selected2 = sym2.candidates.iter().find(|c| c.selected).unwrap();
        assert_eq!(selected1.strategy_id, "s2");
        assert_eq!(selected2.strategy_id, "s2");
    }

    #[test]
    fn one_symbol_conflict_does_not_suppress_unrelated_symbol() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", 20, 0, 1_000, 100_000_000), // differing target -> AAPL refused
            buy(2, "MSFT", "s1", 5, 0, 2_000, 50_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        assert_eq!(
            result_for(&res, "AAPL").disposition,
            ConflictDisposition::RefusedConflict
        );
        assert_eq!(
            result_for(&res, "MSFT").disposition,
            ConflictDisposition::Passthrough
        );
        assert_eq!(result_for(&res, "MSFT").selected_ordinal, Some(2));
    }

    // ── Consensus and conflict ────────────────────────────────────────────

    #[test]
    fn equal_target_consensus_selects_one_exact_original_candidate() {
        let candidates = vec![
            buy(0, "AAPL", "s2", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s1", 10, 0, 2_000, 100_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Selected);
        assert_eq!(aapl.reason_code, REASON_TARGET_CONSENSUS_PASSTHROUGH);
        // tie-break: strategy_id ascending -> "s1" (ordinal 1) wins.
        assert_eq!(aapl.selected_ordinal, Some(1));
    }

    #[test]
    fn differing_increasing_targets_refuse_the_whole_symbol() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", 20, 0, 1_000, 100_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(
            aapl.reason_code,
            REASON_CONFLICTING_INCREASE_TARGETS_REFUSED
        );
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn buy_versus_reduction_reduction_is_selected() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 20, 1_000, 100_000_000),
            sell(1, "AAPL", "s2", 5, 20),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Selected);
        assert_eq!(aapl.reason_code, REASON_RISK_REDUCING_CANDIDATE_SELECTED);
        assert_eq!(aapl.selected_ordinal, Some(1));
        let loser = aapl.candidates.iter().find(|c| c.ordinal == 0).unwrap();
        assert_eq!(
            loser.reason_code,
            REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION
        );
    }

    #[test]
    fn multiple_reductions_selects_smallest_explicit_target() {
        let candidates = vec![
            sell(0, "AAPL", "s1", 3, 20), // target 17
            sell(1, "AAPL", "s2", 8, 20), // target 12 (greater reduction)
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.selected_ordinal, Some(1));
        let winner = aapl.candidates.iter().find(|c| c.selected).unwrap();
        assert_eq!(winner.proposed_target_qty, Some(12));
    }

    #[test]
    fn equal_selected_targets_tie_break_is_stable() {
        let forward = vec![sell(0, "AAPL", "zzz", 8, 20), sell(1, "AAPL", "aaa", 8, 20)];
        let reversed = vec![sell(0, "AAPL", "aaa", 8, 20), sell(1, "AAPL", "zzz", 8, 20)];
        let r1 = resolve_conflict_cycle(ctx(), &forward);
        let r2 = resolve_conflict_cycle(ctx(), &reversed);
        let w1 = result_for(&r1, "AAPL")
            .candidates
            .iter()
            .find(|c| c.selected)
            .unwrap();
        let w2 = result_for(&r2, "AAPL")
            .candidates
            .iter()
            .find(|c| c.selected)
            .unwrap();
        assert_eq!(w1.strategy_id, "aaa");
        assert_eq!(w2.strategy_id, "aaa");
    }

    #[test]
    fn no_target_averaging_netting_or_summing() {
        let candidates = vec![sell(0, "AAPL", "s1", 3, 20), sell(1, "AAPL", "s2", 8, 20)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let winner = result_for(&res, "AAPL")
            .candidates
            .iter()
            .find(|c| c.selected)
            .unwrap();
        // Winner's qty must be exactly one input's qty (8), never 3+8=11 or
        // (3+8)/2=5.5.
        assert_eq!(winner.qty, 8);
    }

    // ── Safety and exactness ──────────────────────────────────────────────

    #[test]
    fn selected_candidate_exactly_equals_one_input_by_ordinal() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            sell(1, "AAPL", "s2", 5, 20),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let selected = res.selected_ordinals();
        assert_eq!(selected, vec![1]);
        let input = &candidates[1];
        let winner = result_for(&res, "AAPL")
            .candidates
            .iter()
            .find(|c| c.ordinal == 1)
            .unwrap();
        assert_eq!(winner.qty, input.qty);
        assert_eq!(winner.side, input.side);
        assert_eq!(winner.strategy_id, input.strategy_id);
    }

    #[test]
    fn no_short_target_ever_selected() {
        let candidates = vec![sell(0, "AAPL", "s1", 30, 20)]; // target -10
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_WOULD_CREATE_SHORT);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn no_oversell_from_multiple_sells_only_one_survives() {
        let candidates = vec![sell(0, "AAPL", "s1", 5, 20), sell(1, "AAPL", "s2", 8, 20)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        let selected_count = aapl.candidates.iter().filter(|c| c.selected).count();
        assert_eq!(selected_count, 1, "only one sell may survive");
    }

    #[test]
    fn checked_add_overflow_is_refused() {
        let candidates = vec![buy(0, "AAPL", "s1", i64::MAX, 1, 1_000, 100_000_000)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_ARITHMETIC_OVERFLOW);
    }

    #[test]
    fn checked_sub_overflow_is_refused() {
        let candidates = vec![sell(0, "AAPL", "s1", i64::MAX, i64::MIN)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_ARITHMETIC_OVERFLOW);
    }

    #[test]
    fn missing_bar_facts_cannot_authorize_increase() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.bar_end_ts = None;
        let res = resolve_conflict_cycle(ctx(), &[c]);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_MISSING_OR_MISMATCHED_BAR_FACTS);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn symbol_mismatch_in_bar_facts_is_refused() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.bar_symbol = Some("MSFT".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn strategy_mismatch_in_bar_facts_is_refused() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.bar_strategy_id = Some("other".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn timeframe_mismatch_in_bar_facts_is_refused() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.bar_timeframe = Some("1h".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn nonpositive_close_cannot_authorize_increase() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.close_micros = Some(0);
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn duplicate_economic_candidate_fails_group_closed() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s1", 10, 0, 1_000, 100_000_000), // identical identity
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(aapl.reason_code, REASON_DUPLICATE_ECONOMIC_CANDIDATE);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn valid_reduction_survives_unrelated_invalid_increasing_candidate() {
        let candidates = vec![
            sell(0, "AAPL", "s1", 5, 20),
            buy(1, "AAPL", "s2", -3, 0, 1_000, 100_000_000), // invalid: qty<=0
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.selected_ordinal, Some(0));
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
    }

    #[test]
    fn no_valid_reduction_and_ambiguity_yields_no_decision() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", 20, 0, 2_000, 50_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.selected_ordinal, None);
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
    }

    #[test]
    fn all_invalid_multi_candidate_group_refused_with_no_valid_candidate() {
        let candidates = vec![
            buy(0, "AAPL", "s1", -1, 0, 1_000, 100_000_000),
            sell(1, "AAPL", "s2", 999, 0), // would create short
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_NO_VALID_CANDIDATE);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn sell_never_requires_bar_facts() {
        let candidates = vec![sell(0, "AAPL", "s1", 5, 20)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
        assert_eq!(aapl.selected_ordinal, Some(0));
    }

    #[test]
    fn invalid_side_string_is_refused() {
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.side = "hold".to_string();
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_INVALID_CANDIDATE_REFUSED
        );
    }

    #[test]
    fn blank_symbol_is_refused() {
        let c = buy(0, "   ", "s1", 10, 0, 1_000, 100_000_000);
        let res = resolve_conflict_cycle(ctx(), &[c]);
        // Groups by trimmed symbol "" -- still resolved, still refused.
        let group = result_for(&res, "");
        assert_eq!(group.reason_code, REASON_INVALID_CANDIDATE_REFUSED);
    }

    #[test]
    fn selected_ordinals_helper_returns_exactly_the_survivors() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            sell(1, "MSFT", "s1", 3, 10),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let mut ordinals = res.selected_ordinals();
        ordinals.sort();
        assert_eq!(ordinals, vec![0, 1]);
    }
}
