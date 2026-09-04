"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01 -- registered diagnostic-artifact
orchestration runner negative controls.

Proves `run_registered_factor_diagnostics` genuinely composes
register -> begin attempt -> evaluate -> write artifact -> finalize, and
that every attempted factor evaluation (succeeded, not_evaluable, or a
genuine exception) remains durably visible in the registry -- never a
hand-built winner-only registration.
"""
from __future__ import annotations

import json

import numpy as np
import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    DIRECTION_LOWER_IS_BETTER,
    EVAL_STATUS_FAILED,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.diagnostics import EvaluationWindowViolation
from mqk_research.factors.registry import get_factor, list_factor_evaluation_attempts, list_factors
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


def _run(registry_db, out_dir, *, observations, spec=None, **overrides):
    kwargs = dict(
        factor_spec=spec or _spec(),
        observations=observations,
        universe_identity=_UNIVERSE_IDENTITY,
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        origin="test_factor_diagnostics_runner",
    )
    kwargs.update(overrides)
    return run_registered_factor_diagnostics(registry_db, out_dir, **kwargs)


def test_succeeded_run_registers_factor_and_writes_a_correctly_bound_artifact(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"

    result = _run(registry_db, out_dir, observations=_monotonic_dataset())

    assert result.status == EVAL_STATUS_SUCCEEDED
    assert get_factor(registry_db, result.factor_id)["factor_id"] == result.factor_id

    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_SUCCEEDED
    assert attempts[0]["evaluation_id"] == result.evaluation_id
    assert attempts[0]["artifact_paths"]["factor_diagnostics"] == str(result.artifact_path)

    assert result.artifact_path.exists()
    artifact = json.loads(result.artifact_path.read_text(encoding="utf-8"))
    # The artifact's own bound identity must match the attempt/result -- never
    # substitutable with a different factor's or evaluation's evidence.
    assert artifact["factor_id"] == result.factor_id
    assert artifact["evaluation_id"] == result.evaluation_id
    assert artifact["status"] == EVAL_STATUS_SUCCEEDED
    assert artifact["metrics"]["mean_ic"] == pytest.approx(1.0, abs=1e-9)


def test_not_evaluable_result_is_durably_registered_never_dropped(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"

    result = _run(registry_db, out_dir, observations=_constant_factor_dataset())

    assert result.status == EVAL_STATUS_NOT_EVALUABLE
    assert result.reason

    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_NOT_EVALUABLE
    assert attempts[0]["failure_reason"] == result.reason

    # Durably visible means an artifact was actually written for this
    # not_evaluable outcome too, not silently skipped.
    assert result.artifact_path.exists()
    artifact = json.loads(result.artifact_path.read_text(encoding="utf-8"))
    assert artifact["status"] == EVAL_STATUS_NOT_EVALUABLE
    assert artifact["metrics"] == {}


def test_retry_of_identical_evaluation_creates_a_new_attempt_not_a_second_factor(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"

    first = _run(registry_db, tmp_path / "out_a", observations=_monotonic_dataset())
    second = _run(registry_db, tmp_path / "out_b", observations=_monotonic_dataset())

    # Same semantic factor + same evaluation window/universe -> identical
    # identity both times; only the attempt/artifact are new.
    assert first.factor_id == second.factor_id
    assert first.evaluation_id == second.evaluation_id
    assert first.attempt_id != second.attempt_id
    assert second.attempt_index == first.attempt_index + 1
    assert first.artifact_path != second.artifact_path

    assert len(list_factors(registry_db, family="momentum")) == 1
    attempts = list_factor_evaluation_attempts(registry_db, first.factor_id)
    assert len(attempts) == 2
    assert {a["attempt_id"] for a in attempts} == {first.attempt_id, second.attempt_id}


def test_horizon_change_registers_a_genuinely_separate_factor(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"

    base = _run(registry_db, tmp_path / "out_h21", observations=_monotonic_dataset(), spec=_spec(horizon_periods=21))
    other = _run(
        registry_db, tmp_path / "out_h63", observations=_monotonic_dataset(), spec=_spec(horizon_periods=63)
    )

    assert base.factor_id != other.factor_id
    assert len(list_factors(registry_db, family="momentum")) == 2


def test_diagnostic_protocol_payload_is_durably_recorded_in_attempt_metadata(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"

    result = _run(registry_db, out_dir, observations=_monotonic_dataset(), n_quantiles=3, min_periods=1)

    attempts = list_factor_evaluation_attempts(registry_db, result.factor_id)
    assert len(attempts) == 1
    protocol = attempts[0]["metadata"]["diagnostic_protocol"]
    assert protocol["n_quantiles"] == 3
    assert protocol["min_periods"] == 1
    assert protocol["direction"] == DIRECTION_HIGHER_IS_BETTER
    assert protocol["min_cross_section"] == 3  # resolved effective value (defaults to n_quantiles)


def test_n_quantiles_change_yields_different_evaluation_identity(tmp_path):
    # min_cross_section is held fixed (and >= both n_quantiles values) so this
    # isolates n_quantiles' OWN contribution to identity from
    # min_cross_section's effective-default coupling to it.
    registry_db = tmp_path / "registry.sqlite3"

    base = _run(
        registry_db, tmp_path / "out_a", observations=_monotonic_dataset(), n_quantiles=2, min_cross_section=6
    )
    other = _run(
        registry_db, tmp_path / "out_b", observations=_monotonic_dataset(), n_quantiles=3, min_cross_section=6
    )

    assert base.factor_id == other.factor_id  # same factor -- only the evaluation protocol differs
    assert base.evaluation_id != other.evaluation_id


def test_min_cross_section_change_yields_different_evaluation_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"

    base = _run(registry_db, tmp_path / "out_a", observations=_monotonic_dataset(), n_quantiles=3, min_cross_section=3)
    other = _run(registry_db, tmp_path / "out_b", observations=_monotonic_dataset(), n_quantiles=3, min_cross_section=4)

    assert base.factor_id == other.factor_id
    assert base.evaluation_id != other.evaluation_id


def test_min_cross_section_default_matches_explicit_effective_value(tmp_path):
    """min_cross_section=None (defaults to n_quantiles) and an explicit
    min_cross_section equal to that same effective value must be the SAME
    identity -- identity binds on effective behavior, not the raw parameter
    the caller happened to spell out."""
    registry_db = tmp_path / "registry.sqlite3"

    implicit = _run(registry_db, tmp_path / "out_a", observations=_monotonic_dataset(), n_quantiles=3)
    explicit = _run(
        registry_db, tmp_path / "out_b", observations=_monotonic_dataset(), n_quantiles=3, min_cross_section=3
    )

    assert implicit.evaluation_id == explicit.evaluation_id


def test_min_periods_change_yields_different_evaluation_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"

    base = _run(registry_db, tmp_path / "out_a", observations=_monotonic_dataset(), min_periods=1)
    other = _run(registry_db, tmp_path / "out_b", observations=_monotonic_dataset(), min_periods=2)

    assert base.factor_id == other.factor_id
    assert base.evaluation_id != other.evaluation_id


def test_runner_has_no_free_direction_override_parameter(tmp_path):
    """A caller must never be able to evaluate a registered factor under a
    direction that contradicts its own identity-bearing
    FactorSpec.direction -- there is no `direction=` kwarg at all to pass
    a contradictory value through."""
    registry_db = tmp_path / "registry.sqlite3"

    with pytest.raises(TypeError):
        _run(
            registry_db,
            tmp_path / "out_a",
            observations=_monotonic_dataset(),
            direction=DIRECTION_LOWER_IS_BETTER,
        )


def test_evaluation_direction_always_matches_factor_spec_direction(tmp_path):
    """Two FactorSpecs that differ ONLY in `direction` are two genuinely
    different, honestly identified factors (direction is already part of
    FactorSpec.identity_payload()) -- never one factor_id evaluated two
    contradictory ways."""
    registry_db = tmp_path / "registry.sqlite3"

    higher = _run(
        registry_db,
        tmp_path / "out_a",
        observations=_monotonic_dataset(),
        spec=_spec(direction=DIRECTION_HIGHER_IS_BETTER),
    )
    lower = _run(
        registry_db,
        tmp_path / "out_b",
        observations=_monotonic_dataset(),
        spec=_spec(direction=DIRECTION_LOWER_IS_BETTER),
    )

    assert higher.factor_id != lower.factor_id
    assert higher.evaluation_id != lower.evaluation_id
    assert higher.status == EVAL_STATUS_SUCCEEDED
    assert lower.status == EVAL_STATUS_SUCCEEDED

    higher_attempts = list_factor_evaluation_attempts(registry_db, higher.factor_id)
    protocol = higher_attempts[0]["metadata"]["diagnostic_protocol"]
    assert protocol["direction"] == DIRECTION_HIGHER_IS_BETTER
    lower_attempts = list_factor_evaluation_attempts(registry_db, lower.factor_id)
    protocol = lower_attempts[0]["metadata"]["diagnostic_protocol"]
    assert protocol["direction"] == DIRECTION_LOWER_IS_BETTER


def test_observation_row_before_evaluation_window_is_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"
    observations = _monotonic_dataset()
    early_row = observations.iloc[[0]].copy()
    early_row["period_ts_utc"] = "2023-12-31T00:00:00+00:00"
    early_row["information_cutoff_ts_utc"] = "2023-12-31T00:00:00+00:00"
    tainted = pd.concat([early_row, observations], ignore_index=True)

    with pytest.raises(EvaluationWindowViolation):
        _run(registry_db, out_dir, observations=tainted)

    # A window violation is a genuine evaluation defect, not silently
    # dropped -- durable failed evidence must exist.
    factors = list_factors(registry_db, family="momentum")
    assert len(factors) == 1
    attempts = list_factor_evaluation_attempts(registry_db, factors[0]["factor_id"])
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED


def test_observation_row_after_evaluation_window_is_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"
    observations = _monotonic_dataset()
    late_row = observations.iloc[[0]].copy()
    late_row["period_ts_utc"] = "2024-06-01T00:00:00+00:00"  # window end is exclusive
    late_row["information_cutoff_ts_utc"] = "2024-06-01T00:00:00+00:00"
    tainted = pd.concat([observations, late_row], ignore_index=True)

    with pytest.raises(EvaluationWindowViolation):
        _run(registry_db, out_dir, observations=tainted)

    factors = list_factors(registry_db, family="momentum")
    assert len(factors) == 1
    attempts = list_factor_evaluation_attempts(registry_db, factors[0]["factor_id"])
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED


def test_result_metric_values_never_define_evaluation_identity(tmp_path):
    """Same exact protocol/window/universe but genuinely different
    observation data (different metric outcome) -- evaluation_id must be
    identical, because identity is a function of the protocol binding, never
    of the result it produces."""
    registry_db = tmp_path / "registry.sqlite3"

    monotonic = _run(registry_db, tmp_path / "out_a", observations=_monotonic_dataset())
    constant = _run(registry_db, tmp_path / "out_b", observations=_constant_factor_dataset())

    assert monotonic.status == EVAL_STATUS_SUCCEEDED
    assert constant.status == EVAL_STATUS_NOT_EVALUABLE
    assert monotonic.evaluation_id == constant.evaluation_id


def test_unexpected_exception_finalizes_failed_and_reraises(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    out_dir = tmp_path / "out"
    # A missing required column is a genuine caller/programmer error --
    # evaluate_factor_ic_ir raises ValueError rather than reporting
    # not_evaluable, and that must still leave durable failed evidence.
    broken_observations = _monotonic_dataset().drop(columns=["label_fwd_ret"])

    with pytest.raises(ValueError):
        _run(registry_db, out_dir, observations=broken_observations)

    factors = list_factors(registry_db, family="momentum")
    assert len(factors) == 1
    attempts = list_factor_evaluation_attempts(registry_db, factors[0]["factor_id"])
    assert len(attempts) == 1
    assert attempts[0]["status"] == EVAL_STATUS_FAILED
    assert "ValueError" in attempts[0]["failure_reason"]
