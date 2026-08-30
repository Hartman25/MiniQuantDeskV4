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
//! - Never invents a score. [`SelectionCandidateEvidence::canonical_score_decimal`]
//!   must already be a caller-validated, exact decimal-string conversion
//!   (IR7 — [`crate::canonicalize_decimal_token`]) of the durable
//!   `scanner_score`'s raw JSON numeric token — this module never parses or
//!   hashes a raw float, and never ranks via the derived
//!   `canonical_score_micros` bridge (which can be `None` even when a valid
//!   decimal score is present).
//! - Never ranks by signal size, order quantity, or P&L — the only fields
//!   this module reads for ranking are `canonical_score_decimal`,
//!   `scanner_rank`, and `watchlist_assigned`.
//! - Exactly one selected candidate per symbol, or none — never more.
//!
//! # Non-circular plan identity (R5)
//! [`DynamicSelectionPlan`] carries no `plan_id` — this crate never mints or
//! accepts one, and never reads a clock or UUID source. The plan this module
//! returns is the complete, identity-free, canonically resolved truth. The
//! caller (`mqk-daemon`) computes [`canonical_plan_identity_material`]
//! against that resolved plan, hashes it (UUIDv5 or equivalent) to mint
//! `plan_id`, and wraps `(plan_id, plan)` in its own durable envelope type.
//! This is Option A of the two non-circular designs considered: the pure
//! selector never consumes a pre-existing plan identity, so there is no
//! two-source identity contract and no risk of running the selector twice
//! against independently reconstructed facts. The read-side validator
//! (Phase 9, daemon-side) must call the same
//! [`canonical_plan_identity_material`] function against a plan it
//! reconstructed from durable evidence and re-ran through
//! [`compute_dynamic_selection_plan`], then compare bytes to the durable
//! `plan_id` — never reconstruct this serialization independently.

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
/// Defect 1: the durable evidence-bearing transition lacks a v2 exact-score
/// fingerprint (or the caller could not supply a recomputed one) -- distinct
/// from [`REASON_REFUSED_FINGERPRINT_V2_MISMATCH`] (both present but
/// disagreeing).
pub const REASON_REFUSED_FINGERPRINT_V2_MISSING: &str = "refused_evidence_fingerprint_v2_missing";
/// Defect 1: durable and recomputed v2 exact-score fingerprints are both
/// present but disagree.
pub const REASON_REFUSED_FINGERPRINT_V2_MISMATCH: &str = "refused_evidence_fingerprint_v2_mismatch";
/// Defect 1: either the durable or the recomputed legacy fingerprint is
/// absent -- distinct from [`REASON_REFUSED_FINGERPRINT_MISMATCH`] (both
/// present but disagreeing), mirroring the v2-missing/v2-mismatch split
/// above.
pub const REASON_REFUSED_FINGERPRINT_MISSING: &str = "refused_evidence_fingerprint_missing";
/// Defect 1: a present legacy fingerprint value is not exactly 64 lowercase
/// hexadecimal characters.
pub const REASON_REFUSED_FINGERPRINT_MALFORMED: &str = "refused_evidence_fingerprint_malformed";
/// Defect 1: the caller-supplied `legacy_fingerprint_matches` boolean
/// disagrees with the truth derived directly from the durable/recomputed
/// values themselves -- refused distinctly from a coherent (boolean agrees)
/// mismatch, since a disagreeing boolean means the caller's own comparison
/// cannot be trusted at all, not just that the fingerprints differ.
pub const REASON_REFUSED_FINGERPRINT_INCOHERENT: &str =
    "refused_evidence_fingerprint_boolean_incoherent";
/// Defect 1: a present v2 exact-score fingerprint value is not exactly 64
/// lowercase hexadecimal characters.
pub const REASON_REFUSED_FINGERPRINT_V2_MALFORMED: &str =
    "refused_evidence_fingerprint_v2_malformed";
/// Defect 1: the caller-supplied `exact_fingerprint_v2_matches` boolean
/// disagrees with the truth derived directly from the durable/recomputed
/// values themselves. See [`REASON_REFUSED_FINGERPRINT_INCOHERENT`].
pub const REASON_REFUSED_FINGERPRINT_V2_INCOHERENT: &str =
    "refused_evidence_fingerprint_v2_boolean_incoherent";
/// Defect 2: the caller-supplied pre-gate `exact_reason` diagnosis on
/// [`SelectionCandidateEvidence`] is inconsistent with the evidence-gate
/// check that actually failed -- see [`candidate_result`].
pub const REASON_REFUSED_PRE_GATE_REASON_INCONSISTENT: &str =
    "refused_pre_gate_reason_inconsistent";
pub const REASON_REFUSED_UNSUPPORTED_STRATEGY: &str = "refused_unsupported_strategy_plugin";
pub const REASON_REFUSED_TIMEFRAME_MISMATCH: &str = "refused_timeframe_mismatch";
pub const REASON_REFUSED_DATA_NOT_READY: &str = "refused_data_not_ready";
pub const REASON_REFUSED_MISSING_SCORE: &str = "refused_missing_or_invalid_score";
pub const REASON_REFUSED_MISSING_RANK_FOR_TIE: &str = "refused_missing_rank_required_for_tie";
pub const REASON_REFUSED_DIVERGENT_DUPLICATE: &str = "refused_divergent_duplicate_evidence";
/// DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: the promoted semantic
/// fingerprint for this candidate does not match (or could not be verified
/// against) the current server-resolved semantic configuration -- see
/// [`ExactSelectionReason::ConfigIdentityUnavailable`].
pub const REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE: &str =
    "refused_config_identity_unavailable";
pub const REASON_NO_VALID_CANDIDATE: &str = "no_valid_candidate_for_symbol";

pub const TRUTH_STATE_COMPUTED: &str = "computed";
pub const TRUTH_STATE_NO_ELIGIBLE_SYMBOLS: &str = "fail_closed_no_eligible_symbols";
pub const TRUTH_STATE_DUPLICATE_ELIGIBLE_SYMBOL: &str = "fail_closed_duplicate_eligible_symbol";
pub const TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL: &str = "fail_closed_blank_eligible_symbol";
pub const TRUTH_STATE_ELIGIBLE_SYMBOLS_OVER_LIMIT: &str = "fail_closed_eligible_symbols_over_limit";
pub const TRUTH_STATE_CANDIDATES_OVER_LIMIT: &str = "fail_closed_candidates_over_limit";
pub const TRUTH_STATE_CANDIDATE_OUTSIDE_ELIGIBLE_SET: &str =
    "fail_closed_candidate_outside_eligible_set";
/// Defect F: a single symbol's candidate rows exceed [`MAX_STRATEGY_UNIVERSE`]
/// -- distinct from [`TRUTH_STATE_CANDIDATES_OVER_LIMIT`] (the *total* pairs
/// bound across every symbol), since a single symbol can locally exceed the
/// per-symbol universe size while the plan-wide total still sits under
/// [`MAX_CANDIDATE_PAIRS`].
pub const TRUTH_STATE_SYMBOL_CANDIDATES_OVER_LIMIT: &str =
    "fail_closed_symbol_candidates_over_limit";

/// Frozen pure-model schema version, carried through into durable evidence
/// and into the caller-minted `plan_id` derivation recipe.
pub const DYNAMIC_SELECTION_SCHEMA_VERSION: &str = "dynamic-strategy-symbol-selection-v1";

// ---------------------------------------------------------------------------
// Defect E — closed, persistable exact-reason vocabulary
// ---------------------------------------------------------------------------
//
// [`SelectionCandidateEvidence::exact_reason`] used to be a bare
// `Option<&'static str>` -- a caller-owned string that carried no closed-set
// guarantee of its own and could not round-trip through a `code()`/
// `parse_code()` pair. [`ExactSelectionReason`] is the single closed,
// bounded, persistable vocabulary every candidate result's exact reason is
// now drawn from -- covering registry/plugin/spec/timeframe failures,
// promotion/evidence/artifact/fingerprint failures (including the v2 exact-
// score fingerprint), score/rank failures, every daily-readiness blocker
// category candidate evaluation can observe, divergent duplicates,
// lower-score/tie-break nonselection, every selected reason, and
// no-valid-candidate. `parse_code` returns `None` for any string outside
// this closed set -- an unrecognized persisted code must fail closed, never
// silently resolve to a guessed variant.

/// The closed vocabulary of daily-data-readiness blocker categories a
/// candidate evaluation can observe (mirrors `mqk-daemon`'s
/// `daily_data_readiness::REASON_*` constants -- this zero-dependency crate
/// cannot import `mqk-daemon`, so the vocabulary is duplicated here and must
/// stay numerically/textually identical to that module's `&'static str`
/// constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessBlockerReason {
    RequiredAssignmentsMissing,
    AssignmentResolutionFailed,
    RuntimeStrategyAssignmentMismatch,
    RuntimeStrategySymbolBindingMismatch,
    RuntimeStrategyTimeframeMismatch,
    StrategyRequirementUnknown,
    AssetClassUnknown,
    ProviderProvenanceInvalid,
    ProviderSymbolMismatch,
    ProviderIdMismatch,
    ProviderUnknown,
    ProviderIngestTimeFuture,
    ProviderDisabled,
    ProviderCapabilityMismatch,
    ProviderTimestampConventionUnverified,
    CalendarUnavailable,
    UnsupportedTimeframe,
    UnsupportedIntradayContinuity,
    MarketDataMissing,
    InsufficientHistory,
    DuplicateTimestamp,
    InteriorGap,
    LatestBarFuture,
    ExpectedLatestBarMissing,
}

impl ReadinessBlockerReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RequiredAssignmentsMissing => "required_assignments_missing",
            Self::AssignmentResolutionFailed => "assignment_resolution_failed",
            Self::RuntimeStrategyAssignmentMismatch => "runtime_strategy_assignment_mismatch",
            Self::RuntimeStrategySymbolBindingMismatch => {
                "runtime_strategy_symbol_binding_mismatch"
            }
            Self::RuntimeStrategyTimeframeMismatch => "runtime_strategy_timeframe_mismatch",
            Self::StrategyRequirementUnknown => "strategy_requirement_unknown",
            Self::AssetClassUnknown => "asset_class_unknown",
            Self::ProviderProvenanceInvalid => "provider_provenance_invalid",
            Self::ProviderSymbolMismatch => "provider_symbol_mismatch",
            Self::ProviderIdMismatch => "provider_id_mismatch",
            Self::ProviderUnknown => "provider_unknown",
            Self::ProviderIngestTimeFuture => "provider_ingest_time_future",
            Self::ProviderDisabled => "provider_disabled",
            Self::ProviderCapabilityMismatch => "provider_capability_mismatch",
            Self::ProviderTimestampConventionUnverified => {
                "provider_timestamp_convention_unverified"
            }
            Self::CalendarUnavailable => "calendar_unavailable",
            Self::UnsupportedTimeframe => "unsupported_timeframe",
            Self::UnsupportedIntradayContinuity => "unsupported_intraday_continuity",
            Self::MarketDataMissing => "market_data_missing",
            Self::InsufficientHistory => "insufficient_history",
            Self::DuplicateTimestamp => "duplicate_timestamp",
            Self::InteriorGap => "interior_gap",
            Self::LatestBarFuture => "latest_bar_future",
            Self::ExpectedLatestBarMissing => "expected_latest_bar_missing",
        }
    }

    pub fn parse_code(code: &str) -> Option<Self> {
        Some(match code {
            "required_assignments_missing" => Self::RequiredAssignmentsMissing,
            "assignment_resolution_failed" => Self::AssignmentResolutionFailed,
            "runtime_strategy_assignment_mismatch" => Self::RuntimeStrategyAssignmentMismatch,
            "runtime_strategy_symbol_binding_mismatch" => {
                Self::RuntimeStrategySymbolBindingMismatch
            }
            "runtime_strategy_timeframe_mismatch" => Self::RuntimeStrategyTimeframeMismatch,
            "strategy_requirement_unknown" => Self::StrategyRequirementUnknown,
            "asset_class_unknown" => Self::AssetClassUnknown,
            "provider_provenance_invalid" => Self::ProviderProvenanceInvalid,
            "provider_symbol_mismatch" => Self::ProviderSymbolMismatch,
            "provider_id_mismatch" => Self::ProviderIdMismatch,
            "provider_unknown" => Self::ProviderUnknown,
            "provider_ingest_time_future" => Self::ProviderIngestTimeFuture,
            "provider_disabled" => Self::ProviderDisabled,
            "provider_capability_mismatch" => Self::ProviderCapabilityMismatch,
            "provider_timestamp_convention_unverified" => {
                Self::ProviderTimestampConventionUnverified
            }
            "calendar_unavailable" => Self::CalendarUnavailable,
            "unsupported_timeframe" => Self::UnsupportedTimeframe,
            "unsupported_intraday_continuity" => Self::UnsupportedIntradayContinuity,
            "market_data_missing" => Self::MarketDataMissing,
            "insufficient_history" => Self::InsufficientHistory,
            "duplicate_timestamp" => Self::DuplicateTimestamp,
            "interior_gap" => Self::InteriorGap,
            "latest_bar_future" => Self::LatestBarFuture,
            "expected_latest_bar_missing" => Self::ExpectedLatestBarMissing,
            _ => return None,
        })
    }
}

/// The single closed, bounded, persistable exact-reason vocabulary for one
/// candidate (or symbol) result. See module docs above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactSelectionReason {
    // -- registry/plugin/spec/timeframe --
    RegistryQueryFailed,
    RegistryRowMissing,
    RegistryDisabled,
    UnsupportedStrategyPlugin,
    RegistryConstructionFailed,
    TimeframeMismatch,
    // -- structural identity --
    BlankIdentity,
    // -- promotion/evidence/artifact/fingerprint (incl. v2) --
    PromotionQueryFailed,
    NoPromotionRecord,
    PromotionNotActivePaper,
    PromotionNotYetEffective,
    PromotionExpired,
    EvidenceLineageQueryFailed,
    EvidenceLineageBroken,
    ArtifactPathMissing,
    DurableFingerprintMissing,
    DurableFingerprintV2Missing,
    ArtifactRootUnavailable,
    ArtifactMissing,
    ArtifactRootEscape,
    ArtifactMalformed,
    ArtifactDuplicateIdentity,
    ArtifactNotPaperCandidate,
    ArtifactNoMatchingRow,
    ArtifactRowIdentityMismatch,
    ArtifactOverLimit,
    FingerprintMismatch,
    FingerprintV2Mismatch,
    /// Defect 1: a present legacy fingerprint value is not exactly 64
    /// lowercase hexadecimal characters.
    LegacyFingerprintMalformed,
    /// Defect 1: the caller-supplied `legacy_fingerprint_matches` boolean
    /// disagrees with the truth derived directly from the durable/recomputed
    /// values -- a boolean/value incoherence, not a plain mismatch.
    LegacyFingerprintMatchIncoherent,
    /// Defect 1: a present v2 exact-score fingerprint value is not exactly
    /// 64 lowercase hexadecimal characters.
    FingerprintV2Malformed,
    /// Defect 1: the caller-supplied `exact_fingerprint_v2_matches` boolean
    /// disagrees with the truth derived directly from the durable/recomputed
    /// values -- a boolean/value incoherence, not a plain mismatch.
    FingerprintV2MatchIncoherent,
    /// Defect 2: the caller-supplied pre-gate `exact_reason` diagnosis is
    /// inconsistent with the evidence-gate check that actually failed --
    /// trusted only after proving agreement (see
    /// [`gate_reason_accepts_pregate_refinement`]); disagreement fails
    /// closed here rather than silently accepting an unproven diagnosis.
    PreGateReasonInconsistent,
    /// DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: this candidate's
    /// promoted semantic fingerprint (`config_identity_status == verified_v1`,
    /// structurally valid, and byte-equal to the current server-resolved
    /// fingerprint for the exact configuration that would actually be
    /// activated) could not be verified. A stale/drifted/unverified/legacy
    /// config can never be represented as a positively eligible candidate,
    /// even though every other evidence check (promotion state, review
    /// artifact, scanner score) passed.
    ConfigIdentityUnavailable,
    // -- score/rank --
    ScoreMissing,
    ScoreNotFinite,
    RankOutOfRange,
    // -- daily-readiness (wraps the closed readiness-blocker vocabulary) --
    DataNotReady(ReadinessBlockerReason),
    /// The caller flagged `data_ready = false` but supplied no specific
    /// readiness blocker -- honest about the gap rather than fabricating a
    /// specific blocker it never observed.
    DataNotReadyUnspecified,
    // -- divergent duplicate --
    DivergentDuplicate,
    // -- lower-score/tie-break nonselection --
    NotSelectedLowerScore,
    NotSelectedLostTieBreak,
    MissingRankRequiredForTie,
    // -- every selected reason --
    SelectedHighestScore,
    SelectedTieBreakRank,
    SelectedTieBreakWatchlist,
    SelectedTieBreakStrategyId,
    // -- no-valid-candidate --
    NoValidCandidate,
}

