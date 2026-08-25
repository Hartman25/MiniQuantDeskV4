"""
RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01 -- identity negative controls.

Proves FactorSpec.compute_factor_id() is a pure function of SEMANTIC inputs
only: identical semantics collide to the same id, each named semantic axis
changes the id, and result/transport-only changes never do.
"""
from __future__ import annotations

import copy

import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationResult,
    FactorEvaluationSpec,
    FactorObservation,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
    TIMING_SAME_BAR_CLOSE,
)


def _base_kwargs() -> dict:
    return dict(
        family="momentum",
        name="12m_1m_lag",
        protocol_version="v1",
        params={"lookback_days": 252, "skip_days": 21},
        required_input_fields=["close"],
        lookback_periods=252,
        horizon_periods=21,
        normalization=NORMALIZATION_CROSS_SECTIONAL_RANK,
        direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity={"universe_id": "sp500_pit_v1"},
        data_provenance_identity={"provider": "alpaca", "adjustment": "split_only"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )


def _spec(**overrides) -> FactorSpec:
    kwargs = _base_kwargs()
    kwargs.update(overrides)
    return FactorSpec(**kwargs)


# 1. identical semantic factor -> identical factor_id
def test_identical_semantic_spec_yields_identical_factor_id():
    assert _spec().compute_factor_id() == _spec().compute_factor_id()


# 2. parameter change -> different factor_id
def test_parameter_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(params={"lookback_days": 252, "skip_days": 5}).compute_factor_id()
    assert base != changed


# 3. lookback change -> different factor_id
def test_lookback_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(lookback_periods=126).compute_factor_id()
    assert base != changed


# 4. timing/lag change -> different factor_id
def test_timing_convention_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(timing_convention=TIMING_SAME_BAR_CLOSE).compute_factor_id()
    assert base != changed


def test_information_lag_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(information_lag_periods=2).compute_factor_id()
    assert base != changed


# 5. universe identity change -> different factor_id
def test_universe_identity_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(universe_identity={"universe_id": "russell1000_pit_v1"}).compute_factor_id()
    assert base != changed


# 6. data/provenance semantic identity change -> different factor_id
def test_data_provenance_identity_change_changes_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(data_provenance_identity={"provider": "alpaca", "adjustment": "split_and_dividend"}).compute_factor_id()
    assert base != changed


# 7. factor result/value change -> SAME factor_id
# factor_id is computed purely from FactorSpec; no FactorObservation value is
# ever an input to identity, regardless of how many observations exist or
# what they contain.
def test_observation_values_never_affect_factor_id():
    spec = _spec()
    factor_id = spec.compute_factor_id()

    obs_a = FactorObservation(
        factor_id=factor_id,
        symbol="AAPL",
        observation_ts_utc="2024-01-02T00:00:00+00:00",
        information_cutoff_ts_utc="2024-01-02T00:00:00+00:00",
        value=1.2345,
    )
    obs_b = FactorObservation(
        factor_id=factor_id,
        symbol="AAPL",
        observation_ts_utc="2024-01-02T00:00:00+00:00",
        information_cutoff_ts_utc="2024-01-02T00:00:00+00:00",
        value=-99.0,
    )
    obs_a.validate()
    obs_b.validate()
    assert spec.compute_factor_id() == factor_id


# 8. output path/layout change -> SAME factor_id
def test_layout_note_change_does_not_change_factor_id():
    base = _spec().compute_factor_id()
    changed = _spec(layout_note="artifacts/v2/moved/here").compute_factor_id()
    assert base == changed
    assert "layout_note" not in _spec().identity_payload()


# 9. retry/attempt -> SAME factor_id (pure identity level: recomputing from
# the same spec any number of times never drifts)
def test_recomputing_factor_id_repeatedly_is_stable():
    spec = _spec()
    ids = {spec.compute_factor_id() for _ in range(5)}
    assert len(ids) == 1


# 10. malformed/noncanonical spec rejected
@pytest.mark.parametrize(
    "overrides",
    [
        {"family": ""},
        {"name": "  "},
        {"protocol_version": ""},
        {"required_input_fields": []},
        {"required_input_fields": [""]},
        {"lookback_periods": -1},
        {"horizon_periods": 0},
        {"information_lag_periods": -1},
        {"direction": "sideways"},
        {"normalization": "not_a_real_normalization"},
        {"timing_convention": "whenever"},
        {"universe_identity": "not-a-dict"},
        {"data_provenance_identity": "not-a-dict"},
        {"params": "not-a-dict"},
    ],
)
def test_malformed_spec_rejected(overrides):
    with pytest.raises(ValueError):
        _spec(**overrides).compute_factor_id()


def test_malformed_observation_causality_violation_rejected():
    obs = FactorObservation(
        factor_id="deadbeef",
        symbol="AAPL",
        observation_ts_utc="2024-01-01T00:00:00+00:00",
        information_cutoff_ts_utc="2024-01-02T00:00:00+00:00",  # after observation -- lookahead
        value=1.0,
    )
    with pytest.raises(ValueError, match="causality"):
        obs.validate()


def test_malformed_observation_naive_timestamp_rejected():
    obs = FactorObservation(
        factor_id="deadbeef",
        symbol="AAPL",
        observation_ts_utc="2024-01-01T00:00:00",  # no UTC offset
        information_cutoff_ts_utc="2024-01-01T00:00:00",
        value=1.0,
    )
    with pytest.raises(ValueError, match="UTC offset"):
        obs.validate()


def test_malformed_observation_non_finite_value_rejected():
    obs = FactorObservation(
        factor_id="deadbeef",
        symbol="AAPL",
        observation_ts_utc="2024-01-01T00:00:00+00:00",
        information_cutoff_ts_utc="2024-01-01T00:00:00+00:00",
        value=float("nan"),
    )
    with pytest.raises(ValueError, match="finite"):
        obs.validate()


# -- FactorEvaluationSpec identity --------------------------------------

def _eval_spec(**overrides) -> FactorEvaluationSpec:
    kwargs = dict(
        factor_id="f" * 32,
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_v1",
    )
    kwargs.update(overrides)
    return FactorEvaluationSpec(**kwargs)


def test_evaluation_identity_stable_for_identical_spec():
    assert _eval_spec().compute_evaluation_id() == _eval_spec().compute_evaluation_id()


def test_evaluation_identity_changes_with_universe():
    base = _eval_spec().compute_evaluation_id()
    changed = _eval_spec(universe_identity={"universe_id": "russell1000_pit_v1"}).compute_evaluation_id()
    assert base != changed


def test_evaluation_identity_rejects_inverted_window():
    with pytest.raises(ValueError):
        _eval_spec(
            evaluation_window_start_utc="2024-06-01T00:00:00+00:00",
            evaluation_window_end_utc="2024-01-01T00:00:00+00:00",
        ).compute_evaluation_id()


def test_evaluation_result_requires_reason_unless_succeeded():
    with pytest.raises(ValueError, match="reason"):
        FactorEvaluationResult(
            eval_id="e" * 32, factor_id="f" * 32, status=EVAL_STATUS_NOT_EVALUABLE
        ).validate()

    # succeeded requires no reason
    FactorEvaluationResult(
        eval_id="e" * 32, factor_id="f" * 32, status=EVAL_STATUS_SUCCEEDED, metrics={"ic_mean": 0.05}
    ).validate()

    # not_evaluable with reason is fine
    FactorEvaluationResult(
        eval_id="e" * 32,
        factor_id="f" * 32,
        status=EVAL_STATUS_NOT_EVALUABLE,
        reason="zero_variance_factor",
    ).validate()


def test_spec_dataclass_is_immutable():
    spec = _spec()
    with pytest.raises(Exception):
        spec.family = "mutated"  # type: ignore[misc]


def test_identity_payload_is_deep_copy_safe():
    spec = _spec()
    payload = spec.identity_payload()
    payload["params"]["lookback_days"] = 999999
    # mutating the returned payload must not corrupt the original spec/identity
    assert spec.identity_payload()["params"]["lookback_days"] == 252
