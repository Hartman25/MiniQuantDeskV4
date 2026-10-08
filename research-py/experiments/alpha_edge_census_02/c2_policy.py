"""Operator-approved Census-02 policy (approved BEFORE result #1). Data only: no result value, no import of other Census-02
modules. The freeze guard requires the frozen decisions to equal APPROVED_DECISIONS exactly."""

from __future__ import annotations

import copy

APPROVAL = {"status": "APPROVED_BY_OPERATOR", "approved_before_result_1": True, "contains_results": False,
            "annual_borrow_fee_bps_meaning": "FROZEN BASE RESEARCH ASSUMPTION, not historical borrow truth",
            "individual_equity_shorts": "HYPOTHESIS / LABEL EVIDENCE ONLY; no executable-P&L claim without point-in-time borrow truth"}

ETF_SHORT_SCOPE = ["DIA", "EEM", "EFA", "GLD", "IEF", "IWM", "QQQ", "SLV", "SPY", "TLT", "VTI", "XLB", "XLE", "XLF", "XLI",
                   "XLK", "XLP", "XLU", "XLV", "XLY"]

APPROVED_DECISIONS = {
    "borrow_policy": "EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION",
    "etf_borrow_assumption": {"etf_short_scope": ETF_SHORT_SCOPE, "annual_borrow_fee_bps": 100.0,
                              "availability": "ALWAYS_AVAILABLE_FOR_SCOPE_ASSUMED", "recall": "NONE_ASSUMED",
                              "short_rebate": "ZERO"},
    "grammar_tiers": "H+L",
    "complement_handling": "REGISTER_ALL_TAG_COMPLEMENTS",
    "benchmark_rule": "SIDE_AWARE_SHORT_NET_AND_PASSIVE_SHORT_ALPHA_LONGSHORT_NET_VS_CASH",
    "multiple_testing_denominator": "LOCAL_WITH_GLOBAL_DISCLOSURE",
    "conditional_scope": "ALL_SEED_SYMBOLS",
    "ssr_handling": "FLAG_ONLY",
    "funnel_thresholds": {
        "trade_count_confidence_band": {
            "kind": "CLASSIFICATION_WITH_TYPED_MINIMUM", "under_5": "INSUFFICIENT", "5_to_14": "LOW_SAMPLE",
            "15_to_29": "MODERATE_SAMPLE", "30_plus": "STRONG_SAMPLE", "hard_gate": "closed_round_trips >= 5 only"},
        "year_stability": "REPORT_ONLY_NO_GATE",
        "regime_concentration": "REPORT_ONLY_NO_GATE",
        "parameter_neighborhood": "REPORT_ONLY_NO_GATE",
        "portfolio_mdd_worst5day": "DEFER_TO_PORTFOLIO_RISK_SUITABILITY_STAGE_USING_MAIN_RISK_BAR"},
}

# Census-01 search population every Census-02 verdict must disclose (local family; not one contemporaneous BH family).
GLOBAL_DISCLOSURE = {"census01_strategy_trials": 38_192, "census01_conditional_factors": 1_095,
                     "statement": "Census-02 is its own complete predeclared local factor family; Census-01 was frozen and "
                                  "evaluated earlier and is NOT part of one contemporaneous BH family. Strategy DSR/PBO stays "
                                  "DEFERRED_FULL_POPULATION."}


def approved_decisions() -> dict:
    return copy.deepcopy(APPROVED_DECISIONS)


def policy_document() -> dict:
    """The machine-readable operator-policy artifact (CENSUS02_OPERATOR_POLICY.json)."""
    return {"schema_version": "census02_operator_policy_v1", "approval": dict(APPROVAL), "decisions": approved_decisions(),
            "global_disclosure": dict(GLOBAL_DISCLOSURE)}
