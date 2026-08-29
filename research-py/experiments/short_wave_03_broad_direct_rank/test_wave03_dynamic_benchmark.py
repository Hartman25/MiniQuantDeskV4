"""WAVE03-DYNAMIC-RANKABLE-BENCHMARK-01 -- focused, network-free tests for
`rankable_set_by_date` and `build_dynamic_rankable_benchmark`. Uses only
synthetic fixture CSVs/DataFrames -- no Alpaca access, no research-py/src
modification.

R2 (WAVE03-DYNAMIC-BENCHMARK-FUTURE-EXECUTION-CHRONOLOGY-REPAIR-02): the
benchmark's causal return attribution is DECISION -> PENDING -> EXECUTED,
advanced one reference date at a time within each fold. A decision recorded
at reference date D is EXECUTED only on D's immediate successor, and first
EARNS a return on the interval ending at D's successor's successor. So
within any fold: the first (reset) date's own computed return is
irrelevant (forced 0.0 by the pre-existing convention); the SECOND date
always produces no return observation (nothing has executed yet, no
matter what was decided); only the THIRD date onward can ever carry a
real observation, governed by the decision made two reference dates
earlier (D_i is governed by RANKABLE_SET(D_{i-2})).

Covers the mission's REQUIRED TESTS 1-10 (each test's docstring cites its
number), the REQUIRED RED CONTROL, the REQUIRED DROP CONTROL, and the
PRODUCTION CONTRACT CROSS-CHECK.
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402

_REAL_RANKABLE_SET_BY_DATE = run_wave.rankable_set_by_date


# ---------------------------------------------------------------------------
# Fixture helpers
# ---------------------------------------------------------------------------


def _write_oos_csv(tmp_path: Path, rows: list[tuple[str, str, float, int]], name: str = "oos_predictions.csv") -> Path:
    """rows: list of (date_str, symbol, ml_score, fold)."""
    df = pd.DataFrame(
        [
            {
                "fold": fold,
                "symbol": sym,
                "decision_ts": f"{date}T00:00:00+00:00",
                "label_end_ts": f"{date}T00:00:00+00:00",
                "ml_score": score,
                "target": 1,
            }
            for date, sym, score, fold in rows
        ]
    )
    path = tmp_path / name
    df.to_csv(path, index=False)
    return path


def _write_economic_returns_csv(tmp_path: Path, date_fold_pairs: list[tuple[str, int]], name: str = "economic_returns.csv") -> Path:
    df = pd.DataFrame([{"fold": fold, "timestamp": f"{date}T00:00:00+00:00"} for date, fold in date_fold_pairs])
    path = tmp_path / name
    df.to_csv(path, index=False)
    return path


def _bars_df(rows: list[tuple[str, str, float]]) -> pd.DataFrame:
    """rows: list of (symbol, date, close)."""
    return pd.DataFrame([{"symbol": sym, "end_ts": f"{date} 00:00:00", "close": close} for sym, date, close in rows])


FAR_FUTURE_HOLDOUT = "2099-01-01T00:00:00Z"


# ---------------------------------------------------------------------------
# REQUIRED TEST 1: dynamic membership A/B -> B/C changes correctly
# (rankable_set_by_date is a pure membership read, unaffected by the R2
# causal-return-timing repair -- unchanged from R1.)
# ---------------------------------------------------------------------------


def test_dynamic_membership_changes_from_ab_to_bc(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1),
            ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "B", 0.5, 1),
            ("2020-01-03", "C", 0.6, 1),
        ],
    )
    by_date = run_wave.rankable_set_by_date(oos_csv)
    assert by_date["2020-01-02"] == {"A", "B"}
    assert by_date["2020-01-03"] == {"B", "C"}


# ---------------------------------------------------------------------------
# REQUIRED TEST 2: dropped symbol disappears immediately from RANKABLE_SET
# itself (a pure membership fact, distinct from when its EXIT executes --
# see the REQUIRED DROP CONTROL below for the executed-holding timing).
# ---------------------------------------------------------------------------


def test_dropped_symbol_disappears_immediately(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1),
            ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "A", 0.6, 1),
            ("2020-01-03", "B", 0.4, 1),
            ("2020-01-06", "B", 0.5, 1),  # A dropped here
        ],
    )
    by_date = run_wave.rankable_set_by_date(oos_csv)
    assert "A" not in by_date["2020-01-06"]
    assert by_date["2020-01-06"] == {"B"}


# ---------------------------------------------------------------------------
# REQUIRED TEST 3: later symbol never appears before its first rankable date
# ---------------------------------------------------------------------------


def test_later_symbol_never_appears_before_first_rankable_date(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1),
            ("2020-01-03", "A", 0.6, 1),
            ("2020-01-03", "B", 0.4, 1),  # B's first appearance
        ],
    )
    by_date = run_wave.rankable_set_by_date(oos_csv)
    assert "B" not in by_date["2020-01-02"]
    assert "B" in by_date["2020-01-03"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 4: no stale carry-forward -- EXACTLY the decision two
# reference dates prior governs a return, never an older decision reached
# by skipping past an intervening empty one, and never a more recent
# decision reached by executing one bar too early.
# ---------------------------------------------------------------------------


def test_no_stale_carry_forward_exactly_two_lag_prior_decision_governs_return(tmp_path: Path) -> None:
    """Fold: D0 (reset, empty decision), D1 (decides {A}), D2 (empty
    decision), D3. Under the R2 state machine: D3's EXECUTED membership is
    D1's decision ({A}) -- two reference dates prior -- never D2's (empty,
    which would wrongly suppress D3 if a one-lag-too-early bug reached for
    the IMMEDIATELY prior decision instead) and never D0's alone (D2's
    return, not D3's, is what D0's decision governs). D2's own return must
    be MISSING (governed by D0's empty decision), proving the implementation
    does not skip past that emptiness to reach D1's non-empty {A} instead --
    exactly the stale/short-circuited-lag defect PREDECLARED_WAVE.json's
    dynamic_cross_section policy forbids."""
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-03", "A", 0.6, 1)])  # only D1 has any OOS row
    econ_csv = _write_economic_returns_csv(
        tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1), ("2020-01-07", 1)]
    )
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0),
            ("A", "2020-01-02", 100.0),
            ("A", "2020-01-03", 100.0),
            ("A", "2020-01-06", 100.0),
            ("A", "2020-01-07", 110.0),  # only the D2->D3 leg moves
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06", "2020-01-07"], FAR_FUTURE_HOLDOUT
    )
    # D0 (2020-01-02): fold reset, forced 0.0.
    # D1 (2020-01-03): always missing -- nothing has executed yet regardless
    # of D0's (empty) decision.
    assert "2020-01-03" in result["dates_with_no_return_observation"]
    # D2 (2020-01-06): governed by D0's decision, which was empty -- missing.
    # A stale-lag bug that instead reached for D1's non-empty {A} would make
    # this a real observation.
    assert "2020-01-06" in result["dates_with_no_return_observation"]
    # D3 (2020-01-07): governed by D1's decision ({A}), two reference dates
    # prior -- real, and equal to exactly A's D2->D3 return.
    assert "2020-01-07" not in result["dates_with_no_return_observation"]
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(110.0 / 100.0 - 1.0)


# ---------------------------------------------------------------------------
# REQUIRED TEST 5: no future backfill
# ---------------------------------------------------------------------------


def test_no_future_backfill_earlier_dates_unaffected_by_later_rows(tmp_path: Path) -> None:
    """Adding a symbol's first-ever OOS row at a LATER date must not change
    any EARLIER date's rankable set -- membership is a pure function of the
    literal rows present up to and including that date, never a projection
    backward from later coverage."""
    oos_csv_without_late_symbol = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1)],
        name="without_c.csv",
    )
    oos_csv_with_late_symbol = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1), ("2020-01-06", "C", 0.7, 1)],
        name="with_c.csv",
    )
    before = run_wave.rankable_set_by_date(oos_csv_without_late_symbol)
    after = run_wave.rankable_set_by_date(oos_csv_with_late_symbol)
    assert before["2020-01-02"] == after["2020-01-02"] == {"A"}
    assert before["2020-01-03"] == after["2020-01-03"] == {"A"}
    assert "C" not in after["2020-01-02"]
    assert "C" not in after["2020-01-03"]
    assert after["2020-01-06"] == {"C"}


# ---------------------------------------------------------------------------
# REQUIRED TEST 6: exact economic date-index alignment
# ---------------------------------------------------------------------------


def test_fails_closed_when_reference_date_missing_from_economic_authority(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1)])
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1)])
    bars = _bars_df([("A", "2020-01-02", 100.0), ("A", "2020-01-03", 101.0)])
    with pytest.raises(RuntimeError, match="not identical"):
        run_wave.build_dynamic_rankable_benchmark(
            bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
        )


def test_fails_closed_when_economic_authority_has_extra_date(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-02", "A", 0.6, 1)])
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df([("A", "2020-01-02", 100.0)])
    with pytest.raises(RuntimeError, match="not identical"):
        run_wave.build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, ["2020-01-02"], FAR_FUTURE_HOLDOUT)


# ---------------------------------------------------------------------------
# REQUIRED TEST 7: fold reset is actually observed, despite a large price
# jump landing on the reset date itself. Uses 3 dates so a real (post-
# bootstrap) observation exists to confirm the reset didn't just coincide
# with an all-missing fold.
# ---------------------------------------------------------------------------


def test_fold_reset_date_forced_to_zero_despite_large_price_jump(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-02", "B", 0.4, 1)],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 200.0), ("A", "2020-01-03", 201.0), ("A", "2020-01-06", 203.0),
            ("B", "2020-01-01", 100.0), ("B", "2020-01-02", 101.0), ("B", "2020-01-03", 102.0), ("B", "2020-01-06", 103.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    # 2020-01-02's huge same-bar jump (100->200) is irrelevant -- fold reset
    # forces it to 0.0 regardless of what the underlying return would be.
    assert result["fold_reset_dates_count"] == 1
    # 2020-01-03 always misses (bootstrap -- nothing executed yet).
    assert "2020-01-03" in result["dates_with_no_return_observation"]
    # 2020-01-06 is governed by 2020-01-02's decision ({A, B}) -- real.
    expected_0106 = float(np.mean([(203.0 / 201.0) - 1.0, (103.0 / 102.0) - 1.0]))
    expected_cumulative = (1.0 + 0.0) * (1.0 + expected_0106) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)


# ---------------------------------------------------------------------------
# REQUIRED TEST 8: deterministic ordering does not affect return
# ---------------------------------------------------------------------------


def test_row_order_permutation_does_not_change_result(tmp_path: Path) -> None:
    rows = [
        ("2020-01-02", "A", 0.6, 1), ("2020-01-02", "B", 0.4, 1),
        ("2020-01-03", "A", 0.6, 1), ("2020-01-03", "B", 0.4, 1), ("2020-01-03", "C", 0.7, 1),
    ]
    oos_forward = _write_oos_csv(tmp_path, rows, name="forward.csv")
    oos_reversed = _write_oos_csv(tmp_path, list(reversed(rows)), name="reversed.csv")
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars_forward = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 103.0), ("A", "2020-01-06", 107.0),
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 49.0), ("B", "2020-01-03", 51.0), ("B", "2020-01-06", 53.0),
            ("C", "2020-01-01", 10.0), ("C", "2020-01-02", 10.5), ("C", "2020-01-03", 10.2), ("C", "2020-01-06", 10.8),
        ]
    )
    bars_shuffled = bars_forward.iloc[::-1].reset_index(drop=True)

    result_forward = run_wave.build_dynamic_rankable_benchmark(
        bars_forward, oos_forward, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    result_reversed = run_wave.build_dynamic_rankable_benchmark(
        bars_shuffled, oos_reversed, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    # 2020-01-06 is governed by 2020-01-02's decision {A, B} (C is excluded --
    # C only first appears in RANKABLE_SET at 2020-01-03) -- a genuine,
    # non-trivial real observation both permutations must agree on exactly.
    assert result_forward["cumulative_return_over_reference_dates"] == pytest.approx(
        result_reversed["cumulative_return_over_reference_dates"]
    )
    assert result_forward["rankable_cross_section_size_by_date"] == result_reversed["rankable_cross_section_size_by_date"]
    assert result_forward["dates_with_no_return_observation"] == result_reversed["dates_with_no_return_observation"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 9: zero/empty rankable set -- explicit documented contracts,
# and REQUIRED CONTROL 10: rankable_cross_section_size_by_date stays tied to
# the exact (unshifted) decision-date OOS membership, never to the shifted
# EXECUTED holdings the causal return attribution actually uses.
# ---------------------------------------------------------------------------


def test_empty_rankable_set_date_produces_no_return_observation_not_an_error(tmp_path: Path) -> None:
    """Explicit contract (see build_dynamic_rankable_benchmark docstring):
    a reference date with zero rankable symbols never raises and is never
    silently defaulted to a 0.0 return -- it is excluded from the return
    series and recorded in dates_with_no_return_observation, UNLESS it is
    also a genuine fold-reset date (a distinct, unrelated 0.0 convention).
    This test only inspects the reset date's own (unshifted) properties, so
    it is unaffected by the R2 causal-timing repair."""
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-03", "A", 0.6, 1)])
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df([("A", "2020-01-02", 100.0), ("A", "2020-01-03", 101.0)])
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    # 2020-01-02 is both the fold-reset date AND has zero rankable symbols --
    # the fold-reset 0.0 convention wins (distinct from the empty-set contract).
    assert result["rankable_cross_section_size_by_date"]["2020-01-02"] == 0
    assert "2020-01-02" not in result["dates_with_no_return_observation"]
    assert result["dates_with_zero_rankable_symbols"] == ["2020-01-02"]


def test_cross_section_size_reflects_unshifted_decision_not_shifted_execution(tmp_path: Path) -> None:
    """REQUIRED CONTROL 10. Fold: D0 (reset, empty decision), D1 (decides
    {A}), D2 (decides {C} -- non-empty), D3. D2's OWN rankable_cross_section
    size must be 1 (RANKABLE_SET(D2)={C}, an unshifted signal-availability
    fact), even though D2's causal EXECUTED return is MISSING (governed by
    D0's decision, which was empty) -- proving the reported cross-section
    size is never silently swapped for whatever the shifted execution
    state happens to be that date."""
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-03", "A", 0.6, 1), ("2020-01-06", "C", 0.5, 1)])
    econ_csv = _write_economic_returns_csv(
        tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1), ("2020-01-07", 1)]
    )
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100.0),
            ("A", "2020-01-06", 100.0), ("A", "2020-01-07", 110.0),
            ("C", "2020-01-01", 10.0), ("C", "2020-01-02", 10.0), ("C", "2020-01-03", 10.0),
            ("C", "2020-01-06", 10.0), ("C", "2020-01-07", 10.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06", "2020-01-07"], FAR_FUTURE_HOLDOUT
    )
    # RANKABLE_SET(2020-01-06) = {C} -- unshifted signal-availability fact.
    assert result["rankable_cross_section_size_by_date"]["2020-01-06"] == 1
    # But 2020-01-06's causal return is MISSING (governed by D0's empty
    # decision) -- decoupled from the non-zero cross-section size above.
    assert "2020-01-06" in result["dates_with_no_return_observation"]
    # 2020-01-07 is governed by D1's decision ({A}) -- real.
    assert "2020-01-07" not in result["dates_with_no_return_observation"]
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(110.0 / 100.0 - 1.0)


# ---------------------------------------------------------------------------
# MISSING BAR SAFETY: an executed, non-empty membership whose price bar is
# simply absent on the reference date must produce a missing observation,
# never a fabricated return.
# ---------------------------------------------------------------------------


def test_missing_bar_for_an_executed_symbol_produces_missing_not_fabricated_return(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-02", "A", 0.6, 1)])
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    # A has no bar row at all on 2020-01-06, the date its D0 decision governs.
    bars = _bars_df([("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100.0)])
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    assert "2020-01-06" in result["dates_with_no_return_observation"]
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(0.0)  # only the forced reset date used


# ---------------------------------------------------------------------------
# REQUIRED TEST 10: benchmark never reads final-holdout rows
# ---------------------------------------------------------------------------


def test_fails_closed_on_reference_date_at_or_after_holdout(tmp_path: Path) -> None:
    bars = _bars_df([("A", "2020-01-02", 100.0)])
    with pytest.raises(RuntimeError, match="reserved holdout"):
        run_wave.build_dynamic_rankable_benchmark(
            bars, Path("unused.csv"), Path("unused2.csv"), ["2020-01-02"], "2020-01-01T00:00:00Z"
        )


def test_holdout_region_rows_never_influence_the_result(tmp_path: Path) -> None:
    """bars/oos_predictions_csv both legitimately span dates AFTER the
    holdout boundary (the full wave fetch/OOS stream is not pre-truncated) --
    proves the benchmark's OWN result is identical whether or not those
    post-holdout rows are present, i.e. it structurally never reads them.
    Uses 3 reference dates so the compared result carries a genuine,
    non-trivial real return rather than an all-missing/reset fold."""
    reference_dates = ["2020-01-02", "2020-01-03", "2020-01-06"]
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])

    oos_without_holdout_rows = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1)],
        name="no_holdout.csv",
    )
    oos_with_holdout_rows = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1),
            ("2020-06-01", "A", 0.99, 2), ("2020-06-01", "Z", 0.01, 2),
        ],
        name="with_holdout.csv",
    )
    bars_without_holdout_rows = _bars_df(
        [("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 102.0), ("A", "2020-01-06", 104.0)]
    )
    bars_with_holdout_rows = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 102.0), ("A", "2020-01-06", 104.0),
            ("A", "2020-06-01", 9999.0), ("Z", "2020-06-01", 1.0),
        ]
    )
    holdout_start = "2020-02-01T00:00:00Z"

    result_without = run_wave.build_dynamic_rankable_benchmark(
        bars_without_holdout_rows, oos_without_holdout_rows, econ_csv, reference_dates, holdout_start
    )
    result_with = run_wave.build_dynamic_rankable_benchmark(
        bars_with_holdout_rows, oos_with_holdout_rows, econ_csv, reference_dates, holdout_start
    )
    assert result_without == result_with
    # 2020-01-06 (governed by 2020-01-02's decision, {A}) carries a genuine
    # real observation in both -- not an all-missing/reset comparison.
    assert "2020-01-06" not in result_without["dates_with_no_return_observation"]


# ---------------------------------------------------------------------------
# REQUIRED RED CONTROL: the mission's exact synthetic fold. A first becomes
# rankable at T1, immediately followed by an enormous T1->T2 price jump. The
# CURRENT (pre-R2, single-lag) implementation incorrectly captures that
# jump; the repaired (R2) implementation must not, and must only pick A up
# for the later, modest T2->T3 interval.
# ---------------------------------------------------------------------------


def _pre_r2_single_lag_BROKEN_build_dynamic_rankable_benchmark(
    bars: pd.DataFrame, oos_predictions_csv: Path, economic_returns_csv: Path,
    reference_dates: list[str], holdout_start_utc: str,
) -> dict:
    """Deliberately buggy reference implementation reproducing the CURRENT
    1f92794 (pre-R2, R1-only single-lag) defect this mission repairs:
    attributes the return ending at reference date T to RANKABLE_SET(T_prev)
    (the IMMEDIATELY preceding reference date), one bar too early -- a
    decision recorded at T_prev is treated as already executed by T,
    instead of only executing at T and first earning a return at T's
    successor. Used ONLY by the mutation/negative proof below -- never call
    from production code."""
    holdout_ts = pd.Timestamp(holdout_start_utc)
    if holdout_ts.tzinfo is None:
        holdout_ts = holdout_ts.tz_localize("UTC")
    authority = run_wave.economic_fold_date_authority(economic_returns_csv)
    fold_start_dates = authority["reset_dates"]
    fold_of_date = authority["fold_of_date"]
    reference_date_set = set(pd.Index(reference_dates).astype(str))
    rankable = run_wave.rankable_set_by_date(oos_predictions_csv)
    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")
    b["daily_ret"] = b.groupby("symbol")["close"].pct_change()
    sorted_dates = sorted(reference_date_set)
    per_date: dict[str, float] = {}
    for i, d in enumerate(sorted_dates):
        if d in fold_start_dates:
            continue
        prev_d = sorted_dates[i - 1]
        if fold_of_date.get(prev_d) != fold_of_date.get(d):
            continue
        syms = rankable.get(prev_d, set())  # BUG: one bar too early
        if not syms:
            per_date[d] = float("nan")
            continue
        day_rets = b.loc[(b["date"] == d) & (b["symbol"].isin(syms)), "daily_ret"].dropna()
        per_date[d] = float(day_rets.mean()) if len(day_rets) else float("nan")
    per_date_series = pd.Series(per_date).reindex(sorted_dates)
    for d in fold_start_dates & reference_date_set:
        per_date_series.loc[d] = 0.0
    daily_series = per_date_series.dropna()
    cumulative_return = float(np.prod(1.0 + daily_series.to_numpy()) - 1.0) if len(daily_series) else None
    return {"cumulative_return_over_reference_dates": cumulative_return, "per_date": per_date_series.to_dict()}


def test_required_red_control_mission_fixture_current_captures_jump_repaired_does_not(tmp_path: Path) -> None:
    """REQUIRED RED CONTROL, exact mission fixture: T0 (reset), T1 (A first
    rankable), T2 (huge T1->T2 jump), T3 (modest T2->T3 return)."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-03", "A", 0.6, 1), ("2020-01-06", "A", 0.6, 1)],  # A first rankable at T1=01-03
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1), ("2020-01-07", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-03", 100.0),      # T1
            ("A", "2020-01-06", 100000.0),   # T2 -- enormous T1->T2 jump
            ("A", "2020-01-07", 101000.0),   # T3 -- modest T2->T3 return
        ]
    )
    reference_dates = ["2020-01-02", "2020-01-03", "2020-01-06", "2020-01-07"]

    broken = _pre_r2_single_lag_BROKEN_build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT)
    repaired = run_wave.build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT)

    # CURRENT (pre-R2, single-lag): T2's return is governed by RANKABLE_SET
    # (T1)={A} -- incorrectly captures the ~1000x T1->T2 jump.
    assert broken["per_date"]["2020-01-06"] == pytest.approx(100000.0 / 100.0 - 1.0)
    assert broken["cumulative_return_over_reference_dates"] > 100.0

    # REPAIRED (R2): T2 is governed by T0's decision (empty, A not yet
    # rankable) -- structurally excluded, never attributed at all.
    assert "2020-01-06" in repaired["dates_with_no_return_observation"]
    # T3 is governed by T1's decision ({A}) -- A's first legitimate
    # contribution, the modest T2->T3 move only.
    assert "2020-01-07" not in repaired["dates_with_no_return_observation"]
    expected_repaired_cumulative = 101000.0 / 100000.0 - 1.0
    assert repaired["cumulative_return_over_reference_dates"] == pytest.approx(expected_repaired_cumulative)
    assert repaired["cumulative_return_over_reference_dates"] < 1.0  # never sees the ~1000x jump


