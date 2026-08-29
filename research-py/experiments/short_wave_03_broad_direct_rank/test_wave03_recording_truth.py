"""WAVE03-RUN-RECORDING-TRUTH-REPAIR-01 -- focused, network-free tests for:
  - Defect A: holdout_status must be DERIVED from the registered economic
    evaluator's own verified output, never hardcoded, and run_one_trial must
    fail closed if that output ever disagrees with the frozen
    reserved_not_evaluated contract.
  - Defect B: actual_gross_exposure/actual_net_exposure must be reconstructed
    from the accepted engine's own discrete weight_to_share_evidence
    (target_qty), priced with a genuinely causal mark (current-or-prior bar,
    never future, never silently dropped), not read from the engine's
    CONTINUOUS aggregate.average_gross_exposure series.
  - Additional cheap regressions: a direct wave03 test of
    build_causal_placebo_targets, and the long_only/long_short OOS-
    prediction-membership parity check.

Uses only synthetic fixture CSVs/DataFrames and monkeypatching -- no Alpaca
access, no real model training, no research-py/src modification.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402
from run_wave import FAMILIES  # noqa: E402

FAR_FUTURE_HOLDOUT = "2099-01-01T00:00:00Z"


# ===========================================================================
# DEFECT A: holdout status derived (not hardcoded), fails closed on
# consumed/evaluated holdout.
# ===========================================================================


def _write_fake_economic_walk_forward(tmp_path: Path, *, holdout_status: str, name: str) -> Path:
    daily_csv = tmp_path / f"{name}_daily.csv"
    pd.DataFrame({"date": ["2020-01-02"], "net_daily_return": [0.001], "turnover": [0.1]}).to_csv(daily_csv, index=False)
    returns_csv = tmp_path / f"{name}_returns.csv"
    pd.DataFrame({"fold": [1], "timestamp": ["2020-01-02T00:00:00+00:00"]}).to_csv(returns_csv, index=False)
    out = {
        "aggregate": {"net_total_return": 0.01, "average_gross_exposure": 0.5},
        "inputs": {},
        "outputs": {
            "economic_daily_returns_csv": {"path": str(daily_csv)},
            "economic_returns_csv": {"path": str(returns_csv)},
        },
        "holdout": {"status": holdout_status},
        "folds": [],
        "registry": {"trial_id": f"fake-trial-{name}"},
        "ids": {"economic_eval_id": f"fake-eval-{name}"},
    }
    out_path = tmp_path / f"{name}.json"
    out_path.write_text(json.dumps(out), encoding="utf-8")
    return out_path


def test_run_one_trial_fails_closed_when_holdout_status_is_not_reserved(monkeypatch, tmp_path: Path) -> None:
    """REQUIRED MUTATION TEST: fake one trial as consumed/evaluated and
    prove run_one_trial (called by run_family, transitively) fails closed."""
    bad_path = _write_fake_economic_walk_forward(tmp_path, holdout_status="consumed", name="bad")
    monkeypatch.setattr(run_wave, "run_registered_economic_walkforward_eval", lambda *a, **k: bad_path)
    with pytest.raises(RuntimeError, match="holdout"):
        run_wave.run_one_trial(
            run_dir=tmp_path / "run", experiment_id="EXP", hypothesis_id="HYP", strategy_id="STRAT",
            direction="long_only", bars_path=tmp_path / "bars.csv", bars_provenance={},
        )


def test_run_one_trial_passes_through_reserved_holdout_status(monkeypatch, tmp_path: Path) -> None:
    good_path = _write_fake_economic_walk_forward(tmp_path, holdout_status="reserved_not_evaluated", name="good")
    monkeypatch.setattr(run_wave, "run_registered_economic_walkforward_eval", lambda *a, **k: good_path)
    result = run_wave.run_one_trial(
        run_dir=tmp_path / "run", experiment_id="EXP", hypothesis_id="HYP", strategy_id="STRAT",
        direction="long_only", bars_path=tmp_path / "bars.csv", bars_provenance={},
    )
    assert result["holdout"] == {"status": "reserved_not_evaluated"}


def _minimal_recording_field_inputs(tmp_path: Path, *, holdout_status_lo: str, holdout_status_ls: str, holdout_status_pb: str):
    oos_csv = tmp_path / "oos.csv"
    pd.DataFrame(
        [{"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.6, "target": 1}]
    ).to_csv(oos_csv, index=False)
    econ_csv = tmp_path / "econ.csv"
    pd.DataFrame([{"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"}]).to_csv(econ_csv, index=False)
    daily_csv = tmp_path / "daily.csv"
    pd.DataFrame({"date": ["2020-01-02"], "turnover": [0.1]}).to_csv(daily_csv, index=False)

    def _trial(holdout_status: str) -> dict:
        return {
            "aggregate": {
                "net_total_return": 0.01, "net_sharpe": 1.0, "max_drawdown": -0.01,
                "cost_drag": 0.001, "total_turnover": 0.1, "average_gross_exposure": 0.5,
            },
            "folds": [{"fold": 1, "net_total_return": 0.01, "weight_to_share_evidence": {}}],
            "economic_returns_csv": str(econ_csv),
            "economic_daily_returns_csv": str(daily_csv),
            "oos_predictions_csv": str(oos_csv),
            "holdout": {"status": holdout_status},
        }

    r_lo, r_ls, r_pb = _trial(holdout_status_lo), _trial(holdout_status_ls), _trial(holdout_status_pb)
    benchmark = {
        "rankable_cross_section_size_by_date": {"2020-01-02": 1},
        "rankable_cross_section_min": 1, "rankable_cross_section_median": 1.0, "rankable_cross_section_max": 1,
    }
    rankable_by_date = {"2020-01-02": {"A"}}
    bars = pd.DataFrame([{"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0}])
    return r_lo, r_ls, r_pb, benchmark, rankable_by_date, bars


def test_compute_family_recording_fields_fails_closed_on_holdout_status_mismatch(tmp_path: Path) -> None:
    r_lo, r_ls, r_pb, benchmark, rankable_by_date, bars = _minimal_recording_field_inputs(
        tmp_path, holdout_status_lo="reserved_not_evaluated", holdout_status_ls="reserved_not_evaluated",
        holdout_status_pb="consumed",  # mutated: placebo trial claims a different status
    )
    with pytest.raises(RuntimeError, match="holdout status"):
        run_wave.compute_family_recording_fields(
            FAMILIES["RANK-01"], r_lo, r_ls, r_pb, benchmark, benchmark, rankable_by_date, bars,
        )


def test_compute_family_recording_fields_derives_holdout_status_when_uniform(tmp_path: Path) -> None:
    r_lo, r_ls, r_pb, benchmark, rankable_by_date, bars = _minimal_recording_field_inputs(
        tmp_path, holdout_status_lo="reserved_not_evaluated", holdout_status_ls="reserved_not_evaluated",
        holdout_status_pb="reserved_not_evaluated",
    )
    result = run_wave.compute_family_recording_fields(
        FAMILIES["RANK-01"], r_lo, r_ls, r_pb, benchmark, benchmark, rankable_by_date, bars,
    )
    assert result["holdout_status"] == "reserved_not_evaluated"


# ===========================================================================
# DEFECT B: discrete (not continuous) actual gross/net exposure, causal
# mark pricing, fail-closed on missing mark for a nonzero position.
# ===========================================================================


def test_discrete_target_qty_zero_despite_nonzero_continuous_desired_weight_yields_zero_exposure() -> None:
    """REQUIRED TEST 1: a discrete target_qty=0 row (whatever the continuous
    engine-side desired weight may have been) must contribute zero
    gross/net exposure."""
    daily_positions = pd.DataFrame([{"fold": 1, "symbol": "A", "date": "2020-01-02", "target_qty": 0}])
    bars = pd.DataFrame([{"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0}])
    gross, net = run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)
    assert gross == pytest.approx(0.0)
    assert net == pytest.approx(0.0)


def test_long_and_short_discrete_positions_gross_and_net_formula() -> None:
    """REQUIRED TEST 2: gross = sum(abs(qty*mark))/equity; net =
    sum(qty*mark)/equity, averaged over dates."""
    daily_positions = pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "date": "2020-01-02", "target_qty": 10},
            {"fold": 1, "symbol": "B", "date": "2020-01-02", "target_qty": -20},
            {"fold": 1, "symbol": "A", "date": "2020-01-03", "target_qty": -5},
            {"fold": 1, "symbol": "B", "date": "2020-01-03", "target_qty": 0},
        ]
    )
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0},
            {"symbol": "B", "end_ts": "2020-01-02 00:00:00", "close": 50.0},
            {"symbol": "A", "end_ts": "2020-01-03 00:00:00", "close": 200.0},
            {"symbol": "B", "end_ts": "2020-01-03 00:00:00", "close": 55.0},
        ]
    )
    gross, net = run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)
    day1_net = (10 * 100.0 + (-20 * 50.0)) / 100_000.0
    day1_gross = (abs(10 * 100.0) + abs(-20 * 50.0)) / 100_000.0
    day2_net = (-5 * 200.0 + 0 * 55.0) / 100_000.0
    day2_gross = (abs(-5 * 200.0) + abs(0 * 55.0)) / 100_000.0
    assert net == pytest.approx((day1_net + day2_net) / 2.0)
    assert gross == pytest.approx((day1_gross + day2_gross) / 2.0)


def test_held_symbol_missing_todays_bar_uses_last_known_causal_close() -> None:
    """REQUIRED TEST 3: a held symbol with no bar on a given date retains
    the most recent PRIOR bar's close, never dropped from exposure."""
    daily_positions = pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "date": "2020-01-02", "target_qty": 10},
            {"fold": 1, "symbol": "A", "date": "2020-01-03", "target_qty": 10},  # no bar this date
            {"fold": 1, "symbol": "A", "date": "2020-01-06", "target_qty": 10},
        ]
    )
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-06 00:00:00", "close": 106.0},
        ]
    )
    gross, net = run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)
    day1 = 10 * 100.0 / 100_000.0
    day2 = 10 * 100.0 / 100_000.0  # last known causal close (01-02), 01-03 has no bar
    day3 = 10 * 106.0 / 100_000.0
    assert net == pytest.approx((day1 + day2 + day3) / 3.0)
    assert gross == pytest.approx((day1 + day2 + day3) / 3.0)  # long-only here, gross == net


