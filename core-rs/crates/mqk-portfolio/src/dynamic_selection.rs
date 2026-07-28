//! mqk-portfolio: dynamic strategy-symbol selection (Bundle 7)
//!
//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 — pure, deterministic model that
//! selects the single best currently-authorized strategy for each eligible
//! runtime symbol from already-validated promotion + scanner-review
//! evidence.
//!
//! Pure: no IO, no DB, no clock, no env, no randomness, no new dependency —
//! every identity/timestamp field is caller-minted (mirrors
//! [`crate::cycle::AllocationCycleContext`] /
//! [`crate::conflict_policy::ConflictCycleContext`]). This module never
//! talks to the promotion registry, the review-artifact filesystem, the
//! strategy plugin registry, or the runtime dispatch loop — it only ranks
//! and selects among candidates whose evidence the caller (`mqk-daemon`) has
//! already durably fetched, recomputed, and compared.
//!
//! # Scope (frozen by
//! docs/specs/dynamic_strategy_symbol_selection_01a_current_truth_and_contract.md)
//! - Never invents a score. [`SelectionCandidateEvidence::canonical_score_micros`]
//!   must already be a caller-validated, finite, decimal-exact scaled-integer
//!   conversion of the durable `scanner_score` — this module never parses or
//!   hashes a raw float.
//! - Never ranks by signal size, order quantity, or P&L — the only fields
//!   this module reads for ranking are `canonical_score_micros`,
//!   `scanner_rank`, and `watchlist_assigned`.
//! - `plan_id` is caller-minted (never computed here), exactly like
//!   `cycle_id`/`conflict_policy`'s `cycle_id` — this zero-dependency crate
//!   never reads a clock or mints a UUID. The caller derives it via UUIDv5
//!   from a canonical, length-prefixed serialization of every
//!   result-affecting fact this module's output exposes, so the read side
//!   can recompute and validate it independently.
//! - Exactly one selected candidate per symbol, or none — never more.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Reason codes — bounded, closed vocabulary
// ---------------------------------------------------------------------------

pub const REASON_SELECTED_HIGHEST_SCORE: &str = "selected_highest_score";
pub const REASON_SELECTED_TIE_BREAK_RANK: &str = "selected_tie_break_lower_rank";
pub const REASON_SELECTED_TIE_BREAK_WATCHLIST: &str = "selected_tie_break_watchlist_assignment";
pub const REASON_SELECTED_TIE_BREAK_STRATEGY_ID: &str = "selected_tie_break_strategy_id_ascending";
pub const REASON_NOT_SELECTED_LOWER_SCORE: &str = "not_selected_lower_score";
pub const REASON_NOT_SELECTED_LOST_TIE_BREAK: &str = "not_selected_lost_tie_break";
pub const REASON_REFUSED_BLANK_IDENTITY: &str = "refused_blank_identity_field";
pub const REASON_REFUSED_PROMOTION_QUERY_FAILED: &str = "refused_promotion_query_failed";
pub const REASON_REFUSED_NOT_ACTIVE_PAPER: &str = "refused_not_active_paper";
pub const REASON_REFUSED_NOT_YET_EFFECTIVE: &str = "refused_promotion_not_yet_effective";
pub const REASON_REFUSED_EXPIRED: &str = "refused_promotion_expired";
pub const REASON_REFUSED_EVIDENCE_READ_FAILED: &str = "refused_evidence_read_failed";
pub const REASON_REFUSED_NOT_PAPER_CANDIDATE: &str = "refused_review_state_not_paper_candidate";
pub const REASON_REFUSED_FINGERPRINT_MISMATCH: &str = "refused_evidence_fingerprint_mismatch";
pub const REASON_REFUSED_UNSUPPORTED_STRATEGY: &str = "refused_unsupported_strategy_plugin";
pub const REASON_REFUSED_TIMEFRAME_MISMATCH: &str = "refused_timeframe_mismatch";
pub const REASON_REFUSED_DATA_NOT_READY: &str = "refused_data_not_ready";
pub const REASON_REFUSED_MISSING_SCORE: &str = "refused_missing_or_invalid_score";
pub const REASON_REFUSED_MISSING_RANK_FOR_TIE: &str = "refused_missing_rank_required_for_tie";
pub const REASON_REFUSED_DIVERGENT_DUPLICATE: &str = "refused_divergent_duplicate_evidence";
pub const REASON_NO_VALID_CANDIDATE: &str = "no_valid_candidate_for_symbol";

pub const TRUTH_STATE_COMPUTED: &str = "computed";
pub const TRUTH_STATE_NO_ELIGIBLE_SYMBOLS: &str = "fail_closed_no_eligible_symbols";
pub const TRUTH_STATE_DUPLICATE_ELIGIBLE_SYMBOL: &str = "fail_closed_duplicate_eligible_symbol";
pub const TRUTH_STATE_CANDIDATE_OUTSIDE_ELIGIBLE_SET: &str =
    "fail_closed_candidate_outside_eligible_set";