# ---------------------------------------------------------------------------
# REQUIRED DROP CONTROL: a symbol already an EXECUTED holding before T,
# dropped from RANKABLE_SET AT T, must still earn T->T+1 (its exit has not
# executed yet) but must NOT earn T+1->T+2 (its exit executes at T+1).
# ---------------------------------------------------------------------------


def test_required_drop_control_symbol_earns_one_more_interval_before_exit_executes(tmp_path: Path) -> None:
    """Fold: D0 (reset), D1 (decides {A, B} -- this governs T's return),
    D2=T-1 (decides {A, B} -- still holding, this governs T->T+1),
    D3=T (drops A, decides {B} -- this governs T+1->T+2), D4=T+1, D5=T+2.
    A is already executed BEFORE T (via D1's decision), gets dropped AT T,
    and must earn exactly one more interval (T->T+1) before its exit
    executes and stops earning (T+1->T+2)."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-03", "A", 0.6, 1), ("2020-01-03", "B", 0.4, 1),  # D1
            ("2020-01-06", "A", 0.6, 1), ("2020-01-06", "B", 0.4, 1),  # D2 = T-1
            ("2020-01-07", "B", 0.4, 1),                                # D3 = T -- A dropped
        ],
    )
    econ_csv = _write_economic_returns_csv(
        tmp_path,
        [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1), ("2020-01-07", 1), ("2020-01-08", 1), ("2020-01-09", 1)],
    )
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100.0),
            ("A", "2020-01-06", 100.0), ("A", "2020-01-07", 100.0),
            ("A", "2020-01-08", 200.0),      # T->T+1: A must earn this ~100% move
            ("A", "2020-01-09", 100000.0),   # T+1->T+2: A must NOT earn this huge move
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 50.0), ("B", "2020-01-03", 50.0),
            ("B", "2020-01-06", 50.0), ("B", "2020-01-07", 50.0), ("B", "2020-01-08", 51.0), ("B", "2020-01-09", 52.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv,
        ["2020-01-02", "2020-01-03", "2020-01-06", "2020-01-07", "2020-01-08", "2020-01-09"],
        FAR_FUTURE_HOLDOUT,
    )
    r_t_to_t_plus_1 = float(np.mean([200.0 / 100.0 - 1.0, 51.0 / 50.0 - 1.0]))  # A STILL earns this (executed via D2)
    r_t_plus_1_to_t_plus_2 = 52.0 / 51.0 - 1.0  # A does NOT earn this -- B only (executed via D3, which dropped A)
    assert r_t_plus_1_to_t_plus_2 < 1.0  # sanity: nowhere near A's huge available move
    expected_cumulative = (1.0 + 0.0) * (1.0 + r_t_to_t_plus_1) * (1.0 + r_t_plus_1_to_t_plus_2) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)
    # D1 (2020-01-03) and D2 (2020-01-06) both miss for unrelated structural
    # reasons (D1 = bootstrap; D2 governed by D0's empty decision).
    assert "2020-01-03" in result["dates_with_no_return_observation"]
    assert "2020-01-06" in result["dates_with_no_return_observation"]
    assert result["daily_return_observations_used"] == 4  # D0(reset,0.0), D3, D4, D5


# ---------------------------------------------------------------------------
# ADDITIONAL COVERAGE: prior membership truthfully governs the interval
# ending at a reference date, distinguishing it from BOTH a one-lag-too-
# recent decision AND a same-date decision.
# ---------------------------------------------------------------------------


def test_prior_membership_truthfully_governs_interval_ending_at_t(tmp_path: Path) -> None:
    """D0 decides {A} (must govern D2's return). D1 decides {Y} (a one-lag-
    too-recent distractor -- would wrongly govern D2 under a pre-R1 same-
    bar-adjacent-off-by-one bug). D2 itself decides {Z} (a same-date
    distractor -- would wrongly govern under the pre-R1 same-bar bug). Y and
    Z both have wild, easily-detectable price moves on the D1->D2 interval;
    A has a modest, clean one. Only A's modest move must show up."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "Y", 0.1, 1), ("2020-01-06", "Z", 0.1, 1)],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100.0), ("A", "2020-01-06", 110.0),
            ("Y", "2020-01-01", 10.0), ("Y", "2020-01-02", 10.0), ("Y", "2020-01-03", 10.0), ("Y", "2020-01-06", 1000.0),
            ("Z", "2020-01-01", 5.0), ("Z", "2020-01-02", 5.0), ("Z", "2020-01-03", 5.0), ("Z", "2020-01-06", 0.05),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    # If Y (100x) or Z (-99%) incorrectly governed, the result would be
    # wildly different from A's clean 10% move.
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(110.0 / 100.0 - 1.0)