def test_no_future_price_is_ever_used() -> None:
    """REQUIRED TEST 4: a symbol whose ONLY bar is AFTER the held position's
    date must not be priced from that future bar -- it has no valid causal
    mark and must fail closed instead."""
    daily_positions = pd.DataFrame([{"fold": 1, "symbol": "A", "date": "2020-01-02", "target_qty": 10}])
    bars = pd.DataFrame([{"symbol": "A", "end_ts": "2020-01-06 00:00:00", "close": 999.0}])  # only a FUTURE bar
    with pytest.raises(run_wave.MissingCausalMarkError):
        run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)


def test_fold_boundary_resets_position_state_before_pricing(tmp_path: Path) -> None:
    """REQUIRED TEST 5: position state never carries across a fold boundary
    (reconstruct_daily_target_qty's own guarantee), and the flat (qty=0)
    fold-2 row needs no causal mark at all even though bars has nothing near
    fold 2's dates."""
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
            {"fold": 2, "timestamp": "2020-04-01T00:00:00+00:00"},
        ]
    )
    econ_csv = tmp_path / "economic_returns.csv"
    econ.to_csv(econ_csv, index=False)
    fold_summaries = [
        {"fold": 1, "weight_to_share_evidence": {"A": [{"timestamp": "2020-01-02T00:00:00+00:00", "target_qty": 10}]}},
        # An explicit flat (qty=0) event for fold 2 -- if fold 1's qty=10
        # ever leaked across the boundary this would be qty=10, not 0.
        {"fold": 2, "weight_to_share_evidence": {"A": [{"timestamp": "2020-04-01T00:00:00+00:00", "target_qty": 0}]}},
    ]
    daily_positions = run_wave.reconstruct_daily_target_qty(fold_summaries, econ_csv)
    fold2_rows = daily_positions[daily_positions["fold"] == 2]
    assert len(fold2_rows) == 1
    assert (fold2_rows["target_qty"] == 0).all()  # fold 2 starts flat, does NOT inherit fold 1's 10

    bars = pd.DataFrame([{"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0}])  # nothing near fold 2
    gross, net = run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)
    expected = (2 * (10 * 100.0 / 100_000.0) + 0.0) / 3.0  # fold1: 2 dates @ 0.001 each, fold2: 1 flat date @ 0
    assert net == pytest.approx(expected)
    assert gross == pytest.approx(expected)


def test_missing_causal_mark_for_nonzero_position_fails_closed() -> None:
    """REQUIRED TEST 6."""
    daily_positions = pd.DataFrame([{"fold": 1, "symbol": "Z", "date": "2020-01-02", "target_qty": 5}])
    bars = pd.DataFrame(columns=["symbol", "end_ts", "close"])  # Z never has a bar
    with pytest.raises(run_wave.MissingCausalMarkError):
        run_wave.actual_gross_and_net_exposure_from_positions(daily_positions, bars, 100_000.0)


def test_actual_gross_and_net_exposure_none_when_empty() -> None:
    gross, net = run_wave.actual_gross_and_net_exposure_from_positions(
        pd.DataFrame(columns=["fold", "symbol", "date", "target_qty"]), pd.DataFrame(), 100_000.0
    )
    assert gross is None
    assert net is None


# ===========================================================================
# ADDITIONAL CHEAP REGRESSIONS
# ===========================================================================


def test_build_causal_placebo_targets_preserves_pair_multiset_within_exact_group() -> None:
    targets = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-02", "fwd_ret": 0.01, "target": 1, "label_end_ts": "2020-01-12"},
            {"symbol": "B", "end_ts": "2020-01-02", "fwd_ret": -0.02, "target": 0, "label_end_ts": "2020-01-12"},
            {"symbol": "C", "end_ts": "2020-01-02", "fwd_ret": 0.03, "target": 1, "label_end_ts": "2020-01-12"},
            {"symbol": "A", "end_ts": "2020-01-03", "fwd_ret": 0.05, "target": 1, "label_end_ts": "2020-01-13"},
            {"symbol": "B", "end_ts": "2020-01-03", "fwd_ret": -0.01, "target": 0, "label_end_ts": "2020-01-13"},
            {"symbol": "C", "end_ts": "2020-01-03", "fwd_ret": 0.07, "target": 1, "label_end_ts": "2020-01-13"},
        ]
    )
    placebo = run_wave.build_causal_placebo_targets(targets, seed=run_wave.PLACEBO_SEED)

    for (end_ts, label_end_ts), group in targets.groupby(["end_ts", "label_end_ts"]):
        placebo_group = placebo[(placebo["end_ts"] == end_ts) & (placebo["label_end_ts"] == label_end_ts)]
        original_pairs = sorted(zip(group["fwd_ret"].round(9), group["target"]))
        placebo_pairs = sorted(zip(placebo_group["fwd_ret"].round(9), placebo_group["target"]))
        assert original_pairs == placebo_pairs  # multiset preserved within the exact group

    # no pair crosses label horizon or the (end_ts,label_end_ts) group boundary
    assert (placebo["end_ts"] == targets["end_ts"]).all()
    assert (placebo["label_end_ts"] == targets["label_end_ts"]).all()

    # at least one assignment actually changed
    assert int((placebo["target"] != targets["target"]).sum()) > 0

    # deterministic for a fixed seed
    placebo_again = run_wave.build_causal_placebo_targets(targets, seed=run_wave.PLACEBO_SEED)
    pd.testing.assert_frame_equal(placebo.reset_index(drop=True), placebo_again.reset_index(drop=True))