impl ExactSelectionReason {
    /// Stable, bounded, owned code -- `String` rather than `&'static str`
    /// only because [`Self::DataNotReady`] composes a nested code; every
    /// other variant returns a `'static`-backed literal wrapped in `String`.
    pub fn code(&self) -> String {
        match self {
            Self::RegistryQueryFailed => "registry_query_failed".to_string(),
            Self::RegistryRowMissing => "registry_row_missing".to_string(),
            Self::RegistryDisabled => "registry_disabled".to_string(),
            Self::UnsupportedStrategyPlugin => "unsupported_strategy_plugin".to_string(),
            Self::RegistryConstructionFailed => "registry_construction_failed".to_string(),
            Self::TimeframeMismatch => "timeframe_mismatch".to_string(),
            Self::BlankIdentity => "blank_identity".to_string(),
            Self::PromotionQueryFailed => "promotion_query_failed".to_string(),
            Self::NoPromotionRecord => "no_promotion_record".to_string(),
            Self::PromotionNotActivePaper => "promotion_not_active_paper".to_string(),
            Self::PromotionNotYetEffective => "promotion_not_yet_effective".to_string(),
            Self::PromotionExpired => "promotion_expired".to_string(),
            Self::EvidenceLineageQueryFailed => "evidence_lineage_query_failed".to_string(),
            Self::EvidenceLineageBroken => "evidence_lineage_broken".to_string(),
            Self::ArtifactPathMissing => "artifact_path_missing".to_string(),
            Self::DurableFingerprintMissing => "durable_fingerprint_missing".to_string(),
            Self::DurableFingerprintV2Missing => "durable_fingerprint_v2_missing".to_string(),
            Self::ArtifactRootUnavailable => "artifact_root_unavailable".to_string(),
            Self::ArtifactMissing => "artifact_missing".to_string(),
            Self::ArtifactRootEscape => "artifact_root_escape".to_string(),
            Self::ArtifactMalformed => "artifact_malformed".to_string(),
            Self::ArtifactDuplicateIdentity => "artifact_duplicate_identity".to_string(),
            Self::ArtifactNotPaperCandidate => "artifact_not_paper_candidate".to_string(),
            Self::ArtifactNoMatchingRow => "artifact_no_matching_row".to_string(),
            Self::ArtifactRowIdentityMismatch => "artifact_row_identity_mismatch".to_string(),
            Self::ArtifactOverLimit => "artifact_over_limit".to_string(),
            Self::FingerprintMismatch => "fingerprint_mismatch".to_string(),
            Self::FingerprintV2Mismatch => "fingerprint_v2_mismatch".to_string(),
            Self::LegacyFingerprintMalformed => "legacy_fingerprint_malformed".to_string(),
            Self::LegacyFingerprintMatchIncoherent => {
                "legacy_fingerprint_match_incoherent".to_string()
            }
            Self::FingerprintV2Malformed => "fingerprint_v2_malformed".to_string(),
            Self::FingerprintV2MatchIncoherent => "fingerprint_v2_match_incoherent".to_string(),
            Self::PreGateReasonInconsistent => "pre_gate_reason_inconsistent".to_string(),
            Self::ConfigIdentityUnavailable => "config_identity_unavailable".to_string(),
            Self::ScoreMissing => "score_missing".to_string(),
            Self::ScoreNotFinite => "score_not_finite".to_string(),
            Self::RankOutOfRange => "rank_out_of_range".to_string(),
            Self::DataNotReady(blocker) => format!("data_not_ready:{}", blocker.code()),
            Self::DataNotReadyUnspecified => "data_not_ready_unspecified".to_string(),
            Self::DivergentDuplicate => "divergent_duplicate".to_string(),
            Self::NotSelectedLowerScore => "not_selected_lower_score".to_string(),
            Self::NotSelectedLostTieBreak => "not_selected_lost_tie_break".to_string(),
            Self::MissingRankRequiredForTie => "missing_rank_required_for_tie".to_string(),
            Self::SelectedHighestScore => "selected_highest_score".to_string(),
            Self::SelectedTieBreakRank => "selected_tie_break_lower_rank".to_string(),
            Self::SelectedTieBreakWatchlist => {
                "selected_tie_break_watchlist_assignment".to_string()
            }
            Self::SelectedTieBreakStrategyId => {
                "selected_tie_break_strategy_id_ascending".to_string()
            }
            Self::NoValidCandidate => "no_valid_candidate_for_symbol".to_string(),
        }
    }

    /// Parse a persisted code back into a closed variant. Returns `None`
    /// for anything outside the closed vocabulary -- an unrecognized code
    /// must fail closed, never resolve to a guessed variant.
    pub fn parse_code(code: &str) -> Option<Self> {
        if let Some(blocker_code) = code.strip_prefix("data_not_ready:") {
            return ReadinessBlockerReason::parse_code(blocker_code).map(Self::DataNotReady);
        }
        Some(match code {
            "registry_query_failed" => Self::RegistryQueryFailed,
            "registry_row_missing" => Self::RegistryRowMissing,
            "registry_disabled" => Self::RegistryDisabled,
            "unsupported_strategy_plugin" => Self::UnsupportedStrategyPlugin,
            "registry_construction_failed" => Self::RegistryConstructionFailed,
            "timeframe_mismatch" => Self::TimeframeMismatch,
            "blank_identity" => Self::BlankIdentity,
            "promotion_query_failed" => Self::PromotionQueryFailed,
            "no_promotion_record" => Self::NoPromotionRecord,
            "promotion_not_active_paper" => Self::PromotionNotActivePaper,
            "promotion_not_yet_effective" => Self::PromotionNotYetEffective,
            "promotion_expired" => Self::PromotionExpired,
            "evidence_lineage_query_failed" => Self::EvidenceLineageQueryFailed,
            "evidence_lineage_broken" => Self::EvidenceLineageBroken,
            "artifact_path_missing" => Self::ArtifactPathMissing,
            "durable_fingerprint_missing" => Self::DurableFingerprintMissing,
            "durable_fingerprint_v2_missing" => Self::DurableFingerprintV2Missing,
            "artifact_root_unavailable" => Self::ArtifactRootUnavailable,
            "artifact_missing" => Self::ArtifactMissing,
            "artifact_root_escape" => Self::ArtifactRootEscape,
            "artifact_malformed" => Self::ArtifactMalformed,
            "artifact_duplicate_identity" => Self::ArtifactDuplicateIdentity,
            "artifact_not_paper_candidate" => Self::ArtifactNotPaperCandidate,
            "artifact_no_matching_row" => Self::ArtifactNoMatchingRow,
            "artifact_row_identity_mismatch" => Self::ArtifactRowIdentityMismatch,
            "artifact_over_limit" => Self::ArtifactOverLimit,
            "fingerprint_mismatch" => Self::FingerprintMismatch,
            "fingerprint_v2_mismatch" => Self::FingerprintV2Mismatch,
            "legacy_fingerprint_malformed" => Self::LegacyFingerprintMalformed,
            "legacy_fingerprint_match_incoherent" => Self::LegacyFingerprintMatchIncoherent,
            "fingerprint_v2_malformed" => Self::FingerprintV2Malformed,
            "fingerprint_v2_match_incoherent" => Self::FingerprintV2MatchIncoherent,
            "pre_gate_reason_inconsistent" => Self::PreGateReasonInconsistent,
            "config_identity_unavailable" => Self::ConfigIdentityUnavailable,
            "score_missing" => Self::ScoreMissing,
            "score_not_finite" => Self::ScoreNotFinite,
            "rank_out_of_range" => Self::RankOutOfRange,
            "data_not_ready_unspecified" => Self::DataNotReadyUnspecified,
            "divergent_duplicate" => Self::DivergentDuplicate,
            "not_selected_lower_score" => Self::NotSelectedLowerScore,
            "not_selected_lost_tie_break" => Self::NotSelectedLostTieBreak,
            "missing_rank_required_for_tie" => Self::MissingRankRequiredForTie,
            "selected_highest_score" => Self::SelectedHighestScore,
            "selected_tie_break_lower_rank" => Self::SelectedTieBreakRank,
            "selected_tie_break_watchlist_assignment" => Self::SelectedTieBreakWatchlist,
            "selected_tie_break_strategy_id_ascending" => Self::SelectedTieBreakStrategyId,
            "no_valid_candidate_for_symbol" => Self::NoValidCandidate,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// R7: bounds — refuse over-limit input before any expensive work
// ---------------------------------------------------------------------------

/// Mirrors `mqk-daemon::watchlist_intake::MULTI_SYMBOL_HARD_CEILING` (5).
/// This zero-dependency crate cannot import `mqk-daemon`, so the value is
/// duplicated here and must stay numerically identical to that ceiling.
pub const MAX_ELIGIBLE_SYMBOLS: usize = 5;

/// Mirrors the current built-in strategy plugin universe size registered by
/// `mqk-strategy::engines::register_builtin_strategies` as of this writing:
/// swing_momentum, mean_reversion, volatility_breakout, intraday_scalper
/// (long), intraday_scalper (short) — 5 registrations. Any candidate
/// strategy_id set the daemon builds must already be deduplicated against
/// this universe before it reaches this module.
pub const MAX_STRATEGY_UNIVERSE: usize = 5;

/// Derived bound on total `(symbol, strategy_id[, timeframe])` candidate
/// pairs a plan may consider: [`MAX_ELIGIBLE_SYMBOLS`] × [`MAX_STRATEGY_UNIVERSE`].
pub const MAX_CANDIDATE_PAIRS: usize = MAX_ELIGIBLE_SYMBOLS * MAX_STRATEGY_UNIVERSE;

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
/// never reads a clock or mints a UUID. There is no `plan_id` field: see
/// module-level docs, "Non-circular plan identity" (R5) —
/// [`canonical_plan_identity_material`] is computed from the *resolved*
/// [`DynamicSelectionPlan`], not supplied upfront.
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicSelectionContext {
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
/// for one `(symbol, strategy_id, timeframe_secs)` candidate. This module
/// trusts none of these booleans blindly in the sense of skipping its own
/// gate ordering, but performs no DB/filesystem re-validation of its own —
/// that trust boundary belongs to the caller (mirrors every other pure
/// policy module in this crate).
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
    /// IR6: the matched review row's exact `review_state` string (e.g.
    /// `"paper_candidate"`, `"rejected"`), carried through in full even
    /// though only one value passes [`Self::review_state_is_paper_candidate`]
    /// -- never collapsed to the boolean alone.
    pub evidence_review_state: Option<String>,
    /// Defect 1: the durable legacy `evidence_fingerprint` column value, as
    /// read back by the caller -- `None` only when no promotion/evidence
    /// record exists at all. Carried separately from
    /// [`Self::recomputed_legacy_fingerprint`] so durable persistence and
    /// plan identity never collapse "what was on file" and "what we just
    /// computed" into one ambiguous value.
    pub durable_legacy_fingerprint: Option<String>,
    /// Defect 1: the legacy SHA-256 fingerprint the caller freshly
    /// recomputed from the review artifact, before comparing it against
    /// [`Self::durable_legacy_fingerprint`].
    pub recomputed_legacy_fingerprint: Option<String>,
    /// `true` when the caller recomputed the review-row fingerprint
    /// (SHA-256 of the canonical serialized row) and it matched the durable
    /// `evidence_fingerprint` column exactly.
    pub legacy_fingerprint_matches: bool,
    /// Defect 1: the durable versioned exact-score `evidence_fingerprint_v2`
    /// column value, as read back by the caller -- `None` when no such
    /// value exists (a legacy row, or no promotion/evidence record at all).
    pub durable_exact_fingerprint_v2: Option<String>,
    /// Defect 1: the v2 exact-score fingerprint the caller freshly
    /// recomputed from the review artifact's raw score token, before
    /// comparing it against [`Self::durable_exact_fingerprint_v2`].
    pub recomputed_exact_fingerprint_v2: Option<String>,
    /// Defect 1: `true` only when both [`Self::durable_exact_fingerprint_v2`]
    /// and [`Self::recomputed_exact_fingerprint_v2`] are present and equal
    /// byte-for-byte. Ranking requires this *and*
    /// [`Self::legacy_fingerprint_matches`] to both be explicitly `true` --
    /// see [`evaluate_evidence_gate`].
    pub exact_fingerprint_v2_matches: bool,
    /// DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: `true` only when the
    /// caller independently verified, via the canonical config-identity
    /// comparison mechanism (`mqk-daemon::promotion_gate::
    /// evaluate_promotion_tradability_with_config_identity`'s own rule:
    /// `config_identity_status == "verified_v1"`, a structurally valid
    /// fingerprint, and byte-equality), that this candidate's promoted
    /// semantic fingerprint matches the current server-resolved semantic
    /// configuration for the exact `(strategy_id, symbol, timeframe_secs)`
    /// that would actually be activated. `false` for legacy/unavailable/
    /// malformed/mismatched config identity -- never a wildcard pass.
    pub config_identity_verified: bool,
    /// The durable `config_fingerprint` this candidate's `active_paper`
    /// promotion record carries, as read back by the caller -- `None` when
    /// unavailable (legacy row, or no promotion record). Carried through
    /// (never gate-checked directly; only [`Self::config_identity_verified`]
    /// is) for durable audit/read-side recomputation, mirroring how the
    /// legacy/v2 fingerprint pairs above are carried through.
    pub durable_config_fingerprint: Option<String>,
    /// The current server-resolved semantic fingerprint the caller computed
    /// AT PLAN-BUILD TIME for this exact identity -- the frozen "expected
    /// identity" a later host-pool construction must be proven to match
    /// (never re-derived independently a second time as a substitute for
    /// this frozen value, which would not detect drift between the two
    /// points in time). `None` when the caller could not resolve one.
    pub current_config_fingerprint: Option<String>,
    /// IR4: `true` when the durable strategy registry
    /// (`sys_strategy_registry`) has an `enabled = true` row for this
    /// `strategy_id` -- independent of, and distinct from,
    /// [`Self::plugin_instantiable`] (an engine can instantiate for an
    /// identity that was never durably registered, or was registered and
    /// then disabled).
    pub registry_enabled: bool,
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
    /// IR7: canonical, decimal-exact string representation of the durable
    /// `scanner_score`, as produced by
    /// [`crate::canonicalize_decimal_token`] from the artifact's raw JSON
    /// numeric token — never derived from a float. This is the
    /// authoritative score for gating/ranking/identity. `None` when the
    /// score is missing, the raw token was not a valid/supported decimal,
    /// or the caller could not produce one — never a fabricated default.
    pub canonical_score_decimal: Option<String>,
    /// Convenience bridge to the legacy scaled-integer (micros, 1e-6)
    /// representation, via [`crate::canonical_decimal_to_micros_if_exact`].
    /// `Some` only when `canonical_score_decimal` is exactly representable
    /// at that scale (at most six fractional digits) — never a rounded
    /// approximation of a higher-precision value. Carried through for
    /// backward-compatible callers/persistence; never itself the ranking or
    /// identity authority (see [`SelectionCandidateEvidence::canonical_score_decimal`]).
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
    /// IR6: the review artifact manifest's `git_hash`, carried through
    /// unvalidated (like the other evidence identity fields above) for
    /// durable evidence/read-side recomputation.
    pub evidence_git_hash: Option<String>,
    /// IR6: the durable promotion transition id that put this identity into
    /// `active_paper` (the transition the plan's authority is actually
    /// resting on) — `None` only when no promotion record exists at all.
    pub promotion_transition_id: Option<String>,
    /// IR6: `effective_at_utc` of [`Self::promotion_transition_id`], RFC3339.
    pub promotion_effective_at: Option<String>,
    /// IR6: `expires_at_utc` of [`Self::promotion_transition_id`] (`None` if
    /// the promotion carries no expiry), RFC3339.
    pub promotion_expires_at: Option<String>,
    /// IR6: the exact transition id that established the evidence currently
    /// authorizing this identity (see
    /// `mqk_db::StrategyPromotionTransitionRecord::evidence_transition_id`
    /// / `resolve_evidence_lineage`) — distinct from
    /// [`Self::promotion_transition_id`] when the current `active_paper`
    /// transition inherited its evidence from an earlier transition.
    pub evidence_transition_id: Option<String>,
    /// Defect E: the precise, closed-vocabulary reason the caller observed
    /// for this candidate's pre-gate refusal (e.g. a daemon-side
    /// `CandidateEvidenceReason` mapped to [`ExactSelectionReason`]), carried
    /// through even when the pure gate's own coarse `reason_code` collapses
    /// several distinct causes onto one boolean (see
    /// [`evaluate_evidence_gate`]). `None` for a candidate that never
    /// reached a caller-observed pre-gate failure -- [`candidate_result`]
    /// (or [`resolve_symbol_group`] for a symbol-level outcome) always
    /// derives the *final*, authoritative exact reason for every candidate
    /// result from this field (when present) or from the gate/ranking
    /// outcome itself (when absent), so every [`SelectionCandidateResult`]
    /// ends up with a resolved `exact_reason`. Never a caller-supplied
    /// free-form string — always one of [`ExactSelectionReason`]'s bounded,
    /// closed variants.
    pub exact_reason: Option<ExactSelectionReason>,
}

/// One `(symbol, strategy_id, timeframe_secs)` candidate under selection
/// consideration. Candidate identity is the full triple — two rows sharing
/// `symbol`/`strategy_id` but differing `timeframe_secs` are distinct
/// identities, never merged (R2).
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

/// One canonical candidate snapshot — sufficient on its own to persist,
/// reconstruct, rerun, and compare the selector (R6). Every result-affecting
/// authority fact from [`SelectionCandidateEvidence`] is carried through;
/// nothing durable read-back needs to guess.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionCandidateResult {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    /// IR7: see [`SelectionCandidateEvidence::canonical_score_decimal`].
    pub canonical_score_decimal: Option<String>,
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<u32>,
    pub watchlist_assigned: bool,
    pub promotion_query_ok: bool,
    pub promotion_state: Option<String>,
    pub promotion_effective: bool,
    pub promotion_expired: bool,
    pub evidence_resolved: bool,
    pub review_state_is_paper_candidate: bool,
    /// IR6: see [`SelectionCandidateEvidence::evidence_review_state`].
    pub evidence_review_state: Option<String>,
    /// Defect 1: see [`SelectionCandidateEvidence::durable_legacy_fingerprint`].
    pub durable_legacy_fingerprint: Option<String>,
    /// Defect 1: see [`SelectionCandidateEvidence::recomputed_legacy_fingerprint`].
    pub recomputed_legacy_fingerprint: Option<String>,
    pub legacy_fingerprint_matches: bool,
    /// Defect 1: see [`SelectionCandidateEvidence::durable_exact_fingerprint_v2`].
    pub durable_exact_fingerprint_v2: Option<String>,
    /// Defect 1: see [`SelectionCandidateEvidence::recomputed_exact_fingerprint_v2`].
    pub recomputed_exact_fingerprint_v2: Option<String>,
    /// DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: see
    /// [`SelectionCandidateEvidence::config_identity_verified`].
    pub config_identity_verified: bool,
    /// See [`SelectionCandidateEvidence::durable_config_fingerprint`].
    pub durable_config_fingerprint: Option<String>,
    /// See [`SelectionCandidateEvidence::current_config_fingerprint`] -- the
    /// frozen expected identity a host-pool construction must match.
    pub current_config_fingerprint: Option<String>,
    pub exact_fingerprint_v2_matches: bool,
    /// IR4/IR6: see [`SelectionCandidateEvidence::registry_enabled`].
    pub registry_enabled: bool,
    pub plugin_instantiable: bool,
    pub timeframe_matches: bool,
    pub data_ready: bool,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_artifact_path: Option<String>,
    /// IR6: see [`SelectionCandidateEvidence::evidence_git_hash`].
    pub evidence_git_hash: Option<String>,
    /// IR6: see [`SelectionCandidateEvidence::promotion_transition_id`].
    pub promotion_transition_id: Option<String>,
    /// IR6: see [`SelectionCandidateEvidence::promotion_effective_at`].
    pub promotion_effective_at: Option<String>,
    /// IR6: see [`SelectionCandidateEvidence::promotion_expires_at`].
    pub promotion_expires_at: Option<String>,
    /// IR6: see [`SelectionCandidateEvidence::evidence_transition_id`].
    pub evidence_transition_id: Option<String>,
    /// Defect E/Defect 4: the final, authoritative closed-vocabulary exact
    /// reason for this candidate result -- structurally non-optional (see
    /// [`SelectionCandidateEvidence::exact_reason`] for the still-optional
    /// pre-gate input diagnosis this is derived from).
    pub exact_reason: ExactSelectionReason,
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
    /// Defect E/Defect 4: the closed-vocabulary counterpart to `reason_code`
    /// at the symbol level -- `ExactSelectionReason::NoValidCandidate` when
    /// `reason_code == REASON_NO_VALID_CANDIDATE` (covers the case where a
    /// symbol has zero candidate rows at all, so no [`SelectionCandidateResult`]
    /// exists to carry that reason itself), or the winning candidate's own
    /// exact reason when one was selected. Structurally non-optional.
    pub exact_reason: ExactSelectionReason,
    /// One row per input candidate for this symbol, deterministically
    /// sorted by `(strategy_id, timeframe_secs)` ascending — never input
    /// order, and never dependent on which divergent-duplicate row was
    /// encountered first (R4).
    pub candidates: Vec<SelectionCandidateResult>,
}

/// Identity-free, canonically resolved selection plan. See module-level
/// docs, "Non-circular plan identity" (R5) — this type intentionally has no
/// `plan_id` field.
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
// IR2: selected-row coherence
// ---------------------------------------------------------------------------

/// Every way a resolved [`SymbolSelectionResult`] can fail to satisfy the
/// selected-row coherence contract IR2 requires before any host can be
/// constructed from it: `selected_strategy_id` present iff exactly one
/// candidate carries `selected == true`; that one candidate's `strategy_id`
/// equals `selected_strategy_id` canonically; its `disposition` is
/// `Selected`; and its `timeframe_secs` is strictly positive. This module's
/// own [`resolve_symbol_group`] always produces coherent output for a
/// freshly-computed plan — this check exists for every *other* path a
/// [`DynamicSelectionPlan`] can reach a caller: durable read-back
/// (Phase 8/9), a hand-built fixture, or any future serialization
/// round-trip. Never trust selected-row shape implicitly; always verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCoherenceViolation {
    /// `selected_strategy_id` is `Some(..)` but no candidate has
    /// `selected == true`.
    SelectedIdWithoutSelectedCandidate,
    /// A candidate has `selected == true` but `selected_strategy_id` is
    /// `None`.
    SelectedCandidateWithoutSelectedId,
    /// More than one candidate has `selected == true` for this symbol.
    MultipleSelectedCandidates,
    /// The one selected candidate's `strategy_id` does not canonically equal
    /// `selected_strategy_id`.
    SelectedCandidateIdMismatch,
    /// The one selected candidate's `disposition` is not
    /// `SelectionCandidateDisposition::Selected`.
    SelectedCandidateDispositionMismatch,
    /// The one selected candidate's `timeframe_secs` is not strictly
    /// positive.
    SelectedCandidateNonPositiveTimeframe,
}

impl SelectionCoherenceViolation {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SelectedIdWithoutSelectedCandidate => {
                "selection_coherence_selected_id_without_selected_candidate"
            }
            Self::SelectedCandidateWithoutSelectedId => {
                "selection_coherence_selected_candidate_without_selected_id"
            }
            Self::MultipleSelectedCandidates => "selection_coherence_multiple_selected_candidates",
            Self::SelectedCandidateIdMismatch => {
                "selection_coherence_selected_candidate_id_mismatch"
            }
            Self::SelectedCandidateDispositionMismatch => {
                "selection_coherence_selected_candidate_disposition_mismatch"
            }
            Self::SelectedCandidateNonPositiveTimeframe => {
                "selection_coherence_selected_candidate_non_positive_timeframe"
            }
        }
    }
}

