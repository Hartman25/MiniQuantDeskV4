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


def test_no_stale_carry_forward_when_date_has_no_oos_rows(tmp_path: Path) -> None:
    """A date entirely absent from the OOS predictions file (no decision
    frame that day) must NOT inherit the previous date's rankable set --
    build_dynamic_rankable_benchmark's per-date return must be a genuine
    missing observation (NaN), never a value carried forward from the prior
    date's rankable set/return."""
    oos_csv = _write_oos_csv(
        tmp_path,
        [
            ("2020-01-02", "A", 0.6, 1),
            ("2020-01-02", "B", 0.4, 1),
            # 2020-01-03 has ZERO oos rows -- must not inherit {A, B}
        ],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1)])
    bars = _bars_df(
        [
            ("A", "2020-01-01", 100.0), ("A", "2020-01-02", 200.0), ("A", "2020-01-03", 400.0),
            ("B", "2020-01-01", 100.0), ("B", "2020-01-02", 101.0), ("B", "2020-01-03", 105.0),
        ]
    )
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03"], FAR_FUTURE_HOLDOUT
    )
    assert result["rankable_cross_section_size_by_date"]["2020-01-03"] == 0
    assert "2020-01-03" in result["dates_with_no_return_observation"]


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


def test_empty_rankable_set_on_non_reset_date_excluded_from_return_series(tmp_path: Path) -> None:
    oos_csv = _write_oos_csv(
        tmp_path,
        [("2020-01-02", "A", 0.6, 1), ("2020-01-06", "A", 0.6, 1)],
    )
    econ_csv = _write_economic_returns_csv(tmp_path, [("2020-01-02", 1), ("2020-01-03", 1), ("2020-01-06", 1)])
    bars = _bars_df([("A", "2020-01-02", 100.0), ("A", "2020-01-03", 101.0), ("A", "2020-01-06", 103.0)])
    result = run_wave.build_dynamic_rankable_benchmark(
        bars, oos_csv, econ_csv, ["2020-01-02", "2020-01-03", "2020-01-06"], FAR_FUTURE_HOLDOUT
    )
    assert result["rankable_cross_section_size_by_date"]["2020-01-03"] == 0
    assert "2020-01-03" in result["dates_with_no_return_observation"]
    assert result["daily_return_observations_used"] == 2  # 01-02 (reset->0.0) and 01-06 only


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
