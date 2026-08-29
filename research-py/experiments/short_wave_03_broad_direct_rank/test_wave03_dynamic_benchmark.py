"""WAVE03-DYNAMIC-RANKABLE-BENCHMARK-01 -- focused, network-free tests for
`rankable_set_by_date` and `build_dynamic_rankable_benchmark`. Uses only
synthetic fixture CSVs/DataFrames -- no Alpaca access, no research-py/src
modification.

Covers the mission's REQUIRED TESTS 1-10 (each test's docstring cites its
number) plus the required mutation/negative proof.
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
# REQUIRED TEST 2: dropped symbol disappears immediately
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
# REQUIRED TEST 4: no stale carry forward
# ---------------------------------------------------------------------------


def test_no_stale_carry_forward_only_immediately_prior_date_governs_return(tmp_path: Path) -> None:
    """R1 (WAVE03-DYNAMIC-BENCHMARK-CAUSALITY-REPAIR-01): under causal return
    attribution, the return ending at reference date T is governed by
    EXACTLY the immediately-preceding reference date's RANKABLE_SET, never
    an earlier date reached through it. A date with an empty rankable set
    must not let a still-earlier, non-empty date's membership "reach
    through" it to govern a later date's return -- that would be exactly
    the stale multi-step carry-forward defect PREDECLARED_WAVE.json's
    dynamic_cross_section policy forbids."""
    oos_csv = _write_oos_csv(tmp_path, [("2020-01-02", "A", 0.6, 1)])  # only 01-02 has any OOS row
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df(
        [("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 300.0), ("A", "2020-01-06", 900.0)]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    # 2020-01-02 is the fold-reset date -> forced 0.0 regardless of membership.
    assert result["rankable_cross_section_size_by_date"]["2020-01-02"] == 1
    # 2020-01-03: prev=01-02, RANKABLE_SET(01-02)={A} (non-empty) -> real observation.
    assert "2020-01-03" not in result["dates_with_no_return_observation"]
    # 2020-01-06: prev=01-03, RANKABLE_SET(01-03)={} (no OOS rows there) -> missing,
    # even though the EARLIER date 01-02 was non-empty -- proves no stale
    # multi-step carry-forward reaching through the empty 01-03.
    assert "2020-01-06" in result["dates_with_no_return_observation"]


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
# REQUIRED TEST 7: fold reset is actually observed
# ---------------------------------------------------------------------------


def test_fold_reset_date_forced_to_zero_despite_large_price_jump(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1), ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "A", 0.6, 1), ("2020-01-03", "B", 0.4, 1),
        ],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 200.0), ("A", "2020-01-03", 202.0),
            ("B", "2020-01-01", 100.0), ("B", "2020-01-02", 101.0), ("B", "2020-01-03", 102.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    expected_second_day = float(np.mean([(202.0 / 200.0) - 1.0, (102.0 / 101.0) - 1.0]))
    expected_cumulative = (1.0 + 0.0) * (1.0 + expected_second_day) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)
    assert result["fold_reset_dates_count"] == 1


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
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars_forward = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 103.0),
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 49.0), ("B", "2020-01-03", 51.0),
            ("C", "2020-01-01", 10.0), ("C", "2020-01-02", 10.5), ("C", "2020-01-03", 10.2),
        ]
    )
    bars_shuffled = bars_forward.iloc[::-1].reset_index(drop=True)

    result_forward = run_wave.build_dynamic_rankable_benchmark(
        bars_forward, oos_forward, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    result_reversed = run_wave.build_dynamic_rankable_benchmark(
        bars_shuffled, oos_reversed, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    assert result_forward["cumulative_return_over_reference_dates"] == pytest.approx(
        result_reversed["cumulative_return_over_reference_dates"]
    )
    assert result_forward["rankable_cross_section_size_by_date"] == result_reversed["rankable_cross_section_size_by_date"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 9: zero/empty rankable set -- one explicit documented contract
# ---------------------------------------------------------------------------


def test_empty_rankable_set_date_produces_no_return_observation_not_an_error(tmp_path: Path) -> None:
    """Explicit contract (see build_dynamic_rankable_benchmark docstring):
    a reference date with zero rankable symbols never raises and is never
    silently defaulted to a 0.0 return -- it is excluded from the return
    series and recorded in dates_with_no_return_observation, UNLESS it is
    also a genuine fold-reset date (a distinct, unrelated 0.0 convention)."""
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


def test_empty_rankable_set_on_non_reset_date_excludes_the_date_it_would_have_governed(tmp_path: Path) -> None:
    """An empty RANKABLE_SET(T) itself still produces a zero cross-section
    size AT T (signal-availability accounting, unshifted), but under causal
    return attribution the return actually excluded is the FOLLOWING
    reference date's (the one T would have governed as its T_prev), never
    T's own same-day return."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-06", "A", 0.6, 1)],  # 01-03 has zero OOS rows
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df([("A", "2020-01-02", 100.0), ("A", "2020-01-03", 101.0), ("A", "2020-01-06", 103.0)])
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    assert result["rankable_cross_section_size_by_date"]["2020-01-03"] == 0  # RANKABLE_SET(01-03) itself is empty
    # 01-03's return is governed by RANKABLE_SET(01-02)={A} (non-empty) -> real observation.
    assert "2020-01-03" not in result["dates_with_no_return_observation"]
    # 01-06's return would be governed by RANKABLE_SET(01-03)={} (empty) -> missing.
    assert "2020-01-06" in result["dates_with_no_return_observation"]
    assert result["daily_return_observations_used"] == 2  # 01-02 (reset->0.0) and 01-03 only


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
    post-holdout rows are present, i.e. it structurally never reads them."""
    reference_dates = ["2020-01-02", "2020-01-03"]
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])

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
        [("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 102.0)]
    )
    bars_with_holdout_rows = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 101.0), ("A", "2020-01-03", 102.0),
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


# ---------------------------------------------------------------------------
# R1 (WAVE03-DYNAMIC-BENCHMARK-CAUSALITY-REPAIR-01) REQUIRED RED CONTROL:
# a symbol whose first-ever rankable date T follows a huge T-1->T price
# jump. The pre-repair SAME-BAR implementation incorrectly credits
# RANKABLE_SET(T) with that jump (crediting a decision-date membership set
# with a return interval that already ended before the decision was
# available); the repaired CAUSAL implementation must not, because
# RANKABLE_SET(T) can only ever govern a return interval starting AT T
# (i.e. ending at some later reference date), never one ending at or
# before T.
# ---------------------------------------------------------------------------


def _same_bar_BROKEN_build_dynamic_rankable_benchmark(
    bars: pd.DataFrame, oos_predictions_csv: Path, economic_returns_csv: Path,
    reference_dates: list[str], holdout_start_utc: str,
) -> dict:
    """Deliberately buggy reference implementation reproducing the PRE-REPAIR
    same-bar defect: attributes the return ENDING at reference date T to
    RANKABLE_SET(T) itself. Used ONLY by the mutation/negative proof below --
    never call from production code."""
    holdout_ts = pd.Timestamp(holdout_start_utc)
    if holdout_ts.tzinfo is None:
        holdout_ts = holdout_ts.tz_localize("UTC")
    authority = run_wave.economic_fold_date_authority(economic_returns_csv)
    fold_start_dates = authority["reset_dates"]
    reference_date_set = set(pd.Index(reference_dates).astype(str))
    rankable = run_wave.rankable_set_by_date(oos_predictions_csv)
    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")
    b["daily_ret"] = b.groupby("symbol")["close"].pct_change()
    sorted_dates = sorted(reference_date_set)
    per_date: dict[str, float] = {}
    for d in sorted_dates:
        syms = rankable.get(d, set())  # BUG: same-date membership, not T_prev
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


def test_mutation_proof_lookahead_jump_captured_by_broken_not_by_repaired(tmp_path: Path) -> None:
    """REQUIRED RED CONTROL 1+2: symbol A first becomes rankable at
    2020-01-03, right after a huge 01-02->01-03 jump (01-02 is a fold-reset
    date so this jump cannot be attributed there either way -- the defect
    must be proven at a NON-reset first-rankable date). The broken same-bar
    implementation incorrectly captures the ~1000x jump; the repaired causal
    implementation excludes A entirely from 01-03 (RANKABLE_SET(01-02) did
    not include A) and only picks A back up for the LEGITIMATE 01-03->01-06
    holding move once A is an established T_prev membership."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "A", 0.6, 1), ("2020-01-03", "B", 0.4, 1),  # A first rankable here
            ("2020-01-06", "A", 0.6, 1), ("2020-01-06", "B", 0.4, 1),
        ],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 100000.0),  # huge jump
            ("A", "2020-01-06", 101000.0),
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 50.0), ("B", "2020-01-03", 50.5), ("B", "2020-01-06", 51.0),
        ]
    )
    reference_dates = ["2020-01-02", "2020-01-03", "2020-01-06"]

    broken = _same_bar_BROKEN_build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT)
    repaired = run_wave.build_dynamic_rankable_benchmark(bars, oos_csv, econ_csv, reference_dates, FAR_FUTURE_HOLDOUT)

    # Broken: RANKABLE_SET(01-03)={A,B} directly governs 01-03's own return
    # -> captures the ~1000x A jump.
    assert broken["per_date"]["2020-01-03"] == pytest.approx(np.mean([100000.0 / 100.0 - 1.0, 50.5 / 50.0 - 1.0]))
    assert broken["cumulative_return_over_reference_dates"] > 100.0

    # Repaired: 01-03's return is governed by RANKABLE_SET(01-02)={B} only
    # (A was not yet rankable at 01-02) -> the jump is structurally excluded.
    assert repaired["rankable_cross_section_size_by_date"]["2020-01-02"] == 1  # {B} only
    assert repaired["cumulative_return_over_reference_dates"] < 1.0  # never sees the ~1000x jump

    # REQUIRED RED CONTROL 2: the T(01-03)->T+1(01-06) move IS captured for A
    # once A is an established T_prev membership (RANKABLE_SET(01-03)={A,B}
    # governs 01-06's return).
    expected_0106 = float(np.mean([101000.0 / 100000.0 - 1.0, 51.0 / 50.5 - 1.0]))
    assert expected_0106 == pytest.approx(0.01, abs=2e-4)  # both legs are genuine ~1% moves
    # Reconstruct 01-06's per-date value the same way the function does, via
    # the overall cumulative return identity: (1+r02)(1+r03)(1+r06)-1.
    r02 = 0.0  # fold reset, forced
    r03 = 50.5 / 50.0 - 1.0  # B only, from RANKABLE_SET(01-02)={B}
    implied_r06 = (1.0 + repaired["cumulative_return_over_reference_dates"]) / ((1.0 + r02) * (1.0 + r03)) - 1.0
    assert implied_r06 == pytest.approx(expected_0106)


