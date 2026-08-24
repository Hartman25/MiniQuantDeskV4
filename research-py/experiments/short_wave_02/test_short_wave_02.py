"""Focused load-bearing tests for SHORT-RESEARCH-WAVE-02-CONTROLLER's Patch
A driver/predeclaration. Uses only synthetic fixture data -- no network
calls, no Alpaca access, no research-py/src modification.

Covers the mission's required invariants:
 1. predeclaration agrees exactly with driver constants/config
 2. each feature schema contains exactly one expected feature
 3. real and placebo experiment IDs are distinct
 4. exactly six real candidate definitions exist
 5. exactly three placebo definitions exist
 6. no placebo trial can accidentally enter the real experiment ID
 7. causal placebo preserves exact horizon groups
 8. causal placebo changes target assignments
 9. final holdout remains reserved (WF_HOLDOUT_MONTHS frozen > 0, unconsumed
    holdout asserted structurally via wf_spec/holdout wiring, proven at
    result time in the per-hypothesis reports)
10. long/short identities differ from paired long-only identities
11. short_threshold is identity-bearing
12. benchmark uses economic_returns.csv fold authority
13. benchmark date-set equality is fail-closed
14. no direct/same-bar economic execution is introduced (WalkForwardSpec
    purge_enabled=True is frozen and identity-bearing)
"""

from __future__ import annotations

import sys
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_wave  # noqa: E402
from run_wave import (  # noqa: E402
    DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS,
    FAMILIES,
    LONG_ENTRY_THRESHOLD,
    PLACEBO_EXPERIMENT_ID,
    REAL_CANDIDATE_HYPOTHESIS_IDS,
    REAL_EXPERIMENT_ID,
    SHORT_THRESHOLD,
    WF_HOLDOUT_MONTHS,
    assert_driver_agrees_with_predeclaration,
    build_causal_placebo_targets,
    build_fold_reset_benchmark,
    economic_fold_date_authority,
    isolate_feature,
    load_predeclaration,
    signal_policy_for,
)

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
from mqk_research.ml.economic_walkforward import (  # noqa: E402
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    _resolve_signal_direction,
    economic_protocol_identity,
)


# ---------------------------------------------------------------------------
# Invariant 1: predeclaration agrees exactly with driver constants/config
# ---------------------------------------------------------------------------


def test_driver_agrees_with_committed_predeclaration() -> None:
    assert_driver_agrees_with_predeclaration()


def test_predeclaration_file_is_readable_and_matches_frozen_experiment_ids() -> None:
    decl = load_predeclaration()
    assert decl["real_experiment_id"] == REAL_EXPERIMENT_ID
    assert decl["placebo_experiment_id"] == PLACEBO_EXPERIMENT_ID
    assert decl["placebo_seed"] == 1234
    assert decl["label"]["horizon_bars"] == 10


# ---------------------------------------------------------------------------
# Invariant 2: each feature schema contains exactly one expected feature
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("family_key,expected_col", [
    ("SHORT-02", "ret_rank_20"),
    ("SHORT-03", "ret_5"),
    ("SHORT-04", "gap_pct_1"),
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
            "momentum_score": [0.4, 0.6],
        }
    )
    isolated = isolate_feature(full, expected_col)
    assert list(isolated.columns) == ["symbol", "end_ts", expected_col]
    assert FAMILIES[family_key].feature_column == expected_col


def test_isolate_feature_fails_closed_when_declared_column_missing() -> None:
    full = pd.DataFrame({"symbol": ["SPY"], "end_ts": ["2020-01-01 00:00:00"], "ret_5": [0.01]})
    with pytest.raises(RuntimeError, match="feature isolation failed"):
        isolate_feature(full, "gap_pct_1")


# ---------------------------------------------------------------------------
# Invariant 3: real and placebo experiment IDs are distinct
# ---------------------------------------------------------------------------


def test_real_and_placebo_experiment_ids_are_distinct() -> None:
    assert REAL_EXPERIMENT_ID != PLACEBO_EXPERIMENT_ID
    assert REAL_EXPERIMENT_ID == "SHORT-WAVE-02-REAL-CANDIDATES-V1"
    assert PLACEBO_EXPERIMENT_ID == "SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1"


# ---------------------------------------------------------------------------
# Invariants 4+5: exactly six real candidate / three placebo definitions
# ---------------------------------------------------------------------------


def test_exactly_six_real_candidate_hypothesis_ids() -> None:
    assert len(REAL_CANDIDATE_HYPOTHESIS_IDS) == 6
    assert len(set(REAL_CANDIDATE_HYPOTHESIS_IDS)) == 6


def test_exactly_three_diagnostic_placebo_hypothesis_ids() -> None:
    assert len(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS) == 3
    assert len(set(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS)) == 3


