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
//!
//! # AUTHORITY-AND-EVIDENCE-REPAIR-01 (frozen by the operating rules for
//! this repair)
//! - A lone structurally valid **buy** never authorizes new exposure when a
//!   sibling candidate in the same symbol group is structurally invalid (or
//!   otherwise prevents exact target consensus) — see
//!   [`REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED`]. A lone structurally
//!   valid **sell/reduction** still survives invalid siblings and retains
//!   safety precedence — refusing a reduction because an unrelated
//!   candidate is malformed would be *less* safe, not more.
//! - Every candidate — buy or sell — must carry exact, matching evaluated
//!   bar provenance (canonical symbol, strategy identity, timeframe,
//!   positive bar end timestamp). Positive close is required only for
//!   new/increasing exposure; a reduction's economics never depend on
//!   price, but its bar identity must still be present and exact.
//! - Symbol grouping, bar-symbol comparison, and evidence all use one
//!   canonical symbol normalization ([`canonical_symbol`]) so `"aapl"` and
//!   `"AAPL"` are always the same economic symbol.

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
/// AUTHORITY-AND-EVIDENCE-REPAIR-01 (Defect 1): the sole structurally valid
/// candidate in a multi-candidate group is a **buy**, but one or more
/// siblings are structurally invalid (or otherwise prevent exact target
/// consensus). Ambiguity that cannot be resolved to a proven target must
/// refuse the whole symbol group rather than authorize new exposure on the
/// strength of one unopposed-but-uncorroborated candidate. Never used for a
/// sole valid sell/reduction — see module docs.
pub const REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED: &str =
    "ambiguous_invalid_competitor_refused";

pub const TRUTH_STATE_COMPUTED: &str = "computed";

/// Frozen policy schema version, carried through into durable evidence.
pub const CONFLICT_POLICY_SCHEMA_VERSION: &str = "multi-strategy-conflict-policy-v1";

// ---------------------------------------------------------------------------
// Canonical symbol normalization
// ---------------------------------------------------------------------------

/// The single canonical equity-symbol normalization used consistently for
/// current-position lookup, grouping, bar-symbol comparison, identity, and
/// evidence throughout this module (and by its `mqk-daemon` caller for the
/// current-position lookup it performs before candidates ever reach here).
/// Trim whitespace, uppercase — mirrors the established repo convention
/// (`PerSymbolPendingBarInputs::normalize_symbol`,
/// `routes::strategy_promotions::normalize_symbol`). `"aapl"` and `"AAPL"`
/// are always the same economic symbol; a caller that reads current
/// quantity for one casing while grouping candidates under the other would
/// silently treat a held position as flat.
pub fn canonical_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