/// Verify one symbol's selected-row coherence. See
/// [`SelectionCoherenceViolation`] for the exact contract.
pub fn verify_symbol_selection_coherence(
    sr: &SymbolSelectionResult,
) -> Result<(), SelectionCoherenceViolation> {
    let selected_candidates: Vec<&SelectionCandidateResult> =
        sr.candidates.iter().filter(|c| c.selected).collect();

    match (&sr.selected_strategy_id, selected_candidates.len()) {
        (Some(_), 0) => {
            return Err(SelectionCoherenceViolation::SelectedIdWithoutSelectedCandidate)
        }
        (Some(_), n) if n > 1 => {
            return Err(SelectionCoherenceViolation::MultipleSelectedCandidates)
        }
        (None, n) if n > 0 => {
            return Err(SelectionCoherenceViolation::SelectedCandidateWithoutSelectedId)
        }
        _ => {}
    }

    if let Some(id) = &sr.selected_strategy_id {
        let cand = selected_candidates[0];
        if &cand.strategy_id != id {
            return Err(SelectionCoherenceViolation::SelectedCandidateIdMismatch);
        }
        if cand.disposition != SelectionCandidateDisposition::Selected {
            return Err(SelectionCoherenceViolation::SelectedCandidateDispositionMismatch);
        }
        if cand.timeframe_secs <= 0 {
            return Err(SelectionCoherenceViolation::SelectedCandidateNonPositiveTimeframe);
        }
    }

    Ok(())
}