# ---------------------------------------------------------------------------
# FOLD BOUNDARY: no membership, pending target, or executed target may
# cross a fold boundary -- each fold bootstraps EXECUTED=empty/PENDING=None
# independently, never seeded from the previous fold's ending state.
# ---------------------------------------------------------------------------


def test_no_membership_crosses_fold_boundaries(tmp_path: Path) -> None:
    """Fold 1 (3 dates) decides {A} throughout and legitimately earns a real
    10% return on its own 3rd date. Fold 2 (3 dates) decides {B} at its own
    reset date and must earn a huge, unambiguous ~20000x jump on ITS OWN
    3rd date -- governed by fold 2's own reset-date decision, never by
    fold 1's ending membership. A is deliberately ALSO given bars on fold
    2's dates with a tiny, easily-distinguished return: if fold 2 leaked
    fold 1's stale {A} decision, the result would show A's tiny move
    instead of B's enormous one."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-04-01", "B", 0.6, 2)],
    )
    econ_csv = _write_economic_returns_csv(
        tmp_path,
        [
            ("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1),
            ("2020-04-01", 2), ("2020-04-02", 2), ("2020-04-03", 2),
        ],
    )
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100.0), ("A", "2020-01-06", 110.0),
            ("A", "2020-04-02", 100.0), ("A", "2020-04-03", 101.0),  # tiny 1% move if (wrongly) leaked into fold 2
            ("B", "2020-03-31", 50.0), ("B", "2020-04-01", 50.0), ("B", "2020-04-02", 50.0), ("B", "2020-04-03", 1000000.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv,
        ["2020-01-02", "2020-01-03", "2020-01-06", "2020-04-01", "2020-04-02", "2020-04-03"],
        FAR_FUTURE_HOLDOUT,
    )
    assert result["fold_reset_dates_count"] == 2
    # Both folds' 2nd dates always miss (bootstrap, independently per fold).
    assert "2020-01-03" in result["dates_with_no_return_observation"]
    assert "2020-04-02" in result["dates_with_no_return_observation"]
    r_fold1 = 110.0 / 100.0 - 1.0
    r_fold2 = 1000000.0 / 50.0 - 1.0
    expected_cumulative = (1.0 + 0.0) * (1.0 + r_fold1) * (1.0 + 0.0) * (1.0 + r_fold2) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)
    assert result["cumulative_return_over_reference_dates"] > 1000.0  # unambiguously B's jump, not A's 1%


# ---------------------------------------------------------------------------
# PRODUCTION CONTRACT CROSS-CHECK: reversing the two operations (executing
# the pending decision BEFORE attributing the incoming return, instead of
# after) collapses back into the pre-R2 single-lag defect -- proving the
# "incoming return before pending execution" ordering is load-bearing, not
# cosmetic. Mirrors execution_rules.md's orchestrator phase-ordering
# invariant (outbox claim before broker submit; inbound apply before
# portfolio update) applied to this benchmark's own return/execution pair.
# ---------------------------------------------------------------------------


def test_production_contract_return_before_execution_ordering_is_load_bearing(tmp_path: Path) -> None:
    """Executing the pending target before attributing the incoming return
    is mathematically identical to the pre-R2 single-lag defect (a decision
    recorded on the immediately preceding date would be treated as already
    executed). Reusing the REQUIRED RED CONTROL fixture: the reversed-order
    (broken) implementation and the correctly-ordered (repaired) production
    function must disagree at T2, proving the ordering is load-bearing."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-03", "A", 0.6, 1), ("2020-01-06", "A", 0.6, 1)],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1), ("2020-01-07", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-03", 100.0),
            ("A", "2020-01-06", 100000.0),
            ("A", "2020-01-07", 101000.0),
        ]
    )
    reference_dates = ["2020-01-02", "2020-01-03", "2020-01-06", "2020-01-07"]

    reversed_order_broken = _pre_r2_single_lag_BROKEN_build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT
    )
    repaired = run_wave.build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT)

    assert reversed_order_broken["cumulative_return_over_reference_dates"] != pytest.approx(
        repaired["cumulative_return_over_reference_dates"]
    )
    with pytest.raises(AssertionError):
        assert reversed_order_broken["cumulative_return_over_reference_dates"] == pytest.approx(
            repaired["cumulative_return_over_reference_dates"]
        )


