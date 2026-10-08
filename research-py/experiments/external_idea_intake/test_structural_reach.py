"""Calendar-only analysis: window construction is checked against independent facts, the accounting-contract facts it
relies on are pinned to the Rust sources, and each reported quantity is tested as exactly what it is (exact exposure,
an upper bound, a hypothetical illustration). No price is read."""

from __future__ import annotations

import datetime as dt
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import structural_reach as sr  # noqa: E402

ROOT = HERE.parents[2]
CRATES = ROOT / "core-rs/crates"
S = sr.summary()
PRE_S = S["EXT-032 pre-holiday, scheduled holidays only"]
PRE_A = S["EXT-032 pre-holiday, every full closure"]
SANTA = S["EXT-169 Santa Claus"]
OPEX = S["EXT-044 opex week (monthly third Friday)"]


# ---- the accounting contract this analysis depends on ---------------------------------------------------------

def test_profitable_months_rule_is_strict_month_over_month_growth_on_the_candidates_own_equity_curve():
    decl = json.loads((ROOT / "research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_03.json").read_text("utf-8"))
    assert decl["promotion_policy"]["MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT"] == sr.MIN_PROFITABLE_MONTHS_PCT
    ev = (CRATES / "mqk-promotion/src/evaluator.rs").read_text("utf-8")
    assert "if w[1] > w[0] {" in ev and "profitable += 1;" in ev                 # a flat month never counts
    assert "let eq = &input.report.equity_curve;" in ev                           # the candidate's own curve
    assert "month_equity.insert(month_id, equity);" in ev                         # last equity of each UTC month
    assert "metrics.profitable_months_pct < config.min_profitable_months_pct" in ev


def test_backtest_equity_is_cash_plus_mark_to_market_with_no_interest_or_financing_term():
    eng = (CRATES / "mqk-backtest/src/engine.rs").read_text("utf-8")
    assert "compute_equity_micros(" in eng and "self.portfolio.cash_micros" in eng
    for term in ("interest", "financing", "carry_rate", "funding_rate"):
        assert term not in eng.lower(), term


def test_benchmark_alpha_is_a_difference_of_two_account_returns_with_the_first_entry_quantity_held_from_the_first_entry():
    src = (CRATES / "mqk-backtest/src/benchmark_capital_fraction.rs").read_text("utf-8")
    assert "alpha_pct: candidate_total_return_pct - account_return_pct," in src
    assert ".min_by_key(|e| e.reference_bar_end_ts)" in src                       # the candidate's FIRST entry
    assert "let qty = if idx >= self.entry_bar_index {" in src                    # held from that bar, not from bar 0
    assert "bench_cfg.sizing_policy = SizingPolicy::FixedQuantityV1;" in src      # one fixed quantity, never re-sized


# ---- exact calendar exposure ------------------------------------------------------------------------------------

def test_evaluation_window_is_the_native_fold_window_and_excludes_the_holdout():
    sess = sr.eval_sessions()
    assert sess[0] == dt.date(2016, 3, 1) and sess[-1] < dt.date(2026, 3, 1)
    assert len(sess) == 2514


def test_pre_holiday_windows_are_two_sessions_before_each_closure_and_scheduled_only_drops_ad_hoc_closures():
    members = sr.pre_holiday(scheduled_only=True)
    assert dt.date(2016, 5, 26) in members and dt.date(2016, 5, 27) in members      # Memorial Day 2016-05-30 (Mon)
    assert dt.date(2016, 5, 25) not in members
    assert dt.date(2016, 3, 23) in members and dt.date(2016, 3, 24) in members      # Good Friday 2016-03-25
    allc = sr.pre_holiday(scheduled_only=False)
    assert dt.date(2018, 12, 4) in allc and dt.date(2018, 12, 4) not in members
    assert len(allc) - len(members) == 4        # two ad hoc closures x two sessions each (inside 2016-2026)


def test_santa_window_is_seven_sessions_per_year_crossing_the_year_end():
    members = sr.santa_claus()
    assert {dt.date(2016, 12, 23), dt.date(2016, 12, 27), dt.date(2016, 12, 30), dt.date(2017, 1, 3), dt.date(2017, 1, 4)} <= members
    assert dt.date(2017, 1, 5) not in members and dt.date(2016, 12, 22) not in members
    per_year = [sum(1 for s in members if (s.year, s.month) in ((y, 12), (y + 1, 1))) for y in range(2016, 2025)]
    assert per_year == [7] * 9


def test_opex_window_ends_on_the_third_friday_or_the_preceding_session_when_it_is_a_closure():
    members = sr.opex_week()
    assert dt.date(2016, 3, 18) in members and dt.date(2016, 3, 14) in members and dt.date(2016, 3, 21) not in members
    assert dt.date(2016, 3, 25) not in members and dt.date(2016, 3, 24) not in members
    assert dt.date(2019, 4, 18) in members and dt.date(2019, 4, 19) not in members   # Good Friday is the third Friday


def test_exact_exposure_numbers_are_pinned():
    assert (PRE_S["exposed_sessions"], PRE_S["months_with_exposure"]) == (186, 89)
    assert (SANTA["exposed_sessions"], SANTA["months_with_exposure"]) == (70, 20)
    assert (OPEX["exposed_sessions"], OPEX["months_with_exposure"]) == (586, 120)
    assert PRE_A["exposed_sessions"] == PRE_S["exposed_sessions"] + 4


