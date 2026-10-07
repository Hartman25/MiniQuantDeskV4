"""Writes the result-independent Census-02 predeclaration PROPOSAL. It is not a freeze: it can never satisfy
c2_protocol.require_freeze, carries no result field, and every open policy is an operator decision."""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_borrow as bw  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_protocol as pr  # noqa: E402

PROPOSED_ETF_SCOPE = ["DIA", "EEM", "EFA", "GLD", "IEF", "IWM", "QQQ", "SLV", "SPY", "TLT", "VTI", "XLB", "XLE", "XLF",
                      "XLI", "XLK", "XLP", "XLU", "XLV", "XLY"]
FUNNEL_ITEM_OPTIONS = {
    "trade_count_confidence_band": ["REUSE_PASS2_MIN_30_TRADES", "OPERATOR_BAND_REPORTED_AS_CONFIDENCE_INTERVAL_NOT_GATE",
                                    "REPORT_ONLY_NO_GATE"],
    "year_stability": ["REUSE_PASS2_6_OF_8_POSITIVE_YEARS_AND_LEAVE_BEST_YEAR_OUT", "REPORT_ONLY_NO_GATE",
                       "OPERATOR_VALUE"],
    "regime_concentration": ["REUSE_PASS2_MAX_SHARE_0_80", "REPORT_ONLY_NO_GATE", "OPERATOR_VALUE"],
    "parameter_neighborhood": ["REUSE_PASS2_ADJACENT_NEIGHBOUR_SHARE_0_25", "REPORT_ONLY_NO_GATE", "OPERATOR_VALUE"],
    "portfolio_mdd_worst5day": ["DEFER_TO_PORTFOLIO_SUITABILITY_STAGE_USING_MAIN_RISK_BAR", "REUSE_PASS2_MDD_20PCT_WORST5_6PCT",
                                "OPERATOR_VALUE"],
}
FEASIBILITY = {
    "short_only_mirrors_SH01_SH13": "IMPLEMENTED_INFRASTRUCTURE; execution borrow-gated",
    "symmetric_long_short_state_pairs_LS01_LS04": "IMPLEMENTED_INFRASTRUCTURE; stop-and-reverse flip priced as exit leg + entry leg",
    "cross_sectional_top_vs_bottom_momentum": "NOT_IMPLEMENTED_DEFERRED: needs a portfolio-level simulator and universe-scope "
                                              "identity; accepted min_cross_section=20 makes a liquid-ETF-only short leg "
                                              "marginal; equity short leg is hypothesis-only without borrow truth. "
                                              "Proposed grammar if included: lookback {21,63,126,252} x month_end x quintile = 4",
    "sector_relative_residual_mean_reversion": "NOT_FEASIBLE: no sector/industry authority in repo or provider contract; "
                                               "inferring it from symbols is forbidden",
    "beta_hedged_market_neutral_spreads": "NOT_IMPLEMENTED_DEFERRED: long-name/short-SPY needs only a liquid-ETF short leg but "
                                          "requires a causal rolling-beta contract and a hedge-ratio identity; applying it to "
                                          "existing long candidates would be retroactive rescue of consumed results",
}


def build_proposal() -> dict:
    cfgs = gr.build_configs()
    structural = pr.build_structural_protocol()
    scenarios = {
        "S0_no_executable_shorts_hypothesis_only": {**gr.candidate_arithmetic(0), "note": "strategy_trials=0; labels only"},
        "S1_frozen_etf_scope_20_H+L": gr.candidate_arithmetic(len(PROPOSED_ETF_SCOPE)),
        "S2_frozen_etf_scope_20_H_only": gr.candidate_arithmetic(len(PROPOSED_ETF_SCOPE), "H"),
        "S3_frozen_etf_scope_20_H+L_complements_excluded": gr.candidate_arithmetic(len(PROPOSED_ETF_SCOPE), complements_excluded=True),
    }
    return {
        "schema_version": "alpha_census02_proposal_v1", "status": pr.STATUS_PROPOSED,
        "mission": "V4-ALPHA-CENSUS-02-SHORT-LONGSHORT-PREDECLARATION-01",
        "structural_protocol_id": pr.sha256_canonical(structural)[:32], "structural_protocol": structural,
        "real_census02_attempts_executed": 0, "confirmation_rows_consumed": 0, "final_holdout_rows_consumed": 0,
        "contaminated_2024_rows_scored": 0, "authority": {"VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE"},
        "candidate_grammar": {
            "tier_H_short_only": {f: gr.EXPECTED_H_COUNTS[f] for f in gr.EXPECTED_H_COUNTS},
            "tier_L_long_short_state_pairs": {f: gr.EXPECTED_L_COUNTS[f] for f in gr.EXPECTED_L_COUNTS},
            "tier_H_configs": gr.EXPECTED_H_TOTAL, "tier_L_configs": gr.EXPECTED_L_TOTAL, "total_configs": len(cfgs),
            "short_conditions": gr.EXPECTED_CONDITION_TOTAL, "horizons": list(gr.HORIZONS),
            "conditional_factors": gr.EXPECTED_CONDITION_TOTAL * len(gr.HORIZONS),
            "complement_tagged_families": list(gr.COMPLEMENT_FAMILIES),
            "complement_tagged_configs": sum(1 for c in cfgs if c["family"] in gr.COMPLEMENT_FAMILIES),
            "calendar_family_S14": "NOT_MIRRORED (no coherent short form)",
            "config_root_sha256": pr.sha256_canonical([c["config_id"] for c in cfgs]),
            "condition_root_sha256": pr.sha256_canonical([c["condition_id"] for c in gr.build_conditions(cfgs)])},
        "candidate_count_scenarios": scenarios,
        "evidence_classes": {"A": bw.EVIDENCE_A, "B": bw.EVIDENCE_B, "C": bw.EVIDENCE_C,
                             "borrow_truth": bw.BORROW_TRUTH_UNAVAILABLE, "class_B_reachable": bw.POINT_IN_TIME_BORROW_SUPPORTED},
        "proposed_etf_scope_for_operator_review": {"status": "PROPOSAL_NOT_FROZEN", "symbols": PROPOSED_ETF_SCOPE,
                                                   "note": "the registry carries no instrument-type field; ETF membership "
                                                           "must be an explicit operator-frozen list"},
        "multiple_testing_proposal": {
            "registered_population": "frozen population root over every (config x scope symbol) trial and every condition x horizon factor",
            "local_family": "Census-02 conditional factors form their own BH family (factor_fdr_bh_v1, alpha 0.10)",
            "global_disclosure": "Census-01 totals (38,192 strategy trials, 1,095 factors) are reported beside every Census-02 verdict",
            "strategy_judge": "DEFERRED_FULL_POPULATION; any later DSR/PBO denominator is the pooled Census-01 + Census-02 population",
            "complements": "tagged, counted in the registered denominator, excluded from the effective-independent estimate"},
        "feasibility": FEASIBILITY,
        "operator_decisions_required": {k: v[0] for k, v in pr.DECISIONS.items()},
        "funnel_threshold_options": FUNNEL_ITEM_OPTIONS,
        "funnel": structural["funnel"],
    }


def write_proposal() -> dict:
    doc = build_proposal()
    pr.PROPOSAL_FILE.write_text(json.dumps(doc, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    return doc


if __name__ == "__main__":
    d = write_proposal()
    print(d["structural_protocol_id"], d["candidate_grammar"]["total_configs"])