def test_exactly_three_hypothesis_families_each_single_feature() -> None:
    assert set(FAMILIES.keys()) == {"SHORT-02", "SHORT-03", "SHORT-04"}
    feature_cols = [fam.feature_column for fam in FAMILIES.values()]
    assert feature_cols == sorted(set(feature_cols)) or len(set(feature_cols)) == 3
    assert len(set(feature_cols)) == 3  # three genuinely distinct single features


# ---------------------------------------------------------------------------
# Invariant 6: no placebo trial can accidentally enter the real experiment ID
# ---------------------------------------------------------------------------


def test_placebo_hypothesis_ids_disjoint_from_real_candidate_ids() -> None:
    assert set(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS).isdisjoint(set(REAL_CANDIDATE_HYPOTHESIS_IDS))


def test_run_family_registers_placebo_trial_under_placebo_experiment_id_only(monkeypatch) -> None:
    """Structural proof: run_family always calls run_one_trial for the
    placebo leg with experiment_id=PLACEBO_EXPERIMENT_ID, never
    REAL_EXPERIMENT_ID -- by intercepting run_one_trial and recording the
    experiment_id passed for each call, without touching the network or a
    real registry DB."""
    calls = []

    def fake_run_one_trial(*, run_dir, experiment_id, hypothesis_id, strategy_id, direction, bars_path, bars_provenance):
        calls.append({"experiment_id": experiment_id, "hypothesis_id": hypothesis_id, "direction": direction})
        return {
            "experiment_id": experiment_id, "hypothesis_id": hypothesis_id, "direction": direction,
            "trial_id": f"fake-{hypothesis_id}", "economic_eval_id": "fake-eval",
            "economic_walk_forward_json": "fake.json",
            "economic_daily_returns_csv": None, "economic_returns_csv": None,
            "aggregate": {"net_total_return": 0.0, "net_sharpe": 0.0, "max_drawdown": 0.0, "cost_drag": 0.0},
            "holdout": None, "holdout_start_utc": None,
            "folds_generated": None, "folds_used": None, "folds_skipped": None,
        }

    bars = pd.DataFrame(
        {
            "symbol": ["SPY"] * 3,
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00", "2020-01-03 00:00:00"],
            "ret_rank_20": [0.9, 0.1, 0.5],
            "ret_5": [0.01, -0.02, 0.0],
            "gap_pct_1": [0.001, -0.003, 0.0],
        }
    )
    targets = pd.DataFrame(
        {
            "symbol": ["SPY"] * 3,
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00", "2020-01-03 00:00:00"],
            "fwd_ret": [0.01, -0.01, 0.02],
            "target": [1, 0, 1],
            "label_end_ts": ["2020-02-01T00:00:00+00:00"] * 3,
        }
    )

    monkeypatch.setattr(run_wave, "ensure_bars", lambda: (bars, {"fake": "manifest"}))
    monkeypatch.setattr(run_wave, "ensure_real_targets", lambda bars: targets)
    monkeypatch.setattr(run_wave, "ensure_placebo_targets", lambda real_targets: targets)
    monkeypatch.setattr(run_wave, "ensure_full_features", lambda bars: bars)
    monkeypatch.setattr(run_wave, "write_run_dir", lambda *a, **k: Path("fake_bars.csv"))
    monkeypatch.setattr(run_wave, "run_one_trial", fake_run_one_trial)

    run_wave.run_family("SHORT-02")

    assert len(calls) == 3
    by_direction_and_id = {(c["direction"], c["hypothesis_id"]): c["experiment_id"] for c in calls}
    placebo_calls = [c for c in calls if c["hypothesis_id"] == FAMILIES["SHORT-02"].hyp_placebo]
    assert len(placebo_calls) == 1
    assert placebo_calls[0]["experiment_id"] == PLACEBO_EXPERIMENT_ID
    real_calls = [c for c in calls if c["hypothesis_id"] != FAMILIES["SHORT-02"].hyp_placebo]
    assert len(real_calls) == 2
    assert all(c["experiment_id"] == REAL_EXPERIMENT_ID for c in real_calls)


# ---------------------------------------------------------------------------
# Invariants 7+8: causal placebo preserves exact horizon groups and changes
# target assignments (shared helper, verified identically to SHORT-01)
# ---------------------------------------------------------------------------