/// Frozen pure-model schema version, carried through into durable evidence
/// and into the caller-minted `plan_id` derivation recipe.
pub const DYNAMIC_SELECTION_SCHEMA_VERSION: &str = "dynamic-strategy-symbol-selection-v1";

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

/// Closed mode vocabulary for `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE`.
/// An unknown configured value must never parse to any of these — callers
/// treat that as `effective = Off` plus an `invalid_configuration` truth
/// state (a daemon/API-layer concern; this pure type only models the three
/// closed values themselves).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicSelectionMode {
    Off,
    Shadow,
    PaperEnforced,
}

impl DynamicSelectionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::PaperEnforced => "paper_enforced",
        }
    }

    /// Parse the closed vocabulary exactly (trimmed). Returns `None` for any
    /// other value — callers must treat `None` as unknown/invalid
    /// configuration, never default it to a guessed mode.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "off" => Some(Self::Off),
            "shadow" => Some(Self::Shadow),
            "paper_enforced" => Some(Self::PaperEnforced),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical symbol normalization (mirrors conflict_policy::canonical_symbol)
// ---------------------------------------------------------------------------

/// The single canonical equity-symbol normalization used consistently for
/// eligible-symbol dedup, candidate grouping, and selection identity.
/// Mirrors [`crate::conflict_policy::canonical_symbol`] exactly (this crate
/// intentionally does not import across its own modules for a one-line
/// helper — both copies must stay textually identical).
pub fn canonical_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

fn canonical_strategy_id(strategy_id: &str) -> String {
    strategy_id.trim().to_string()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Immutable facts about one selection plan, supplied by the caller. Every
/// identity/timestamp field is caller-minted — this zero-dependency crate
/// never reads a clock or mints a UUID.
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicSelectionContext {
    /// Deterministic plan identity (caller-minted UUIDv5 or equivalent).
    pub plan_id: String,
    pub run_id: String,
    pub schema_version: String,
    pub configured_mode: DynamicSelectionMode,
    pub effective_mode: DynamicSelectionMode,
    /// `true` when `effective_mode` was forced to `Off` by the live-capital
    /// hard lock (any deployment/adapter combination other than
    /// paper+Alpaca) — carried through for durable evidence and read-side
    /// truth, even though this pure module performs no dispatch of its own.
    pub live_lock_applied: bool,
    /// `"watchlist_v2"` or `"env_single_symbol_fallback"`.
    pub source_kind: String,
    /// Watchlist path, or `"env"` for the legacy fallback.
    pub source_identity: String,
    /// `"YYYY-MM-DD"`.
    pub market_date: String,
}

/// Evidence already durably fetched, recomputed, and compared by the caller
/// for one `(symbol, strategy_id)` candidate. This module trusts none of
/// these booleans blindly in the sense of skipping its own gate ordering,
/// but performs no DB/filesystem re-validation of its own — that trust
/// boundary belongs to the caller (mirrors every other pure policy module in
/// this crate).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionCandidateEvidence {
    /// `false` when the durable promotion query itself failed (DB
    /// unavailable, query error) — distinct from a query that succeeded and
    /// found no row.
    pub promotion_query_ok: bool,
    /// The exact current promotion state string (e.g. `"active_paper"`),
    /// `None` when no promotion row exists for this identity.
    pub promotion_state: Option<String>,
    /// `true` when the promotion's `effective_at_utc <= authority_ts`.
    pub promotion_effective: bool,
    /// `true` when the promotion has expired as of the authority timestamp.
    pub promotion_expired: bool,
    /// `true` when [`resolve_evidence_lineage`]-equivalent resolution
    /// (walking back to the evidence-bearing transition) succeeded.
    pub evidence_resolved: bool,
    /// `true` when the resolved evidence transition's matched review row has
    /// `review_state == "paper_candidate"`.
    pub review_state_is_paper_candidate: bool,
    /// `true` when the caller recomputed the review-row fingerprint
    /// (SHA-256 of the canonical serialized row) and it matched the durable
    /// `evidence_fingerprint` column exactly.
    pub fingerprint_matches: bool,
    /// `true` when the strategy is registered/enabled in the strategy
    /// registry and `PluginRegistry::instantiate_verified` succeeded for
    /// this exact symbol.
    pub plugin_instantiable: bool,
    /// `true` when the strategy spec's own `timeframe_secs` equals the
    /// assignment's canonical timeframe.
    pub timeframe_matches: bool,
    /// `true` when daily data readiness/freshness covers this exact
    /// `(symbol, timeframe)` pair.
    pub data_ready: bool,
    /// Canonical, decimal-exact scaled-integer (micros) representation of
    /// the durable `scanner_score`. `None` when the score is missing,
    /// non-finite, or the caller could not produce an unambiguous
    /// conversion — never a fabricated default.
    pub canonical_score_micros: Option<i64>,
    /// `None` when the durable `scanner_rank` is absent.
    pub scanner_rank: Option<u32>,
    /// `true` when this `(symbol, strategy_id)` pair is exactly the
    /// approved watchlist-v2 assignment for `symbol`.
    pub watchlist_assigned: bool,
    /// Evidence identity fields carried through unvalidated for durable
    /// evidence/read-side recomputation — never inspected for ranking.
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_fingerprint: Option<String>,
}

