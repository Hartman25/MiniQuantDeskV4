"""
RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01 -- durable registry negative controls.

Mirrors the discipline proven for hypothesis/trial/attempt in
test_experiment_registry.py: idempotent registration, fail-closed collision
detection, append-only attempts, no winner-only registration, unknown-factor
rejection, and stable reopen/migration.
"""
from __future__ import annotations

import pytest

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_FAILED,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationResult,
    FactorEvaluationSpec,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.registry import (
    begin_factor_evaluation,
    finalize_factor_evaluation,
    get_factor,
    list_factor_evaluation_attempts,
    list_factors,
    register_factor,
)


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
        universe_identity={"universe_id": "sp500_pit_v1"},
        data_provenance_identity={"provider": "alpaca"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )
    kwargs.update(overrides)
    return FactorSpec(**kwargs)


def _eval_spec(factor_id: str, **overrides) -> FactorEvaluationSpec:
    kwargs = dict(
        factor_id=factor_id,
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_v1",
    )
    kwargs.update(overrides)
    return FactorEvaluationSpec(**kwargs)


# -- duplicate exact registration is idempotent --------------------------

def test_duplicate_exact_registration_is_idempotent(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec()
    id_a = register_factor(registry_db, spec)
    id_b = register_factor(registry_db, spec)
    assert id_a == id_b
    assert len(list_factors(registry_db, family="momentum")) == 1


# -- conflicting same ID / different semantic payload fails closed -------

def test_conflicting_identity_under_same_factor_id_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_factor(
        factor_id="collision-id",
        family="momentum",
        name="a",
        protocol_version="v1",
        identity={"x": 1},
    )
    with pytest.raises(RuntimeError, match="collision"):
        store.register_factor(
            factor_id="collision-id",
            family="momentum",
            name="b",  # different semantic payload under the SAME id
            protocol_version="v1",
            identity={"x": 2},
        )


# -- failed evaluation is durably recorded --------------------------------

def test_failed_evaluation_is_durably_recorded(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    attempt_id, evaluation_id, attempt_index = begin_factor_evaluation(
        registry_db, _eval_spec(factor_id), origin="test"
    )
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(
            eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_FAILED, reason="synthetic failure"
        ),
    )
    attempts = list_factor_evaluation_attempts(registry_db, factor_id)
    assert len(attempts) == 1
    assert attempts[0]["status"] == "failed"
    assert attempts[0]["failure_reason"] == "synthetic failure"


# -- successful evaluation is durably recorded ----------------------------

def test_successful_evaluation_is_durably_recorded(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(
            eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={"ic_mean": 0.03}
        ),
    )
    attempts = list_factor_evaluation_attempts(registry_db, factor_id)
    assert attempts[0]["status"] == "succeeded"
    assert attempts[0]["result_summary"]["metrics"]["ic_mean"] == 0.03
    assert attempts[0]["evaluation_id"] == evaluation_id


# -- retry does not create another factor identity ------------------------

def test_retry_does_not_create_another_factor_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    eval_spec = _eval_spec(factor_id)

    attempt_1, eval_id_1, idx_1 = begin_factor_evaluation(registry_db, eval_spec)
    finalize_factor_evaluation(
        registry_db,
        attempt_1,
        FactorEvaluationResult(eval_id=eval_id_1, factor_id=factor_id, status=EVAL_STATUS_FAILED, reason="transient"),
    )
    attempt_2, eval_id_2, idx_2 = begin_factor_evaluation(registry_db, eval_spec)
    finalize_factor_evaluation(
        registry_db,
        attempt_2,
        FactorEvaluationResult(eval_id=eval_id_2, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={}),
    )

    assert eval_id_1 == eval_id_2  # same evaluation identity across retries
    assert idx_1 == 1 and idx_2 == 2  # distinct append-only attempts
    assert len(list_factors(registry_db)) == 1  # still exactly one factor


# -- winner-only registration is not required -----------------------------

def test_winner_only_registration_not_required(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    # No evaluation has ever run. The factor is still fully registered and
    # queryable -- registration never depends on a result existing.
    record = get_factor(registry_db, factor_id)
    assert record["factor_id"] == factor_id
    assert list_factor_evaluation_attempts(registry_db, factor_id) == []


# -- unknown factor cannot receive a successful authoritative evaluation --

def test_unknown_factor_cannot_begin_evaluation(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    never_registered_id = "f" * 32
    with pytest.raises(KeyError):
        begin_factor_evaluation(registry_db, _eval_spec(never_registered_id))


# -- schema reopen/migration is stable ------------------------------------

def test_schema_reopen_is_stable_and_preserves_data(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={}),
    )

    # Simulate a process restart: reopen against the same db file.
    reopened = ResearchResultStore(registry_db)
    record = reopened.get_factor(factor_id)
    assert record["factor_id"] == factor_id
    attempts = reopened.list_factor_evaluation_attempts(factor_id)
    assert len(attempts) == 1
    assert attempts[0]["status"] == "succeeded"


# -- terminal attempts are never reopened ---------------------------------

def test_terminal_factor_evaluation_attempt_cannot_be_refinalized(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={}),
    )
    with pytest.raises(RuntimeError, match="already terminal"):
        finalize_factor_evaluation(
            registry_db,
            attempt_id,
            FactorEvaluationResult(
                eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_FAILED, reason="should not apply"
            ),
        )


# -- not_evaluable is a distinct durable terminal status -------------------

def test_not_evaluable_evaluation_is_durably_recorded(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(
            eval_id=evaluation_id,
            factor_id=factor_id,
            status=EVAL_STATUS_NOT_EVALUABLE,
            reason="zero_variance_factor",
        ),
    )
    attempts = list_factor_evaluation_attempts(registry_db, factor_id)
    assert attempts[0]["status"] == "not_evaluable"
    assert attempts[0]["failure_reason"] == "zero_variance_factor"


# -- universe identity change on an evaluation spec changes evaluation_id
# but never the underlying factor_id -------------------------------------

def test_evaluation_universe_change_does_not_alter_factor_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())

    eval_spec_a = _eval_spec(factor_id, universe_identity={"universe_id": "sp500_pit_v1"})
    eval_spec_b = _eval_spec(factor_id, universe_identity={"universe_id": "russell1000_pit_v1"})

    _, eval_id_a, idx_a = begin_factor_evaluation(registry_db, eval_spec_a)
    _, eval_id_b, idx_b = begin_factor_evaluation(registry_db, eval_spec_b)

    assert eval_id_a != eval_id_b
    assert idx_a == 1 and idx_b == 2
    assert len(list_factors(registry_db)) == 1
    assert get_factor(registry_db, factor_id)["factor_id"] == factor_id