# ---------------------------------------------------------------------------
# R1 REQUIRED ADDITIONAL COVERAGE: dropped symbol cannot earn T->T+1; prior
# membership truthfully governs the interval ending at T; no membership
# crosses a fold boundary; a later first-rankable date cannot earn
# pre-entry/same-decision returns.
# ---------------------------------------------------------------------------


def test_symbol_dropped_at_t_cannot_earn_t_to_t_plus_one(tmp_path: Path) -> None:
    """A symbol rankable at T-1 but DROPPED at T (not in RANKABLE_SET(T))
    must not earn the T->T+1 return, because T->T+1's return is governed by
    RANKABLE_SET(T), which no longer includes it."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1), ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "B", 0.4, 1),  # A dropped at 01-03
        ],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 999999.0),  # A would move huge
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 50.0), ("B", "2020-01-03", 50.5),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    # 01-03's return is governed by RANKABLE_SET(01-02)={A,B} -- but the huge
    # A move only shows up if A is actually included; verify it IS included
    # here (A was still rankable AT the governing date 01-02), then re-run
    # with A dropped ALREADY at 01-02 (the governing date) to prove exclusion.
    assert result["cumulative_return_over_reference_dates"] > 100.0  # A's move IS captured (rankable at 01-02)

    oos_csv_dropped_earlier = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "B", 0.4, 1), ("2020-01-03", "B", 0.4, 1)],  # A never rankable at all
        name="dropped_earlier.csv",
    )
    result_dropped = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv_dropped_earlier, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    assert result_dropped["cumulative_return_over_reference_dates"] == pytest.approx(0.01)  # only B's ~1% move, reset then B


def test_prior_membership_truthfully_governs_interval_ending_at_t(tmp_path: Path) -> None:
    """The return ending at reference date T is governed by RANKABLE_SET
    decided at the PRIOR reference date, not by whatever is (or isn't)
    rankable AT T itself."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-03", "Z", 0.1, 1)],  # A gone, Z appears -- irrelevant to 01-03's return
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 110.0),
            ("Z", "2020-01-01", 10.0), ("Z", "2020-01-02", 10.0), ("Z", "2020-01-03", 5.0),  # Z's -50% move must NOT count
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(110.0 / 100.0 - 1.0)  # A only, not Z