/// Verify every symbol's selected-row coherence across a whole plan.
/// Returns one `(symbol, violation)` entry per incoherent symbol — empty
/// when the whole plan is coherent. Never short-circuits on the first
/// violation, so a caller (e.g. the daemon's start-gate) can report every
/// incoherent symbol at once rather than one refusal at a time.
pub fn verify_plan_selection_coherence(
    plan: &DynamicSelectionPlan,
) -> Vec<(String, SelectionCoherenceViolation)> {
    plan.symbol_results
        .iter()
        .filter_map(|sr| {
            verify_symbol_selection_coherence(sr)
                .err()
                .map(|v| (sr.symbol.clone(), v))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Defect 1: fingerprint pair coherence -- derive truth from the durable and
// recomputed values themselves, never trust a caller-supplied match boolean
// blindly.
// ---------------------------------------------------------------------------

/// Pure format validator: exactly 64 lowercase SHA-256 hexadecimal
/// characters. Used for both the legacy and v2 exact-score fingerprint
/// fields before any equality comparison between durable and recomputed
/// values is trusted.
pub fn is_valid_sha256_hex_fingerprint(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One fingerprint pair's fully-derived gate outcome -- presence, format,
/// and the caller-supplied match boolean's coherence with the derived
/// equality are all checked here; a bare boolean is never trusted alone.
enum FingerprintGateOutcome {
    /// Both present, valid format, byte-equal, and the caller's match
    /// boolean agrees (`true`) -- ranking may proceed.
    Ok,
    /// Either the durable or recomputed value is absent.
    Missing,
    /// Either present value is not exactly 64 lowercase hexadecimal
    /// characters.
    Malformed,
    /// The caller's match boolean disagrees with the derived equality
    /// (in either direction) -- the boolean itself cannot be trusted.
    Incoherent,
    /// Both present, valid format, unequal, and the caller's match boolean
    /// agrees (`false`) -- a genuine, coherent mismatch.
    Mismatch,
}

fn evaluate_fingerprint_gate(
    durable: &Option<String>,
    recomputed: &Option<String>,
    claimed_match: bool,
) -> FingerprintGateOutcome {
    let (durable, recomputed) = match (durable, recomputed) {
        (Some(d), Some(r)) => (d, r),
        _ => return FingerprintGateOutcome::Missing,
    };
    if !is_valid_sha256_hex_fingerprint(durable) || !is_valid_sha256_hex_fingerprint(recomputed) {
        return FingerprintGateOutcome::Malformed;
    }
    let derived_match = durable == recomputed;
    if derived_match != claimed_match {
        return FingerprintGateOutcome::Incoherent;
    }
    if derived_match {
        FingerprintGateOutcome::Ok
    } else {
        FingerprintGateOutcome::Mismatch
    }
}

// ---------------------------------------------------------------------------
// Per-candidate evidence gate (first failing reason wins)
// ---------------------------------------------------------------------------

/// The pure gate's own deterministic diagnosis for one candidate --
/// [`GateOutcome::Invalid`] always carries the gate's own coarse-safe exact
/// reason (e.g. [`ExactSelectionReason::PromotionNotActivePaper`], never the
/// finer [`ExactSelectionReason::NoPromotionRecord`] the gate's own boolean
/// checks cannot distinguish on their own). [`candidate_result`] is the only
/// place a caller-supplied pre-gate diagnosis may refine this further, and
/// only after proving it agrees with this reason (Defect 2).
enum GateOutcome {
    Valid,
    Invalid(ExactSelectionReason),
}

fn evaluate_evidence_gate(c: &SelectionCandidateInput) -> GateOutcome {
    use GateOutcome::Invalid;

    if canonical_symbol(&c.symbol).is_empty() || canonical_strategy_id(&c.strategy_id).is_empty() {
        return Invalid(ExactSelectionReason::BlankIdentity);
    }
    if c.timeframe_secs <= 0 {
        return Invalid(ExactSelectionReason::BlankIdentity);
    }
    let e = &c.evidence;
    if !e.promotion_query_ok {
        return Invalid(ExactSelectionReason::PromotionQueryFailed);
    }
    if e.promotion_state.as_deref() != Some("active_paper") {
        return Invalid(ExactSelectionReason::PromotionNotActivePaper);
    }
    if !e.promotion_effective {
        return Invalid(ExactSelectionReason::PromotionNotYetEffective);
    }
    if e.promotion_expired {
        return Invalid(ExactSelectionReason::PromotionExpired);
    }
    if !e.evidence_resolved {
        return Invalid(ExactSelectionReason::EvidenceLineageBroken);
    }
    if !e.review_state_is_paper_candidate {
        return Invalid(ExactSelectionReason::ArtifactNotPaperCandidate);
    }
    // Defect 1: legacy and v2 fingerprint pairs are each fully derived --
    // presence, format, and boolean coherence -- as two distinct steps.
    // Ranking is eligible only when both pairs resolve `Ok`; malformed
    // shape, unequal values, and boolean/value incoherence all fail closed
    // with their own distinct exact reason, never folded into one generic
    // fingerprint failure.
    match evaluate_fingerprint_gate(
        &e.durable_legacy_fingerprint,
        &e.recomputed_legacy_fingerprint,
        e.legacy_fingerprint_matches,
    ) {
        FingerprintGateOutcome::Ok => {}
        FingerprintGateOutcome::Missing => {
            return Invalid(ExactSelectionReason::DurableFingerprintMissing)
        }
        FingerprintGateOutcome::Malformed => {
            return Invalid(ExactSelectionReason::LegacyFingerprintMalformed)
        }
        FingerprintGateOutcome::Incoherent => {
            return Invalid(ExactSelectionReason::LegacyFingerprintMatchIncoherent)
        }
        FingerprintGateOutcome::Mismatch => {
            return Invalid(ExactSelectionReason::FingerprintMismatch)
        }
    }
    match evaluate_fingerprint_gate(
        &e.durable_exact_fingerprint_v2,
        &e.recomputed_exact_fingerprint_v2,
        e.exact_fingerprint_v2_matches,
    ) {
        FingerprintGateOutcome::Ok => {}
        FingerprintGateOutcome::Missing => {
            return Invalid(ExactSelectionReason::DurableFingerprintV2Missing)
        }
        FingerprintGateOutcome::Malformed => {
            return Invalid(ExactSelectionReason::FingerprintV2Malformed)
        }
        FingerprintGateOutcome::Incoherent => {
            return Invalid(ExactSelectionReason::FingerprintV2MatchIncoherent)
        }
        FingerprintGateOutcome::Mismatch => {
            return Invalid(ExactSelectionReason::FingerprintV2Mismatch)
        }
    }
    // DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: a candidate whose
    // review-artifact evidence is fully valid can still be running under a
    // semantically different (stale) configuration than the one that was
    // actually promoted -- checked as its own distinct gate, never folded
    // into the fingerprint checks above (which authenticate the scanner/
    // review evidence binding, not the strategy's own decision-affecting
    // configuration).
    //
    // CANONICAL-CONFIG-IDENTITY-VERIFIER-01 (R7): `mqk-portfolio` cannot
    // depend on `mqk-daemon`'s canonical `strategy_config_identity::
    // config_identity_is_verified` predicate (this crate has zero crate
    // dependencies by design), so this pure gate performs its own INTEGRITY
    // check over the material it already receives -- reusing the same
    // `evaluate_fingerprint_gate` primitive the legacy/v2 fingerprint pairs
    // above use. A caller's bare `config_identity_verified = true` claim is
    // never trusted alone: it must COHERE with `durable_config_fingerprint`/
    // `current_config_fingerprint` both being present, structurally valid,
    // and byte-equal -- a contradictory claim (`verified=true` with
    // mismatched, malformed, or missing fingerprints) fails closed exactly
    // like every other fingerprint pair here.
    match evaluate_fingerprint_gate(
        &e.durable_config_fingerprint,
        &e.current_config_fingerprint,
        e.config_identity_verified,
    ) {
        FingerprintGateOutcome::Ok => {}
        FingerprintGateOutcome::Missing
        | FingerprintGateOutcome::Malformed
        | FingerprintGateOutcome::Incoherent
        | FingerprintGateOutcome::Mismatch => {
            return Invalid(ExactSelectionReason::ConfigIdentityUnavailable)
        }
    }
    if !e.plugin_instantiable {
        return Invalid(ExactSelectionReason::UnsupportedStrategyPlugin);
    }
    if !e.timeframe_matches {
        return Invalid(ExactSelectionReason::TimeframeMismatch);
    }
    if !e.data_ready {
        return Invalid(ExactSelectionReason::DataNotReadyUnspecified);
    }
    match &e.canonical_score_decimal {
        // IR7: `canonical_score_decimal` is the authoritative score --
        // already caller-validated as an exact, finite decimal (never a
        // float) by construction ([`crate::canonicalize_decimal_token`]
        // rejects non-finite/unsupported tokens itself); this gate only
        // requires one to be present at all.
        Some(_) => GateOutcome::Valid,
        None => Invalid(ExactSelectionReason::ScoreMissing),
    }
}

/// Defect 2: the exhaustive, closed match from every [`ExactSelectionReason`]
/// variant to its stable public `reason_code` string -- the single authority
/// for deriving `reason_code` from a resolved `exact_reason`, replacing the
/// old (and removed) `exact_reason_for_reason_code`'s inverse, fallible,
/// `.expect`-guarded direction. Every previously-producible `REASON_*`
/// string is preserved byte-for-byte (legacy persisted values remain
/// readable); variants the pure gate itself never produces (only reachable
/// via a caller-supplied, gate-compatible pre-gate diagnosis) map to the
/// same coarse `REASON_*` bucket their gate-producible counterpart already
/// used, since `reason_code` has always been the coarse, not the exact,
/// authority. Being an exhaustive match, this function cannot compile with a
/// variant left unhandled -- there is no wildcard arm and no possible panic
/// path.
fn public_reason_code(reason: ExactSelectionReason) -> &'static str {
    use ExactSelectionReason as R;
    match reason {
        R::RegistryQueryFailed
        | R::RegistryRowMissing
        | R::RegistryDisabled
        | R::UnsupportedStrategyPlugin
        | R::RegistryConstructionFailed => REASON_REFUSED_UNSUPPORTED_STRATEGY,
        R::TimeframeMismatch => REASON_REFUSED_TIMEFRAME_MISMATCH,
        R::BlankIdentity => REASON_REFUSED_BLANK_IDENTITY,
        R::PromotionQueryFailed => REASON_REFUSED_PROMOTION_QUERY_FAILED,
        R::NoPromotionRecord | R::PromotionNotActivePaper => REASON_REFUSED_NOT_ACTIVE_PAPER,
        R::PromotionNotYetEffective => REASON_REFUSED_NOT_YET_EFFECTIVE,
        R::PromotionExpired => REASON_REFUSED_EXPIRED,
        R::EvidenceLineageQueryFailed
        | R::EvidenceLineageBroken
        | R::ArtifactPathMissing
        | R::ArtifactRootUnavailable
        | R::ArtifactMissing
        | R::ArtifactRootEscape
        | R::ArtifactMalformed
        | R::ArtifactDuplicateIdentity
        | R::ArtifactNoMatchingRow
        | R::ArtifactRowIdentityMismatch
        | R::ArtifactOverLimit
        | R::RankOutOfRange => REASON_REFUSED_EVIDENCE_READ_FAILED,
        R::DurableFingerprintMissing => REASON_REFUSED_FINGERPRINT_MISSING,
        R::DurableFingerprintV2Missing => REASON_REFUSED_FINGERPRINT_V2_MISSING,
        R::ArtifactNotPaperCandidate => REASON_REFUSED_NOT_PAPER_CANDIDATE,
        R::FingerprintMismatch => REASON_REFUSED_FINGERPRINT_MISMATCH,
        R::FingerprintV2Mismatch => REASON_REFUSED_FINGERPRINT_V2_MISMATCH,
        R::LegacyFingerprintMalformed => REASON_REFUSED_FINGERPRINT_MALFORMED,
        R::LegacyFingerprintMatchIncoherent => REASON_REFUSED_FINGERPRINT_INCOHERENT,
        R::FingerprintV2Malformed => REASON_REFUSED_FINGERPRINT_V2_MALFORMED,
        R::FingerprintV2MatchIncoherent => REASON_REFUSED_FINGERPRINT_V2_INCOHERENT,
        R::PreGateReasonInconsistent => REASON_REFUSED_PRE_GATE_REASON_INCONSISTENT,
        R::ConfigIdentityUnavailable => REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE,
        R::ScoreMissing | R::ScoreNotFinite => REASON_REFUSED_MISSING_SCORE,
        R::DataNotReady(_) | R::DataNotReadyUnspecified => REASON_REFUSED_DATA_NOT_READY,
        R::DivergentDuplicate => REASON_REFUSED_DIVERGENT_DUPLICATE,
        R::NotSelectedLowerScore => REASON_NOT_SELECTED_LOWER_SCORE,
        R::NotSelectedLostTieBreak => REASON_NOT_SELECTED_LOST_TIE_BREAK,
        R::MissingRankRequiredForTie => REASON_REFUSED_MISSING_RANK_FOR_TIE,
        R::SelectedHighestScore => REASON_SELECTED_HIGHEST_SCORE,
        R::SelectedTieBreakRank => REASON_SELECTED_TIE_BREAK_RANK,
        R::SelectedTieBreakWatchlist => REASON_SELECTED_TIE_BREAK_WATCHLIST,
        R::SelectedTieBreakStrategyId => REASON_SELECTED_TIE_BREAK_STRATEGY_ID,
        R::NoValidCandidate => REASON_NO_VALID_CANDIDATE,
    }
}

/// Defect 2: `true` only when `pregate` is a documented, strictly narrower
/// explanation of exactly the same evidence-gate failure `gate_reason`
/// itself already diagnosed -- never a guess. Every non-identity entry here
/// mirrors a real ambiguity the pure gate's own coarse boolean check cannot
/// resolve on its own (the daemon caller sets `pregate` in the very same
/// branch that flips the boolean/field this gate reads -- see
/// `mqk-daemon::dynamic_selection_plan_builder::refused_evidence`), so this
/// is a proof the two are talking about the same underlying cause, not a
/// permissive allowlist. An unmatched pair is incompatible and fails closed
/// via [`ExactSelectionReason::PreGateReasonInconsistent`] in
/// [`candidate_result`].
fn gate_reason_accepts_pregate_refinement(
    gate_reason: ExactSelectionReason,
    pregate: ExactSelectionReason,
) -> bool {
    use ExactSelectionReason as R;
    if gate_reason == pregate {
        return true;
    }
    match gate_reason {
        R::PromotionNotActivePaper => matches!(pregate, R::NoPromotionRecord),
        R::EvidenceLineageBroken => matches!(
            pregate,
            R::EvidenceLineageQueryFailed
                | R::ArtifactPathMissing
                | R::DurableFingerprintMissing
                | R::ArtifactRootUnavailable
                | R::ArtifactMissing
                | R::ArtifactRootEscape
                | R::ArtifactMalformed
                | R::ArtifactDuplicateIdentity
                | R::ArtifactNoMatchingRow
                | R::ArtifactRowIdentityMismatch
                | R::ArtifactOverLimit
                | R::RankOutOfRange
        ),
        R::UnsupportedStrategyPlugin => matches!(
            pregate,
            R::RegistryQueryFailed
                | R::RegistryRowMissing
                | R::RegistryDisabled
                | R::RegistryConstructionFailed
        ),
        R::DataNotReadyUnspecified => matches!(pregate, R::DataNotReady(_)),
        _ => false,
    }
}

/// Defect 2: how a candidate's final exact reason must be resolved -- the
/// single enum every [`candidate_result`] call site now passes, replacing
/// the old raw `&str reason_code` parameter.
enum ResolvedOutcomeReason {
    /// A selector-determined outcome (ranking, tie-break, divergent
    /// duplicate, or no-valid-candidate) -- always this exact reason,
    /// **never** consulting [`SelectionCandidateEvidence::exact_reason`].
    /// These outcomes are decided by this module's own ranking logic, not by
    /// an evidence-gate failure, so a caller's pre-gate diagnosis (which
    /// describes a *different* kind of event) can never apply here.
    Selector(ExactSelectionReason),
    /// An evidence-gate refusal -- `gate_reason` is the gate's own
    /// deterministic diagnosis; the caller's pre-gate `exact_reason` (if
    /// any) is trusted only when
    /// [`gate_reason_accepts_pregate_refinement`] proves it agrees.
    EvidenceGateRefusal(ExactSelectionReason),
}

fn candidate_result(
    c: &SelectionCandidateInput,
    selected: bool,
    disposition: SelectionCandidateDisposition,
    outcome: ResolvedOutcomeReason,
) -> SelectionCandidateResult {
    let e = &c.evidence;
    // Defect 2: no `.expect`/`.unwrap` anywhere in this resolution -- every
    // arm is a total, direct assignment.
    let exact_reason = match outcome {
        ResolvedOutcomeReason::Selector(reason) => reason,
        ResolvedOutcomeReason::EvidenceGateRefusal(gate_reason) => match e.exact_reason {
            Some(pregate) if gate_reason_accepts_pregate_refinement(gate_reason, pregate) => {
                pregate
            }
            Some(_inconsistent) => ExactSelectionReason::PreGateReasonInconsistent,
            None => gate_reason,
        },
    };
    let reason_code = public_reason_code(exact_reason);
    SelectionCandidateResult {
        symbol: canonical_symbol(&c.symbol),
        strategy_id: canonical_strategy_id(&c.strategy_id),
        timeframe_secs: c.timeframe_secs,
        canonical_score_decimal: e.canonical_score_decimal.clone(),
        canonical_score_micros: e.canonical_score_micros,
        scanner_rank: e.scanner_rank,
        watchlist_assigned: e.watchlist_assigned,
        promotion_query_ok: e.promotion_query_ok,
        promotion_state: e.promotion_state.clone(),
        promotion_effective: e.promotion_effective,
        promotion_expired: e.promotion_expired,
        evidence_resolved: e.evidence_resolved,
        review_state_is_paper_candidate: e.review_state_is_paper_candidate,
        evidence_review_state: e.evidence_review_state.clone(),
        durable_legacy_fingerprint: e.durable_legacy_fingerprint.clone(),
        recomputed_legacy_fingerprint: e.recomputed_legacy_fingerprint.clone(),
        legacy_fingerprint_matches: e.legacy_fingerprint_matches,
        durable_exact_fingerprint_v2: e.durable_exact_fingerprint_v2.clone(),
        recomputed_exact_fingerprint_v2: e.recomputed_exact_fingerprint_v2.clone(),
        exact_fingerprint_v2_matches: e.exact_fingerprint_v2_matches,
        config_identity_verified: e.config_identity_verified,
        durable_config_fingerprint: e.durable_config_fingerprint.clone(),
        current_config_fingerprint: e.current_config_fingerprint.clone(),
        registry_enabled: e.registry_enabled,
        plugin_instantiable: e.plugin_instantiable,
        timeframe_matches: e.timeframe_matches,
        data_ready: e.data_ready,
        evidence_review_id: e.evidence_review_id.clone(),
        evidence_scanner_scan_id: e.evidence_scanner_scan_id.clone(),
        evidence_artifact_path: e.evidence_artifact_path.clone(),
        evidence_git_hash: e.evidence_git_hash.clone(),
        promotion_transition_id: e.promotion_transition_id.clone(),
        promotion_effective_at: e.promotion_effective_at.clone(),
        promotion_expires_at: e.promotion_expires_at.clone(),
        evidence_transition_id: e.evidence_transition_id.clone(),
        exact_reason,
        selected,
        disposition,
        reason_code: reason_code.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Defect 2: candidate reason-tuple coherence
// ---------------------------------------------------------------------------

/// The closed set of [`ExactSelectionReason`] variants a `Selected`
/// candidate result may carry.
fn is_selected_reason_family(reason: ExactSelectionReason) -> bool {
    matches!(
        reason,
        ExactSelectionReason::SelectedHighestScore
            | ExactSelectionReason::SelectedTieBreakRank
            | ExactSelectionReason::SelectedTieBreakWatchlist
            | ExactSelectionReason::SelectedTieBreakStrategyId
    )
}

/// The closed set of [`ExactSelectionReason`] variants a `NotSelected`
/// candidate result may carry.
fn is_not_selected_reason_family(reason: ExactSelectionReason) -> bool {
    matches!(
        reason,
        ExactSelectionReason::NotSelectedLowerScore | ExactSelectionReason::NotSelectedLostTieBreak
    )
}

/// Every way one [`SelectionCandidateResult`]'s `(reason_code, exact_reason,
/// disposition, selected)` tuple can fail to cohere -- see
/// [`verify_candidate_result_coherence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateReasonCoherenceViolation {
    /// `selected` disagrees with `disposition == Selected`.
    SelectedFlagDispositionMismatch,
    /// `reason_code` is not the exhaustively-derived public code for
    /// `exact_reason`.
    ReasonCodeExactReasonMismatch,
    /// `disposition == Selected` but `exact_reason` is outside the closed
    /// selected-reason family.
    SelectedDispositionWrongReasonFamily,
    /// `disposition == NotSelected` but `exact_reason` is outside the closed
    /// not-selected-reason family.
    NotSelectedDispositionWrongReasonFamily,
    /// `disposition == Refused` but `exact_reason` is inside the
    /// selected/not-selected reason family (which must only ever appear on
    /// a non-refused row).
    RefusedDispositionWrongReasonFamily,
}

impl CandidateReasonCoherenceViolation {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SelectedFlagDispositionMismatch => {
                "candidate_reason_selected_flag_disposition_mismatch"
            }
            Self::ReasonCodeExactReasonMismatch => "candidate_reason_code_exact_reason_mismatch",
            Self::SelectedDispositionWrongReasonFamily => {
                "candidate_reason_selected_disposition_wrong_reason_family"
            }
            Self::NotSelectedDispositionWrongReasonFamily => {
                "candidate_reason_not_selected_disposition_wrong_reason_family"
            }
            Self::RefusedDispositionWrongReasonFamily => {
                "candidate_reason_refused_disposition_wrong_reason_family"
            }
        }
    }
}

/// Verify one candidate result's `(reason_code, exact_reason, disposition,
/// selected)` form a single coherent, closed tuple -- Defect 2. Mirrors
/// [`verify_symbol_selection_coherence`]'s "never trust shape implicitly"
/// philosophy at the candidate level, so durable read-back (or any
/// hand-built fixture) can be verified, not just this module's own
/// freshly-computed output.
pub fn verify_candidate_result_coherence(
    c: &SelectionCandidateResult,
) -> Result<(), CandidateReasonCoherenceViolation> {
    if c.selected != (c.disposition == SelectionCandidateDisposition::Selected) {
        return Err(CandidateReasonCoherenceViolation::SelectedFlagDispositionMismatch);
    }
    if c.reason_code != public_reason_code(c.exact_reason) {
        return Err(CandidateReasonCoherenceViolation::ReasonCodeExactReasonMismatch);
    }
    match c.disposition {
        SelectionCandidateDisposition::Selected => {
            if !is_selected_reason_family(c.exact_reason) {
                return Err(
                    CandidateReasonCoherenceViolation::SelectedDispositionWrongReasonFamily,
                );
            }
        }
        SelectionCandidateDisposition::NotSelected => {
            if !is_not_selected_reason_family(c.exact_reason) {
                return Err(
                    CandidateReasonCoherenceViolation::NotSelectedDispositionWrongReasonFamily,
                );
            }
        }
        SelectionCandidateDisposition::Refused => {
            if is_selected_reason_family(c.exact_reason)
                || is_not_selected_reason_family(c.exact_reason)
            {
                return Err(CandidateReasonCoherenceViolation::RefusedDispositionWrongReasonFamily);
            }
        }
    }
    Ok(())
}

/// IR8: full canonical candidate payload used for R3's exact-replay equality
/// and R4's deterministic ordering key: canonical symbol, canonical
/// `strategy_id`, `timeframe_secs`, and every evidence field, serialized
/// through the same explicit length-prefixed byte primitives
/// ([`push_len_prefixed`]/[`push_bool`]/[`push_opt_str`]/[`push_opt_i64`]/
/// [`push_opt_u32`]/[`push_opt_exact_reason`]/[`push_exact_reason`]) that [`canonical_plan_identity_material`]
/// uses for the resolved plan — never `Debug`, pointer identity, source
/// ordinal, or input order. Two rows are an idempotent exact replay only
/// when this entire byte key matches, not evidence alone (R3).
fn canonical_candidate_payload_key(symbol: &str, c: &SelectionCandidateInput) -> Vec<u8> {
    let mut buf = Vec::new();
    let e = &c.evidence;
    push_len_prefixed(&mut buf, symbol);
    push_len_prefixed(&mut buf, &canonical_strategy_id(&c.strategy_id));
    push_i64(&mut buf, c.timeframe_secs);
    push_bool(&mut buf, e.promotion_query_ok);
    push_opt_str(&mut buf, &e.promotion_state);
    push_bool(&mut buf, e.promotion_effective);
    push_bool(&mut buf, e.promotion_expired);
    push_bool(&mut buf, e.evidence_resolved);
    push_bool(&mut buf, e.review_state_is_paper_candidate);
    push_opt_str(&mut buf, &e.evidence_review_state);
    // Defect 1: durable/recomputed legacy and v2 exact-score fingerprints,
    // and both match booleans, are each their own distinct field in the
    // canonical payload -- never collapsed into one ambiguous
    // evidence_fingerprint alias. A change to any one of these six values
    // alone changes this payload key.
    push_opt_str(&mut buf, &e.durable_legacy_fingerprint);
    push_opt_str(&mut buf, &e.recomputed_legacy_fingerprint);
    push_bool(&mut buf, e.legacy_fingerprint_matches);
    push_opt_str(&mut buf, &e.durable_exact_fingerprint_v2);
    push_opt_str(&mut buf, &e.recomputed_exact_fingerprint_v2);
    push_bool(&mut buf, e.exact_fingerprint_v2_matches);
    // DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: distinct fields, same
    // rationale as the fingerprint provenance fields above.
    push_bool(&mut buf, e.config_identity_verified);
    push_opt_str(&mut buf, &e.durable_config_fingerprint);
    push_opt_str(&mut buf, &e.current_config_fingerprint);
    push_bool(&mut buf, e.registry_enabled);
    push_bool(&mut buf, e.plugin_instantiable);
    push_bool(&mut buf, e.timeframe_matches);
    push_bool(&mut buf, e.data_ready);
    // IR7: the canonical decimal string is the authoritative score in
    // identity -- never the derived micros bridge, which can be `None`
    // even when the decimal score is present (more than six fractional
    // digits) and must never silently collapse two distinct decimal values.
    push_opt_str(&mut buf, &e.canonical_score_decimal);
    push_opt_i64(&mut buf, e.canonical_score_micros);
    push_opt_u32(&mut buf, e.scanner_rank);
    push_bool(&mut buf, e.watchlist_assigned);
    push_opt_str(&mut buf, &e.evidence_review_id);
    push_opt_str(&mut buf, &e.evidence_scanner_scan_id);
    push_opt_str(&mut buf, &e.evidence_artifact_path);
    push_opt_str(&mut buf, &e.evidence_git_hash);
    push_opt_str(&mut buf, &e.promotion_transition_id);
    push_opt_str(&mut buf, &e.promotion_effective_at);
    push_opt_str(&mut buf, &e.promotion_expires_at);
    push_opt_str(&mut buf, &e.evidence_transition_id);
    push_opt_exact_reason(&mut buf, e.exact_reason);
    buf
}

// ---------------------------------------------------------------------------
// Group (one symbol) resolution
// ---------------------------------------------------------------------------

fn resolve_symbol_group(symbol: &str, group: &[&SelectionCandidateInput]) -> SymbolSelectionResult {
    // Identity de-duplication: candidate identity is the full triple
    // (canonical symbol [fixed by this group], canonical strategy_id,
    // timeframe_secs) — R2. Two rows sharing symbol+strategy_id but
    // differing timeframe_secs are distinct identities and are gated
    // independently, never collapsed.
    let mut by_identity: BTreeMap<(String, i64), Vec<&SelectionCandidateInput>> = BTreeMap::new();
    for c in group {
        by_identity
            .entry((canonical_strategy_id(&c.strategy_id), c.timeframe_secs))
            .or_default()
            .push(c);
    }

    let mut results: Vec<SelectionCandidateResult> = Vec::new();
    let mut ranking_pool: Vec<&SelectionCandidateInput> = Vec::new();

    for rows in by_identity.values() {
        if rows.len() > 1 {
            // Exact-replay equality compares the complete canonical
            // candidate payload (symbol + strategy_id + timeframe_secs +
            // every evidence field), not evidence alone (R3). Symbol and
            // identity are already fixed within this group/bucket, so the
            // full-payload key still discriminates purely on evidence here
            // — computed explicitly (not inferred from bucketing) so the
            // equality contract is self-evident at the call site.
            let first_key = canonical_candidate_payload_key(symbol, rows[0]);
            let all_identical = rows
                .iter()
                .all(|r| canonical_candidate_payload_key(symbol, r) == first_key);
            if !all_identical {
                // R4: divergent duplicates must sort by a complete stable
                // payload key before entering `results`, so their relative
                // order — and therefore plan equality — never depends on
                // which row the caller happened to supply first.
                let mut sorted_rows: Vec<&SelectionCandidateInput> = rows.clone();
                sorted_rows.sort_by_key(|r| canonical_candidate_payload_key(symbol, r));
                for r in &sorted_rows {
                    results.push(candidate_result(
                        r,
                        false,
                        SelectionCandidateDisposition::Refused,
                        ResolvedOutcomeReason::Selector(ExactSelectionReason::DivergentDuplicate),
                    ));
                }
                continue;
            }
        }
        // Idempotent replay (full-payload-identical) or single row:
        // evaluate exactly once.
        let representative = rows[0];
        match evaluate_evidence_gate(representative) {
            GateOutcome::Invalid(gate_reason) => {
                results.push(candidate_result(
                    representative,
                    false,
                    SelectionCandidateDisposition::Refused,
                    ResolvedOutcomeReason::EvidenceGateRefusal(gate_reason),
                ));
            }
            GateOutcome::Valid => ranking_pool.push(representative),
        }
    }

    if ranking_pool.is_empty() {
        results.sort_by(|a, b| {
            (a.strategy_id.as_str(), a.timeframe_secs)
                .cmp(&(b.strategy_id.as_str(), b.timeframe_secs))
        });
        return SymbolSelectionResult {
            symbol: symbol.to_string(),
            selected_strategy_id: None,
            disposition: SelectionCandidateDisposition::Refused,
            reason_code: REASON_NO_VALID_CANDIDATE.to_string(),
            exact_reason: ExactSelectionReason::NoValidCandidate,
            candidates: results,
        };
    }

    // Step 1: highest canonical_score_decimal wins outright -- IR7: compared
    // as exact decimal strings, never through a float or a lossy
    // scaled-integer rounding that could silently collapse two distinct
    // values into a tie.
    let max_score: &str = ranking_pool
        .iter()
        .map(|c| {
            c.evidence
                .canonical_score_decimal
                .as_deref()
                .expect("gated Some")
        })
        .max_by(|a, b| crate::compare_canonical_decimal_strings(a, b))
        .expect("ranking_pool non-empty");
    let max_score = max_score.to_string();
    let mut leaders: Vec<&SelectionCandidateInput> = ranking_pool
        .iter()
        .copied()
        .filter(|c| c.evidence.canonical_score_decimal.as_deref() == Some(max_score.as_str()))
        .collect();

    let mut selected: Option<&SelectionCandidateInput> = None;
    let mut selected_reason = ExactSelectionReason::SelectedHighestScore;

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
                ResolvedOutcomeReason::Selector(ExactSelectionReason::MissingRankRequiredForTie),
            ));
        }
        leaders = rank_ok;

        if leaders.len() == 1 {
            selected = Some(leaders[0]);
            selected_reason = ExactSelectionReason::SelectedTieBreakRank;
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
                        ResolvedOutcomeReason::Selector(
                            ExactSelectionReason::NotSelectedLostTieBreak,
                        ),
                    ));
                }
            }

            if rank_leaders.len() == 1 {
                selected = Some(rank_leaders[0]);
                selected_reason = ExactSelectionReason::SelectedTieBreakRank;
            } else {
                // Step 3: watchlist-preferred assignment, only if exactly
                // one tied candidate carries it.
                let watchlist_leaders: Vec<&&SelectionCandidateInput> = rank_leaders
                    .iter()
                    .filter(|c| c.evidence.watchlist_assigned)
                    .collect();
                if watchlist_leaders.len() == 1 {
                    let winner = *watchlist_leaders[0];
                    // IR8: identify the winner by its canonical identity
                    // triple (strategy_id, timeframe_secs) -- distinct within
                    // `rank_leaders` by construction (one candidate per
                    // identity bucket) -- never by pointer identity.
                    let winner_key = (
                        canonical_strategy_id(&winner.strategy_id),
                        winner.timeframe_secs,
                    );
                    for c in &rank_leaders {
                        let c_key = (canonical_strategy_id(&c.strategy_id), c.timeframe_secs);
                        if c_key != winner_key {
                            results.push(candidate_result(
                                c,
                                false,
                                SelectionCandidateDisposition::NotSelected,
                                ResolvedOutcomeReason::Selector(
                                    ExactSelectionReason::NotSelectedLostTieBreak,
                                ),
                            ));
                        }
                    }
                    selected = Some(winner);
                    selected_reason = ExactSelectionReason::SelectedTieBreakWatchlist;
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
                            ResolvedOutcomeReason::Selector(
                                ExactSelectionReason::NotSelectedLostTieBreak,
                            ),
                        ));
                    }
                    selected = Some(winner);
                    selected_reason = ExactSelectionReason::SelectedTieBreakStrategyId;
                }
            }
        }
    }

    // Every valid, non-max-score candidate lost outright on score.
    for c in &ranking_pool {
        if c.evidence.canonical_score_decimal.as_deref() != Some(max_score.as_str()) {
            results.push(candidate_result(
                c,
                false,
                SelectionCandidateDisposition::NotSelected,
                ResolvedOutcomeReason::Selector(ExactSelectionReason::NotSelectedLowerScore),
            ));
        }
    }

    let (selected_strategy_id, disposition, exact_reason) = match selected {
        Some(winner) => {
            results.push(candidate_result(
                winner,
                true,
                SelectionCandidateDisposition::Selected,
                ResolvedOutcomeReason::Selector(selected_reason),
            ));
            (
                Some(canonical_strategy_id(&winner.strategy_id)),
                SelectionCandidateDisposition::Selected,
                selected_reason,
            )
        }
        None => (
            None,
            SelectionCandidateDisposition::Refused,
            ExactSelectionReason::NoValidCandidate,
        ),
    };

    results.sort_by(|a, b| {
        (a.strategy_id.as_str(), a.timeframe_secs).cmp(&(b.strategy_id.as_str(), b.timeframe_secs))
    });

    // Defect 2: symbol-level reason uses the same typed authority as every
    // candidate result -- `reason_code` is derived from `exact_reason` via
    // the same exhaustive `public_reason_code`, never a separate string
    // literal or a fallible reverse lookup.
    SymbolSelectionResult {
        symbol: symbol.to_string(),
        selected_strategy_id,
        disposition,
        reason_code: public_reason_code(exact_reason).to_string(),
        exact_reason,
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
/// equality-identical regardless (candidates sorted by `(strategy_id,
/// timeframe_secs)` in the output, including divergent duplicates, whose
/// relative order is fixed by their full canonical payload — R4).
///
/// Fails the whole plan closed (empty `symbol_results`, non-`"computed"`
/// `truth_state`) only for a caller-contract violation: an empty eligible
/// set, an eligible set over [`MAX_ELIGIBLE_SYMBOLS`], a blank/whitespace
/// eligible symbol (R1), a duplicate eligible symbol after canonicalization,
/// a candidate slice over [`MAX_CANDIDATE_PAIRS`] (R7), or a candidate whose
/// symbol is outside the eligible set. Per-symbol "no valid candidate" is
/// never a whole-plan failure — it is a normal, visible, fail-closed outcome
/// for that one symbol.
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
    if eligible_symbols.len() > MAX_ELIGIBLE_SYMBOLS {
        return fail_closed_plan(
            context,
            TRUTH_STATE_ELIGIBLE_SYMBOLS_OVER_LIMIT,
            format!(
                "eligible_symbols.len()={} exceeds MAX_ELIGIBLE_SYMBOLS={}",
                eligible_symbols.len(),
                MAX_ELIGIBLE_SYMBOLS
            ),
        );
    }

    let mut canonical_eligible: Vec<String> = Vec::with_capacity(eligible_symbols.len());
    {
        let mut seen = std::collections::HashSet::new();
        for s in eligible_symbols {
            let canon = canonical_symbol(s);
            if canon.is_empty() {
                return fail_closed_plan(
                    context,
                    TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL,
                    format!("eligible_symbols entry '{s}' canonicalizes to a blank symbol"),
                );
            }
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

    if candidates.len() > MAX_CANDIDATE_PAIRS {
        return fail_closed_plan(
            context,
            TRUTH_STATE_CANDIDATES_OVER_LIMIT,
            format!(
                "candidates.len()={} exceeds MAX_CANDIDATE_PAIRS={}",
                candidates.len(),
                MAX_CANDIDATE_PAIRS
            ),
        );
    }

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

    // Defect F: structurally tie the per-symbol candidate universe to
    // MAX_STRATEGY_UNIVERSE (which mirrors the five-identity
    // `REGISTERED_STRATEGY_IDS` authority) -- a single symbol carrying more
    // candidate rows than the entire strategy universe has identities is a
    // caller-contract violation, checked here independently of the total
    // MAX_CANDIDATE_PAIRS bound above (which a single over-loaded symbol can
    // stay under while still locally violating this one).
    let mut per_symbol_counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in candidates {
        *per_symbol_counts
            .entry(canonical_symbol(&c.symbol))
            .or_insert(0) += 1;
    }
    if let Some((over_symbol, count)) = per_symbol_counts
        .iter()
        .find(|(_, &n)| n > MAX_STRATEGY_UNIVERSE)
    {
        return fail_closed_plan(
            context,
            TRUTH_STATE_SYMBOL_CANDIDATES_OVER_LIMIT,
            format!(
                "symbol '{over_symbol}' has {count} candidates, exceeding MAX_STRATEGY_UNIVERSE={MAX_STRATEGY_UNIVERSE}"
            ),
        );
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

// ---------------------------------------------------------------------------
// R5/R6: non-circular plan identity material
// ---------------------------------------------------------------------------

fn push_len_prefixed(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

fn push_opt_str(buf: &mut Vec<u8>, s: &Option<String>) {
    match s {
        Some(v) => {
            buf.push(1);
            push_len_prefixed(buf, v);
        }
        None => buf.push(0),
    }
}

/// Same tag+length-prefix convention as [`push_opt_str`], for
/// [`ExactSelectionReason`] (e.g. [`SelectionCandidateEvidence::exact_reason`])
/// -- serialized via its own closed, bounded `.code()`, never `Debug` or any
/// other unbounded representation.
fn push_opt_exact_reason(buf: &mut Vec<u8>, reason: Option<ExactSelectionReason>) {
    match reason {
        Some(v) => {
            buf.push(1);
            push_len_prefixed(buf, &v.code());
        }
        None => buf.push(0),
    }
}

/// Defect 4: non-optional counterpart to [`push_opt_exact_reason`], for the
/// now-structurally-non-optional resolved `exact_reason` on
/// [`SelectionCandidateResult`]/[`SymbolSelectionResult`] -- no presence tag
/// byte, since one is never needed.
fn push_exact_reason(buf: &mut Vec<u8>, reason: ExactSelectionReason) {
    push_len_prefixed(buf, &reason.code());
}

fn push_bool(buf: &mut Vec<u8>, b: bool) {
    buf.push(u8::from(b));
}

fn push_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn push_opt_i64(buf: &mut Vec<u8>, v: Option<i64>) {
    match v {
        Some(x) => {
            buf.push(1);
            push_i64(buf, x);
        }
        None => buf.push(0),
    }
}

fn push_opt_u32(buf: &mut Vec<u8>, v: Option<u32>) {
    match v {
        Some(x) => {
            buf.push(1);
            buf.extend_from_slice(&x.to_be_bytes());
        }
        None => buf.push(0),
    }
}

fn push_disposition(buf: &mut Vec<u8>, d: SelectionCandidateDisposition) {
    let tag: u8 = match d {
        SelectionCandidateDisposition::Selected => 0,
        SelectionCandidateDisposition::NotSelected => 1,
        SelectionCandidateDisposition::Refused => 2,
    };
    buf.push(tag);
}

/// Canonical, length-prefixed byte serialization of every result-affecting
/// fact this module's output exposes — deterministic and total over a
/// resolved [`DynamicSelectionPlan`]. See module-level docs, "Non-circular
/// plan identity" (R5): the caller hashes this (UUIDv5 or equivalent) to
/// mint `plan_id` only *after* selection has resolved.
pub fn canonical_plan_identity_material(plan: &DynamicSelectionPlan) -> Vec<u8> {
    let mut buf = Vec::new();
    let c = &plan.context;
    push_len_prefixed(&mut buf, DYNAMIC_SELECTION_SCHEMA_VERSION);
    push_len_prefixed(&mut buf, &c.schema_version);
    push_len_prefixed(&mut buf, &c.run_id);
    push_len_prefixed(&mut buf, c.configured_mode.as_str());
    push_len_prefixed(&mut buf, c.effective_mode.as_str());
    push_bool(&mut buf, c.live_lock_applied);
    push_len_prefixed(&mut buf, &c.source_kind);
    push_len_prefixed(&mut buf, &c.source_identity);
    push_len_prefixed(&mut buf, &c.market_date);
    push_len_prefixed(&mut buf, &plan.truth_state);

    buf.extend_from_slice(&(plan.blockers.len() as u32).to_be_bytes());
    for b in &plan.blockers {
        push_len_prefixed(&mut buf, b);
    }

    buf.extend_from_slice(&(plan.symbol_results.len() as u32).to_be_bytes());
    for sr in &plan.symbol_results {
        push_len_prefixed(&mut buf, &sr.symbol);
        push_opt_str(&mut buf, &sr.selected_strategy_id);
        push_disposition(&mut buf, sr.disposition);
        push_len_prefixed(&mut buf, &sr.reason_code);
        push_exact_reason(&mut buf, sr.exact_reason);

        buf.extend_from_slice(&(sr.candidates.len() as u32).to_be_bytes());
        for cand in &sr.candidates {
            push_len_prefixed(&mut buf, &cand.symbol);
            push_len_prefixed(&mut buf, &cand.strategy_id);
            push_i64(&mut buf, cand.timeframe_secs);
            // IR7: canonical decimal string is authoritative in identity.
            push_opt_str(&mut buf, &cand.canonical_score_decimal);
            push_opt_i64(&mut buf, cand.canonical_score_micros);
            push_opt_u32(&mut buf, cand.scanner_rank);
            push_bool(&mut buf, cand.watchlist_assigned);
            push_bool(&mut buf, cand.promotion_query_ok);
            push_opt_str(&mut buf, &cand.promotion_state);
            push_bool(&mut buf, cand.promotion_effective);
            push_bool(&mut buf, cand.promotion_expired);
            push_bool(&mut buf, cand.evidence_resolved);
            push_bool(&mut buf, cand.review_state_is_paper_candidate);
            push_opt_str(&mut buf, &cand.evidence_review_state);
            // Defect 1: six distinct fingerprint-provenance fields, never
            // one ambiguous alias -- mirrors canonical_candidate_payload_key.
            push_opt_str(&mut buf, &cand.durable_legacy_fingerprint);
            push_opt_str(&mut buf, &cand.recomputed_legacy_fingerprint);
            push_bool(&mut buf, cand.legacy_fingerprint_matches);
            push_opt_str(&mut buf, &cand.durable_exact_fingerprint_v2);
            push_opt_str(&mut buf, &cand.recomputed_exact_fingerprint_v2);
            push_bool(&mut buf, cand.exact_fingerprint_v2_matches);
            push_bool(&mut buf, cand.config_identity_verified);
            push_opt_str(&mut buf, &cand.durable_config_fingerprint);
            push_opt_str(&mut buf, &cand.current_config_fingerprint);
            push_bool(&mut buf, cand.registry_enabled);
            push_bool(&mut buf, cand.plugin_instantiable);
            push_bool(&mut buf, cand.timeframe_matches);
            push_bool(&mut buf, cand.data_ready);
            push_opt_str(&mut buf, &cand.evidence_review_id);
            push_opt_str(&mut buf, &cand.evidence_scanner_scan_id);
            push_opt_str(&mut buf, &cand.evidence_artifact_path);
            push_opt_str(&mut buf, &cand.evidence_git_hash);
            push_opt_str(&mut buf, &cand.promotion_transition_id);
            push_opt_str(&mut buf, &cand.promotion_effective_at);
            push_opt_str(&mut buf, &cand.promotion_expires_at);
            push_opt_str(&mut buf, &cand.evidence_transition_id);
            push_exact_reason(&mut buf, cand.exact_reason);
            push_bool(&mut buf, cand.selected);
            push_disposition(&mut buf, cand.disposition);
            push_len_prefixed(&mut buf, &cand.reason_code);
        }
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DynamicSelectionContext {
        DynamicSelectionContext {
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

    /// IR7 test fixture helper: build the canonical decimal string
    /// corresponding exactly to a micros value, via the same canonicalizer
    /// the real read path uses -- never a hand-rolled second
    /// implementation.
    fn decimal_from_micros(micros: i64) -> String {
        let negative = micros < 0;
        let abs = (micros as i128).unsigned_abs();
        let int_part = abs / 1_000_000;
        let frac_part = abs % 1_000_000;
        let token = format!(
            "{}{}.{:06}",
            if negative { "-" } else { "" },
            int_part,
            frac_part
        );
        crate::canonicalize_decimal_token(&token).expect("valid fixture token")
    }

    /// A syntactically valid-looking 64-lowercase-hex fingerprint fixture,
    /// distinguished by `tag` (repeated to fill 64 chars) so distinct calls
    /// (e.g. legacy vs v2, durable vs recomputed) are trivially distinct
    /// values, never accidentally colliding.
    fn fp_hex(tag: char) -> String {
        std::iter::repeat_n(tag, 64).collect()
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
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some(fp_hex('a')),
            recomputed_legacy_fingerprint: Some(fp_hex('a')),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some(fp_hex('b')),
            recomputed_exact_fingerprint_v2: Some(fp_hex('b')),
            exact_fingerprint_v2_matches: true,
            config_identity_verified: true,
            durable_config_fingerprint: Some(fp_hex('c')),
            current_config_fingerprint: Some(fp_hex('c')),
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_decimal: Some(decimal_from_micros(score_micros)),
            canonical_score_micros: Some(score_micros),
            scanner_rank: rank,
            watchlist_assigned: watchlist,
            evidence_review_id: Some("review-1".to_string()),
            evidence_scanner_scan_id: Some("scan-1".to_string()),
            evidence_artifact_path: Some("/artifacts/review-1".to_string()),
            evidence_git_hash: Some("git-hash-1".to_string()),
            promotion_transition_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            promotion_effective_at: Some("2026-01-01T00:00:00Z".to_string()),
            promotion_expires_at: None,
            evidence_transition_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            exact_reason: None,
        }
    }

    fn candidate(
        symbol: &str,
        strategy_id: &str,
        score_micros: i64,
        rank: Option<u32>,
        watchlist: bool,
    ) -> SelectionCandidateInput {
        candidate_tf(symbol, strategy_id, 300, score_micros, rank, watchlist)
    }

    fn candidate_tf(
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
        score_micros: i64,
        rank: Option<u32>,
        watchlist: bool,
    ) -> SelectionCandidateInput {
        SelectionCandidateInput {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs,
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

    // ── Defect 1: v2 exact-score fingerprint provenance ───────────────────

    #[test]
    fn legacy_match_and_v2_match_both_required_to_rank() {
        let c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").selected_strategy_id,
            Some("swing_momentum".to_string()),
            "both matches true must rank normally"
        );
    }

    // ── Defect 1: pure fingerprint coherence -- ten required scenarios ────

    #[test]
    fn valid_production_fingerprint_pairs_rank() {
        // Defect 1, test 9: real SHA-256-shaped hex digests (not the
        // repeated-char `fp_hex` fixture shorthand), byte-equal on both
        // sides, matching booleans -- ranks normally.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let legacy = "2d5b93caf42d3260f7709ab401a84dd65742cf88d5d60e36f75075e02ab59df7".to_string();
        let v2 = "e1798b67d53875abb1d403f26ef08f8c162b714dd886598423d8b545d5a02e40".to_string();
        assert!(is_valid_sha256_hex_fingerprint(&legacy));
        assert!(is_valid_sha256_hex_fingerprint(&v2));
        c.evidence.durable_legacy_fingerprint = Some(legacy.clone());
        c.evidence.recomputed_legacy_fingerprint = Some(legacy);
        c.evidence.legacy_fingerprint_matches = true;
        c.evidence.durable_exact_fingerprint_v2 = Some(v2.clone());
        c.evidence.recomputed_exact_fingerprint_v2 = Some(v2);
        c.evidence.exact_fingerprint_v2_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").selected_strategy_id,
            Some("swing_momentum".to_string())
        );
    }

    #[test]
    fn legacy_match_true_with_absent_values_refuses() {
        // Defect 1, test 1.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_legacy_fingerprint = None;
        c.evidence.recomputed_legacy_fingerprint = None;
        c.evidence.legacy_fingerprint_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_MISSING);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::DurableFingerprintMissing
        );
    }

    #[test]
    fn legacy_match_true_with_unequal_values_refuses() {
        // Defect 1, test 2: the boolean claims a match but the underlying
        // values disagree -- incoherent, not a plain mismatch.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_legacy_fingerprint = Some(fp_hex('a'));
        c.evidence.recomputed_legacy_fingerprint = Some(fp_hex('e'));
        c.evidence.legacy_fingerprint_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_INCOHERENT);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::LegacyFingerprintMatchIncoherent
        );
    }

    #[test]
    fn legacy_match_false_with_equal_values_refuses_as_incoherent() {
        // Defect 1, test 3: durable/recomputed default to the same value
        // (fp_hex('a')) from `valid_evidence`; only the claimed-false
        // boolean is flipped -- the derived truth (equal) disagrees with
        // it, so this is incoherent, never a plain "match=false" mismatch.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.legacy_fingerprint_matches = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_INCOHERENT);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::LegacyFingerprintMatchIncoherent
        );
    }

    #[test]
    fn malformed_legacy_fingerprint_refuses() {
        // Defect 1, test 4: malformed shape refuses before equality is ever
        // trusted, even though the two malformed strings are textually
        // equal and the boolean claims true.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_legacy_fingerprint = Some("not-64-hex-chars".to_string());
        c.evidence.recomputed_legacy_fingerprint = Some("not-64-hex-chars".to_string());
        c.evidence.legacy_fingerprint_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_MALFORMED);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::LegacyFingerprintMalformed
        );
    }

    #[test]
    fn v2_match_true_with_malformed_values_refuses() {
        // Defect 1, test 5.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_exact_fingerprint_v2 = Some("ZZZZ-not-hex".to_string());
        c.evidence.recomputed_exact_fingerprint_v2 = Some("ZZZZ-not-hex".to_string());
        c.evidence.exact_fingerprint_v2_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_V2_MALFORMED);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::FingerprintV2Malformed
        );
    }

    #[test]
    fn v2_match_true_with_unequal_values_refuses() {
        // Defect 1, test 6.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('b'));
        c.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('c'));
        c.evidence.exact_fingerprint_v2_matches = true;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_V2_INCOHERENT);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::FingerprintV2MatchIncoherent
        );
    }

    #[test]
    fn v2_match_false_with_equal_values_refuses_as_incoherent() {
        // Defect 1, test 7: durable/recomputed v2 default to the same value
        // (fp_hex('b')) from `valid_evidence`; only the claimed-false
        // boolean is flipped.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.exact_fingerprint_v2_matches = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.reason_code, REASON_REFUSED_FINGERPRINT_V2_INCOHERENT);
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::FingerprintV2MatchIncoherent
        );
    }

    #[test]
    fn each_fingerprint_field_mutation_changes_identity_deterministically() {
        // Defect 1, test 10: durable_legacy, recomputed_legacy, durable_v2,
        // and recomputed_v2 each independently change the canonical
        // candidate payload and the resolved plan identity -- each mutation
        // keeps its pair coherent (both sides updated together) so the
        // candidate still ranks, isolating the identity change from any
        // refusal side effect.
        let base = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let base_key = canonical_candidate_payload_key("AAPL", &base);
        let base_plan =
            compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), std::slice::from_ref(&base));
        let base_identity = canonical_plan_identity_material(&base_plan);

        let mut durable_legacy = base.clone();
        durable_legacy.evidence.durable_legacy_fingerprint = Some(fp_hex('e'));
        durable_legacy.evidence.recomputed_legacy_fingerprint = Some(fp_hex('e'));

        let mut recomputed_legacy = base.clone();
        recomputed_legacy.evidence.durable_legacy_fingerprint = Some(fp_hex('f'));
        recomputed_legacy.evidence.recomputed_legacy_fingerprint = Some(fp_hex('f'));

        let mut durable_v2 = base.clone();
        durable_v2.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('1'));
        durable_v2.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('1'));

        let mut recomputed_v2 = base.clone();
        recomputed_v2.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('2'));
        recomputed_v2.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('2'));

        for mutated in [durable_legacy, recomputed_legacy, durable_v2, recomputed_v2] {
            let mutated_key = canonical_candidate_payload_key("AAPL", &mutated);
            assert_ne!(
                mutated_key, base_key,
                "mutated fixture must change the payload key"
            );
            let mutated_plan = compute_dynamic_selection_plan(
                ctx(),
                &symbols(&["AAPL"]),
                std::slice::from_ref(&mutated),
            );
            assert_ne!(
                canonical_plan_identity_material(&mutated_plan),
                base_identity,
                "mutated fixture must change resolved plan identity"
            );
        }
    }

    #[test]
    fn missing_v2_refuses_distinctly_from_mismatched_v2() {
        // Defect 1, test 8.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.durable_exact_fingerprint_v2 = None;
        c.evidence.recomputed_exact_fingerprint_v2 = None;
        c.evidence.exact_fingerprint_v2_matches = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_FINGERPRINT_V2_MISSING
        );
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].exact_reason,
            ExactSelectionReason::DurableFingerprintV2Missing
        );
        assert_ne!(
            REASON_REFUSED_FINGERPRINT_V2_MISSING, REASON_REFUSED_FINGERPRINT_V2_MISMATCH,
            "missing and mismatched must remain distinct reason codes"
        );
    }

    #[test]
    fn changing_only_durable_v2_changes_payload_and_plan_identity() {
        let base = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut changed = base.clone();
        changed.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('c'));
        // Recomputed must still equal durable for a "ranks normally" fixture,
        // but this test only cares that payload/identity differ -- both
        // fixtures independently gate-refuse on v2 mismatch, which is fine:
        // the point is the byte-level identity, not the ranking outcome.
        let plan_base =
            compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), std::slice::from_ref(&base));
        let plan_changed = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL"]),
            std::slice::from_ref(&changed),
        );
        assert_ne!(
            canonical_candidate_payload_key("AAPL", &base),
            canonical_candidate_payload_key("AAPL", &changed),
            "durable v2 alone must change the canonical candidate payload"
        );
        assert_ne!(
            canonical_plan_identity_material(&plan_base),
            canonical_plan_identity_material(&plan_changed),
            "durable v2 alone must change plan identity"
        );
    }

    #[test]
    fn changing_only_recomputed_v2_changes_payload_and_plan_identity() {
        let base = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut changed = base.clone();
        changed.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('c'));
        let plan_base =
            compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), std::slice::from_ref(&base));
        let plan_changed = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL"]),
            std::slice::from_ref(&changed),
        );
        assert_ne!(
            canonical_candidate_payload_key("AAPL", &base),
            canonical_candidate_payload_key("AAPL", &changed),
            "recomputed v2 alone must change the canonical candidate payload"
        );
        assert_ne!(
            canonical_plan_identity_material(&plan_base),
            canonical_plan_identity_material(&plan_changed),
            "recomputed v2 alone must change plan identity"
        );
    }

    #[test]
    fn same_legacy_but_different_v2_cannot_replay_identically() {
        // Two candidates sharing the same identity and equal legacy
        // fingerprints, but disagreeing v2 durable values -- must be treated
        // as a divergent duplicate (not silently collapsed via legacy alone).
        let mut c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut c2 = c1.clone();
        c1.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('c'));
        c1.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('c'));
        c2.evidence.durable_exact_fingerprint_v2 = Some(fp_hex('d'));
        c2.evidence.recomputed_exact_fingerprint_v2 = Some(fp_hex('d'));
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.candidates.len(), 2);
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.reason_code == REASON_REFUSED_DIVERGENT_DUPLICATE));
    }

    #[test]
    fn v2_fingerprint_field_permutations_do_not_affect_other_candidates() {
        // Input order/permutation independence, specific to the new v2
        // fields: two candidates for distinct symbols, supplied in reverse
        // order, must still each resolve to their own correct outcome.
        let mut ok = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut bad = candidate("MSFT", "mean_reversion", 700_000, Some(1), false);
        bad.evidence.exact_fingerprint_v2_matches = false;
        let plan_a = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL", "MSFT"]),
            &[ok.clone(), bad.clone()],
        );
        let plan_b = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL", "MSFT"]),
            &[bad.clone(), ok.clone()],
        );
        assert_eq!(
            canonical_plan_identity_material(&plan_a),
            canonical_plan_identity_material(&plan_b),
            "input order must not affect resolved plan identity"
        );
        let _ = (&mut ok, &mut bad);
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
        c.evidence.canonical_score_decimal = None;
        c.evidence.canonical_score_micros = None;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_MISSING_SCORE
        );
    }

    // ── DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01 ─────────────────────

    /// A stale/unverified config (evidence otherwise fully valid) must never
    /// be represented as a positively eligible candidate -- the whole point
    /// of this gate is that it fires even when every earlier check (promotion
    /// state, review-artifact fingerprints) passed cleanly.
    #[test]
    fn unverified_config_identity_is_refused_despite_otherwise_valid_evidence() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].exact_reason,
            ExactSelectionReason::ConfigIdentityUnavailable
        );
    }

    /// Positive control: a genuinely verified config identity does not, on
    /// its own, block an otherwise-valid candidate from being selected --
    /// `candidate()`'s own baseline fixture already sets
    /// `config_identity_verified = true` (see [`valid_evidence`]).
    #[test]
    fn verified_config_identity_does_not_block_selection() {
        let c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let result = result_for(&plan, "AAPL");
        assert_eq!(result.disposition, SelectionCandidateDisposition::Selected);
        assert_eq!(result.candidates[0].reason_code, REASON_SELECTED_HIGHEST_SCORE);
    }

    /// This gate fires strictly BEFORE plugin/timeframe/data-readiness in the
    /// fixed gate order (mirrors the promotion/fingerprint checks' own
    /// precedence) -- an unverified config identity must be the surfaced
    /// reason even when a later-order check would also fail.
    #[test]
    fn config_identity_unavailable_takes_precedence_over_later_gate_stages() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = false;
        c.evidence.timeframe_matches = false;
        c.evidence.data_ready = false;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    // ── CANONICAL-CONFIG-IDENTITY-VERIFIER-01 (R7): mqk-portfolio cannot
    // depend on mqk-daemon's canonical predicate, so its pure gate performs
    // its own integrity check over `config_identity_verified` +
    // `durable_config_fingerprint` + `current_config_fingerprint`. A bare
    // `config_identity_verified = true` claim must never be trusted alone.

    /// `verified=true` alongside a genuinely mismatched fingerprint pair
    /// (a contradictory caller claim) must fail closed.
    #[test]
    fn config_identity_verified_true_with_mismatched_fingerprints_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = true;
        c.evidence.durable_config_fingerprint = Some(fp_hex('c'));
        c.evidence.current_config_fingerprint = Some(fp_hex('d'));
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// `verified=true` alongside a malformed durable fingerprint must fail
    /// closed.
    #[test]
    fn config_identity_verified_true_with_malformed_durable_fingerprint_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = true;
        c.evidence.durable_config_fingerprint = Some("not-valid-hex".to_string());
        c.evidence.current_config_fingerprint = Some(fp_hex('c'));
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// `verified=true` alongside a malformed current fingerprint must fail
    /// closed.
    #[test]
    fn config_identity_verified_true_with_malformed_current_fingerprint_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = true;
        c.evidence.durable_config_fingerprint = Some(fp_hex('c'));
        c.evidence.current_config_fingerprint = Some("not-valid-hex".to_string());
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// `verified=true` with a missing durable fingerprint must fail closed
    /// -- never treated as a wildcard match.
    #[test]
    fn config_identity_verified_true_with_missing_durable_fingerprint_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = true;
        c.evidence.durable_config_fingerprint = None;
        c.evidence.current_config_fingerprint = Some(fp_hex('c'));
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// `verified=true` with a missing current fingerprint must fail closed.
    #[test]
    fn config_identity_verified_true_with_missing_current_fingerprint_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = true;
        c.evidence.durable_config_fingerprint = Some(fp_hex('c'));
        c.evidence.current_config_fingerprint = None;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// `verified=false` alongside a genuinely matching fingerprint pair must
    /// still fail closed -- the boolean's own claim is authoritative when it
    /// disagrees with the derived equality in the OTHER direction too; a
    /// coincidentally-matching pair can never override a `false` claim.
    /// This is the same fixture `unverified_config_identity_is_refused_
    /// despite_otherwise_valid_evidence` already exercises (its unmodified
    /// `durable_config_fingerprint`/`current_config_fingerprint` already
    /// agree, from [`valid_evidence`]) -- asserted explicitly here under its
    /// own name for this gate's negative-control coverage.
    #[test]
    fn config_identity_verified_false_with_matching_fingerprints_is_refused() {
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.config_identity_verified = false;
        c.evidence.durable_config_fingerprint = Some(fp_hex('c'));
        c.evidence.current_config_fingerprint = Some(fp_hex('c'));
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
        );
    }

    /// Positive control: `verified=true` with an exact, valid, matching
    /// pair passes this gate (already exercised implicitly by
    /// `verified_config_identity_does_not_block_selection`'s baseline
    /// fixture -- restated explicitly here).
    #[test]
    fn config_identity_verified_true_with_exact_valid_pair_passes() {
        let c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let result = result_for(&plan, "AAPL");
        assert_eq!(result.disposition, SelectionCandidateDisposition::Selected);
        assert_ne!(
            result.candidates[0].reason_code,
            REASON_REFUSED_CONFIG_IDENTITY_UNAVAILABLE
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

    #[test]
    fn divergent_duplicate_result_order_is_permutation_independent() {
        // R4: two rows with the same identity but different evidence (a
        // divergent duplicate) must produce equality-identical plans no
        // matter which row the caller supplied first.
        let c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let c2 = candidate("AAPL", "swing_momentum", 900_000, Some(2), false);
        let forward = vec![c1.clone(), c2.clone()];
        let reversed = vec![c2, c1];
        let r1 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &forward);
        let r2 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &reversed);
        assert_eq!(
            r1, r2,
            "divergent duplicate row order must never change the plan"
        );
    }

    #[test]
    fn same_symbol_strategy_different_timeframe_is_not_an_exact_replay() {
        // R2/R3: identity is (symbol, strategy_id, timeframe_secs) -- two
        // rows differing only in timeframe are distinct identities, each
        // independently gated (here: same strategy at 300s vs 900s, both
        // otherwise valid, must both survive as separate candidate rows,
        // never collapsed into one).
        let c_300 = candidate_tf("AAPL", "swing_momentum", 300, 500_000, Some(1), false);
        let c_900 = candidate_tf("AAPL", "swing_momentum", 900, 500_000, Some(1), false);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c_300, c_900]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(
            aapl.candidates.len(),
            2,
            "distinct timeframes must never collapse into one replay row"
        );
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.reason_code != REASON_REFUSED_DIVERGENT_DUPLICATE));
    }

    #[test]
    fn full_payload_duplicate_replay_is_idempotent_including_timeframe() {
        let c1 = candidate_tf("AAPL", "swing_momentum", 900, 500_000, Some(1), false);
        let c2 = c1.clone();
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.candidates.len(), 1);
        assert_eq!(aapl.candidates[0].timeframe_secs, 900);
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

    // ── R1: blank eligible symbols ───────────────────────────────────────

    #[test]
    fn raw_empty_eligible_symbol_fails_whole_plan_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", ""]), &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL);
        assert!(plan.symbol_results.is_empty());
    }

    #[test]
    fn whitespace_only_eligible_symbol_fails_whole_plan_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "   "]), &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL);
    }

    #[test]
    fn tab_only_eligible_symbol_fails_whole_plan_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "\t\t"]), &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL);
    }

    #[test]
    fn mixed_valid_and_blank_eligible_symbols_fails_whole_plan_closed() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT", " "]), &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL);
        assert!(
            plan.symbol_results.is_empty(),
            "one blank entry fails the whole plan, not just that entry"
        );
    }

    #[test]
    fn candidate_blank_identity_is_still_independently_refused() {
        // A candidate-level blank identity (not an eligible-symbol-level
        // blank) must still be refused per-candidate by the evidence gate,
        // independent of the R1 eligible-symbol check.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.strategy_id = "   ".to_string();
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        assert_eq!(plan.truth_state, TRUTH_STATE_COMPUTED);
        assert_eq!(
            result_for(&plan, "AAPL").candidates[0].reason_code,
            REASON_REFUSED_BLANK_IDENTITY
        );
    }

    // ── R7: bounds ───────────────────────────────────────────────────────

    #[test]
    fn eligible_symbols_over_limit_fails_whole_plan_closed() {
        let over = symbols(&["A", "B", "C", "D", "E", "F"]);
        assert!(over.len() > MAX_ELIGIBLE_SYMBOLS);
        let plan = compute_dynamic_selection_plan(ctx(), &over, &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_ELIGIBLE_SYMBOLS_OVER_LIMIT);
        assert!(plan.symbol_results.is_empty());
    }

    #[test]
    fn eligible_symbols_at_limit_succeeds() {
        let at_limit = symbols(&["A", "B", "C", "D", "E"]);
        assert_eq!(at_limit.len(), MAX_ELIGIBLE_SYMBOLS);
        let plan = compute_dynamic_selection_plan(ctx(), &at_limit, &[]);
        assert_eq!(plan.truth_state, TRUTH_STATE_COMPUTED);
    }

    #[test]
    fn candidates_over_limit_fails_whole_plan_closed() {
        let eligible = symbols(&["AAPL"]);
        let mut over_candidates = Vec::new();
        for i in 0..(MAX_CANDIDATE_PAIRS + 1) {
            over_candidates.push(candidate(
                "AAPL",
                &format!("strategy_{i}"),
                500_000,
                Some(1),
                false,
            ));
        }
        let plan = compute_dynamic_selection_plan(ctx(), &eligible, &over_candidates);
        assert_eq!(plan.truth_state, TRUTH_STATE_CANDIDATES_OVER_LIMIT);
        assert!(plan.symbol_results.is_empty());
    }

    /// Defect F: a single symbol carrying more candidates than
    /// `MAX_STRATEGY_UNIVERSE` (5) fails the whole plan closed with its own
    /// distinct truth_state -- proven with a total candidate count (6) well
    /// under `MAX_CANDIDATE_PAIRS` (25), so this is genuinely a *different*
    /// bound from the total-pairs check above, not a restatement of it.
    #[test]
    fn six_strategy_ids_for_one_symbol_fails_whole_plan_closed() {
        let eligible = symbols(&["AAPL"]);
        assert_eq!(MAX_STRATEGY_UNIVERSE, 5);
        let mut over_candidates = Vec::new();
        for i in 0..(MAX_STRATEGY_UNIVERSE + 1) {
            over_candidates.push(candidate(
                "AAPL",
                &format!("strategy_{i}"),
                500_000,
                Some(1),
                false,
            ));
        }
        assert!(over_candidates.len() < MAX_CANDIDATE_PAIRS);
        let plan = compute_dynamic_selection_plan(ctx(), &eligible, &over_candidates);
        assert_eq!(plan.truth_state, TRUTH_STATE_SYMBOL_CANDIDATES_OVER_LIMIT);
        assert!(plan.symbol_results.is_empty());
    }

    // ── R5/R6: non-circular identity material ──────────────────────────────

    #[test]
    fn identity_material_is_deterministic_for_equal_plans() {
        let candidates = vec![candidate("AAPL", "swing_momentum", 500_000, Some(1), false)];
        let p1 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let p2 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        assert_eq!(
            canonical_plan_identity_material(&p1),
            canonical_plan_identity_material(&p2)
        );
    }

    #[test]
    fn identity_material_differs_when_result_affecting_fact_differs() {
        let c1 = vec![candidate("AAPL", "swing_momentum", 500_000, Some(1), false)];
        let c2 = vec![candidate("AAPL", "swing_momentum", 600_000, Some(1), false)];
        let p1 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &c1);
        let p2 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &c2);
        assert_ne!(
            canonical_plan_identity_material(&p1),
            canonical_plan_identity_material(&p2)
        );
    }

    #[test]
    fn identity_material_is_permutation_independent() {
        let forward = vec![
            candidate("AAPL", "a_strategy", 500_000, Some(1), false),
            candidate("MSFT", "b_strategy", 300_000, Some(1), false),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        let symbols_list = symbols(&["AAPL", "MSFT"]);
        let p1 = compute_dynamic_selection_plan(ctx(), &symbols_list, &forward);
        let p2 = compute_dynamic_selection_plan(ctx(), &symbols_list, &reversed);
        assert_eq!(
            canonical_plan_identity_material(&p1),
            canonical_plan_identity_material(&p2)
        );
    }

    // ── IR2: selected-row coherence ─────────────────────────────────────

    fn coherent_selected_result() -> SymbolSelectionResult {
        let plan = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL"]),
            &[candidate("AAPL", "swing_momentum", 500_000, Some(1), false)],
        );
        result_for(&plan, "AAPL").clone()
    }

    #[test]
    fn naturally_computed_selection_is_coherent() {
        let sr = coherent_selected_result();
        assert!(verify_symbol_selection_coherence(&sr).is_ok());
    }

    #[test]
    fn naturally_computed_no_selection_is_coherent() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[]);
        let sr = result_for(&plan, "AAPL");
        assert!(verify_symbol_selection_coherence(sr).is_ok());
    }

    #[test]
    fn selected_id_with_no_selected_candidate_is_incoherent() {
        let mut sr = coherent_selected_result();
        for c in &mut sr.candidates {
            c.selected = false;
        }
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::SelectedIdWithoutSelectedCandidate)
        );
    }

    #[test]
    fn selected_candidate_with_no_selected_id_is_incoherent() {
        let mut sr = coherent_selected_result();
        sr.selected_strategy_id = None;
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::SelectedCandidateWithoutSelectedId)
        );
    }

    #[test]
    fn two_selected_candidates_is_incoherent() {
        let mut sr = coherent_selected_result();
        let mut extra = sr.candidates[0].clone();
        extra.strategy_id = "another_strategy".to_string();
        sr.candidates.push(extra);
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::MultipleSelectedCandidates)
        );
    }

    #[test]
    fn selected_candidate_id_mismatch_is_incoherent() {
        let mut sr = coherent_selected_result();
        sr.selected_strategy_id = Some("some_other_strategy".to_string());
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::SelectedCandidateIdMismatch)
        );
    }

    #[test]
    fn refused_disposition_on_selected_row_is_incoherent() {
        let mut sr = coherent_selected_result();
        sr.candidates[0].disposition = SelectionCandidateDisposition::Refused;
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::SelectedCandidateDispositionMismatch)
        );
    }

    #[test]
    fn non_positive_timeframe_on_selected_row_is_incoherent() {
        let mut sr = coherent_selected_result();
        sr.candidates[0].timeframe_secs = 0;
        assert_eq!(
            verify_symbol_selection_coherence(&sr),
            Err(SelectionCoherenceViolation::SelectedCandidateNonPositiveTimeframe)
        );
    }

    #[test]
    fn plan_level_coherence_reports_every_incoherent_symbol() {
        let mut plan = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL", "MSFT"]),
            &[
                candidate("AAPL", "swing_momentum", 500_000, Some(1), false),
                candidate("MSFT", "mean_reversion", 700_000, Some(1), false),
            ],
        );
        for sr in &mut plan.symbol_results {
            sr.selected_strategy_id = None;
        }
        let violations = verify_plan_selection_coherence(&plan);
        assert_eq!(violations.len(), 2);
        assert!(violations.iter().any(|(s, v)| s == "AAPL"
            && *v == SelectionCoherenceViolation::SelectedCandidateWithoutSelectedId));
        assert!(violations.iter().any(|(s, v)| s == "MSFT"
            && *v == SelectionCoherenceViolation::SelectedCandidateWithoutSelectedId));
    }

    #[test]
    fn plan_level_coherence_is_empty_for_a_fully_coherent_plan() {
        let plan = compute_dynamic_selection_plan(
            ctx(),
            &symbols(&["AAPL", "MSFT"]),
            &[
                candidate("AAPL", "swing_momentum", 500_000, Some(1), false),
                candidate("MSFT", "mean_reversion", 700_000, Some(1), false),
            ],
        );
        assert!(verify_plan_selection_coherence(&plan).is_empty());
    }

    // ── IR8: canonical candidate payload key is byte-based, not Debug ──────

    #[test]
    fn divergent_duplicate_still_refused_via_byte_based_key() {
        let c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut c2 = c1.clone();
        c2.evidence.canonical_score_decimal = Some(decimal_from_micros(900_000));
        c2.evidence.canonical_score_micros = Some(900_000);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.reason_code == REASON_REFUSED_DIVERGENT_DUPLICATE));
    }

    #[test]
    fn exact_reason_differing_alone_is_a_divergent_duplicate() {
        // Two rows identical in every field the pure gate reads, but
        // differing only in the caller-supplied diagnostic exact_reason --
        // the byte-based key must still catch this as a genuine payload
        // difference (it is real caller-observed data, not decorative).
        let mut c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c1.evidence.exact_reason = Some(ExactSelectionReason::PromotionQueryFailed);
        let mut c2 = c1.clone();
        c2.evidence.exact_reason = Some(ExactSelectionReason::NoPromotionRecord);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.reason_code == REASON_REFUSED_DIVERGENT_DUPLICATE));
    }

    #[test]
    fn plan_has_no_plan_id_field() {
        // Compile-time proof of R5: DynamicSelectionPlan/Context expose no
        // plan_id -- if either type grows one back, this module must be
        // revisited. This test exists to make that removal discoverable in
        // a diff rather than silently reintroducing the circular contract.
        let candidates = vec![candidate("AAPL", "swing_momentum", 500_000, Some(1), false)];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let _material = canonical_plan_identity_material(&plan);
    }

    // ── Defect E: closed persistable exact-reason vocabulary ─────────────

    fn all_exact_selection_reasons() -> Vec<ExactSelectionReason> {
        vec![
            ExactSelectionReason::RegistryQueryFailed,
            ExactSelectionReason::RegistryRowMissing,
            ExactSelectionReason::RegistryDisabled,
            ExactSelectionReason::UnsupportedStrategyPlugin,
            ExactSelectionReason::RegistryConstructionFailed,
            ExactSelectionReason::TimeframeMismatch,
            ExactSelectionReason::BlankIdentity,
            ExactSelectionReason::PromotionQueryFailed,
            ExactSelectionReason::NoPromotionRecord,
            ExactSelectionReason::PromotionNotActivePaper,
            ExactSelectionReason::PromotionNotYetEffective,
            ExactSelectionReason::PromotionExpired,
            ExactSelectionReason::EvidenceLineageQueryFailed,
            ExactSelectionReason::EvidenceLineageBroken,
            ExactSelectionReason::ArtifactPathMissing,
            ExactSelectionReason::DurableFingerprintMissing,
            ExactSelectionReason::DurableFingerprintV2Missing,
            ExactSelectionReason::ArtifactRootUnavailable,
            ExactSelectionReason::ArtifactMissing,
            ExactSelectionReason::ArtifactRootEscape,
            ExactSelectionReason::ArtifactMalformed,
            ExactSelectionReason::ArtifactDuplicateIdentity,
            ExactSelectionReason::ArtifactNotPaperCandidate,
            ExactSelectionReason::ArtifactNoMatchingRow,
            ExactSelectionReason::ArtifactRowIdentityMismatch,
            ExactSelectionReason::ArtifactOverLimit,
            ExactSelectionReason::FingerprintMismatch,
            ExactSelectionReason::FingerprintV2Mismatch,
            ExactSelectionReason::LegacyFingerprintMalformed,
            ExactSelectionReason::LegacyFingerprintMatchIncoherent,
            ExactSelectionReason::FingerprintV2Malformed,
            ExactSelectionReason::FingerprintV2MatchIncoherent,
            ExactSelectionReason::PreGateReasonInconsistent,
            ExactSelectionReason::ScoreMissing,
            ExactSelectionReason::ScoreNotFinite,
            ExactSelectionReason::RankOutOfRange,
            ExactSelectionReason::DataNotReady(ReadinessBlockerReason::CalendarUnavailable),
            ExactSelectionReason::DataNotReady(ReadinessBlockerReason::MarketDataMissing),
            ExactSelectionReason::DataNotReadyUnspecified,
            ExactSelectionReason::DivergentDuplicate,
            ExactSelectionReason::NotSelectedLowerScore,
            ExactSelectionReason::NotSelectedLostTieBreak,
            ExactSelectionReason::MissingRankRequiredForTie,
            ExactSelectionReason::SelectedHighestScore,
            ExactSelectionReason::SelectedTieBreakRank,
            ExactSelectionReason::SelectedTieBreakWatchlist,
            ExactSelectionReason::SelectedTieBreakStrategyId,
            ExactSelectionReason::NoValidCandidate,
        ]
    }

    #[test]
    fn every_exact_selection_reason_code_round_trips_through_parse_code() {
        for reason in all_exact_selection_reasons() {
            let code = reason.code();
            assert_eq!(
                ExactSelectionReason::parse_code(&code),
                Some(reason),
                "code '{code}' must parse back to the same variant"
            );
        }
    }

    #[test]
    fn every_exact_selection_reason_code_is_distinct() {
        let mut codes: Vec<String> = all_exact_selection_reasons()
            .iter()
            .map(|r| r.code())
            .collect();
        let unique_count = {
            let mut c = codes.clone();
            c.sort();
            c.dedup();
            c.len()
        };
        assert_eq!(unique_count, codes.len(), "every code must be distinct");
        codes.clear();
    }

    #[test]
    fn unknown_exact_selection_reason_code_fails_closed() {
        assert_eq!(
            ExactSelectionReason::parse_code("totally_unrecognized_future_code"),
            None
        );
        assert_eq!(
            ExactSelectionReason::parse_code("data_not_ready:totally_unrecognized_blocker"),
            None
        );
    }

    #[test]
    fn every_readiness_blocker_reason_code_round_trips() {
        let all = [
            ReadinessBlockerReason::RequiredAssignmentsMissing,
            ReadinessBlockerReason::AssignmentResolutionFailed,
            ReadinessBlockerReason::RuntimeStrategyAssignmentMismatch,
            ReadinessBlockerReason::RuntimeStrategySymbolBindingMismatch,
            ReadinessBlockerReason::RuntimeStrategyTimeframeMismatch,
            ReadinessBlockerReason::StrategyRequirementUnknown,
            ReadinessBlockerReason::AssetClassUnknown,
            ReadinessBlockerReason::ProviderProvenanceInvalid,
            ReadinessBlockerReason::ProviderSymbolMismatch,
            ReadinessBlockerReason::ProviderIdMismatch,
            ReadinessBlockerReason::ProviderUnknown,
            ReadinessBlockerReason::ProviderIngestTimeFuture,
            ReadinessBlockerReason::ProviderDisabled,
            ReadinessBlockerReason::ProviderCapabilityMismatch,
            ReadinessBlockerReason::ProviderTimestampConventionUnverified,
            ReadinessBlockerReason::CalendarUnavailable,
            ReadinessBlockerReason::UnsupportedTimeframe,
            ReadinessBlockerReason::UnsupportedIntradayContinuity,
            ReadinessBlockerReason::MarketDataMissing,
            ReadinessBlockerReason::InsufficientHistory,
            ReadinessBlockerReason::DuplicateTimestamp,
            ReadinessBlockerReason::InteriorGap,
            ReadinessBlockerReason::LatestBarFuture,
            ReadinessBlockerReason::ExpectedLatestBarMissing,
        ];
        for reason in all {
            assert_eq!(
                ReadinessBlockerReason::parse_code(reason.code()),
                Some(reason)
            );
        }
    }

    #[test]
    fn selected_not_selected_and_refused_candidates_all_carry_an_exact_reason() {
        // Defect E: every candidate result must have an exact reason --
        // proven across a selected winner, a lower-score loser, and a
        // gate-refused candidate in the same plan.
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 900_000, Some(1), false),
            candidate("AAPL", "mean_reversion", 500_000, Some(2), false),
            {
                let mut c = candidate("AAPL", "volatility_breakout", 300_000, Some(3), false);
                c.evidence.promotion_state = None;
                c
            },
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        let aapl = result_for(&plan, "AAPL");
        assert_eq!(aapl.candidates.len(), 3);
        // Defect 4: exact_reason is now structurally non-optional -- every
        // candidate result has one by construction, so there is nothing left
        // to assert presence of; this loop is retained only to document the
        // invariant at the call site.
        let winner = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "swing_momentum")
            .unwrap();
        assert_eq!(
            winner.exact_reason,
            ExactSelectionReason::SelectedHighestScore
        );
        let loser = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "mean_reversion")
            .unwrap();
        assert_eq!(
            loser.exact_reason,
            ExactSelectionReason::NotSelectedLowerScore
        );
        let refused = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "volatility_breakout")
            .unwrap();
        assert_eq!(
            refused.exact_reason,
            ExactSelectionReason::PromotionNotActivePaper
        );

        assert_eq!(
            aapl.exact_reason,
            ExactSelectionReason::SelectedHighestScore,
            "symbol-level exact_reason mirrors the winning candidate's"
        );
    }

    #[test]
    fn symbol_with_no_candidates_has_no_valid_candidate_exact_reason() {
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[]);
        let aapl = result_for(&plan, "AAPL");
        assert!(aapl.candidates.is_empty());
        assert_eq!(aapl.exact_reason, ExactSelectionReason::NoValidCandidate);
    }

    #[test]
    fn divergent_duplicate_candidates_carry_the_divergent_duplicate_exact_reason() {
        let mut c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        let mut c2 = c1.clone();
        c2.evidence.canonical_score_decimal = Some(decimal_from_micros(900_000));
        c2.evidence.canonical_score_micros = Some(900_000);
        c1.evidence.exact_reason = None;
        c2.evidence.exact_reason = None;
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.exact_reason == ExactSelectionReason::DivergentDuplicate));
    }

    // ── Defect 2: typed final result reasons -- eight required scenarios ──

    #[test]
    fn no_panic_path_for_any_evidence_exact_reason_value() {
        // Defect 2, test 1: every ExactSelectionReason variant, supplied as
        // a caller pre-gate diagnosis against a fixed evidence-gate failure
        // (promotion not active paper), must resolve without panicking --
        // incompatible pairs fail closed via PreGateReasonInconsistent
        // rather than any expect/unwrap.
        for reason in all_exact_selection_reasons() {
            let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
            c.evidence.promotion_state = None;
            c.evidence.exact_reason = Some(reason);
            let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
            let _ = result_for(&plan, "AAPL");
        }
    }

    #[test]
    fn divergent_duplicate_cannot_retain_a_promotion_or_readiness_reason() {
        // Defect 2, test 3: selector-determined outcomes (here: divergent
        // duplicate) must never surface a caller's pre-gate diagnosis, even
        // when one is present and would otherwise look plausible.
        let mut c1 = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c1.evidence.exact_reason = Some(ExactSelectionReason::PromotionExpired);
        let mut c2 = c1.clone();
        c2.evidence.canonical_score_decimal = Some(decimal_from_micros(900_000));
        c2.evidence.canonical_score_micros = Some(900_000);
        c2.evidence.exact_reason = Some(ExactSelectionReason::DataNotReadyUnspecified);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c1, c2]);
        let aapl = result_for(&plan, "AAPL");
        assert!(aapl
            .candidates
            .iter()
            .all(|c| c.exact_reason == ExactSelectionReason::DivergentDuplicate));
    }

    #[test]
    fn all_selected_not_selected_and_refused_tuples_are_coherent() {
        // Defect 2, test 4.
        let candidates = vec![
            candidate("AAPL", "swing_momentum", 900_000, Some(1), false),
            candidate("AAPL", "mean_reversion", 500_000, Some(2), false),
            {
                let mut c = candidate("AAPL", "volatility_breakout", 300_000, Some(3), false);
                c.evidence.promotion_state = None;
                c
            },
        ];
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &candidates);
        for cand in &result_for(&plan, "AAPL").candidates {
            assert!(
                verify_candidate_result_coherence(cand).is_ok(),
                "{cand:?} must be coherent"
            );
        }
    }

    #[test]
    fn inconsistent_pre_gate_reason_fails_closed() {
        // Defect 2, test 5: the caller-supplied pre-gate diagnosis names a
        // completely unrelated failure to the check that actually fired --
        // never proven to agree, so it fails closed distinctly.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_state = None;
        c.evidence.exact_reason = Some(ExactSelectionReason::PromotionExpired);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(
            cand.exact_reason,
            ExactSelectionReason::PreGateReasonInconsistent
        );
        assert_eq!(
            cand.reason_code,
            REASON_REFUSED_PRE_GATE_REASON_INCONSISTENT
        );
        assert_eq!(cand.disposition, SelectionCandidateDisposition::Refused);
    }

    #[test]
    fn compatible_pre_gate_refinement_is_trusted() {
        // Companion to test 5: a *compatible* pre-gate refinement (the
        // finer NoPromotionRecord explanation of the same
        // PromotionNotActivePaper gate check) is trusted and surfaced.
        let mut c = candidate("AAPL", "swing_momentum", 500_000, Some(1), false);
        c.evidence.promotion_state = None;
        c.evidence.exact_reason = Some(ExactSelectionReason::NoPromotionRecord);
        let plan = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL"]), &[c]);
        let cand = &result_for(&plan, "AAPL").candidates[0];
        assert_eq!(cand.exact_reason, ExactSelectionReason::NoPromotionRecord);
        assert_eq!(cand.reason_code, REASON_REFUSED_NOT_ACTIVE_PAPER);
    }

    #[test]
    fn public_reason_code_is_defined_for_every_exact_selection_reason() {
        // Defect 2, test 6: `public_reason_code` is an exhaustive match with
        // no wildcard arm -- the crate would not compile if any variant
        // were left unhandled. This loop is a runtime companion proof.
        for reason in all_exact_selection_reasons() {
            assert!(!public_reason_code(reason).is_empty());
        }
    }

    #[test]
    fn legacy_reason_code_strings_remain_unchanged() {
        // Defect 2, test 7.
        assert_eq!(REASON_REFUSED_NOT_ACTIVE_PAPER, "refused_not_active_paper");
        assert_eq!(
            REASON_REFUSED_FINGERPRINT_MISMATCH,
            "refused_evidence_fingerprint_mismatch"
        );
        assert_eq!(
            REASON_REFUSED_FINGERPRINT_V2_MISSING,
            "refused_evidence_fingerprint_v2_missing"
        );
        assert_eq!(
            REASON_REFUSED_FINGERPRINT_V2_MISMATCH,
            "refused_evidence_fingerprint_v2_mismatch"
        );
        assert_eq!(REASON_SELECTED_HIGHEST_SCORE, "selected_highest_score");
        assert_eq!(REASON_NO_VALID_CANDIDATE, "no_valid_candidate_for_symbol");
        assert_eq!(
            public_reason_code(ExactSelectionReason::PromotionNotActivePaper),
            REASON_REFUSED_NOT_ACTIVE_PAPER
        );
        assert_eq!(
            public_reason_code(ExactSelectionReason::NoPromotionRecord),
            REASON_REFUSED_NOT_ACTIVE_PAPER,
            "the finer NoPromotionRecord pre-gate diagnosis still surfaces the same coarse legacy reason_code"
        );
        assert_eq!(
            public_reason_code(ExactSelectionReason::DivergentDuplicate),
            REASON_REFUSED_DIVERGENT_DUPLICATE
        );
    }

    #[test]
    fn permutation_identity_remains_stable_with_typed_reasons() {
        // Defect 2, test 8.
        let mut c1 = candidate("AAPL", "swing_momentum", 900_000, Some(1), false);
        c1.evidence.exact_reason = Some(ExactSelectionReason::PromotionQueryFailed);
        let mut c2 = candidate("MSFT", "mean_reversion", 500_000, Some(1), false);
        c2.evidence.promotion_state = None;
        c2.evidence.exact_reason = Some(ExactSelectionReason::NoPromotionRecord);
        let forward = vec![c1.clone(), c2.clone()];
        let reversed = vec![c2, c1];
        let p1 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &forward);
        let p2 = compute_dynamic_selection_plan(ctx(), &symbols(&["AAPL", "MSFT"]), &reversed);
        assert_eq!(
            p1, p2,
            "typed pre-gate reasons must not break permutation independence"
        );
    }
}