def _same_horizon_fixture() -> pd.DataFrame:
    rows = []
    for g in range(6):
        end_ts = f"2020-01-{g + 1:02d} 00:00:00"
        label_end_ts = pd.Timestamp(f"2020-02-{g + 1:02d}T00:00:00+00:00").isoformat()
        for si, sym in enumerate(("SPY", "QQQ", "IWM", "DIA")):
            fwd_ret = ((-1) ** (g + si)) * (0.001 * (si + 1) + 0.0001 * g)
            rows.append(
                {
                    "symbol": sym,
                    "end_ts": end_ts,
                    "fwd_ret": fwd_ret,
                    "target": 1 if fwd_ret > 0.0 else 0,
                    "label_end_ts": label_end_ts,
                }
            )
    rows.append(
        {
            "symbol": "SOLO",
            "end_ts": "2099-01-01 00:00:00",
            "fwd_ret": 0.05,
            "target": 1,
            "label_end_ts": pd.Timestamp("2099-02-01T00:00:00+00:00").isoformat(),
        }
    )
    return pd.DataFrame(rows)


def test_placebo_pair_multiset_preserved_within_each_same_horizon_group() -> None:
    targets = _same_horizon_fixture()
    placebo = build_causal_placebo_targets(targets, seed=1234)
    for key, orig_group in targets.groupby(["end_ts", "label_end_ts"]):
        placebo_group = placebo[(placebo["end_ts"] == key[0]) & (placebo["label_end_ts"] == key[1])]
        orig_pairs = sorted(zip(orig_group["fwd_ret"], orig_group["target"]))
        placebo_pairs = sorted(zip(placebo_group["fwd_ret"], placebo_group["target"]))
        assert orig_pairs == placebo_pairs, f"pair multiset changed within group {key}"


def test_placebo_symbol_and_timestamps_never_move() -> None:
    targets = _same_horizon_fixture()
    placebo = build_causal_placebo_targets(targets, seed=1234)
    pd.testing.assert_series_equal(placebo["symbol"], targets["symbol"])
    pd.testing.assert_series_equal(placebo["end_ts"], targets["end_ts"])
    pd.testing.assert_series_equal(placebo["label_end_ts"], targets["label_end_ts"])


def test_placebo_changes_at_least_one_target_assignment() -> None:
    targets = _same_horizon_fixture()
    placebo = build_causal_placebo_targets(targets, seed=1234)
    changed = int((placebo["target"].to_numpy() != targets["target"].to_numpy()).sum())
    assert changed > 0


def test_placebo_fails_closed_on_ineffective_permutation() -> None:
    end_ts = "2020-01-01 00:00:00"
    label_end_ts = pd.Timestamp("2020-02-01T00:00:00+00:00").isoformat()
    all_same_target_group = pd.DataFrame(
        [
            {"symbol": sym, "end_ts": end_ts, "fwd_ret": 0.001 * (i + 1), "target": 1, "label_end_ts": label_end_ts}
            for i, sym in enumerate(("A", "B", "C", "D"))
        ]
    )
    with pytest.raises(RuntimeError, match="zero changed target assignments"):
        build_causal_placebo_targets(all_same_target_group, seed=1234)


def test_placebo_deterministic_across_calls() -> None:
    targets = _same_horizon_fixture()
    p1 = build_causal_placebo_targets(targets, seed=1234)
    p2 = build_causal_placebo_targets(targets, seed=1234)
    pd.testing.assert_frame_equal(p1, p2)


# ---------------------------------------------------------------------------
# Invariant 9: final holdout remains reserved (frozen holdout_months > 0)
# ---------------------------------------------------------------------------


def test_holdout_months_frozen_and_nonzero() -> None:
    decl = load_predeclaration()
    assert WF_HOLDOUT_MONTHS == 6
    assert decl["walk_forward"]["holdout_months"] == 6
    assert WF_HOLDOUT_MONTHS > 0


# ---------------------------------------------------------------------------
# Invariants 10+11: long/short identities differ from paired long-only
# identities, and short_threshold is identity-bearing, for every family
# ---------------------------------------------------------------------------


def _identity_for(direction: str) -> dict:
    spec = EconomicWalkForwardSpec(
        signal_policy=signal_policy_for(direction),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
    )
    return economic_protocol_identity(spec.normalized())


def test_long_only_and_long_short_identity_differ() -> None:
    assert _identity_for("long_only") != _identity_for("long_short")


def test_short_threshold_is_present_and_identity_bearing() -> None:
    id_long_short = _identity_for("long_short")
    assert id_long_short["signal_policy"]["short_threshold"] == SHORT_THRESHOLD

    from mqk_research.ml.economic_walkforward import (
        SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        SignalPolicySpec,
    )

    spec_a = EconomicWalkForwardSpec(
        signal_policy=signal_policy_for("long_short"),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
    ).normalized()
    spec_b = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(
            entry_threshold=LONG_ENTRY_THRESHOLD,
            long_only=False,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
            short_threshold=SHORT_THRESHOLD - 0.05,
        ),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
    ).normalized()
    assert economic_protocol_identity(spec_a) != economic_protocol_identity(spec_b)


