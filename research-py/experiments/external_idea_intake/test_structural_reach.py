"""Calendar-only structural reach: window construction is checked against independent facts, the gate constants are
tied to the frozen declarations and the Rust evaluator, and the headline numbers are pinned. No price is read."""

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
S = sr.summary()
PRE_S = S["EXT-032 pre-holiday, scheduled holidays only"]
PRE_A = S["EXT-032 pre-holiday, every full closure"]
SANTA = S["EXT-169 Santa Claus"]
OPEX = S["EXT-044 opex week (monthly third Friday)"]


def test_gate_constants_match_the_frozen_declaration_and_the_rust_evaluator():
    decl = json.loads((ROOT / "research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_03.json").read_text("utf-8"))
    assert decl["promotion_policy"]["MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT"] == sr.MIN_PROFITABLE_MONTHS_PCT
    ev = (ROOT / "core-rs/crates/mqk-promotion/src/evaluator.rs").read_text("utf-8")
    assert "if w[1] > w[0] {" in ev and "profitable += 1;" in ev       # strictly greater: a flat month never counts
    assert "metrics.profitable_months_pct < config.min_profitable_months_pct" in ev


def test_evaluation_window_is_the_native_fold_window_and_excludes_the_holdout():
    sess = sr.eval_sessions()
    assert sess[0] == dt.date(2016, 3, 1) and sess[-1] < dt.date(2026, 3, 1)
    assert len(sess) == 2514


def test_pre_holiday_windows_are_two_sessions_before_each_closure_and_scheduled_only_drops_ad_hoc_closures():
    members = sr.pre_holiday(scheduled_only=True)
    assert dt.date(2016, 5, 26) in members and dt.date(2016, 5, 27) in members      # Memorial Day 2016-05-30 (Mon)
    assert dt.date(2016, 5, 25) not in members
    assert dt.date(2016, 3, 23) in members and dt.date(2016, 3, 24) in members      # Good Friday 2016-03-25
    # ad hoc closure 2018-12-05: its predecessors 12-03 and 12-04 are only members when every closure is used
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
    assert dt.date(2016, 3, 25) not in members                          # Good Friday closure is never a session
    assert dt.date(2016, 3, 24) not in members                          # 2016-03-18 was the March expiry, not the 24th
    # April 2019: third Friday (04-19) is Good Friday, a closure, so the expiry shifts to Thursday 04-18
    assert dt.date(2019, 4, 18) in members and dt.date(2019, 4, 19) not in members


def test_headline_structural_numbers_are_pinned():
    assert (PRE_S["exposed_sessions"], PRE_S["months_with_exposure"]) == (186, 89)
    assert (SANTA["exposed_sessions"], SANTA["months_with_exposure"]) == (70, 20)
    assert (OPEX["exposed_sessions"], OPEX["months_with_exposure"]) == (586, 120)
    assert round(SANTA["max_profitable_months_fraction"], 4) == 0.1681
    assert round(PRE_S["max_profitable_months_fraction"], 4) == 0.7395
    assert round(SANTA["required_exposed_session_return_multiple_of_passive"], 1) == 35.9
    assert round(PRE_S["required_exposed_session_return_multiple_of_passive"], 1) == 13.5
    assert round(OPEX["required_exposed_session_return_multiple_of_passive"], 1) == 4.3


def test_santa_claus_cannot_reach_the_declared_profitable_months_gate_whatever_it_earns():
    assert SANTA["max_profitable_months_fraction"] < sr.MIN_PROFITABLE_MONTHS_PCT
    assert SANTA["profitable_months_gate_reachable"] is False
    for other in (PRE_S, PRE_A, OPEX):
        assert other["profitable_months_gate_reachable"] is True


def test_reach_is_a_pure_function_of_the_calendar_not_of_prices():
    src = (HERE / "structural_reach.py").read_text("utf-8")
    for banned in ("pandas", "numpy", "sqlite3", "requests", "alpaca", "close", "return_pct"):
        assert banned not in src.replace("returns", "").replace("return multiple", ""), banned
    assert sr.reach({dt.date(2016, 3, 1)})["exposed_sessions"] == 1


@pytest.mark.parametrize("members,expected", [
    (set(), 0), ({dt.date(2016, 3, 1)}, 0), ({dt.date(2016, 4, 1)}, 1),
])
def test_reach_counts_only_months_that_have_a_previous_month_end_to_compare_with(members, expected):
    r = sr.reach(members)
    assert round(r["max_profitable_months_fraction"] * (r["months"] - 1)) == expected
