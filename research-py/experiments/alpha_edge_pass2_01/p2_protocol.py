"""Pass-2 robustness protocol: every threshold, scenario rule, NA/ranking/retry semantic. build_protocol() is the single
source of the predeclared document; PASS2_PROTOCOL_ID = sha256(canonical protocol). No threshold may change after the
predeclaration commit: any change moves PASS2_PROTOCOL_ID and therefore every robustness_candidate_id."""

from __future__ import annotations

import hashlib
import json

EXPERIMENT_ID = "alpha_edge_pass2_01"
SCHEMA = "alpha_edge_pass2_protocol_v1"
ORIGIN = "alpha_pass2_robustness"

KIND_STRATEGY, KIND_CONDITIONAL = "STRATEGY_ROBUSTNESS", "CONDITIONAL_ROBUSTNESS"
PASS1_STRATEGY_CLASS = "DISCOVERED_MODERATE"
PASS1_CONDITIONAL_CLASS = "DISCOVERED_STRONG"
EXPECTED_STRATEGY_COHORT, EXPECTED_CONDITIONAL_COHORT = 789, 135
EXPECTED_STRATEGY_ATTEMPTS, EXPECTED_CONDITIONAL_ATTEMPTS = 38_192, 1_095

YEARS = tuple(range(2016, 2024))
MIN_POSITIVE_YEARS = 6                       # ceil(0.70 * 8)
MIN_TRADES = 30                              # Pass-1 flag_thresholds.tiny_sample_trades
STRESS_2X, STRESS_3X, DELAY_SESSIONS = 2, 3, 1
MAX_REGIME_CONCENTRATION = 0.80
MIN_NEIGHBOR_SHARE = 0.25
INITIAL_CAPITAL_USD = 100_000.0
MAX_DRAWDOWN_FRAC = 0.20
ROLLING_SESSIONS, MIN_ROLLING_RETURN = 5, -0.06
BROADLY_SHARE = 0.50
C_MIN_EVENTS = 30
C_MAX_TOP_SYMBOL_SHARE = 0.50
C_MIN_NEIGHBOR_SHARE = 0.25
C_HORIZON_P_MAX = 0.10
C_REGIME_CONCENTRATION = 0.80
HORIZONS = (1, 3, 5, 10, 20)
FDR_ALPHA = 0.10

STRATEGY_HARD_GATES = ("S1", "S2", "S3", "S5", "S6", "S7", "S8", "S9", "S10")
CONDITIONAL_HARD_GATES = ("C1", "C2", "C3", "C4", "C5", "C6")
VERDICTS_STRATEGY = ("PASS2_STRATEGY_SURVIVOR", "PASS2_STRATEGY_REJECTED", "PASS2_STRATEGY_BLOCKED")
VERDICTS_CONDITIONAL = ("PASS2_CONDITIONAL_SURVIVOR", "PASS2_CONDITIONAL_REJECTED", "PASS2_CONDITIONAL_BLOCKED")
INTERRUPTED_REASON = "infrastructure_interrupted"

LABELS = {"VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE",
          "EVIDENCE_GRADE": "DISCOVERY_ROBUSTNESS_NOT_P9_PROMOTION_GRADE"}


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), allow_nan=False)


def sha256_canonical(obj) -> str:
    return hashlib.sha256(canonical(obj).encode("utf-8")).hexdigest()