# ---- months in which equity can change; the profitable-months upper bound -----------------------------------------

def test_the_month_of_the_closing_fill_is_a_month_in_which_equity_can_change():
    # held Thu 2016-04-28 and Fri 2016-04-29; the next session is Mon 2016-05-02, where the exit fills
    assert sr.pnl_months({dt.date(2016, 4, 28), dt.date(2016, 4, 29)}) == {(2016, 4), (2016, 5)}
    # a run ending mid-month adds no extra month
    assert sr.pnl_months({dt.date(2016, 4, 12)}) == {(2016, 4)}
    # two adjacent held sessions form one run: only the session after the last one is added
    assert sr.pnl_months({dt.date(2016, 4, 14), dt.date(2016, 4, 15)}) == {(2016, 4)}


def test_pre_holiday_bound_includes_the_closing_fill_month_and_is_a_valid_upper_bound_not_a_forecast():
    assert PRE_S["months_with_possible_equity_change"] == 96 > PRE_S["months_with_exposure"]
    assert round(PRE_S["max_profitable_months_fraction"], 4) == 0.7983
    assert PRE_S["profitable_months_gate_reachable"] is True       # not excluded; whether it is met needs prices


def test_santa_claus_cannot_reach_the_declared_profitable_months_gate_on_any_continuous_equity_curve_over_six_months():
    assert SANTA["months_with_possible_equity_change"] == 20
    assert round(SANTA["max_profitable_months_fraction"], 4) == 0.1681 and SANTA["profitable_months_gate_reachable"] is False
    members = sr.santa_claus()
    assert sr.max_reachable_span_months(members) == 6
    # independent arithmetic: Nov..Apr has 5 transitions of which Dec and Jan can rise: 2/5 = 0.40 (reachable);
    # Nov..May has 6 transitions: 2/6 < 0.40
    assert 2 / 5 >= sr.MIN_PROFITABLE_MONTHS_PCT > 2 / 6


def test_span_search_never_counts_the_first_month_of_a_span_because_it_has_no_previous_month_end():
    # activity only in the first covered month can never produce a transition inside the coverage
    assert sr.max_reachable_span_months({dt.date(2016, 1, 4)}) == 0
    # activity in February 2016 can: January -> February is a transition (span of 2 months, 1/1)
    assert sr.max_reachable_span_months({dt.date(2016, 2, 1)}) == 3      # Jan, Feb, Mar: 1 of 2 transitions >= 0.40


def test_low_exposure_alone_does_not_make_the_gate_unreachable():
    for other in (PRE_S, PRE_A, OPEX):
        assert other["profitable_months_gate_reachable"] is True
    assert sr.max_reachable_span_months(sr.pre_holiday(scheduled_only=True)) == 132    # the whole 2016-2026 coverage
    assert sr.max_reachable_span_months(sr.opex_week()) == 132


# ---- hypothetical illustration is labelled as such and is not a Promotion requirement --------------------------------

def test_dilution_multiple_is_reported_only_under_the_hypothetical_name():
    for r in (PRE_S, SANTA, OPEX):
        assert "required_exposed_session_return_multiple_of_passive" not in r
        assert r["hypothetical_uniform_dilution_multiple"] == r["eval_sessions"] / r["exposed_sessions"]
    assert round(SANTA["hypothetical_uniform_dilution_multiple"], 1) == 35.9
    assert round(PRE_S["hypothetical_uniform_dilution_multiple"], 1) == 13.5
    assert round(OPEX["hypothetical_uniform_dilution_multiple"], 1) == 4.3
    doc = sr.__doc__
    assert "HYPOTHETICAL ILLUSTRATION (not a requirement)" in doc and "depends on prices" in doc


def test_reach_is_a_pure_function_of_the_calendar_not_of_prices():
    import ast
    tree = ast.parse((HERE / "structural_reach.py").read_text("utf-8"))
    idents = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Name):
            idents.add(node.id)
        elif isinstance(node, ast.Attribute):
            idents.add(node.attr)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            idents.update(a.name.split(".")[0] for a in node.names)
            if isinstance(node, ast.ImportFrom) and node.module:
                idents.add(node.module.split(".")[0])
    for banned in ("pandas", "numpy", "sqlite3", "requests", "alpaca", "urllib", "socket", "open", "read_csv"):
        assert banned not in idents, banned
    r = sr.reach({dt.date(2016, 3, 1)})
    assert r["exposed_sessions"] == 1 and r["hypothetical_uniform_dilution_multiple"] == r["eval_sessions"]


@pytest.mark.parametrize("members,expected", [
    (set(), 0), ({dt.date(2016, 3, 1)}, 0), ({dt.date(2016, 4, 1)}, 1),
])
def test_reach_counts_only_months_that_have_a_previous_month_end_to_compare_with(members, expected):
    r = sr.reach(members)
    assert r["hypothetical_uniform_dilution_multiple"] is None or r["exposed_sessions"] > 0
    assert round(r["max_profitable_months_fraction"] * (r["months"] - 1)) == expected