def test_run_family_fails_closed_when_long_only_and_long_short_oos_membership_differs(monkeypatch, tmp_path: Path) -> None:
    oos_lo_csv = tmp_path / "oos_lo.csv"
    pd.DataFrame(
        [{"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.6, "target": 1}]
    ).to_csv(oos_lo_csv, index=False)
    oos_ls_csv = tmp_path / "oos_ls.csv"
    pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.6, "target": 1},
            {"fold": 1, "symbol": "B", "decision_ts": "2020-01-02T00:00:00+00:00", "label_end_ts": "2020-02-01T00:00:00+00:00", "ml_score": 0.4, "target": 0},
        ]
    ).to_csv(oos_ls_csv, index=False)
    econ_csv = tmp_path / "econ.csv"
    pd.DataFrame([{"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"}]).to_csv(econ_csv, index=False)
    daily_csv = tmp_path / "daily.csv"
    pd.DataFrame({"date": ["2020-01-02"], "turnover": [0.1]}).to_csv(daily_csv, index=False)

    def _artifact(direction: str, oos_path: Path) -> dict:
        return {
            "aggregate": {
                "net_total_return": 0.01, "net_sharpe": 1.0, "max_drawdown": -0.01,
                "cost_drag": 0.001, "total_turnover": 0.1, "average_gross_exposure": 0.5,
            },
            "folds": [{"fold": 1, "net_total_return": 0.01, "weight_to_share_evidence": {}}],
            "economic_returns_csv": str(econ_csv),
            "economic_daily_returns_csv": str(daily_csv),
            "oos_predictions_csv": str(oos_path),
            "holdout": {"status": "reserved_not_evaluated"},
            "holdout_start_utc": FAR_FUTURE_HOLDOUT,
        }

    def fake_run_one_trial(*, run_dir, experiment_id, hypothesis_id, strategy_id, direction, bars_path, bars_provenance):
        if hypothesis_id == FAMILIES["RANK-01"].hyp_long_only:
            return _artifact("long_only", oos_lo_csv)
        if hypothesis_id == FAMILIES["RANK-01"].hyp_long_short:
            return _artifact("long_short", oos_ls_csv)
        return _artifact("placebo", oos_lo_csv)

    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 101.0},
            {"symbol": "B", "end_ts": "2020-01-01 00:00:00", "close": 50.0},
            {"symbol": "B", "end_ts": "2020-01-02 00:00:00", "close": 49.0},
        ]
    )
    bars["ret_rank_20"] = 0.5
    bars["ret_5"] = 0.01
    bars["gap_pct_1"] = 0.0
    targets = pd.DataFrame(
        {"symbol": ["A"], "end_ts": ["2020-01-01 00:00:00"], "fwd_ret": [0.01], "target": [1], "label_end_ts": ["2020-02-01T00:00:00+00:00"]}
    )

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "ensure_bars", lambda: (bars, {"fake": "manifest"}))
    monkeypatch.setattr(run_wave, "ensure_real_targets", lambda bars: targets)
    monkeypatch.setattr(run_wave, "ensure_placebo_targets", lambda real_targets: targets)
    monkeypatch.setattr(run_wave, "ensure_full_features", lambda bars: bars)
    monkeypatch.setattr(run_wave, "write_run_dir", lambda *a, **k: Path("fake_bars.csv"))
    monkeypatch.setattr(run_wave, "run_one_trial", fake_run_one_trial)

    with pytest.raises(RuntimeError, match="OOS prediction membership"):
        run_wave.run_family("RANK-01")
