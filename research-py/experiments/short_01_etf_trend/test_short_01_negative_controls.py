"""Focused negative-control proofs for SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND.

Covers the invariants specific to THIS experiment's own parameters
(slope_60 feature isolation, entry_threshold=0.55/short_threshold=0.45
direction resolution, long-only-vs-long-short trial identity separation),
the driver's now-self-contained (SHORT-01-DRIVER-PORTABILITY-01) inlined
causal placebo helper, and the fold-reset benchmark comparator
(SHORT-01-BENCHMARK-MEASUREMENT-PARITY-01). `build_causal_placebo_targets`
was originally imported live from the accepted
research-alpha-gap-discovery-01-clean worktree; it is now inlined verbatim
in run_experiment.py so this branch is runnable from a bare checkout, so
this file also carries its own focused correctness proofs (same-horizon
pair-multiset preservation, fail-closed-on-ineffective-placebo,
determinism) rather than relying on that sibling worktree's test suite.

Uses only synthetic fixture data -- no network calls, no Alpaca access, no
research-py/src modification.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_experiment  # noqa: E402
from run_experiment import (  # noqa: E402
    HYPOTHESIS_ID_LONG_ONLY,
    HYPOTHESIS_ID_LONG_SHORT,
    LONG_ENTRY_THRESHOLD,
    SHORT_THRESHOLD,
    build_causal_placebo_targets,
    build_fold_reset_benchmark,
    fold_first_oos_dates,
    isolate_slope_feature,
    _signal_policy_for,
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
# Invariant 1: feature_columns exactly ["slope_60"]
# ---------------------------------------------------------------------------


def test_isolate_slope_feature_selects_only_symbol_end_ts_slope_60() -> None:
    full = pd.DataFrame(
        {
            "symbol": ["SPY", "SPY"],
            "end_ts": ["2020-01-01 00:00:00", "2020-01-02 00:00:00"],
            "slope_60": [0.001, -0.002],
            "r2_60": [0.5, 0.6],
            "ret_20": [0.01, -0.01],
            "momentum_score": [0.4, 0.6],
        }
    )
    isolated = isolate_slope_feature(full)
    assert list(isolated.columns) == ["symbol", "end_ts", "slope_60"]


def test_isolate_slope_feature_fails_closed_when_slope_60_missing() -> None:
    full = pd.DataFrame({"symbol": ["SPY"], "end_ts": ["2020-01-01 00:00:00"], "r2_60": [0.5]})
    with pytest.raises(RuntimeError, match="feature isolation failed"):
        isolate_slope_feature(full)


# ---------------------------------------------------------------------------
# Invariants 5+6: long-only vs long-short trial identity differs, and
# short_threshold participates in that identity, for OUR ACTUAL configured
# specs (not a generic/arbitrary pair of specs).
# ---------------------------------------------------------------------------


def _identity_for(hypothesis_id: str) -> dict:
    spec = EconomicWalkForwardSpec(
        signal_policy=_signal_policy_for(hypothesis_id),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
    )
    return economic_protocol_identity(spec.normalized())


def test_long_only_and_long_short_identity_differ_for_configured_specs() -> None:
    id_long_only = _identity_for(HYPOTHESIS_ID_LONG_ONLY)
    id_long_short = _identity_for(HYPOTHESIS_ID_LONG_SHORT)
    assert id_long_only != id_long_short


def test_short_threshold_is_present_and_identity_bearing_for_long_short() -> None:
    id_long_short = _identity_for(HYPOTHESIS_ID_LONG_SHORT)
    assert id_long_short["signal_policy"]["short_threshold"] == SHORT_THRESHOLD

    # Changing ONLY short_threshold (holding entry_threshold fixed) must
    # change the identity -- proves short_threshold actually participates,
    # not merely present-but-ignored.
    spec_a = EconomicWalkForwardSpec(
        signal_policy=_signal_policy_for(HYPOTHESIS_ID_LONG_SHORT),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
    ).normalized()
    from mqk_research.ml.economic_walkforward import (
        SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        SignalPolicySpec,
    )

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


# ---------------------------------------------------------------------------
# Invariant 7: Trial B's OWN configured thresholds (entry=0.55, short=0.45)
# actually produce a SHORT (-1) direction for a bearish score, LONG (+1)
# for a bullish score, and FLAT (0) in between.
# ---------------------------------------------------------------------------


def test_configured_long_short_thresholds_resolve_direction_correctly() -> None:
    policy = _signal_policy_for(HYPOTHESIS_ID_LONG_SHORT).normalized()
    assert policy.entry_threshold == LONG_ENTRY_THRESHOLD
    assert policy.short_threshold == SHORT_THRESHOLD

    assert _resolve_signal_direction(0.60, policy) == 1   # bullish -> long
    assert _resolve_signal_direction(0.30, policy) == -1  # bearish -> short
    assert _resolve_signal_direction(0.50, policy) == 0   # between thresholds -> flat
    assert _resolve_signal_direction(LONG_ENTRY_THRESHOLD, policy) == 1
    assert _resolve_signal_direction(SHORT_THRESHOLD, policy) == -1


def test_long_only_policy_never_returns_short_direction() -> None:
    policy = _signal_policy_for(HYPOTHESIS_ID_LONG_ONLY).normalized()
    for score in (0.0, 0.1, 0.3, 0.5, 0.54, 0.55, 0.6, 1.0):
        assert _resolve_signal_direction(score, policy) in (0, 1)


# ---------------------------------------------------------------------------
# Driver wiring: the accepted causal placebo helper is actually reachable
# and behaves as expected when invoked through THIS driver's own import.
# ---------------------------------------------------------------------------


def test_driver_reuses_accepted_placebo_helper_and_it_changes_targets() -> None:
    targets = pd.DataFrame(
        [
            {"symbol": "SPY", "end_ts": "2020-01-01 00:00:00", "fwd_ret": 0.01, "target": 1,
             "label_end_ts": pd.Timestamp("2020-02-01T00:00:00+00:00").isoformat()},
            {"symbol": "QQQ", "end_ts": "2020-01-01 00:00:00", "fwd_ret": -0.01, "target": 0,
             "label_end_ts": pd.Timestamp("2020-02-01T00:00:00+00:00").isoformat()},
        ]
    )
    placebo = build_causal_placebo_targets(targets, seed=1234)
    pd.testing.assert_series_equal(placebo["end_ts"], targets["end_ts"])
    pd.testing.assert_series_equal(placebo["label_end_ts"], targets["label_end_ts"])
    assert int((placebo["target"].to_numpy() != targets["target"].to_numpy()).sum()) > 0


# ---------------------------------------------------------------------------
# SHORT-01-DRIVER-PORTABILITY-01: build_causal_placebo_targets is now
# inlined verbatim in run_experiment.py and no longer depends on the
# sibling research-alpha-gap-discovery-01-clean worktree existing on disk
# at a fixed Windows path. Prove portability structurally, and re-prove the
# placebo's own core correctness invariants locally now that this file no
# longer inherits coverage from that sibling worktree's test suite.
# ---------------------------------------------------------------------------


def test_driver_has_no_sibling_worktree_dependency() -> None:
    # No path/HEAD pointer to the sibling worktree, and no dynamic-loader
    # machinery left over from the old importlib-based live import.
    assert not hasattr(run_experiment, "ACCEPTED_ALPHA_WORKTREE")
    assert not hasattr(run_experiment, "ACCEPTED_ALPHA_DRIVER")
    assert not hasattr(run_experiment, "_load_accepted_build_causal_placebo_targets")
    assert "importlib" not in run_experiment.__dict__
    assert "subprocess" not in run_experiment.__dict__

    # build_causal_placebo_targets is DEFINED in this module's own source
    # file, not dynamically loaded from another file at runtime.
    assert build_causal_placebo_targets.__module__ == run_experiment.__name__
    import inspect
    assert inspect.getsourcefile(build_causal_placebo_targets) == run_experiment.__file__


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


def test_placebo_singleton_group_unchanged() -> None:
    targets = _same_horizon_fixture()
    placebo = build_causal_placebo_targets(targets, seed=1234)
    solo_orig = targets[targets["symbol"] == "SOLO"].iloc[0]
    solo_placebo = placebo[placebo["symbol"] == "SOLO"].iloc[0]
    assert solo_orig["fwd_ret"] == solo_placebo["fwd_ret"]
    assert solo_orig["target"] == solo_placebo["target"]


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


def test_placebo_mixed_label_fixture_changes_at_least_one_target() -> None:
    targets = _same_horizon_fixture()
    placebo = build_causal_placebo_targets(targets, seed=1234)
    changed = int((placebo["target"].to_numpy() != targets["target"].to_numpy()).sum())
    assert changed > 0


# ---------------------------------------------------------------------------
# SHORT-01-BENCHMARK-MEASUREMENT-PARITY-01: fold-reset benchmark comparator.
# ---------------------------------------------------------------------------


def test_fold_first_oos_dates_reads_one_first_date_per_fold(tmp_path: Path) -> None:
    oos = pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-03T00:00:00+00:00"},
            {"fold": 2, "symbol": "A", "decision_ts": "2020-06-01T00:00:00+00:00"},
            {"fold": 2, "symbol": "A", "decision_ts": "2020-06-02T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "walk_forward_oos_predictions.csv"
    oos.to_csv(csv_path, index=False)
    assert fold_first_oos_dates(csv_path) == {"2020-01-02", "2020-06-01"}


def test_fold_first_oos_dates_fails_closed_on_ambiguous_fold_assignment(tmp_path: Path) -> None:
    oos = pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00"},
            {"fold": 2, "symbol": "B", "decision_ts": "2020-01-02T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "walk_forward_oos_predictions.csv"
    oos.to_csv(csv_path, index=False)
    with pytest.raises(RuntimeError, match="more than one walk-forward fold"):
        fold_first_oos_dates(csv_path)


def test_fold_reset_benchmark_zeroes_large_pre_fold_jump(tmp_path: Path) -> None:
    """A large overnight/pre-fold return must not leak into the benchmark's
    first fold day: the strategy starts every fold flat with
    net_daily_return=0 on that date, so the benchmark must match."""
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 200.0},  # huge pre-fold jump
            {"symbol": "A", "end_ts": "2020-01-03 00:00:00", "close": 202.0},
        ]
    )
    oos = pd.DataFrame(
        [
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-02T00:00:00+00:00"},
            {"fold": 1, "symbol": "A", "decision_ts": "2020-01-03T00:00:00+00:00"},
        ]
    )
    csv_path = tmp_path / "walk_forward_oos_predictions.csv"
    oos.to_csv(csv_path, index=False)

    result = build_fold_reset_benchmark(
        bars, ["A"], csv_path, ["2020-01-02", "2020-01-03"], "2099-01-01T00:00:00Z"
    )
    # naive pct_change would give (200-100)/100=1.0 on 2020-01-02; fold-reset
    # forces it to 0.0, leaving only the 2020-01-03 return of (202-200)/200.
    assert result["cumulative_return_over_reference_dates"] == pytest.approx((202.0 / 200.0) - 1.0)


def test_fold_reset_benchmark_fails_closed_on_holdout_date() -> None:
    bars = pd.DataFrame(
        [
            {"symbol": "A", "end_ts": "2020-01-01 00:00:00", "close": 100.0},
            {"symbol": "A", "end_ts": "2020-01-02 00:00:00", "close": 101.0},
        ]
    )
    with pytest.raises(RuntimeError, match="reserved holdout"):
        build_fold_reset_benchmark(bars, ["A"], Path("unused.csv"), ["2020-01-02"], "2020-01-01T00:00:00Z")