/// Deterministic, pure `timeframe_secs -> canonical timeframe string`
/// conversion. Deliberately mirrors `mqk_artifacts::timeframe_from_secs`'s
/// fixed table exactly (this crate is zero-dependency and cannot import
/// that private helper) so a candidate's own `timeframe_secs` — never a
/// caller-supplied, potentially-stale global string — is the single source
/// of truth a candidate's `bar_timeframe` is checked against.
fn canonical_timeframe_str(timeframe_secs: i64) -> Option<String> {
    let s = match timeframe_secs {
        60 => "1m".to_string(),
        300 => "5m".to_string(),
        900 => "15m".to_string(),
        3_600 => "1h".to_string(),
        86_400 => "1D".to_string(),
        secs if secs > 0 => format!("{secs}s"),
        _ => return None,
    };
    Some(s)
}

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
    pub timeframe_secs: i64,
    /// `"buy"` or `"sell"`, case-insensitive; anything else is structurally
    /// invalid.
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    /// Order type used for economic identity (`"market"` / `"limit"`, etc.)
    /// — never inspected for validity here (that is
    /// `decision.rs::validate_fields`'s job); carried through for exact
    /// economic identity and durable evidence.
    pub order_type: String,
    /// Time-in-force used for economic identity — carried through
    /// unvalidated, same rationale as `order_type`.
    pub time_in_force: String,
    /// Limit price (when present) used for economic identity — carried
    /// through unvalidated, same rationale as `order_type`.
    pub limit_price: Option<i64>,
    /// Exact evaluated-bar identity this decision was derived from.
    /// Required (and must match `symbol`/`strategy_id`/`timeframe_secs`)
    /// for **every** candidate — buy or sell. A reduction's economics never
    /// depend on price, so `close_micros` is not required to be positive
    /// for a sell, but the bar identity itself must still be present and
    /// exact (AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2).
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
    /// The whole symbol group was refused (conflicting increase targets, a
    /// duplicate economic candidate identity within the group, or an
    /// ambiguous invalid competitor blocking proven target consensus).
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
    pub order_type: String,
    pub time_in_force: String,
    pub limit_price: Option<i64>,
    /// `None` only when the candidate's own delta arithmetic overflowed.
    pub proposed_target_qty: Option<i64>,
    pub bar_symbol: Option<String>,
    pub bar_strategy_id: Option<String>,
    pub bar_timeframe: Option<String>,
    pub bar_end_ts: Option<i64>,
    pub close_micros: Option<i64>,
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

    // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: every candidate -- buy or
    // sell -- must carry exact, matching evaluated-bar provenance: the
    // canonical symbol, the exact strategy identity, and the timeframe
    // implied by the candidate's own `timeframe_secs` (never a caller
    // global). Positive close is required only for new/increasing
    // exposure; a reduction's own economics never depend on price.
    let expected_timeframe = canonical_timeframe_str(c.timeframe_secs);
    let bar_ok = match (
        &c.bar_symbol,
        &c.bar_strategy_id,
        &c.bar_timeframe,
        c.bar_end_ts,
        c.close_micros,
    ) {
        (Some(bs), Some(bsid), Some(btf), Some(bts), Some(close)) => {
            canonical_symbol(bs) == canonical_symbol(&c.symbol)
                && bsid.trim() == c.strategy_id.trim()
                && expected_timeframe.as_deref() == Some(btf.trim())
                && bts > 0
                && match side {
                    Side::Buy => close > 0,
                    Side::Sell => true,
                }
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
/// symbol's group. AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 3: two
/// same-strategy/same-bar candidates with different quantities/targets are
/// *conflicting* economic proposals, not identical duplicates -- this
/// identity must include every canonical economic field capable of
/// distinguishing two genuinely different candidates (quantity, current
/// position, order semantics, and full bar provenance), not just
/// `(strategy_id, side, bar_end_ts)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct EconomicIdentity {
    strategy_id: String,
    side: String,
    qty: i64,
    current_qty: i64,
    order_type: String,
    time_in_force: String,
    limit_price: Option<i64>,
    bar_symbol: Option<String>,
    bar_strategy_id: Option<String>,
    bar_timeframe: Option<String>,
    bar_end_ts: Option<i64>,
    close_micros: Option<i64>,
}

fn economic_identity(c: &ConflictCandidateInput) -> EconomicIdentity {
    EconomicIdentity {
        strategy_id: c.strategy_id.trim().to_string(),
        side: c.side.trim().to_ascii_lowercase(),
        qty: c.qty,
        current_qty: c.current_qty,
        order_type: c.order_type.trim().to_ascii_lowercase(),
        time_in_force: c.time_in_force.trim().to_ascii_lowercase(),
        limit_price: c.limit_price,
        bar_symbol: c.bar_symbol.as_deref().map(canonical_symbol),
        bar_strategy_id: c.bar_strategy_id.as_deref().map(|s| s.trim().to_string()),
        bar_timeframe: c.bar_timeframe.as_deref().map(|s| s.trim().to_string()),
        bar_end_ts: c.bar_end_ts,
        close_micros: c.close_micros,
    }
}

// ---------------------------------------------------------------------------
// Candidate-result construction helper
// ---------------------------------------------------------------------------

fn candidate_result(
    c: &ConflictCandidateInput,
    v: &Validated,
    selected: bool,
    disposition: ConflictDisposition,
    reason_code: &str,
) -> ConflictCandidateResult {
    ConflictCandidateResult {
        ordinal: c.ordinal,
        strategy_id: c.strategy_id.clone(),
        timeframe_secs: c.timeframe_secs,
        side: c.side.clone(),
        qty: c.qty,
        current_qty: c.current_qty,
        order_type: c.order_type.clone(),
        time_in_force: c.time_in_force.clone(),
        limit_price: c.limit_price,
        proposed_target_qty: v.proposed_target_qty,
        bar_symbol: c.bar_symbol.clone(),
        bar_strategy_id: c.bar_strategy_id.clone(),
        bar_timeframe: c.bar_timeframe.clone(),
        bar_end_ts: c.bar_end_ts,
        close_micros: c.close_micros,
        selected,
        disposition,
        reason_code: reason_code.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Group (one symbol) resolution
// ---------------------------------------------------------------------------

fn resolve_symbol_group(symbol: &str, group: &[&ConflictCandidateInput]) -> ConflictSymbolResult {
    let validations: Vec<Validated> = group.iter().map(|c| validate_candidate(c)).collect();

    // Duplicate economic candidate identity among structurally valid
    // candidates fails the whole group closed (Q: never silently
    // deduplicate; never pick one arbitrarily).
    let mut seen: std::collections::HashSet<EconomicIdentity> = std::collections::HashSet::new();
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
            .map(|(c, v)| {
                candidate_result(
                    c,
                    v,
                    false,
                    ConflictDisposition::RefusedConflict,
                    REASON_DUPLICATE_ECONOMIC_CANDIDATE,
                )
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
        let candidates = vec![candidate_result(
            c,
            v,
            v.valid,
            disposition,
            reason_code,
        )];
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
        let candidates = group
            .iter()
            .zip(validations.iter())
            .map(|(c, v)| {
                candidate_result(
                    c,
                    v,
                    false,
                    ConflictDisposition::RefusedInvalid,
                    v.reason_code,
                )
            })
            .collect();
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal: None,
            disposition: ConflictDisposition::RefusedInvalid,
            reason_code: REASON_NO_VALID_CANDIDATE.to_string(),
            candidates,
        };
    }

    if valid_indices.len() == 1 {
        let i = valid_indices[0];
        let (winner, winner_v) = (group[i], &validations[i]);
        // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 1: this group has more
        // than one input candidate (group.len() > 1, guaranteed by the
        // group.len() == 1 early return above), but only one of them is
        // structurally valid. A sole valid SELL/reduction still survives
        // its invalid siblings and keeps safety precedence -- refusing a
        // reduction because an unrelated candidate is malformed would be
        // less safe, not more. A sole valid BUY, however, must NOT
        // authorize new exposure while ambiguity (an invalid or otherwise
        // non-corroborating sibling) prevents exact target consensus: the
        // whole symbol group is refused instead.
        let winner_is_sell = matches!(parse_side(&winner.side), Some(Side::Sell));
        if winner_is_sell {
            let candidates = group
                .iter()
                .zip(validations.iter())
                .enumerate()
                .map(|(idx, (c, v))| {
                    let is_winner = idx == i;
                    candidate_result(
                        c,
                        v,
                        is_winner,
                        if is_winner {
                            ConflictDisposition::Passthrough
                        } else {
                            ConflictDisposition::RefusedInvalid
                        },
                        if is_winner {
                            REASON_SINGLE_CANDIDATE_PASSTHROUGH
                        } else {
                            v.reason_code
                        },
                    )
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

        // Sole valid candidate is a BUY with at least one invalid sibling:
        // refuse the whole group closed.
        let _ = winner_v; // proposed_target_qty still recorded per-candidate below
        let candidates = group
            .iter()
            .zip(validations.iter())
            .enumerate()
            .map(|(idx, (c, v))| {
                let is_winner = idx == i;
                candidate_result(
                    c,
                    v,
                    false,
                    if is_winner {
                        ConflictDisposition::RefusedConflict
                    } else {
                        ConflictDisposition::RefusedInvalid
                    },
                    if is_winner {
                        REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED
                    } else {
                        v.reason_code
                    },
                )
            })
            .collect();
        return ConflictSymbolResult {
            symbol: symbol.to_string(),
            selected_ordinal: None,
            disposition: ConflictDisposition::RefusedConflict,
            reason_code: REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED.to_string(),
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
                            REASON_CONFLICTING_INCREASE_TARGETS_REFUSED,
                        )
                    } else {
                        (ConflictDisposition::RefusedInvalid, v.reason_code)
                    };
                    candidate_result(c, v, false, disposition, reason_code)
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
                return candidate_result(c, v, true, group_disposition, group_reason);
            }
            if !v.valid {
                return candidate_result(c, v, false, ConflictDisposition::RefusedInvalid, v.reason_code);
            }
            let is_increasing = parse_side(&c.side).map(|s| matches!(s, Side::Buy)) == Some(true);
            let reason = if selected_via_reduction && is_increasing {
                REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION
            } else {
                REASON_NOT_SELECTED
            };
            candidate_result(c, v, false, ConflictDisposition::NotSelected, reason)
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
/// per [`canonical_symbol`] (trim + uppercase — `"aapl"` and `"AAPL"` are
/// always the same group). Deterministic: identical inputs, in any order,
/// always produce a bit-identical [`ConflictCycleResult`] (symbol groups
/// sorted by symbol ascending in the output; within-group ordering is
/// `candidates` in original input order for audit readability, but the
/// *decision* of which one is selected never depends on that order).
pub fn resolve_conflict_cycle(
    context: ConflictCycleContext,
    candidates: &[ConflictCandidateInput],
) -> ConflictCycleResult {
    let mut groups: BTreeMap<String, Vec<&ConflictCandidateInput>> = BTreeMap::new();
    for c in candidates {
        groups
            .entry(canonical_symbol(&c.symbol))
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
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty,
            current_qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some(symbol.to_string()),
            bar_strategy_id: Some(strategy_id.to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(bar_end_ts),
            close_micros: Some(close_micros),
        }
    }

    /// A sell with exact, matching bar provenance (positive close is not
    /// required for a reduction, but the identity fields still are).
    fn sell(
        ordinal: usize,
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        current_qty: i64,
        bar_end_ts: i64,
    ) -> ConflictCandidateInput {
        ConflictCandidateInput {
            ordinal,
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            side: "sell".to_string(),
            qty,
            current_qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some(symbol.to_string()),
            bar_strategy_id: Some(strategy_id.to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(bar_end_ts),
            close_micros: Some(0),
        }
    }

    /// A sell with no bar facts at all -- structurally invalid after the
    /// Defect 2 repair (previously always valid).
    fn unbound_sell(
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
            timeframe_secs: 300,
            side: "sell".to_string(),
            qty,
            current_qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
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
        let forward = vec![
            sell(0, "AAPL", "s1", 5, 20, 1_000),
            sell(1, "AAPL", "s2", 8, 20, 2_000),
        ];
        let reversed = vec![
            sell(0, "AAPL", "s2", 8, 20, 2_000),
            sell(1, "AAPL", "s1", 5, 20, 1_000),
        ];
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
            sell(1, "AAPL", "s2", 5, 20, 2_000),
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
            sell(0, "AAPL", "s1", 3, 20, 1_000), // target 17
            sell(1, "AAPL", "s2", 8, 20, 2_000), // target 12 (greater reduction)
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.selected_ordinal, Some(1));
        let winner = aapl.candidates.iter().find(|c| c.selected).unwrap();
        assert_eq!(winner.proposed_target_qty, Some(12));
    }

    #[test]
    fn equal_selected_targets_tie_break_is_stable() {
        let forward = vec![
            sell(0, "AAPL", "zzz", 8, 20, 1_000),
            sell(1, "AAPL", "aaa", 8, 20, 1_000),
        ];
        let reversed = vec![
            sell(0, "AAPL", "aaa", 8, 20, 1_000),
            sell(1, "AAPL", "zzz", 8, 20, 1_000),
        ];
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
        let candidates = vec![
            sell(0, "AAPL", "s1", 3, 20, 1_000),
            sell(1, "AAPL", "s2", 8, 20, 2_000),
        ];
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
            sell(1, "AAPL", "s2", 5, 20, 2_000),
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
        let candidates = vec![unbound_sell(0, "AAPL", "s1", 30, 20)]; // target -10, fails before bar check
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_WOULD_CREATE_SHORT);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn no_oversell_from_multiple_sells_only_one_survives() {
        let candidates = vec![
            sell(0, "AAPL", "s1", 5, 20, 1_000),
            sell(1, "AAPL", "s2", 8, 20, 2_000),
        ];
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
        let candidates = vec![unbound_sell(0, "AAPL", "s1", i64::MAX, i64::MIN)];
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
    fn differing_quantities_are_not_treated_as_duplicates() {
        // Same strategy/side/bar, but different qty/target: a genuine
        // conflicting proposal, not a duplicate -- must reach the normal
        // ambiguous-increase-consensus path, never the duplicate-identity
        // short-circuit (AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 3).
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s1", 20, 0, 1_000, 100_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(
            aapl.reason_code,
            REASON_CONFLICTING_INCREASE_TARGETS_REFUSED
        );
        assert_ne!(aapl.reason_code, REASON_DUPLICATE_ECONOMIC_CANDIDATE);
    }

    #[test]
    fn valid_reduction_survives_unrelated_invalid_increasing_candidate() {
        let candidates = vec![
            sell(0, "AAPL", "s1", 5, 20, 1_000),
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
            unbound_sell(1, "AAPL", "s2", 999, 0), // would create short
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_NO_VALID_CANDIDATE);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn sell_with_matching_bar_facts_is_valid_passthrough() {
        let candidates = vec![sell(0, "AAPL", "s1", 5, 20, 1_000)];
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
        // Groups by canonical (trimmed+uppercased) symbol "" -- still
        // resolved, still refused.
        let group = result_for(&res, "");
        assert_eq!(group.reason_code, REASON_INVALID_CANDIDATE_REFUSED);
    }

    #[test]
    fn selected_ordinals_helper_returns_exactly_the_survivors() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            sell(1, "MSFT", "s1", 3, 10, 2_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let mut ordinals = res.selected_ordinals();
        ordinals.sort();
        assert_eq!(ordinals, vec![0, 1]);
    }

    // ── AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 1: ambiguous invalid
    // competitor must refuse a sole valid BUY, never a sole valid SELL ─────

    #[test]
    fn valid_buy_plus_invalid_buy_sibling_is_refused_ambiguous() {
        let candidates = vec![
            buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", -5, 0, 1_000, 100_000_000), // invalid: qty<=0
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(
            aapl.reason_code,
            REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED
        );
        assert_eq!(aapl.selected_ordinal, None);
        assert!(aapl.candidates.iter().all(|c| !c.selected));
        let winner_row = aapl.candidates.iter().find(|c| c.ordinal == 0).unwrap();
        assert_eq!(
            winner_row.reason_code,
            REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED
        );
    }

    #[test]
    fn valid_buy_plus_malformed_side_sibling_is_refused_ambiguous() {
        let mut malformed = buy(1, "AAPL", "s2", 10, 0, 1_000, 100_000_000);
        malformed.side = "hold".to_string();
        let candidates = vec![buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000), malformed];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(
            aapl.reason_code,
            REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED
        );
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn valid_buy_plus_overflow_sibling_is_refused_ambiguous() {
        let overflow = buy(1, "AAPL", "s2", i64::MAX, 1, 1_000, 100_000_000);
        let candidates = vec![buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000), overflow];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedConflict);
        assert_eq!(
            aapl.reason_code,
            REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED
        );
        assert_eq!(aapl.selected_ordinal, None);
        let overflow_row = aapl.candidates.iter().find(|c| c.ordinal == 1).unwrap();
        assert_eq!(overflow_row.reason_code, REASON_ARITHMETIC_OVERFLOW);
    }

    #[test]
    fn valid_reduction_plus_invalid_buy_sibling_still_selected() {
        let candidates = vec![
            sell(0, "AAPL", "s1", 5, 20, 1_000),
            buy(1, "AAPL", "s2", -3, 0, 1_000, 100_000_000), // invalid: qty<=0
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
        assert_eq!(aapl.reason_code, REASON_SINGLE_CANDIDATE_PASSTHROUGH);
        assert_eq!(aapl.selected_ordinal, Some(0));
    }

    // ── AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: bar/timeframe authority
    // for both sides, per-candidate timeframe_secs, canonical symbol ──────

    #[test]
    fn sell_missing_bar_facts_is_refused() {
        let candidates = vec![unbound_sell(0, "AAPL", "s1", 5, 20)];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::RefusedInvalid);
        assert_eq!(aapl.reason_code, REASON_MISSING_OR_MISMATCHED_BAR_FACTS);
        assert_eq!(aapl.selected_ordinal, None);
    }

    #[test]
    fn sell_mismatched_bar_facts_is_refused() {
        let mut c = sell(0, "AAPL", "s1", 5, 20, 1_000);
        c.bar_symbol = Some("MSFT".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn sell_zero_close_does_not_block_a_reduction() {
        // A reduction's economics never depend on price -- zero/absent
        // close must not itself invalidate an otherwise fully-bound sell.
        let mut c = sell(0, "AAPL", "s1", 5, 20, 1_000);
        c.close_micros = Some(0);
        let res = resolve_conflict_cycle(ctx(), &[c]);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
        assert_eq!(aapl.selected_ordinal, Some(0));
    }

    #[test]
    fn per_candidate_timeframe_mismatch_is_refused_even_with_correct_secs() {
        // The candidate's own timeframe_secs (300 == "5m") must govern the
        // bar-timeframe check -- a bar stamped "1h" is wrong regardless of
        // what any other candidate in the same tick was dispatched under.
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.bar_timeframe = Some("1h".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        assert_eq!(
            result_for(&res, "AAPL").reason_code,
            REASON_MISSING_OR_MISMATCHED_BAR_FACTS
        );
    }

    #[test]
    fn per_candidate_timeframe_secs_drives_expected_bar_timeframe_string() {
        // 900s canonicalizes to "15m", not "5m" -- proves the check is
        // driven by *this* candidate's own timeframe_secs, not a fixed
        // constant.
        let mut c = buy(0, "AAPL", "s1", 10, 0, 1_000, 100_000_000);
        c.timeframe_secs = 900;
        c.bar_timeframe = Some("15m".to_string());
        let res = resolve_conflict_cycle(ctx(), &[c]);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Passthrough);
    }

    #[test]
    fn case_insensitive_canonical_symbol_groups_together() {
        let candidates = vec![
            buy(0, "aapl", "s1", 10, 0, 1_000, 100_000_000),
            buy(1, "AAPL", "s2", 10, 0, 1_000, 100_000_000),
        ];
        let res = resolve_conflict_cycle(ctx(), &candidates);
        // Exactly one canonical "AAPL" group -- not two separate groups.
        assert_eq!(res.symbol_results.len(), 1);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, ConflictDisposition::Selected);
        assert_eq!(aapl.reason_code, REASON_TARGET_CONSENSUS_PASSTHROUGH);
    }

    #[test]
    fn canonical_symbol_trims_and_uppercases() {
        assert_eq!(canonical_symbol(" aapl "), "AAPL");
        assert_eq!(canonical_symbol("AAPL"), "AAPL");
    }
}
