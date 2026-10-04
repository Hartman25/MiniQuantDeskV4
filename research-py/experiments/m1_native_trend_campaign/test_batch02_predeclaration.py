"""Guards the frozen Batch 02 predeclaration.

Every economic policy value an observed result could tempt someone to tune is frozen below BY PATH against
the accepted corrected Batch 01 contract, and every new Batch 02 field is pinned literally. Descriptive
prose is deliberately NOT required to equal Batch 01: copied prose is audited separately
(test_batch02_predeclaration_erratum.py), because byte-equality with an older campaign proves a sentence
was copied, not that it is true.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import batch02_erratum as ea  # noqa: E402

OLD = json.loads((HERE / "PREDECLARED_BATCH_01_CORRECTED.json").read_text(encoding="utf-8"))
NEW = json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8"))
SYMS = ["SPY", "EFA", "IEF", "VNQ", "GLD"]
STRATS = ["turn_of_month_last1_first3", "halloween_nov_apr", "trading_range_breakout_50d_hold10"]
CAL_ID = "us_equity_regular_sessions_v1"


def test_historical_batch01_stop_field_is_untouched_and_not_a_batch02_gate():
    assert OLD["batch_stopping_rule"]["no_batch_02"] is True
    assert "no_batch_02" not in NEW["batch_stopping_rule"]
    assert "HISTORICAL" in NEW["governance"]["historical_batch_01_stop_field"]


def test_identity_and_scope():
    assert NEW["mission_id"] == "V4-M1-NATIVE-HYPOTHESIS-BATCH-02-01"
    assert NEW["batch_id"] == "m1_native_hypothesis_batch_02"
    assert NEW["experiment"]["real_experiment_id"] == "M1-NATIVE-HYPOTHESIS-BATCH-02-REAL"
    assert NEW["run_dir"] == "runs/run_batch_02"
    assert NEW["experiment"]["registry_db_relative_path"] == "runs/run_batch_02/registry/research.sqlite3"
    assert NEW["experiment"]["real_experiment_id"] != OLD["experiment"]["real_experiment_id"]


FROZEN_POLICY_PATHS = (
    # DATA
    "/data/path", "/data/feed", "/data/timeframe", "/data/adjustment", "/data/start_utc", "/data/end_utc",
    "/data/asof", "/data/completed_bars_only", "/data/fallback_if_corporate_action_gate_refuses/end_utc",
    "/data/reuse_verified_data_from/run_dir", "/data/reuse_verified_data_from/expected_artifact_sha256",
    "/data/reuse_verified_data_from/expected_row_count",
    "/data/reuse_verified_data_from/expected_canonical_semantic_bars_hash",
    "/data/reuse_verified_data_from/expected_source_attestation_id",
    # PARTITION and HOLDOUT
    "/partition/evaluation_start_utc", "/partition/test_months", "/partition/holdout_months",
    "/partition/expected_folds", "/partition/holdout_rule", "/holdout/status",
    # ECONOMIC POLICY
    "/economic_protocol/protocol_id", "/economic_protocol/signal_stream_protocol",
    "/economic_protocol/signal_policy/direction_policy", "/economic_protocol/signal_policy/sizing",
    "/economic_protocol/signal_policy/entry_threshold", "/economic_protocol/signal_policy/long_only",
    "/economic_protocol/signal_policy/max_gross_exposure",
    "/economic_protocol/cost_model/commission_bps_per_side", "/economic_protocol/cost_model/slippage_bps_per_side",
    "/economic_protocol/execution_pricing/pricing_model_id", "/economic_protocol/execution_pricing/slippage_bps",
    "/economic_protocol/execution_pricing/volatility_mult_bps",
    "/economic_protocol/weight_to_share/equity_usd", "/economic_protocol/weight_to_share/max_position_notional_usd",
    "/economic_protocol/annualization/annualization_days", "/economic_protocol/annualization/risk_free_rate_annual",
    "/economic_protocol/quantity_semantics/id",
    # CAPITAL and Backtest identity inputs
    "/native_backtest/initial_cash_micros", "/native_backtest/timeframe_secs", "/native_backtest/integrity_calendar",
    # PROMOTION THRESHOLDS, rejection gates, fidelity floor
    "/promotion_policy/MQK_PROMOTION_MIN_SHARPE", "/promotion_policy/MQK_PROMOTION_MAX_MDD",
    "/promotion_policy/MQK_PROMOTION_MIN_CAGR", "/promotion_policy/MQK_PROMOTION_MIN_PROFIT_FACTOR",
    "/promotion_policy/MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT",
    "/promotion_policy/MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO",
    "/promotion_policy/MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING",
    "/promotion_policy/MQK_RESEARCH_REQUIRE_NATIVE_SEMANTIC_BINDING",
    "/rejection_gates/0", "/rejection_gates/1", "/rejection_gates/2", "/rejection_gates/3", "/rejection_gates/4",
    "/rejection_gates/5", "/execution_fidelity/floor",
    # ROBUSTNESS: block counts, sensitivity ceilings, non-sizing stress execution knobs
    "/robustness/block_counts/0", "/robustness/block_counts/1", "/robustness/block_counts/2",
    "/robustness/dsr_max_sensitivity_range", "/robustness/pbo_max_sensitivity_range",
    "/robustness/p7a_p7b_stress/stress_execution_slippage_bps",
    "/robustness/p7a_p7b_stress/stress_execution_volatility_mult_bps",
    "/robustness/p7a_p7b_stress/max_drawdown_ceiling",
    "/activity_report/gate", "/activity_report/descriptive_only",
)


def test_frozen_economic_policy_is_carried_over_explicitly_not_by_whole_block():
    old, new = ea.leaves(OLD), ea.leaves(NEW)
    for path in FROZEN_POLICY_PATHS:
        assert path in old and path in new, path
        assert type(new[path]) is type(old[path]) and new[path] == old[path], path
    assert len(set(FROZEN_POLICY_PATHS)) == len(FROZEN_POLICY_PATHS)
    # The only block-level policy that must NOT carry over is the superseded fixed-quantity stress cap.
    assert "/robustness/p7a_p7b_stress/stress_max_position_notional_usd" in old
    assert "/robustness/p7a_p7b_stress/stress_max_position_notional_usd" not in new
    assert NEW["promotion_policy"]["MQK_PROMOTION_MIN_SHARPE"] == 0.5
    assert NEW["promotion_policy"]["MQK_RESEARCH_REQUIRE_NATIVE_SEMANTIC_BINDING"] == 1


def test_fifteen_trials_in_the_frozen_order():
    trials = NEW["universe"]["trials"]
    assert NEW["universe"]["symbols"] == SYMS
    assert [(t["order"], t["strategy_id"], t["symbol"]) for t in trials] == [
        (i * 5 + j + 1, STRATS[i], SYMS[j]) for i in range(3) for j in range(5)]
    assert NEW["universe"]["universe_mode"] == "fixed_ex_ante" and NEW["universe"]["point_in_time_membership"] is False
    assert NEW["universe"]["max_trials"] == 15
    assert [h["strategy_id"] for h in NEW["hypotheses"]] == STRATS
    assert [h["required_history_bars"] for h in NEW["hypotheses"]] == [1, 1, 60]
    assert all(h["timeframe_secs"] == 86400 and h["direction"] == "long_only_flat" for h in NEW["hypotheses"])


def test_baseline_capital_sizing_is_1000_bps_of_100k_and_the_cap_is_independent():
    cs = NEW["capital_sizing"]
    assert cs["policy_id"] == "fixed_initial_capital_fraction_v1"
    assert cs["allocation_fraction_bps"] == 1000
    assert cs["initial_capital_micros"] == NEW["native_backtest"]["initial_cash_micros"] == 100_000_000_000
    assert cs["nominal_entry_budget_micros"] == cs["initial_capital_micros"] * cs["allocation_fraction_bps"] // 10_000 == 10_000_000_000
    assert cs["capital_basis"] == "native_backtest.initial_cash_micros"
    assert cs["max_position_notional_usd"] == 50000 == NEW["economic_protocol"]["weight_to_share"]["max_position_notional_usd"]
    assert "NOT a half-exposure mechanism" in cs["cap_note"]


def test_half_exposure_stress_is_a_500_bps_resized_quantity_not_a_cap():
    st = NEW["robustness"]["p7a_p7b_stress"]
    s = st["stress_sizing"]
    base = NEW["capital_sizing"]
    assert s["scenario_id"] == "half_exposure_capital_fraction_500bps_v1"
    assert s["policy_id"] == base["policy_id"]
    assert s["allocation_fraction_bps"] == 500 == base["allocation_fraction_bps"] // 2
    assert s["initial_capital_micros"] == base["initial_capital_micros"]
    assert s["nominal_entry_budget_micros"] == 5_000_000_000 == s["initial_capital_micros"] * 500 // 10_000
    assert s["is_a_trial"] is False
    assert "do not cap a 1000-bps baseline Q" in s["quantity_rule"]
    assert (st["stress_execution_slippage_bps"], st["stress_execution_volatility_mult_bps"],
            st["max_drawdown_ceiling"]) == (15, 10, 0.4)
    # Neither a USD cap on the 1000-bps quantity nor the historical 25k cap encodes half exposure.
    assert "stress_max_position_notional_usd" not in st and "stress_max_target_qty" not in st
    assert "25000" not in json.dumps(st)
    assert "stress_max_position_notional_usd" in st["stress_notional_cap_policy"]
    assert "NOT used" in st["stress_notional_cap_policy"]


def test_benchmark_is_capital_fraction_and_v2_is_not_authorized():
    cf = "capital_fraction_matched_passive_buy_hold_v1"
    assert NEW["benchmark"]["policy_id"] == cf and NEW["scanner_review"]["benchmark_policy"] == cf
    assert NEW["benchmark"]["historical_benchmark_v2_not_authorized"].startswith("capital_matched_exact_target_buy_hold_v1")
    assert "capital_matched_exact_target_buy_hold_v1" != NEW["scanner_review"]["benchmark_policy"]
    assert NEW["economic_protocol"]["signal_policy"]["direction_policy"] == "native_exact_target_qty_v1"
    assert NEW["economic_protocol"]["signal_stream_protocol"] == "native_strategy_signal_stream_v2"


def test_hypothesis_rules_are_frozen_literally():
    h1, h2, h3 = NEW["hypotheses"]
    assert "month_ordinal(N(t)) in {1,2,3}" in h1["rule"] and "is_last_session_of_month(N(t))" in h1["rule"]
    assert "{11,12,1,2,3,4}" in h2["rule"] and "{5,6,7,8,9,10}" in h2["rule"]
    assert h1["calendar_contract_id"] == h2["calendar_contract_id"] == CAL_ID
    assert "close_micros[j-50 .. j-1]" in h3["rule"] and "t-9" in h3["rule"]
    assert "equality is no event" in h3["rule"]
    assert "exactly 60" in h3["required_history_derivation"]
    for h in (h1, h2, h3):
        assert h["forbidden_additions"] and "short side" in h["forbidden_additions"] or "lower breakout short" in h["forbidden_additions"]


def _nth(y, m, wd, n):
    d = dt.date(y, m, 1)
    while d.weekday() != wd:
        d += dt.timedelta(1)
    return d + dt.timedelta(7 * (n - 1))


def _last(y, m, wd):
    d = dt.date(y + (m == 12), m % 12 + 1, 1) - dt.timedelta(1)
    while d.weekday() != wd:
        d -= dt.timedelta(1)
    return d


def _easter(y):
    a, b, c = y % 19, y // 100, y % 100
    d, e = b // 4, b % 4
    f = (b + 8) // 25
    g = (b - f + 1) // 3
    h = (19 * a + b - d - g + 15) % 30
    i, k = c // 4, c % 4
    el = (32 + 2 * e + 2 * i - h - k) % 7
    m = (a + 11 * h + 22 * el) // 451
    mo = (h + el - 7 * m + 114) // 31
    return dt.date(y, mo, (h + el - 7 * m + 114) % 31 + 1)


def _obs(d):
    return d - dt.timedelta(1) if d.weekday() == 5 else d + dt.timedelta(1) if d.weekday() == 6 else d


def rule_closures():
    out = {dt.date(2018, 12, 5), dt.date(2025, 1, 9)}
    for y in range(2016, 2027):
        if dt.date(y, 1, 1).weekday() != 5:
            out.add(_obs(dt.date(y, 1, 1)))
        out |= {_nth(y, 1, 0, 3), _nth(y, 2, 0, 3), _easter(y) - dt.timedelta(2), _last(y, 5, 0),
                _obs(dt.date(y, 7, 4)), _nth(y, 9, 0, 1), _nth(y, 11, 3, 4), _obs(dt.date(y, 12, 25))}
        if y >= 2022:
            out.add(_obs(dt.date(y, 6, 19)))
    return sorted(d for d in out if d.weekday() < 5 and dt.date(2016, 1, 1) <= d <= dt.date(2026, 12, 31))


def test_calendar_contract_content_hash_and_independent_rule_derivation():
    cal = NEW["calendar"]
    assert cal["contract_id"] == CAL_ID
    assert (cal["coverage_start_date"], cal["coverage_end_date"]) == ("2016-01-01", "2026-12-31")
    closures = cal["full_closure_dates_weekdays"]
    assert closures == sorted(set(closures)) and len(closures) == cal["full_closure_count"] == 105
    assert closures == [d.isoformat() for d in rule_closures()], "closure table must equal the independent rule derivation"
    content = f"{CAL_ID}\ncoverage=2016-01-01..2026-12-31\n" + "\n".join(f"closure={d}" for d in closures)
    assert hashlib.sha256(content.encode("utf-8")).hexdigest() == cal["content_identity"]["expected_content_sha256"]
    # Known boundary / exceptional dates.
    assert "2018-12-05" in closures and "2025-01-09" in closures
    assert "2021-12-31" not in closures, "Saturday New Year's Day is not observed on the preceding Friday"
    assert "2022-06-20" in closures and "2021-06-18" not in closures, "Juneteenth is a holiday from 2022"
    sessions_per_year = {}
    d = dt.date(2016, 1, 1)
    closed = set(closures)
    while d <= dt.date(2026, 12, 31):
        if d.weekday() < 5 and d.isoformat() not in closed:
            sessions_per_year[d.year] = sessions_per_year.get(d.year, 0) + 1
        d += dt.timedelta(1)
    published = {2016: 252, 2017: 251, 2018: 251, 2019: 252, 2020: 253, 2021: 252, 2022: 251, 2023: 250,
                 2024: 252, 2025: 250}
    assert {y: sessions_per_year[y] for y in published} == published
    assert "weekday arithmetic is never a fallback" in cal["coverage_rule"].lower()


def test_stopping_rule_and_paper_live_state():
    s = NEW["batch_stopping_rule"]
    for k in ("run_all_fifteen_regardless_of_outcome", "no_early_selection", "no_h4_or_further_hypothesis",
              "no_10_month_sma_reserve", "no_batch_02b", "no_batch_03", "no_holdout_consumption"):
        assert s[k] is True
    assert NEW["selection_authority"]["max_selected"] == 1
    assert NEW["paper_live"] == {"paper": "NOT ACTIVATED", "production_promotion_state": "NOT WRITTEN",
                                 "live": "NOT TOUCHED"}
    assert NEW["holdout"]["status"] == "RESERVED / UNCONSUMED"