/// One `(symbol, strategy_id)` candidate under selection consideration.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionCandidateInput {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub evidence: SelectionCandidateEvidence,
}

/// Closed disposition vocabulary for one candidate's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionCandidateDisposition {
    /// The one candidate selected for its symbol.
    Selected,
    /// Structurally/evidence-valid but lost to a selected sibling.
    NotSelected,
    /// Refused on its own terms (evidence gate failure, missing score,
    /// unresolved tie-break rank requirement, or divergent duplicate).
    Refused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionCandidateResult {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<u32>,
    pub watchlist_assigned: bool,
    pub evidence_review_id: Option<String>,
    pub evidence_fingerprint: Option<String>,
    pub selected: bool,
    pub disposition: SelectionCandidateDisposition,
    pub reason_code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolSelectionResult {
    pub symbol: String,
    pub selected_strategy_id: Option<String>,
    pub disposition: SelectionCandidateDisposition,
    pub reason_code: String,
    /// One row per input candidate for this symbol, deterministically
    /// sorted by canonical `strategy_id` ascending (never input order).
    pub candidates: Vec<SelectionCandidateResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicSelectionPlan {
    pub context: DynamicSelectionContext,
    /// Sorted by symbol ascending — deterministic regardless of the input
    /// candidate slice's order.
    pub symbol_results: Vec<SymbolSelectionResult>,
    pub truth_state: String,
    pub blockers: Vec<String>,
}

impl DynamicSelectionPlan {
    pub fn eligible_symbol_count(&self) -> usize {
        self.symbol_results.len()
    }

    pub fn candidate_count(&self) -> usize {
        self.symbol_results.iter().map(|s| s.candidates.len()).sum()
    }

    pub fn selected_count(&self) -> usize {
        self.symbol_results
            .iter()
            .filter(|s| s.selected_strategy_id.is_some())
            .count()
    }

    pub fn refused_count(&self) -> usize {
        self.symbol_results
            .iter()
            .flat_map(|s| &s.candidates)
            .filter(|c| c.disposition == SelectionCandidateDisposition::Refused)
            .count()
    }

    /// Every selected `(symbol, strategy_id)` pair, in symbol order — the
    /// exact set the caller should instantiate one host per.
    pub fn selected_pairs(&self) -> Vec<(String, String)> {
        self.symbol_results
            .iter()
            .filter_map(|s| {
                s.selected_strategy_id
                    .as_ref()
                    .map(|id| (s.symbol.clone(), id.clone()))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Per-candidate evidence gate (first failing reason wins)
// ---------------------------------------------------------------------------

struct GateOutcome {
    valid: bool,
    reason_code: &'static str,
}

fn evaluate_evidence_gate(c: &SelectionCandidateInput) -> GateOutcome {
    let invalid = |reason: &'static str| GateOutcome {
        valid: false,
        reason_code: reason,
    };

    if canonical_symbol(&c.symbol).is_empty() || canonical_strategy_id(&c.strategy_id).is_empty() {
        return invalid(REASON_REFUSED_BLANK_IDENTITY);
    }
    if c.timeframe_secs <= 0 {
        return invalid(REASON_REFUSED_BLANK_IDENTITY);
    }
    let e = &c.evidence;
    if !e.promotion_query_ok {
        return invalid(REASON_REFUSED_PROMOTION_QUERY_FAILED);
    }
    if e.promotion_state.as_deref() != Some("active_paper") {
        return invalid(REASON_REFUSED_NOT_ACTIVE_PAPER);
    }
    if !e.promotion_effective {
        return invalid(REASON_REFUSED_NOT_YET_EFFECTIVE);
    }
    if e.promotion_expired {
        return invalid(REASON_REFUSED_EXPIRED);
    }
    if !e.evidence_resolved {
        return invalid(REASON_REFUSED_EVIDENCE_READ_FAILED);
    }
    if !e.review_state_is_paper_candidate {
        return invalid(REASON_REFUSED_NOT_PAPER_CANDIDATE);
    }
    if !e.fingerprint_matches {
        return invalid(REASON_REFUSED_FINGERPRINT_MISMATCH);
    }
    if !e.plugin_instantiable {
        return invalid(REASON_REFUSED_UNSUPPORTED_STRATEGY);
    }
    if !e.timeframe_matches {
        return invalid(REASON_REFUSED_TIMEFRAME_MISMATCH);
    }
    if !e.data_ready {
        return invalid(REASON_REFUSED_DATA_NOT_READY);
    }
    match e.canonical_score_micros {
        // The caller is responsible for finiteness (this type is already an
        // integer, not a float) and for scaling; this gate only requires a
        // score to be present at all -- "must be finite and present" per the
        // ranking contract, nothing more.
        Some(_) => GateOutcome {
            valid: true,
            reason_code: TRUTH_STATE_COMPUTED,
        },
        None => invalid(REASON_REFUSED_MISSING_SCORE),
    }
}

fn candidate_result(
    c: &SelectionCandidateInput,
    selected: bool,
    disposition: SelectionCandidateDisposition,
    reason_code: &str,
) -> SelectionCandidateResult {
    SelectionCandidateResult {
        symbol: canonical_symbol(&c.symbol),
        strategy_id: canonical_strategy_id(&c.strategy_id),
        timeframe_secs: c.timeframe_secs,
        canonical_score_micros: c.evidence.canonical_score_micros,
        scanner_rank: c.evidence.scanner_rank,
        watchlist_assigned: c.evidence.watchlist_assigned,
        evidence_review_id: c.evidence.evidence_review_id.clone(),
        evidence_fingerprint: c.evidence.evidence_fingerprint.clone(),
        selected,
        disposition,
        reason_code: reason_code.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Group (one symbol) resolution
// ---------------------------------------------------------------------------

fn resolve_symbol_group(symbol: &str, group: &[&SelectionCandidateInput]) -> SymbolSelectionResult {
    // Identity de-duplication: two rows sharing the same canonical
    // strategy_id are either an idempotent exact replay (kept once) or a
    // divergent duplicate (both refused) — never silently pick one.
    let mut by_strategy: BTreeMap<String, Vec<&SelectionCandidateInput>> = BTreeMap::new();
    for c in group {
        by_strategy
            .entry(canonical_strategy_id(&c.strategy_id))
            .or_default()
            .push(c);
    }

    let mut results: Vec<SelectionCandidateResult> = Vec::new();
    let mut ranking_pool: Vec<&SelectionCandidateInput> = Vec::new();

    for rows in by_strategy.values() {
        if rows.len() > 1 {
            let first_evidence = &rows[0].evidence;
            let all_identical = rows.iter().all(|r| &r.evidence == first_evidence);
            if !all_identical {
                for r in rows {
                    results.push(candidate_result(
                        r,
                        false,
                        SelectionCandidateDisposition::Refused,
                        REASON_REFUSED_DIVERGENT_DUPLICATE,
                    ));
                }
                continue;
            }
        }
        // Idempotent replay or single row: evaluate exactly once.
        let representative = rows[0];
        let gate = evaluate_evidence_gate(representative);
        if !gate.valid {
            results.push(candidate_result(
                representative,
                false,
                SelectionCandidateDisposition::Refused,
                gate.reason_code,
            ));
        } else {
            ranking_pool.push(representative);
        }
    }

    if ranking_pool.is_empty() {
        results.sort_by(|a, b| a.strategy_id.cmp(&b.strategy_id));
        return SymbolSelectionResult {
            symbol: symbol.to_string(),
            selected_strategy_id: None,
            disposition: SelectionCandidateDisposition::Refused,
            reason_code: REASON_NO_VALID_CANDIDATE.to_string(),
            candidates: results,
        };
    }

    // Step 1: highest canonical_score_micros wins outright.
    let max_score = ranking_pool
        .iter()
        .map(|c| c.evidence.canonical_score_micros.expect("gated Some"))
        .max()
        .expect("ranking_pool non-empty");
    let mut leaders: Vec<&SelectionCandidateInput> = ranking_pool
        .iter()
        .copied()
        .filter(|c| c.evidence.canonical_score_micros == Some(max_score))
        .collect();

    let mut selected: Option<&SelectionCandidateInput> = None;
    let mut selected_reason = REASON_SELECTED_HIGHEST_SCORE;

    if leaders.len() == 1 {
        selected = Some(leaders[0]);
    } else {
        // Step 2: lower positive scanner_rank wins. Any leader missing a
        // positive rank cannot participate in — or be resolved by — this
        // tie and is refused rather than silently defaulting past it.
        let (rank_ok, rank_missing): (Vec<_>, Vec<_>) = leaders
            .drain(..)
            .partition(|c| matches!(c.evidence.scanner_rank, Some(r) if r > 0));
        for c in &rank_missing {
            results.push(candidate_result(
                c,
                false,
                SelectionCandidateDisposition::Refused,
                REASON_REFUSED_MISSING_RANK_FOR_TIE,
            ));
        }
        leaders = rank_ok;

        if leaders.len() == 1 {
            selected = Some(leaders[0]);
            selected_reason = REASON_SELECTED_TIE_BREAK_RANK;
        } else if leaders.len() > 1 {
            let min_rank = leaders
                .iter()
                .map(|c| c.evidence.scanner_rank.expect("rank_ok"))
                .min()
                .expect("leaders non-empty");
            let mut rank_leaders: Vec<&SelectionCandidateInput> = leaders
                .iter()
                .copied()
                .filter(|c| c.evidence.scanner_rank == Some(min_rank))
                .collect();
            for c in &leaders {
                if c.evidence.scanner_rank != Some(min_rank) {
                    results.push(candidate_result(
                        c,
                        false,
                        SelectionCandidateDisposition::NotSelected,
                        REASON_NOT_SELECTED_LOST_TIE_BREAK,
                    ));
                }
            }

            if rank_leaders.len() == 1 {
                selected = Some(rank_leaders[0]);
                selected_reason = REASON_SELECTED_TIE_BREAK_RANK;
            } else {
                // Step 3: watchlist-preferred assignment, only if exactly
                // one tied candidate carries it.
                let watchlist_leaders: Vec<&&SelectionCandidateInput> = rank_leaders
                    .iter()
                    .filter(|c| c.evidence.watchlist_assigned)
                    .collect();
                if watchlist_leaders.len() == 1 {
                    let winner = *watchlist_leaders[0];
                    for c in &rank_leaders {
                        if !std::ptr::eq(*c, winner) {
                            results.push(candidate_result(
                                c,
                                false,
                                SelectionCandidateDisposition::NotSelected,
                                REASON_NOT_SELECTED_LOST_TIE_BREAK,
                            ));
                        }
                    }
                    selected = Some(winner);
                    selected_reason = REASON_SELECTED_TIE_BREAK_WATCHLIST;
                } else {
                    // Step 4: final deterministic tie-break, canonical
                    // strategy_id ascending — always resolves since every
                    // remaining candidate here has a distinct strategy_id.
                    rank_leaders.sort_by_key(|c| canonical_strategy_id(&c.strategy_id));
                    let winner = rank_leaders[0];
                    for c in &rank_leaders[1..] {
                        results.push(candidate_result(
                            c,
                            false,
                            SelectionCandidateDisposition::NotSelected,
                            REASON_NOT_SELECTED_LOST_TIE_BREAK,
                        ));
                    }
                    selected = Some(winner);
                    selected_reason = REASON_SELECTED_TIE_BREAK_STRATEGY_ID;
                }
            }
        }
    }

    // Every valid, non-max-score candidate lost outright on score.
    for c in &ranking_pool {
        if c.evidence.canonical_score_micros != Some(max_score) {
            results.push(candidate_result(
                c,
                false,
                SelectionCandidateDisposition::NotSelected,
                REASON_NOT_SELECTED_LOWER_SCORE,
            ));
        }
    }

    let (selected_strategy_id, disposition, reason_code) = match selected {
        Some(winner) => {
            results.push(candidate_result(
                winner,
                true,
                SelectionCandidateDisposition::Selected,
                selected_reason,
            ));
            (
                Some(canonical_strategy_id(&winner.strategy_id)),
                SelectionCandidateDisposition::Selected,
                selected_reason.to_string(),
            )
        }
        None => (
            None,
            SelectionCandidateDisposition::Refused,
            REASON_NO_VALID_CANDIDATE.to_string(),
        ),
    };

    results.sort_by(|a, b| a.strategy_id.cmp(&b.strategy_id));

    SymbolSelectionResult {
        symbol: symbol.to_string(),
        selected_strategy_id,
        disposition,
        reason_code,
        candidates: results,
    }
}

// ---------------------------------------------------------------------------
// Plan (whole eligible-symbol universe) resolution
// ---------------------------------------------------------------------------

fn fail_closed_plan(
    context: DynamicSelectionContext,
    truth_state: &str,
    blocker: String,
) -> DynamicSelectionPlan {
    DynamicSelectionPlan {
        context,
        symbol_results: Vec::new(),
        truth_state: truth_state.to_string(),
        blockers: vec![blocker],
    }
}

/// Compute one dynamic selection plan: for every eligible symbol, select the
/// single best `active_paper`, fingerprint-validated candidate strategy —
/// or none, visibly.
///
/// `eligible_symbols` is the complete, caller-built runtime symbol universe
/// (from `MultiSymbolRuntimeConfig`) — every entry gets exactly one
/// [`SymbolSelectionResult`] in the output, even when zero candidates
/// were supplied for it (no silent omission). `candidates` may be supplied
/// in any order and for symbols/strategies in any order — the result is
/// byte/equality-identical regardless (candidates sorted by symbol then
/// strategy_id in the output).
///
/// Fails the whole plan closed (empty `symbol_results`, non-`"computed"`
/// `truth_state`) only for a caller-contract violation: an empty eligible
/// set, a duplicate eligible symbol after canonicalization, or a candidate
/// whose symbol is outside the eligible set. Per-symbol "no valid
/// candidate" is never a whole-plan failure — it is a normal, visible,
/// fail-closed outcome for that one symbol.
pub fn compute_dynamic_selection_plan(
    context: DynamicSelectionContext,
    eligible_symbols: &[String],
    candidates: &[SelectionCandidateInput],
) -> DynamicSelectionPlan {
    if eligible_symbols.is_empty() {
        return fail_closed_plan(
            context,
            TRUTH_STATE_NO_ELIGIBLE_SYMBOLS,
            "eligible_symbols must not be empty".to_string(),
        );
    }

    let mut canonical_eligible: Vec<String> = Vec::with_capacity(eligible_symbols.len());
    {
        let mut seen = std::collections::HashSet::new();
        for s in eligible_symbols {
            let canon = canonical_symbol(s);
            if !seen.insert(canon.clone()) {
                return fail_closed_plan(
                    context,
                    TRUTH_STATE_DUPLICATE_ELIGIBLE_SYMBOL,
                    format!("symbol '{canon}' appears more than once in eligible_symbols"),
                );
            }
            canonical_eligible.push(canon);
        }
    }
    let eligible_set: std::collections::HashSet<&str> =
        canonical_eligible.iter().map(String::as_str).collect();

    for c in candidates {
        let canon = canonical_symbol(&c.symbol);
        if !eligible_set.contains(canon.as_str()) {
            return fail_closed_plan(
                context,
                TRUTH_STATE_CANDIDATE_OUTSIDE_ELIGIBLE_SET,
                format!("candidate symbol '{canon}' is outside the eligible symbol set"),
            );
        }
    }

    let mut groups: BTreeMap<String, Vec<&SelectionCandidateInput>> = BTreeMap::new();
    for symbol in &canonical_eligible {
        groups.entry(symbol.clone()).or_default();
    }
    for c in candidates {
        groups
            .entry(canonical_symbol(&c.symbol))
            .or_default()
            .push(c);
    }

    let symbol_results: Vec<SymbolSelectionResult> = groups
        .into_iter()
        .map(|(symbol, group)| resolve_symbol_group(&symbol, &group))
        .collect();

    DynamicSelectionPlan {
        context,
        symbol_results,
        truth_state: TRUTH_STATE_COMPUTED.to_string(),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DynamicSelectionContext {
        DynamicSelectionContext {
            plan_id: "plan-1".to_string(),
            run_id: "run-1".to_string(),
            schema_version: DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "env_single_symbol_fallback".to_string(),
            source_identity: "env".to_string(),
            market_date: "2026-07-28".to_string(),
        }
    }

    fn valid_evidence(
        score_micros: i64,
        rank: Option<u32>,
        watchlist: bool,
    ) -> SelectionCandidateEvidence {
        SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            fingerprint_matches: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_micros: Some(score_micros),
            scanner_rank: rank,
            watchlist_assigned: watchlist,
            evidence_review_id: Some("review-1".to_string()),
            evidence_scanner_scan_id: Some("scan-1".to_string()),
            evidence_artifact_path: Some("/artifacts/review-1".to_string()),
            evidence_fingerprint: Some("fp-1".to_string()),
        }
    }

    fn candidate(
        symbol: &str,
        strategy_id: &str,
        score_micros: i64,
        rank: Option<u32>,
        watchlist: bool,
    ) -> SelectionCandidateInput {
        SelectionCandidateInput {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            evidence: valid_evidence(score_micros, rank, watchlist),
        }
    }

    fn symbols(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn result_for<'a>(plan: &'a DynamicSelectionPlan, symbol: &str) -> &'a SymbolSelectionResult {
        plan.symbol_results
            .iter()
            .find(|s| s.symbol == symbol)
            .unwrap_or_else(|| panic!("no result for {symbol}"))
    }

    // ── Basic behavior ────────────────────────────────────────────────────

    #[test]
    fn empty_eligible_symbols_fails_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &[], &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_NO_ELIGIBLE_SYMBOLS);
        assert!(plan.symbol_results.is_empty());
    }

    #[test]
    fn duplicate_eligible_symbol_fails_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "aapl"]), &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_DUPLICATE_ELIGIBLE_SYMBOL);
    }

    #[test]
    fn candidate_outside_eligible_set_fails_closed() {
        let candidates = vec![candidate("MSFT", "swing_momentum", 500_000, Some(1), false)];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        assert_eq!(plan.truth_state, TRUTH_STATE_CANDIDATE_OUTSIDE_ELIGIBLE_SET);
    }

    #[test]
    fn symbol_with_no_candidates_is_visible_not_silently_omitted() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &[]);
        assert_eq!(
            plan.symbol_results.len(),
            2,
            "every eligible symbol appears"
        );
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.selected_strategy_id, None);
        assert_eq!(aapl.reason_code, REASON_NO_VALID_CANDIDATE);
    }

    #[test]
    fn single_valid_candidate_is_selected() {
        let candidates = vec![candidate("AAPL", "swing_momentum", 500_000, Some(1), false)];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.selected_strategy_id,
            Some("swing_momentum".to_string())
        );
        assert_eq!(aapl.reason_code, REASON_SELECTED_HIGHEST_SCORE);
        assert_eq!(plan.selected_count(), 1);
    }

    // ── Ranking contract ───────────────────────────────────────────────────

    #[test]
    fn higher_score_wins() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, Some(1), false),
            candidate("AAPL", "mean_reversion", 900_000, Some(2), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.selected_strategy_id,
            Some("mean_reversion".to_string())
        );
        assert_eq!(aapl.reason_code, REASON_SELECTED_HIGHEST_SCORE);
    }

    #[test]
    fn equal_score_lower_positive_rank_wins() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, Some(3), false),
            candidate("AAPL", "mean_reversion", 500_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.selected_strategy_id,
            Some("mean_reversion".to_string())
        );
        assert_eq!(aapl.reason_code, REASON_SELECTED_TIE_BREAK_RANK);
    }

    #[test]
    fn exact_tie_uses_watchlist_then_strategy_id() {
        // Same score, same rank -> watchlist-assigned candidate wins.
        let candidates = vec![
            candidate("AAPL", "zzz_strategy", 500_000, Some(1), false),
            candidate("AAPL", "aaa_strategy", 500_000, Some(1), true),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.selected_strategy_id, Some("aaa_strategy".to_string()));
        assert_eq!(aapl.reason_code, REASON_SELECTED_TIE_BREAK_WATCHLIST);

        // Same score, same rank, neither/both watchlist -> strategy_id ascending.
        let candidates2 = vec![
            candidate("AAPL", "zzz_strategy", 500_000, Some(1), false),
            candidate("AAPL", "aaa_strategy", 500_000, Some(1), false),
        ];
        let plan2 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates2);
        let aapl2 = result_for(&plan2, "AAPL");
        assert_eq!(aapl2.selected_strategy_id, Some("aaa_strategy".to_string()));
        assert_eq!(aapl2.reason_code, REASON_SELECTED_TIE_BREAK_STRATEGY_ID);
    }

    #[test]
    fn missing_rank_required_for_tie_is_refused() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, None, false),
            candidate("AAPL", "mean_reversion", 500_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.selected_strategy_id,
            Some("mean_reversion".to_string())
        );
        let refused = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "swing_momentum")
            .unwrap();
        assert_eq!(refused.reason_code, REASON_REFUSED_MISSING_RANK_FOR_TIE);
        assert_eq!(refused.disposition, SelectionCandidateDisposition::Refused);
    }

    #[test]
    fn all_tied_leaders_missing_rank_yields_no_valid_candidate() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, None, false),
            candidate("AAPL", "mean_reversion", 500_000, None, false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.selected_strategy_id, None);
        assert_eq!(aapl.reason_code, REASON_NO_VALID_CANDIDATE);
    }

    #[test]
    fn never_ranks_by_qty_or_pnl_only_score_rank_watchlist_strategy_id() {
        // Two candidates carry identical evidence except score -- this test
        // only exercises fields this module actually reads; there is no qty
        // or P&L field on SelectionCandidateEvidence at all, so ranking by
        // those is structurally impossible.
        let candidates = vec![
            candidate("AAPL", "low", 100_000, Some(1), false),
            candidate("AAPL", "high", 200_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        assert_eq!(
            result_for(&plan, "AAPL").selected_strategy_id,
            Some("high".to_string())
        );
    }

    // ── Evidence gates ─────────────────────────────────────────────────────

    #[test]
    fn not_active_paper_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_state = Some("paper_approved".to_string());
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.selected_strategy_id, None);
        assert_eq!(
            aapl.candidates[0].reason_code,
            REASON_REFUSED_NOT_ACTIVE_PAPER
        );
    }

    #[test]
    fn missing_promotion_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_state = None;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_NOT_ACTIVE_PAPER
        );
    }

    #[test]
    fn expired_promotion_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_expired = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_EXPIRED
        );
    }

    #[test]
    fn not_yet_effective_promotion_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_effective = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_NOT_YET_EFFECTIVE
        );
    }

    #[test]
    fn paper_candidate_without_active_paper_alone_never_authorizes() {
        // review_state is paper_candidate but promotion state is not
        // active_paper -- must still refuse (promotion gate runs first).
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_state = Some("shadow_approved".to_string());
        assert!(c.evidence.review_state_is_paper_candidate);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_NOT_ACTIVE_PAPER
        );
    }

    #[test]
    fn fingerprint_mismatch_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.fingerprint_matches = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_FINGERPRINT_MISMATCH
        );
    }

    #[test]
    fn unsupported_plugin_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.plugin_instantiable = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_UNSUPPORTED_STRATEGY
        );
    }

    #[test]
    fn timeframe_mismatch_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.timeframe_matches = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_TIMEFRAME_MISMATCH
        );
    }

    #[test]
    fn missing_score_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.canonical_score_micros = None;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_MISSING_SCORE
        );
    }

    #[test]
    fn promotion_query_failure_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_query_ok = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_PROMOTION_QUERY_FAILED
        );
    }

    #[test]
    fn evidence_read_failure_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.evidence_resolved = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_EVIDENCE_READ_FAILED
        );
    }

    #[test]
    fn review_state_not_paper_candidate_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.review_state_is_paper_candidate = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_NOT_PAPER_CANDIDATE
        );
    }

    #[test]
    fn data_not_ready_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.data_ready = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_DATA_NOT_READY
        );
    }

    // ── Duplicate identity ─────────────────────────────────────────────────

    #[test]
    fn identical_duplicate_is_idempotent() {
        let c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let c2 = c1.clone();
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.candidates.len(),
            1,
            "exact replay collapses to one row"
        );
        assert_eq!(
            aapl.selected_strategy_id,
            Some("swing_momentum".to_string())
        );
    }

    #[test]
    fn divergent_duplicate_identity_is_refused() {
        let c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let c2 = candidate("AAPL", "swing_momentum", 900_000, Some(1), false); // same identity, different score
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.selected_strategy_id, None);
        assert_eq!(aapl.reason_code, REASON_NO_VALID_CANDIDATE);
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.reason_code == REASON_REFUSED_DIVERGENT_DUPLICATE));
    }

    // ── Determinism / input-order independence ─────────────────────────────

    #[test]
    fn input_order_does_not_change_plan() {
        let forward = vec![
            candidate("AAPL", "a_strategy", 500_000, Some(1), false),
            candidate("MSFT", "b_strategy", 300_000, Some(1), false),
            candidate("AAPL", "c_strategy", 900_000, Some(2), false),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        let symbols_list = symbols(&["AAPL", "MSFT"]);
        let r1 = compute_dynamic_selection_plan(ctx(), &symbols_list, &forward);
        let r2 = compute_dynamic_selection_plan(ctx(), &symbols_list, &reversed);
        assert_eq!(r1, r2, "candidate input order must never change the plan");
    }

    #[test]
    fn eligible_symbol_input_order_does_not_change_plan() {
        let candidates = vec![
            candidate("AAPL", "a_strategy", 500_000, Some(1), false),
            candidate("MSFT", "b_strategy", 300_000, Some(1), false),
        ];
        let r1 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &candidates);
        let r2 = compute_dynamic_selection_plan(ctx(), &symbols(&["MSFT", "AAPL"]), &candidates);
        assert_eq!(r1, r2);
    }

    // ── Multi-symbol independence ───────────────────────────────────────────

    #[test]
    fn two_symbols_select_independently_and_can_pick_different_strategies() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, Some(1), false),
            candidate("MSFT", "mean_reversion", 700_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &candidates);
        assert_eq!(
            result_for(&plan, "AAPL").selected_strategy_id,
            Some("swing_momentum".to_string())
        );
        assert_eq!(
            result_for(&plan, "MSFT").selected_strategy_id,
            Some("mean_reversion".to_string())
        );
    }

    #[test]
    fn one_symbol_no_valid_candidate_does_not_suppress_other_symbol() {
        let mut bad = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        bad.evidence.promotion_state = None;
        let candidates = vec![
            bad,
            candidate("MSFT", "mean_reversion", 700_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &candidates);
        assert_eq!(result_for(&plan, "AAPL").selected_strategy_id, None);
        assert_eq!(
            result_for(&plan, "MSFT").selected_strategy_id,
            Some("mean_reversion".to_string())
        );
    }

    // ── Plan helper methods ─────────────────────────────────────────────────

    #[test]
    fn plan_counts_are_coherent() {
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 500_000, Some(1), false),
            candidate("AAPL", "mean_reversion", 300_000, Some(2), false),
            candidate("MSFT", "swing_momentum", 700_000, Some(1), false),
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &candidates);
        assert_eq!(plan.eligible_symbol_count(), 2);
        assert_eq!(plan.candidate_count(), 3);
        assert_eq!(plan.selected_count(), 2);
        assert_eq!(plan.refused_count(), 0);
        let mut pairs = plan.selected_pairs();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("AAPL".to_string(), "swing_momentum".to_string()),
                ("MSFT".to_string(), "swing_momentum".to_string()),
            ]
        );
    }

    #[test]
    fn mode_parse_is_closed_vocabulary() {
        assert_eq!(
            DynamicSelectionMode::parse("off"),
            Some(DynamicSelectionMode::Off)
        );
        assert_eq!(
            DynamicSelectionMode::parse("shadow"),
            Some(DynamicSelectionMode::Shadow)
        );
        assert_eq!(
            DynamicSelectionMode::parse("paper_enforced"),
            Some(DynamicSelectionMode::PaperEnforced)
        );
        assert_eq!(DynamicSelectionMode::parse("live"), None);
        assert_eq!(DynamicSelectionMode::parse(""), None);
        assert_eq!(DynamicSelectionMode::parse("OFF"), None);
    }

    #[test]
    fn canonical_symbol_trims_and_uppercases() {
        assert_eq!(canonical_symbol(" aapl "), "AAPL");
        assert_eq!(canonical_symbol("AAPL"), "AAPL");
    }
}