# ---------------------------------------------------------------------------
# MUTATION / NEGATIVE PROOF: a deliberately stale-membership implementation
# fails the "dropped symbol disappears immediately" invariant (REQUIRED
# TEST 2, above). Tests rankable_set_by_date only, unaffected by R2.
# ---------------------------------------------------------------------------


def _stale_carry_forward_rankable_set_by_date_BROKEN(oos_predictions_csv: Path) -> dict[str, set[str]]:
    """Deliberately buggy variant used ONLY by the negative-mutation proof
    below: unions each date's rankable set into a running total instead of
    reading each date's literal rows -- exactly the stale-membership defect
    PREDECLARED_WAVE.json's dynamic_cross_section policy forbids
    ('no_stale_carry_forward': true). Never call this from production code."""
    real = _REAL_RANKABLE_SET_BY_DATE(oos_predictions_csv)
    out: dict[str, set[str]] = {}
    running: set[str] = set()
    for d in sorted(real.keys()):
        running = running | real[d]
        out[d] = set(running)
    return out


def test_mutation_proof_stale_membership_implementation_fails_dropped_symbol_invariant(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1),
            ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "A", 0.6, 1),
            ("2020-01-03", "B", 0.4, 1),
            ("2020-01-06", "B", 0.5, 1),  # A dropped here
        ],
    )
    real_result = run_wave.rankable_set_by_date(oos_csv)
    assert "A" not in real_result["2020-01-06"]  # REQUIRED TEST 2 passes for the real implementation

    broken_result = _stale_carry_forward_rankable_set_by_date_BROKEN(oos_csv)
    assert "A" in broken_result["2020-01-06"]  # the stale-carry-forward defect keeps A around
    with pytest.raises(AssertionError):
        assert "A" not in broken_result["2020-01-06"]  # REQUIRED TEST 2's own assertion, proven to fail here