def build_protocol() -> dict:
    return {
        "schema_version": SCHEMA, "experiment_id": EXPERIMENT_ID,
        "scope": "Discovery-period robustness purge of the accepted Pass-1 census; no new families, no new trials",
        "evidence_grade": LABELS["EVIDENCE_GRADE"],
        "p9_disposition": "generic S01-S14 research configs are not production-native strategies and have no P9 "
                          "registration seam; the same accepted concepts (cost stress x2, decision delay, year stability, "
                          "regime concentration, MAIN risk bar) are re-implemented as the narrow research analogue",
        "partitions": {"discovery": "[2016-01-01,2024-01-01)", "read_only_inputs": "census bars manifest V2 (discovery-fenced)",
                       "forbidden": ["[2024-01-01,2025-01-01) CONTAMINATED_BY_REJECTED_RUN",
                                     "[2025-01-01,2026-03-01) REMAINING_CONFIRMATION_RESERVE",
                                     "[2026-03-01,) FINAL_HOLDOUT"]},
        "cohorts": {
            "strategy": {"rule": f"every Pass-1 StrategyEdge with edge_class == {PASS1_STRATEGY_CLASS}",
                         "expected_count": EXPECTED_STRATEGY_COHORT, "weak_advances": False,
                         "strong_present_refuses": True},
            "conditional": {"rule": f"every Pass-1 ConditionalEdge with edge_class == {PASS1_CONDITIONAL_CLASS}",
                            "expected_count": EXPECTED_CONDITIONAL_COHORT, "weak_or_moderate_advance": False},
            "count_mismatch": "REFUSE_BEFORE_ATTEMPT_1"},
        "candidate_identity": {
            "robustness_candidate_id": "sha256(PASS2_PROTOCOL_ID, kind, Pass-1 edge_id, Pass-1 trial_id|factor_id, "
                                       "Pass-1 search_space/protocol/universe/partitions ids)[:32]",
            "result_independent": True, "retry": "same candidate, new attempt; a poor result never mints a candidate"},
        "attempts": {"unit": "one durable attempt per robustness candidate covering every scenario (stored in the "
                             "ResearchResultStore attempt seam under a Pass-2-only experiment_id/registry)",
                     "started_before_evaluation": True, "terminal_never_reexecuted": True,
                     "retry_eligible_only": INTERRUPTED_REASON, "outcome_based_retry": "forbidden",
                     "stress_runs_are_trials": False},
        "verdict_rule": {
            "SURVIVOR": "every applicable hard gate PASS",
            "REJECTED": "at least one applicable hard gate FAIL (complete ordered failure list recorded; no early stop)",
            "BLOCKED": "no applicable hard gate FAILs but required proof is genuinely unavailable (never poor economics)",
            "NOT_APPLICABLE": "scenario excluded from the applicable gate set; never a fabricated pass or failure",
            "baseline_contradiction": "S1/C1 disagreement is a deterministic system contradiction: attempt fails, run hard-stops"},
        "strategy": {
            "S1": {"rule": "accepted evaluate_cell replay equals the Pass-1 registry metrics (canonical JSON, exact; no tolerance); "
                           "the net P&L stream behind the metrics must reproduce net_pnl_usd exactly", "gate": True},
            "S2": {"rule": "trade_count >= MIN_TRADES (accepted Pass-1 trade definition: entries with qty>0)",
                   "min_trades": MIN_TRADES, "gate": True},
            "S3": {"rule": "every commission and execution-slippage input x2; SAME matched passive benchmark under the SAME "
                           "stressed costs; PASS iff stressed net P&L > 0 AND stressed benchmark alpha > 0",
                   "multiplier": STRESS_2X, "gate": True},
            "S4": {"rule": "as S3 at x3", "multiplier": STRESS_3X, "gate": False, "role": "diagnostic strength"},
            "S5": {"rule": "own completed-bar decision stream delayed by exactly one completed session; causal fill rule "
                           "unchanged; benchmark and evaluation window identical to Pass 1; PASS iff delayed net P&L > 0 "
                           "AND delayed alpha > 0", "delay_sessions": DELAY_SESSIONS, "gate": True},
            "S6": {"rule": "baseline per-bar benchmark-relative alpha by calendar year; a year absent from the evaluation "
                           "window contributes 0 (not positive); PASS iff positive years >= MIN_POSITIVE_YEARS of 8",
                   "years": list(YEARS), "min_positive_years": MIN_POSITIVE_YEARS, "gate": True},
            "S7": {"rule": "remove the year with the largest positive alpha contribution (earliest year on ties); "
                           "PASS iff remaining total alpha > 0", "gate": True},
            "S8": {"rule": "Pass-1 regime_concentration_share (max positive bucket / sum positive buckets; RISK_ON iff SPY "
                           "prior-completed-bar close > SPY SMA200) <= MAX; absent share is BLOCKED",
                   "max_share": MAX_REGIME_CONCENTRATION, "gate": True},
            "S9": {"rule": "same family/symbol, exactly one numeric param moved to the adjacent frozen-grid value (Pass-1 "
                           "neighbor authority); neighbor supports iff it is a Pass-1 DISCOVERED_MODERATE StrategyEdge; "
                           "PASS iff supported share >= MIN; zero valid neighbors => NOT_APPLICABLE",
                   "min_share": MIN_NEIGHBOR_SHARE, "supporting_class": PASS1_STRATEGY_CLASS, "gate": True},
            "S10": {"rule": "strategy daily net P&L over the evaluation window on the accepted initial capital: max "
                            "drawdown (peak-to-trough of cumulative net incl. zero start) <= MAX_DD x capital AND worst "
                            "rolling ROLLING_SESSIONS-session net P&L / capital >= MIN_ROLLING",
                    "initial_capital_usd": INITIAL_CAPITAL_USD, "max_drawdown_frac": MAX_DRAWDOWN_FRAC,
                    "rolling_sessions": ROLLING_SESSIONS, "min_rolling_return": MIN_ROLLING_RETURN, "gate": True,
                    "source": "docs/specs/strategy_evaluation_and_ranking.md MAIN hard gates (no code implementation exists)"},
            "S11": {"rule": "per frozen config over evaluable symbols: ZERO moderate => contradiction (BLOCKED); exactly 1 => "
                            "SYMBOL_SPECIFIC; moderate share >= BROADLY_SHARE => BROADLY_REPLICATED; else CLUSTER_REPLICATED",
                    "broadly_share": BROADLY_SHARE, "gate": False, "role": "classification"}},
        "conditional": {
            "executable_pnl": False,
            "effect_estimator": "E(S): over observation rows S, per-symbol baseline = mean raw close_{t+h}/close_t-1 of that "
                                "symbol's rows in S; effect = mean over event rows in S of (return - symbol baseline); "
                                "direction higher_is_better; on the full frame E equals the accepted Pass-1 effect exactly",
            "C1": {"rule": "factor_id, evaluation_id, observation content hash, event count, direction-adjusted effect, "
                           "empirical-null p, FDR status/q all reproduce the accepted Pass-1 evidence", "gate": True},
            "C2": {"rule": "E on each calendar-year slice (baseline recomputed within the year); a year with no events is "
                           "not positive; PASS iff positive years >= MIN_POSITIVE_YEARS of 8",
                   "years": list(YEARS), "min_positive_years": MIN_POSITIVE_YEARS, "gate": True},
            "C3": {"rule": "year with the largest positive aggregate event contribution (sum of accepted labels; earliest on "
                           "ties) removed; E on the remaining rows > 0 AND remaining events >= MIN_EVENTS",
                   "min_events": C_MIN_EVENTS, "gate": True},
            "C4": {"rule": "top_symbol_event_share <= MAX and more than one represented symbol (single symbol FAILS)",
                   "max_share": C_MAX_TOP_SYMBOL_SHARE, "gate": True},
            "C5": {"rule": "for EVERY represented symbol: effect on the remaining rows > 0 AND remaining events >= MIN_EVENTS; "
                           "a slice below the floor FAILS (never dropped)", "min_events": C_MIN_EVENTS, "gate": True},
            "C6": {"rule": "same horizon, adjacent semantic numeric condition parameter (Pass-1 conditional neighbor "
                           "authority, 219-condition grid, execution-only params forbidden); neighbor supports iff it is a "
                           "Pass-1 DISCOVERED_STRONG ConditionalEdge; PASS iff share >= MIN; zero neighbors => NOT_APPLICABLE",
                   "min_share": C_MIN_NEIGHBOR_SHARE, "supporting_class": PASS1_CONDITIONAL_CLASS, "gate": True},
            "C7": {"rule": "regime share = max positive / sum positive of (effect x n) over risk_on/risk_off/unknown; "
                           "> MAX => REGIME_CONCENTRATED else GENERAL_REGIME", "max_share": C_REGIME_CONCENTRATION,
                   "gate": False, "role": "classification"},
            "C8": {"rule": "adjacent horizon (frozen 1,3,5,10,20) of the SAME condition with positive finite effect and "
                           "empirical-null p <= P_MAX in Pass 1 => HORIZON_SUPPORTED; evaluable adjacent horizons but none "
                           "supporting => HORIZON_ISOLATED; no evaluable adjacent horizon => NOT_APPLICABLE",
                   "p_max": C_HORIZON_P_MAX, "horizons": list(HORIZONS), "gate": False, "role": "classification"}},
        "multiple_testing": {"strategy_dsr_pbo": "DEFERRED_FULL_POPULATION unless the accepted judge consumes the Pass-1 "
                                                 "artifacts directly; population narrowing forbidden; denominator 38,192",
                             "conditional_fdr": "accepted complete family BH/FDR alpha 0.10 over 1,095 (unchanged)"},
        "ranking_readonly_not_selection": {
            "strategy": ["passes_3x_cost_true_first", "alpha_2x_desc", "alpha_delayed_desc", "leave_best_year_out_alpha_desc",
                         "positive_years_desc", "max_drawdown_usd_asc", "trial_id_asc"],
            "conditional": ["min_leave_one_symbol_out_effect_desc", "leave_best_year_out_effect_desc", "positive_years_desc",
                            "q_value_asc", "factor_id_asc"],
            "alters_pass_fail_or_identity": False},
        "labels": LABELS, "executable_pnl": False,
        "confirmation": "NOT_RUN", "final_holdout": "RESERVED_UNCONSUMED", "robustness_attempt_count_at_freeze": 0}


def protocol_id(protocol: dict | None = None) -> str:
    return sha256_canonical(protocol if protocol is not None else build_protocol())


PASS2_PROTOCOL_ID = protocol_id()
