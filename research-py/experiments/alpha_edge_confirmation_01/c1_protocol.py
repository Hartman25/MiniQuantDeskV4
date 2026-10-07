"""Confirmation protocol for the 18 Pass-2 Conditional survivors: every window, estimator, null, FDR rule and disposition
threshold. build_protocol() is the single source of the predeclared document; CONFIRMATION_PROTOCOL_ID = sha256(canonical
protocol). No value may change after the freeze commit: the freeze guard binds the source files that carry them."""

from __future__ import annotations

import hashlib
import json
import math

EXPERIMENT_ID = "alpha_edge_confirmation_01"
SCHEMA = "alpha_edge_confirmation_protocol_v1"
ORIGIN = "alpha_confirmation"
KIND = "CONFIRMATION_EVALUATION"

EXPECTED_COHORT = 18
EXPECTED_FAMILY_COUNTS = {"S05": 4, "S06": 12, "S13": 2}
PASS2_DENOMINATOR, PASS2_REJECTED = 135, 117
PASS2_SURVIVOR_VERDICT = "PASS2_CONDITIONAL_SURVIVOR"
DIRECTION = "higher_is_better"
HORIZONS = (1, 3, 5, 10, 20)

WARMUP_START = "2024-01-01"
SCORE_START = "2025-01-01"
SCORE_END_EXCLUSIVE = "2026-03-01"
REQUEST_ASOF = "2026-10-06"

MIN_EVENTS = 30
N_PERMUTATIONS = 200
BASE_SEED = 0
FDR_ALPHA = 0.10
P_STRONG_MAX = 0.10
N_QUANTILES = 2
MIN_CROSS_SECTION = 10
MIN_PERIODS = 30
LABEL_PROTOCOL = "alpha_confirmation_fwd_close_return_minus_symbol_baseline_v1"
NULL_PROTOCOL = "alpha_census_fast_empirical_null_two_sided_v1"
FDR_PROTOCOL = "benjamini_hochberg_complete_frozen_family_v1"
INTERRUPTED_REASON = "infrastructure_interrupted"

CONFIRMED_STRONG = "CONFIRMED_STRONG"
CONFIRMED_DIRECTIONAL_ONLY = "CONFIRMED_DIRECTIONAL_ONLY"
NOT_CONFIRMED = "NOT_CONFIRMED"
NOT_EVALUABLE = "NOT_EVALUABLE_IN_CONFIRMATION"
BLOCKED = "BLOCKED"
DISPOSITIONS = (CONFIRMED_STRONG, CONFIRMED_DIRECTIONAL_ONLY, NOT_CONFIRMED, NOT_EVALUABLE, BLOCKED)
REASON_INSUFFICIENT_EVENTS = "insufficient_events"

LABELS = {"VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE", "EXECUTABLE_PNL": False,
          "EVIDENCE_GRADE": "CONFIRMATION_FACTOR_DIAGNOSTIC_NOT_PROMOTION_GRADE"}

REQUEST_CONTRACT = {
    "provider": "alpaca", "feed": "sip", "adjustment": "all", "timeframe": "1Day",
    "start_utc": "2024-01-01T00:00:00+00:00", "end_utc_exclusive": "2026-03-01T00:00:00+00:00",
    "asof": REQUEST_ASOF, "extractor": "mqk_research.data.alpaca_historical.extract_research_bars_with_provenance",
}
PARTITION_IDENTITY = {
    "warmup": "[2024-01-01,2025-01-01) CONTAMINATED_BY_REJECTED_RUN: indicator warm-up context only; never scored, never baseline",
    "scored": "[2025-01-01,2026-03-01) CONFIRMATION_RESERVE",
    "label_endpoint_exclusive_max": SCORE_END_EXCLUSIVE, "final_holdout": "[2026-03-01,) never requested"}


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), allow_nan=False)


def sha256_canonical(obj) -> str:
    return hashlib.sha256(canonical(obj).encode("utf-8")).hexdigest()