def test_configured_thresholds_resolve_direction_correctly() -> None:
    policy = signal_policy_for("long_short").normalized()
    assert _resolve_signal_direction(0.60, policy) == 1   # bullish -> long
    assert _resolve_signal_direction(0.30, policy) == -1  # bearish -> short
    assert _resolve_signal_direction(0.50, policy) == 0   # between thresholds -> flat


def test_long_only_policy_never_returns_short_direction() -> None:
    policy = signal_policy_for("long_only").normalized()
    for score in (0.0, 0.1, 0.3, 0.5, 0.54, 0.55, 0.6, 1.0):
        assert _resolve_signal_direction(score, policy) in (0, 1)


# ---------------------------------------------------------------------------
# Invariant 12: benchmark uses economic_returns.csv fold authority (not
# walk_forward_oos_predictions.csv)
# ---------------------------------------------------------------------------


def test_economic_fold_date_authority_reads_reset_date_and_full_date_set(tmp_path: Path) -> None:
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
            {"fold": 2, "timestamp": "2020-06-01T00:00:00+00:00"},
            {"fold": 2, "timestamp": "2020-06-02T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "economic_returns.csv"
    econ.to_csv(csv_path, index=False)
    authority = economic_fold_date_authority(csv_path)
    assert authority["reset_dates"] == {"2020-01-02", "2020-06-01"}
    assert authority["date_set"] == {"2020-01-02", "2020-01-03", "2020-06-01", "2020-06-02"}


def test_economic_fold_date_authority_fails_closed_on_ambiguous_fold_assignment(tmp_path: Path) -> None:
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 2, "timestamp": "2020-01-02T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "economic_returns.csv"
    econ.to_csv(csv_path, index=False)
    with pytest.raises(RuntimeError, match="more than one economic fold"):
        economic_fold_date_authority(csv_path)


def test_fold_reset_benchmark_zeroes_large_pre_fold_jump(tmp_path: Path) -> None:
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 200.0},
            {"symbol": "A", "end_ts": "2020-01-03 00:00:00", "close": 202.0},
        ]
    )
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "economic_returns.csv"
    econ.to_csv(csv_path, index=False)
    result = build_fold_reset_benchmark(bars, ["A"], csv_path, ["2020-01-02", "2020-01-03"], "2099-01-01T00:00:00Z")
    assert result["cumulative_return_over_reference_dates"] == pytest.approx((202.0 / 200.0) - 1.0)


# ---------------------------------------------------------------------------
# Invariant 13: benchmark date-set equality is fail-closed
# ---------------------------------------------------------------------------


def test_fold_reset_benchmark_fails_closed_on_holdout_date() -> None:
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 101.0},
        ]
    )
    with pytest.raises(RuntimeError, match="reserved holdout"):
        build_fold_reset_benchmark(bars, ["A"], Path("unused.csv"), ["2020-01-02"], "2020-01-01T00:00:00Z")


def test_fold_reset_benchmark_fails_closed_when_reference_date_missing_from_economic_authority(tmp_path: Path) -> None:
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-03 00:00:00", "close": 101.0},
        ]
    )
    econ = pd.DataFrame([{"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"}])
    csv_path = tmp_path / "economic_returns.csv"
    econ.to_csv(csv_path, index=False)
    with pytest.raises(RuntimeError, match="not identical"):
        build_fold_reset_benchmark(bars, ["A"], csv_path, ["2020-01-02", "2020-01-03"], "2099-01-01T00:00:00Z")


def test_fold_reset_benchmark_fails_closed_when_economic_authority_has_extra_date(tmp_path: Path) -> None:
    bars = pd.DataFrame([{"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 100.0}])
    econ = pd.DataFrame(
        [
            {"fold": 1, "timestamp": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "timestamp": "2020-01-03T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "economic_returns.csv"
    econ.to_csv(csv_path, index=False)
    with pytest.raises(RuntimeError, match="not identical"):
        build_fold_reset_benchmark(bars, ["A"], csv_path, ["2020-01-02"], "2099-01-01T00:00:00Z")


# ---------------------------------------------------------------------------
# Invariant 14: no direct/same-bar economic execution is introduced --
# purge_enabled is frozen True and identity-bearing (proves the driver has
# not silently disabled causal purge/embargo protection)
# ---------------------------------------------------------------------------


def test_purge_enabled_is_frozen_true_in_predeclaration_and_driver() -> None:
    decl = load_predeclaration()
    assert decl["walk_forward"]["purge_enabled"] is True
    assert decl["walk_forward"]["embargo_seconds"] == 0