def test_no_membership_crosses_fold_boundaries(tmp_path: Path) -> None:
    """A fold's first date must be forced to 0.0 (no valid in-fold T_prev)
    even when the PRECEDING fold's last date had real membership -- proves
    the causal shift never bridges a fold boundary."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1), ("2020-01-03", "A", 0.6, 1),  # fold 1
            ("2020-04-01", "A", 0.6, 2), ("2020-04-02", "A", 0.6, 2),  # fold 2
        ],
    )
    econ_csv = _write_economic_returns_csv(
        tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-04-01", 2), ("2020-04-02", 2)]
    )
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 101.0),
            ("A", "2020-03-31", 500.0), ("A", "2020-04-01", 999999.0),  # huge jump landing on fold 2's reset date
            ("A", "2020-04-02", 1000000.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-04-01", "2020-04-02"], FAR_FUTURE_HOLDOUT
    )
    assert result["fold_reset_dates_count"] == 2
    # fold 2's first date (04-01) is forced to 0.0 despite fold 1 ending with
    # A rankable on 01-03 -- membership never crosses the fold boundary.
    expected_0402 = 1000000.0 / 999999.0 - 1.0  # governed by RANKABLE_SET(04-01)={A}, within fold 2 only
    expected_0103 = 101.0 / 100.0 - 1.0  # governed by RANKABLE_SET(01-02)={A}, within fold 1 only
    expected_cumulative = (1.0 + 0.0) * (1.0 + expected_0103) * (1.0 + 0.0) * (1.0 + expected_0402) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)


def test_later_first_rankable_date_cannot_earn_pre_entry_or_same_decision_returns(tmp_path: Path) -> None:
    """A symbol whose first-ever rankable date is T cannot earn the return
    ending at T (that would require having been decided at T-1, before it
    was ever rankable) nor any return before T -- it only starts
    contributing to the return ending at the NEXT reference date after T."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "B", 0.4, 1),
            ("2020-01-03", "A", 0.6, 1), ("2020-01-03", "B", 0.4, 1),  # A's first-ever rankable date
            ("2020-01-06", "A", 0.6, 1), ("2020-01-06", "B", 0.4, 1),
        ],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 100.0), ("A", "2020-01-03", 900.0), ("A", "2020-01-06", 909.0),
            ("B", "2020-01-01", 50.0), ("B", "2020-01-02", 50.0), ("B", "2020-01-03", 50.5), ("B", "2020-01-06", 51.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    r03 = 50.5 / 50.0 - 1.0  # B only -- A's 01-02->01-03 jump (100->900) never earned
    r06 = float(np.mean([909.0 / 900.0 - 1.0, 51.0 / 50.5 - 1.0]))  # A now legitimately included
    expected_cumulative = (1.0 + 0.0) * (1.0 + r03) * (1.0 + r06) - 1.0
    assert result["cumulative_return_over_reference_dates"] == pytest.approx(expected_cumulative)


# ---------------------------------------------------------------------------
# MUTATION / NEGATIVE PROOF: a deliberately stale-membership implementation
# fails the "dropped symbol disappears immediately" invariant (REQUIRED
# TEST 2, above).
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