def build_protocol() -> dict:
    return {
        "schema_version": SCHEMA, "experiment_id": EXPERIMENT_ID,
        "scope": "Independent Confirmation of exactly the 18 Pass-2 Conditional survivors as factor hypotheses; no new "
                 "hypotheses; no Strategy candidate; no Promotion/Paper/Live; the final holdout is never requested",
        "evidence_grade": LABELS["EVIDENCE_GRADE"],
        "partitions": PARTITION_IDENTITY,
        "cohort": {"rule": f"every Pass-2 conditional_robustness_ledger row with verdict == {PASS2_SURVIVOR_VERDICT}, derived "
                           "from the committed Pass-2 evidence bound by PASS2_PROCESS_DISPOSITION sha256_lf",
                   "expected_count": EXPECTED_COHORT, "expected_family_counts": EXPECTED_FAMILY_COUNTS,
                   "pass2_denominator": PASS2_DENOMINATOR, "pass2_rejected": PASS2_REJECTED,
                   "count_mismatch": "REFUSE_BEFORE_ANY_CONFIRMATION_READ", "rejected_factors_added": False,
                   "strategy_rows_confirmed": False, "strategy_s2_near_misses_rescued": False,
                   "frozen_fields": ["factor_id", "condition_id", "edge_id", "family", "params", "horizon", "direction"],
                   "discovery_fields_diagnostic_only": ["effect", "p_value", "q_value", "event_count", "horizon_class"]},
        "data_contract": {**REQUEST_CONTRACT, "universe": "ALPHA_CENSUS_UNIVERSE_V2 (88 symbols, unchanged, not point-in-time)",
                          "calendar": "us_equity_regular_sessions_v1",
                          "provenance": "registered bars provenance + SIP/all attestation + physical sha256, per symbol",
                          "provider_rows_ge_end_refused": True, "march_2026_fetch_forbidden": True,
                          "corporate_action_metadata_caveat": "corporate-action discovery uses process-date bounds up to the "
                                                              "wall clock, so metadata may name post-fence events; no bar "
                                                              "value at or after the fence is ever requested or retained",
                          "per_symbol_failure": "typed per-symbol disposition, never a silent universe shrink; SPY missing => BLOCKED"},
        "observations": {"warmup_contributes_observations": False, "scored_window": [SCORE_START, SCORE_END_EXCLUSIVE],
                         "label": "close_{t+h}/close_t - 1 minus same-symbol same-horizon mean over the scored valid rows "
                                  "(diagnostic forward LABEL, never executable P&L)", "label_protocol": LABEL_PROTOCOL,
                         "label_endpoint_rule": f"observation dropped unless the t+h bar exists and its session is < {SCORE_END_EXCLUSIVE}",
                         "factor_value": "completed-bar 0/1 condition, causal, unchanged Pass-1 builders (signals.PER_SYMBOL)"},
        "estimator": {"accepted": "census conditional.event_diagnostics + mqk_research.factors.diagnostics.evaluate_factor_ic_ir",
                      "effect": "mean over event rows of (label - same-symbol baseline), direction applied; direction frozen "
                                "per factor (no flip, no absolute value)",
                      "n_quantiles": N_QUANTILES, "min_cross_section": MIN_CROSS_SECTION, "min_periods": MIN_PERIODS,
                      "min_events": MIN_EVENTS, "below_floor": NOT_EVALUABLE, "events_counted_over": "valid scored frame rows"},
        "null": {"protocol": NULL_PROTOCOL, "n_permutations": N_PERMUTATIONS, "base_seed": BASE_SEED, "tails": "two_sided",
                 "p": "(exceed+1)/(n+1)"},
        "fdr": {"protocol": FDR_PROTOCOL, "alpha": FDR_ALPHA, "family": "exactly the frozen 18",
                "non_evaluable_p": 1.0, "non_evaluable_stay_in_denominator": True, "winner_only_fdr": False},
        "dispositions": {
            CONFIRMED_STRONG: f"events >= {MIN_EVENTS}, finite effect > 0, p <= {P_STRONG_MAX}, complete-family BH q <= {FDR_ALPHA}; "
                              "the only status that may advance toward Final Holdout",
            CONFIRMED_DIRECTIONAL_ONLY: f"events >= {MIN_EVENTS}, finite effect > 0, STRONG p/q requirement not met",
            NOT_CONFIRMED: f"events >= {MIN_EVENTS} and effect <= 0",
            NOT_EVALUABLE: f"events < {MIN_EVENTS} or typed evaluation insufficiency; never merged with {NOT_CONFIRMED}",
            BLOCKED: "only a genuine system/provider/provenance contradiction"},
        "diagnostics_not_gates": ["retention_ratio", "per_month", "per_quarter", "per_symbol", "symbols_represented",
                                  "top_symbol_event_share", "regime_distribution", "pass2_horizon_class"],
        "forbidden_gates": ["6-of-8 years", "leave-best-year-out", "neighborhood", "max drawdown", "trade count", "delay", "cost"],
        "identity": "evaluation_id = sha256(protocol_id, factor_id, partition identity, data provenance identity, label "
                    "protocol, null protocol, FDR-family identity)[:32]; result-independent",
        "attempts": {"store": "separate Confirmation-only ResearchResultStore", "opened_before_computation": True,
                     "retry_eligible_only": INTERRUPTED_REASON, "statistical_failure_terminal": True,
                     "foreign_evaluation_id": "fail closed before mutation", "terminal_never_reexecuted": True},
        "ranking_readonly_not_selection": {"strong": ["q_asc", "p_asc", "effect_desc", "events_desc", "factor_id_asc"],
                                           "directional_only_table_separate": True, "alters_status_or_identity": False},
        "consumption_record": ["CONFIRMATION_RESERVE_CONSUMED", "first_authorized_read", "requested_bounds",
                               "scored_min_max", "warmup_min_max", "max_scored_observation", "max_label_endpoint",
                               "final_holdout_rows_read", "final_holdout_rows_scored"],
        "labels": LABELS, "promotion": "NOT_CLAIMED", "final_holdout": "RESERVED_UNCONSUMED",
        "confirmation_attempts_at_freeze": 0}


def protocol_id(protocol: dict | None = None) -> str:
    return sha256_canonical(protocol if protocol is not None else build_protocol())


CONFIRMATION_PROTOCOL_ID = protocol_id()


def require_finite(x, what: str) -> float:
    if x is None or not math.isfinite(float(x)):
        raise ValueError(f"{what} must be finite")
    return float(x)
