"""WAVE03-FAMILY-EXECUTION-HARNESS-01 -- focused, network-free tests for
run_family/run_one_trial and the required-output-recording-field helpers.
Uses only synthetic fixture CSVs/JSON and monkeypatching -- no Alpaca
access, no real model training, no research-py/src modification. Mirrors
SHORT-WAVE-02's own test_short_wave_02.py monkeypatch conventions.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pandas as pd
import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402
from run_wave import FAMILIES  # noqa: E402

FAR_FUTURE_HOLDOUT = "2099-01-01T00:00:00Z"


# ---------------------------------------------------------------------------
# 1. FEATURE ISOLATION: exactly one predeclared feature per family
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("family_key,expected_col", [
    ("RANK-01", "ret_rank_20"),
    ("RANK-02", "ret_5"),
    ("RANK-03", "gap_pct_1"),
])
def test_isolate_feature_selects_only_symbol_end_ts_and_declared_column(family_key: str, expected_col: str) -> None:
    full = pd.DataFrame(
        {
            "symbol": ["SPY", "SPY"],
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00"],
            "ret_rank_20": [0.9, 0.1],
            "ret_5": [0.01, -0.02],
            "gap_pct_1": [0.001, -0.003],
            "vol_20": [0.02, 0.03],
        }
    )
    isolated = run_wave.isolate_feature(full, expected_col)
    assert list(isolated.columns) == ["symbol", "end_ts", expected_col]
    assert FAMILIES[family_key].feature_column == expected_col


def test_isolate_feature_fails_closed_when_declared_column_missing() -> None:
    full = pd.DataFrame({"symbol": ["SPY"], "end_ts": ["2020-01-01 00:00:00"], "ret_5": [0.01]})
    with pytest.raises(RuntimeError, match="feature isolation failed"):
        run_wave.isolate_feature(full, "gap_pct_1")


def test_write_run_dir_fails_closed_on_full_feature_matrix_negative_control(tmp_path: Path) -> None:
    """Mutation/negative proof for the historical ALPHA-01 full-feature-
    matrix-consumption defect: if a caller accidentally passed the FULL,
    un-isolated FeatureSetV1 output into write_run_dir instead of the
    single-feature-isolated frame, assert_single_feature_schema must catch
    it and fail closed rather than silently training on every column."""
    full_features = pd.DataFrame(
        {
            "symbol": ["A", "A"],
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00"],
            "ret_rank_20": [0.9, 0.1],
            "ret_5": [0.01, -0.02],
            "gap_pct_1": [0.001, -0.003],
        }
    )
    targets = pd.DataFrame(
        {
            "symbol": ["A", "A"],
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00"],
            "fwd_ret": [0.01, -0.01],
            "target": [1, 0],
            "label_end_ts": ["2020-02-01T00:00:00+00:00"] * 2,
        }
    )
    bars = pd.DataFrame(
        {
            "symbol": ["A", "A"],
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00"],
            "close": [100.0, 101.0],
        }
    )
    run_dir = tmp_path / "run"
    with pytest.raises(RuntimeError, match="FEATURE ISOLATION INVARIANT VIOLATED"):
        run_wave.write_run_dir(run_dir, bars, full_features, targets, feature_column="ret_rank_20")


# ---------------------------------------------------------------------------
# 2. DATA AUTHORITY / CACHE SAFETY -- static source proofs (no network
# module import, matching test_check_mode_never_contacts_alpaca's own
# convention)
# ---------------------------------------------------------------------------


def test_seed_symbols_never_calls_live_registry_builder() -> None:
    source = (EXPERIMENT_ROOT / "run_wave.py").read_text(encoding="utf-8")
    start = source.index("def seed_symbols(")
    end = source.index("\n\n", start)
    body = source[start:end]
    assert "build_current_enabled_equity_registry_snapshot" not in body


# ---------------------------------------------------------------------------
# 3. Required-output-recording-field helpers
# ---------------------------------------------------------------------------


def test_rankable_cross_section_targets_long_only_vs_long_short() -> None:
    sizes = {"2020-01-02": 5, "2020-01-03": 10, "2020-01-06": 3}
    lo = run_wave.rankable_cross_section_targets(sizes, rank_side_count=5, long_only=True)
    assert lo["target_long_symbol_days"] == 10  # 01-02 and 01-03 both meet K=5
    assert lo["target_short_symbol_days"] == 0
    assert lo["dates_below_rank_minimum"] == ["2020-01-06"]
    assert lo["max_concurrent_target_longs"] == 5
    assert lo["max_concurrent_target_shorts"] == 0

    ls = run_wave.rankable_cross_section_targets(sizes, rank_side_count=5, long_only=False)
    assert ls["target_long_symbol_days"] == 5  # only 01-03 meets 2K=10
    assert ls["target_short_symbol_days"] == 5
    assert ls["dates_below_rank_minimum"] == ["2020-01-02", "2020-01-06"]
    assert ls["max_concurrent_target_longs"] == 5
    assert ls["max_concurrent_target_shorts"] == 5


def test_symbols_ever_never_rankable() -> None:
    rankable_by_date = {"2020-01-02": {"A", "B"}, "2020-01-03": {"B", "C"}}
    result = run_wave.symbols_ever_never_rankable(rankable_by_date, ["A", "B", "C", "D"])
    assert result["symbols_ever_rankable"] == ["A", "B", "C"]
    assert result["symbols_never_rankable"] == ["D"]


def test_first_last_rankable_date_per_symbol() -> None:
    rankable_by_date = {
        "2020-01-02": {"A"},
        "2020-01-03": {"A", "B"},
        "2020-01-06": {"B"},
    }
    result = run_wave.first_last_rankable_date_per_symbol(rankable_by_date)
    assert result["A"] == {"first": "2020-01-02", "last": "2020-01-03"}
    assert result["B"] == {"first": "2020-01-03", "last": "2020-01-06"}


def test_reconstruct_daily_target_qty_and_executed_symbol_day_stats(tmp_path: Path) -> None:
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-06T00:00:00+00:00"},
        ]
    )
    econ_csv = tmp_path / "economic_returns.csv"
    econ.to_csv(econ_csv, index=False)

    fold_summaries = [
        {
            "fold": 1,
            "weight_to_share_evidence": {
                "A": [
                    {"timestamp": "2020-01-02T00:00:00+00:00", "target_qty": 10},
                    {"timestamp": "2020-01-03T00:00:00+00:00", "target_qty": -5},
                ]
            },
        }
    ]
    daily_positions = run_wave.reconstruct_daily_target_qty(fold_summaries, econ_csv)
    by_date = {row["date"]: row["target_qty"] for _, row in daily_positions.iterrows()}
    assert by_date == {"2020-01-02": 10, "2020-01-03": -5, "2020-01-06": -5}  # ffill carries -5 to 01-06

    stats = run_wave.executed_symbol_day_stats(daily_positions)
    assert stats["executed_long_symbol_days"] == 1
    assert stats["executed_short_symbol_days"] == 2
    assert stats["max_concurrent_longs"] == 1
    assert stats["max_concurrent_shorts"] == 1


def test_reconstruct_daily_target_qty_empty_when_no_events(tmp_path: Path) -> None:
    econ = pd.DataFrame([{"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"}])
    econ_csv = tmp_path / "economic_returns.csv"
    econ.to_csv(econ_csv, index=False)
    result = run_wave.reconstruct_daily_target_qty([{"fold": 1, "weight_to_share_evidence": {}}], econ_csv)
    assert result.empty
    stats = run_wave.executed_symbol_day_stats(result)
    assert stats == {
        "executed_long_symbol_days": 0, "executed_short_symbol_days": 0,
        "max_concurrent_longs": 0, "max_concurrent_shorts": 0,
    }


def test_fold_concentration_ratio_and_none_on_all_zero() -> None:
    concentrated = run_wave.fold_concentration(
        [{"net_total_return": 0.1}, {"net_total_return": -0.3}, {"net_total_return": 0.05}]
    )
    assert concentrated == pytest.approx(0.3 / 0.45)
    assert run_wave.fold_concentration([{"net_total_return": 0.0}, {"net_total_return": 0.0}]) is None
    assert run_wave.fold_concentration([]) is None


def test_compute_paired_delta_and_placebo_delta(tmp_path: Path) -> None:
    lo_daily = tmp_path / "lo_daily.csv"
    pd.DataFrame({"date": ["2020-01-02"], "turnover": [0.2]}).to_csv(lo_daily, index=False)
    ls_daily = tmp_path / "ls_daily.csv"
    pd.DataFrame({"date": ["2020-01-02"], "turnover": [0.5]}).to_csv(ls_daily, index=False)

    long_only = {"economic_daily_returns_csv": str(lo_daily), "aggregate": {
        "net_total_return": 0.05, "net_sharpe": 1.0, "max_drawdown": -0.02, "cost_drag": 0.01,
    }}
    long_short = {"economic_daily_returns_csv": str(ls_daily), "aggregate": {
        "net_total_return": 0.10, "net_sharpe": 1.5, "max_drawdown": -0.03, "cost_drag": 0.02,
    }}
    placebo = {"aggregate": {"net_total_return": 0.01, "net_sharpe": 0.1, "max_drawdown": -0.01, "cost_drag": 0.005}}

    paired = run_wave.compute_paired_delta(long_only, long_short)
    assert paired["delta_net_total_return"] == pytest.approx(0.05)
    assert paired["delta_turnover"] == pytest.approx(0.3)
    assert paired["long_only_turnover"] == pytest.approx(0.2)
    assert paired["long_short_turnover"] == pytest.approx(0.5)

    placebo_delta = run_wave.compute_placebo_delta(long_short, placebo)
    assert placebo_delta["delta_net_total_return"] == pytest.approx(0.09)


# ---------------------------------------------------------------------------
# 4. run_family: real/placebo experiment_id routing (structural, mirrors
# SHORT-WAVE-02's own monkeypatched proof) and end-to-end recording fields
# ---------------------------------------------------------------------------


def _build_fake_family_artifacts(tmp_path: Path, direction: str) -> dict:
    oos_csv = tmp_path / f"oos_{direction}.csv"
    pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.6, "target": 1},
            {"fold": 1, "symbol": "B", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.4, "target": 0},
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-03T00:00:00+00:00", "label_end_ts": "2020-02-02T00:00:00+00:00", "ml_score": 0.6, "target": 1},
            {"fold": 1, "symbol": "B", "decision_ts": "2020-01-03T00:00:00+00:00", "label_end_ts": "2020-02-02T00:00:00+00:00", "ml_score": 0.4, "target": 0},
        ]
    ).to_csv(oos_csv, index=False)

    econ_csv = tmp_path / f"econ_{direction}.csv"
    pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
        ]
    ).to_csv(econ_csv, index=False)

    daily_csv = tmp_path / f"daily_{direction}.csv"
    pd.DataFrame({"date": ["2020-01-02", "2020-01-03"], "turnover": [0.2, 0.1]}).to_csv(daily_csv, index=False)

    return {
        "experiment_id": "unused-placeholder",
        "hypothesis_id": "unused-placeholder",
        "direction": direction,
        "trial_id": f"fake-trial-{direction}",
        "economic_eval_id": f"fake-eval-{direction}",
        "economic_walk_forward_json": "fake.json",
        "economic_daily_returns_csv": str(daily_csv),
        "economic_returns_csv": str(econ_csv),
        "oos_predictions_csv": str(oos_csv),
        "aggregate": {
            "net_total_return": 0.05, "net_sharpe": 1.0, "max_drawdown": -0.02,
            "cost_drag": 0.01, "total_turnover": 0.3, "average_gross_exposure": 0.8,
        },
        "folds": [
            {
                "fold": 1,
                "net_total_return": 0.05,
                "weight_to_share_evidence": {
                    "A": [{"timestamp": "2020-01-02T00:00:00+00:00", "target_qty": 100}],
                    "B": [{"timestamp": "2020-01-02T00:00:00+00:00", "target_qty": -50 if direction == "long_short" else 0}],
                },
            }
        ],
        "holdout": {"status": "reserved_not_evaluated"},
        "holdout_start_utc": FAR_FUTURE_HOLDOUT,
        "folds_generated": 1, "folds_used": 1, "folds_skipped": 0,
    }


def test_run_family_routes_real_and_placebo_trials_and_computes_recording_fields(monkeypatch, tmp_path: Path) -> None:
    calls: list[dict] = []

    fake_artifacts = {
        "long_only": _build_fake_family_artifacts(tmp_path, "long_only"),
        "long_short": _build_fake_family_artifacts(tmp_path, "long_short"),
        "placebo": _build_fake_family_artifacts(tmp_path, "long_short"),
    }

    def fake_run_one_trial(*, run_dir, experiment_id, hypothesis_id, strategy_id, direction, bars_path, bars_provenance):
        calls.append({"experiment_id": experiment_id, "hypothesis_id": hypothesis_id, "direction": direction})
        key = "placebo" if experiment_id == run_wave.PLACEBO_EXPERIMENT_ID else direction
        art = dict(fake_artifacts[key])
        art["experiment_id"] = experiment_id
        art["hypothesis_id"] = hypothesis_id
        return art

    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 101.0},
            {"symbol": "A", "end_ts": "2020-01-03 00:00:00", "close": 102.0},
            {"symbol": "B", "end_ts": "2020-01-01 00:00:00", "close": 50.0},
            {"symbol": "B", "end_ts": "2020-01-02 00:00:00", "close": 49.0},
            {"symbol": "B", "end_ts": "2020-01-03 00:00:00", "close": 51.0},
        ]
    )
    # ensure_full_features is monkeypatched to hand this same frame back
    # (see below) -- it needs every family's declared feature column present
    # so isolate_feature(full_features, fam.feature_column) succeeds.
    bars["ret_rank_20"] = 0.5
    bars["ret_5"] = 0.01
    bars["gap_pct_1"] = 0.0
    targets = pd.DataFrame(
        {
            "symbol": ["A"], "end_ts": ["2020-01-01 00:00:00"], "fwd_ret": [0.01],
            "target": [1], "label_end_ts": ["2020-02-01T00:00:00+00:00"],
        }
    )

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "ensure_bars", lambda: (bars, {"fake": "manifest"}))
    monkeypatch.setattr(run_wave, "ensure_real_targets", lambda bars: targets)
    monkeypatch.setattr(run_wave, "ensure_placebo_targets", lambda real_targets: targets)
    monkeypatch.setattr(run_wave, "ensure_full_features", lambda bars: bars)
    monkeypatch.setattr(run_wave, "write_run_dir", lambda *a, **k: Path("fake_bars.csv"))
    monkeypatch.setattr(run_wave, "run_one_trial", fake_run_one_trial)

    result = run_wave.run_family("RANK-01")

    assert len(calls) == 3
    placebo_calls = [c for c in calls if c["hypothesis_id"] == FAMILIES["RANK-01"].hyp_placebo]
    assert len(placebo_calls) == 1
    assert placebo_calls[0]["experiment_id"] == run_wave.PLACEBO_EXPERIMENT_ID
    real_calls = [c for c in calls if c["hypothesis_id"] != FAMILIES["RANK-01"].hyp_placebo]
    assert len(real_calls) == 2
    assert all(c["experiment_id"] == run_wave.REAL_EXPERIMENT_ID for c in real_calls)
    assert {c["direction"] for c in real_calls} == {"long_only", "long_short"}

    rf = result["recording_fields"]
    assert rf["family"] == "RANK-01"
    assert rf["seed_universe_count"] == 88
    assert rf["holdout_status"] == "reserved_not_evaluated"
    assert set(rf["long_only"].keys()) >= {
        "target_long_symbol_days", "target_short_symbol_days", "dates_below_rank_minimum",
        "executed_long_symbol_days", "executed_short_symbol_days",
        "max_concurrent_longs", "max_concurrent_shorts",
        "desired_gross_exposure", "desired_net_exposure",
        "actual_gross_exposure", "actual_net_exposure",
        "turnover", "cost_drag", "net_return", "sharpe", "max_drawdown", "fold_concentration",
    }
    assert rf["long_only"]["desired_gross_exposure"] == run_wave.MAX_GROSS_EXPOSURE
    assert rf["long_only"]["desired_net_exposure"] == run_wave.MAX_GROSS_EXPOSURE
    assert rf["long_short"]["desired_net_exposure"] == 0.0
    assert "long_short_minus_long_only_paired_deltas" in rf
    assert "matched_placebo_comparison" in rf

    fam_result_path = fake_run_root / "rank_01" / "family_result.json"
    assert fam_result_path.exists()
    on_disk = json.loads(fam_result_path.read_text(encoding="utf-8"))
    assert on_disk["family"] == "RANK-01"
