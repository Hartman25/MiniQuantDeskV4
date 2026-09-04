"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01 -- horizon/decay aggregate report
negative controls.

Builds real registered evaluations via `run_registered_factor_diagnostics`
(never a hand-built fixture), then proves
`build_factor_horizon_decay_report` derives BOTH its horizon family and
each member's authoritative attempt entirely from registry truth: a
caller can never omit an unfavorable (failed/not_evaluable) horizon,
never smuggle in an unrelated factor identity or a mismatched comparison
scope, and never have a retry create a second horizon point or resurrect
an older favorable success once a later terminal retry of the same scope
exists.
"""
from __future__ import annotations

import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_FAILED,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.horizon_decay import (
    HORIZON_STATUS_COMPLETE,
    HORIZON_STATUS_INCOMPLETE,
    build_factor_horizon_decay_report,
)
from mqk_research.factors.registry import begin_factor_evaluation, list_factor_evaluation_attempts, register_factor
from mqk_research.factors.runner import UNIVERSE_MODE_FIXED_EX_ANTE, run_registered_factor_diagnostics

N_SYMBOLS = 6
N_PERIODS = 8
_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"
_UNIVERSE_IDENTITY = {"universe_id": "sp500_pit_v1", "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE}


def _periods():
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, N_PERIODS + 1)]


def _symbols():
    return [f"SYM{i}" for i in range(N_SYMBOLS)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _monotonic_dataset() -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": float(i), "label_fwd_ret": float(i)}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _shifted_monotonic_dataset() -> pd.DataFrame:
    """Same shape as `_monotonic_dataset` but with periods in February, for
    tests that need observations falling inside a shifted evaluation
    window (P1's window-binding check would otherwise reject the January
    periods before the scope-mismatch is ever reached)."""
    rows = []
    for d in range(1, N_PERIODS + 1):
        period = f"2024-02-{d:02d}T00:00:00+00:00"
        for i, sym in enumerate(_symbols()):
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": float(i), "label_fwd_ret": float(i)}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _constant_factor_dataset() -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": 1.0, "label_fwd_ret": float(i)}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _spec(**overrides) -> FactorSpec:
    kwargs = dict(
        family="momentum",
        name="12m_1m_lag",
        protocol_version="v1",
        params={"lookback_days": 252},
        required_input_fields=["close"],
        lookback_periods=252,
        horizon_periods=21,
        normalization=NORMALIZATION_CROSS_SECTIONAL_RANK,
        direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity=_UNIVERSE_IDENTITY,
        data_provenance_identity={"provider": "alpaca"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )
    kwargs.update(overrides)
    return FactorSpec(**kwargs)


def _run(registry_db, out_dir, *, observations, spec, **overrides):
    kwargs = dict(
        factor_spec=spec,
        observations=observations,
        universe_identity=_UNIVERSE_IDENTITY,
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        origin="test_factor_horizon_decay",
    )
    kwargs.update(overrides)
    return run_registered_factor_diagnostics(registry_db, out_dir, **kwargs)


def test_report_derives_full_family_from_registry_ordered_by_horizon(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    r63 = _run(registry_db, tmp_path / "h63", observations=_monotonic_dataset(), spec=_spec(horizon_periods=63))
    r21 = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    r5 = _run(registry_db, tmp_path / "h5", observations=_monotonic_dataset(), spec=_spec(horizon_periods=5))

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=r21.factor_id, anchor_evaluation_id=r21.evaluation_id
    )

    assert report["status"] == HORIZON_STATUS_COMPLETE
    assert report["incomplete_factor_ids"] == []
    assert [h["horizon_periods"] for h in report["horizons"]] == [5, 21, 63]
    for h in report["horizons"]:
        assert h["status"] == EVAL_STATUS_SUCCEEDED
        assert h["mean_ic"] == pytest.approx(1.0, abs=1e-9)
    assert "horizon_periods" not in report["family_identity"]
    assert {r5.factor_id, r21.factor_id, r63.factor_id} == {h["factor_id"] for h in report["horizons"]}


def test_not_evaluable_horizon_cannot_be_omitted(tmp_path):
    """A degenerate horizon must appear even though nobody explicitly
    named it -- the family is derived from the registry, not a caller
    list, so an unfavorable member can never be left out."""
    registry_db = tmp_path / "registry.sqlite3"
    good = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    _run(registry_db, tmp_path / "h63", observations=_constant_factor_dataset(), spec=_spec(horizon_periods=63))

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=good.factor_id, anchor_evaluation_id=good.evaluation_id
    )

    assert report["status"] == HORIZON_STATUS_COMPLETE
    statuses = {h["horizon_periods"]: h["status"] for h in report["horizons"]}
    assert statuses == {21: EVAL_STATUS_SUCCEEDED, 63: EVAL_STATUS_NOT_EVALUABLE}
    degenerate_row = next(h for h in report["horizons"] if h["horizon_periods"] == 63)
    assert degenerate_row["reason"]
    assert degenerate_row["mean_ic"] is None


def test_failed_horizon_cannot_be_omitted(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    good = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    broken_observations = _monotonic_dataset().drop(columns=["label_fwd_ret"])
    with pytest.raises(ValueError):
        _run(registry_db, tmp_path / "h63", observations=broken_observations, spec=_spec(horizon_periods=63))

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=good.factor_id, anchor_evaluation_id=good.evaluation_id
    )

    assert report["status"] == HORIZON_STATUS_COMPLETE
    statuses = {h["horizon_periods"]: h["status"] for h in report["horizons"]}
    assert statuses == {21: EVAL_STATUS_SUCCEEDED, 63: EVAL_STATUS_FAILED}
    failed_row = next(h for h in report["horizons"] if h["horizon_periods"] == 63)
    assert failed_row["reason"]


def test_success_then_later_failed_retry_resolves_to_the_later_attempt(tmp_path):
    """A horizon factor that succeeded, then was retried under the SAME
    comparison scope and failed, must report the LATER (failed) attempt
    as authoritative -- never fall back to the earlier favorable
    success."""
    registry_db = tmp_path / "registry.sqlite3"
    anchor = _run(registry_db, tmp_path / "anchor", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    spec63 = _spec(horizon_periods=63)
    first = _run(registry_db, tmp_path / "h63_a", observations=_monotonic_dataset(), spec=spec63)
    assert first.status == EVAL_STATUS_SUCCEEDED
    broken_observations = _monotonic_dataset().drop(columns=["label_fwd_ret"])
    with pytest.raises(ValueError):
        _run(registry_db, tmp_path / "h63_b", observations=broken_observations, spec=spec63)

    attempts = list_factor_evaluation_attempts(registry_db, first.factor_id)
    assert len(attempts) == 2
    assert attempts[-1]["status"] == EVAL_STATUS_FAILED

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=anchor.factor_id, anchor_evaluation_id=anchor.evaluation_id
    )

    row63 = next(h for h in report["horizons"] if h["horizon_periods"] == 63)
    assert row63["status"] == EVAL_STATUS_FAILED
    assert row63["attempt_id"] == attempts[-1]["attempt_id"]
    assert row63["mean_ic"] is None


def test_success_then_later_started_retry_makes_member_incomplete(tmp_path):
    """A horizon factor that succeeded, then had a SECOND attempt opened
    under the SAME comparison scope that is still `started`, must be
    reported incomplete immediately -- the in-flight retry supersedes the
    older success the instant it is opened, not only once it too becomes
    terminal, so a stale success can never be surfaced while the current
    retry is unresolved."""
    registry_db = tmp_path / "registry.sqlite3"
    anchor = _run(registry_db, tmp_path / "anchor", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    spec63 = _spec(horizon_periods=63)
    first = _run(registry_db, tmp_path / "h63_a", observations=_monotonic_dataset(), spec=spec63)
    assert first.status == EVAL_STATUS_SUCCEEDED

    eval_spec = FactorEvaluationSpec(
        factor_id=first.factor_id,
        universe_identity=_UNIVERSE_IDENTITY,
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version=_protocol_version_of(registry_db, first),
    )
    begin_factor_evaluation(registry_db, eval_spec)

    attempts = list_factor_evaluation_attempts(registry_db, first.factor_id)
    assert len(attempts) == 2
    assert attempts[-1]["status"] == "started"

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=anchor.factor_id, anchor_evaluation_id=anchor.evaluation_id
    )

    assert report["status"] == HORIZON_STATUS_INCOMPLETE
    assert first.factor_id in report["incomplete_factor_ids"]
    assert all(h["factor_id"] != first.factor_id for h in report["horizons"])


def test_retry_of_same_horizon_factor_never_creates_two_points(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(horizon_periods=21)
    first = _run(registry_db, tmp_path / "a", observations=_monotonic_dataset(), spec=spec)
    second = _run(registry_db, tmp_path / "b", observations=_monotonic_dataset(), spec=spec)
    assert first.factor_id == second.factor_id
    assert second.attempt_id != first.attempt_id

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=first.factor_id, anchor_evaluation_id=first.evaluation_id
    )

    matching = [h for h in report["horizons"] if h["horizon_periods"] == 21]
    assert len(matching) == 1
    assert matching[0]["attempt_id"] == second.attempt_id  # the later attempt is authoritative


def test_unregistered_anchor_factor_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    good = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))

    with pytest.raises(KeyError):
        build_factor_horizon_decay_report(
            registry_db, anchor_factor_id="never-registered-factor-id", anchor_evaluation_id=good.evaluation_id
        )


def test_unknown_anchor_evaluation_id_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    good = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))

    with pytest.raises(ValueError):
        build_factor_horizon_decay_report(
            registry_db, anchor_factor_id=good.factor_id, anchor_evaluation_id="never-attempted-evaluation-id"
        )


def test_unrelated_factor_identity_never_appears_in_family(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    momentum = _run(registry_db, tmp_path / "mom", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    _run(
        registry_db,
        tmp_path / "vol",
        observations=_monotonic_dataset(),
        spec=_spec(family="volatility", name="realized_vol_20d", horizon_periods=21),
    )

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=momentum.factor_id, anchor_evaluation_id=momentum.evaluation_id
    )

    assert len(report["horizons"]) == 1
    assert report["horizons"][0]["factor_id"] == momentum.factor_id


def test_in_flight_attempt_is_not_authoritative_and_leaves_report_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    anchor = _run(registry_db, tmp_path / "anchor", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))

    spec63 = _spec(horizon_periods=63)
    factor_id = register_factor(registry_db, spec63)
    eval_spec = FactorEvaluationSpec(
        factor_id=factor_id,
        universe_identity=_UNIVERSE_IDENTITY,
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version=_protocol_version_of(registry_db, anchor),
    )
    begin_factor_evaluation(registry_db, eval_spec)

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=anchor.factor_id, anchor_evaluation_id=anchor.evaluation_id
    )

    assert report["status"] == HORIZON_STATUS_INCOMPLETE
    assert factor_id in report["incomplete_factor_ids"]
    assert all(h["factor_id"] != factor_id for h in report["horizons"])


def _protocol_version_of(registry_db, result):
    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    attempt = next(a for a in attempts if a["attempt_id"] == result.attempt_id)
    return attempt["evaluation_identity"]["evaluation_protocol_version"]


def test_different_evaluation_window_leaves_member_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    r21 = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    _run(
        registry_db,
        tmp_path / "h63",
        observations=_shifted_monotonic_dataset(),
        spec=_spec(horizon_periods=63),
        evaluation_window_start_utc="2024-02-01T00:00:00+00:00",
    )

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=r21.factor_id, anchor_evaluation_id=r21.evaluation_id
    )
    assert report["status"] == HORIZON_STATUS_INCOMPLETE
    assert len(report["incomplete_factor_ids"]) == 1
    assert all(h["horizon_periods"] != 63 for h in report["horizons"])


def test_different_universe_is_a_different_family_not_an_incomplete_member(tmp_path):
    """universe_identity is bound into FactorSpec identity itself (the
    runner requires factor_spec.universe_identity to already equal the
    resolved evaluation universe identity) -- a genuinely different
    universe therefore produces a genuinely different factor_id, which
    falls outside the anchor's horizon family entirely, exactly like an
    unrelated factor identity. It can never masquerade as a same-family
    "incomplete" member."""
    registry_db = tmp_path / "registry.sqlite3"
    r21 = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    different_universe_identity = {"universe_id": "different_universe_v1", "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE}
    _run(
        registry_db,
        tmp_path / "h63",
        observations=_monotonic_dataset(),
        spec=_spec(horizon_periods=63, universe_identity=different_universe_identity),
        universe_identity=different_universe_identity,
    )

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=r21.factor_id, anchor_evaluation_id=r21.evaluation_id
    )
    assert report["status"] == HORIZON_STATUS_COMPLETE
    assert all(h["horizon_periods"] != 63 for h in report["horizons"])


def test_different_label_protocol_leaves_member_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    r21 = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    _run(
        registry_db,
        tmp_path / "h63",
        observations=_monotonic_dataset(),
        spec=_spec(horizon_periods=63),
        label_protocol_version="fwd_ret_label_v2",
    )

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=r21.factor_id, anchor_evaluation_id=r21.evaluation_id
    )
    assert report["status"] == HORIZON_STATUS_INCOMPLETE
    assert all(h["horizon_periods"] != 63 for h in report["horizons"])


def test_different_diagnostic_protocol_leaves_member_incomplete(tmp_path):
    """Two horizons evaluated with different n_quantiles are not a
    comparable decay curve -- P1's protocol identity must be honored here
    too, not just window/universe/label protocol."""
    registry_db = tmp_path / "registry.sqlite3"
    r21 = _run(registry_db, tmp_path / "h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    _run(
        registry_db,
        tmp_path / "h63",
        observations=_monotonic_dataset(),
        spec=_spec(horizon_periods=63),
        n_quantiles=3,
    )

    report = build_factor_horizon_decay_report(
        registry_db, anchor_factor_id=r21.factor_id, anchor_evaluation_id=r21.evaluation_id
    )
    assert report["status"] == HORIZON_STATUS_INCOMPLETE
    assert all(h["horizon_periods"] != 63 for h in report["horizons"])
