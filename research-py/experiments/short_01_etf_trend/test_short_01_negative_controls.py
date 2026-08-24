"""Focused negative-control proofs for SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND.

Covers ONLY the invariants that are specific to THIS experiment's own
parameters (slope_60 feature isolation, entry_threshold=0.55/
short_threshold=0.45 direction resolution, long-only-vs-long-short trial
identity separation) and the driver's own reuse wiring of the accepted
causal placebo helper. Deliberately does NOT re-derive the placebo
function's own general correctness proofs (pair-multiset preservation,
no-cross-horizon-leak, fail-closed-on-ineffective-placebo, etc.) -- those
already exist, unmodified, in the accepted
research-alpha-gap-discovery-01-clean worktree's
test_causal_placebo.py and apply unchanged since `build_causal_placebo_
targets` is imported, not reimplemented (CLAUDE.md #13: test quality over
test count; mission instruction: reuse existing production tests/contracts
rather than duplicating).

Uses only synthetic fixture data -- no network calls, no Alpaca access, no
research-py/src modification.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_experiment import (  # noqa: E402
    HYPOTHESIS_ID_LONG_ONLY,
    HYPOTHESIS_ID_LONG_SHORT,
    LONG_ENTRY_THRESHOLD,
    SHORT_THRESHOLD,
    build_causal_placebo_targets,
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
