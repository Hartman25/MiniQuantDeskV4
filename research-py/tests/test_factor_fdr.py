"""
RESEARCH-FACTOR-FDR-01 -- Benjamini-Hochberg + empirical p-value negative controls.

Proves: population is read from the durable registry (never a caller-
assembled winners list), retries/evaluation slices never inflate hypothesis
count, ordering never changes corrected results, a classic textbook BH
example reproduces the expected q-values/rejections, and result values
never alter factor identity.
"""
from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.diagnostics import evaluate_factor_ic_ir
from mqk_research.factors.fdr import (
    FDR_POPULATION_STATUS_COMPLETE,
    FDR_POPULATION_STATUS_INCOMPLETE,
    FDR_POPULATION_STATUS_NOT_EVALUABLE,
    FactorPValueEvidence,
    benjamini_hochberg,
    build_fdr_population_report,
    compute_empirical_pvalue,
)
from mqk_research.factors.registry import begin_factor_evaluation, finalize_factor_evaluation, register_factor
from mqk_research.factors.contracts import FactorEvaluationResult


_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"


def _periods(n):
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, n + 1)]


def _symbols(n):
    return [f"SYM{i}" for i in range(n)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _signal_dataset(n_symbols=6, n_periods=10, seed=1) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods(n_periods):
        perm = rng.permutation(n_symbols).astype(float)
        for sym, val in zip(_symbols(n_symbols), perm):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": val, "label_fwd_ret": val})
    return _with_causal_columns(pd.DataFrame(rows))


def _noise_dataset(n_symbols=6, n_periods=10, seed=99) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods(n_periods):
        fv = rng.permutation(n_symbols).astype(float)
        lv = rng.permutation(n_symbols).astype(float)
        for sym, f, l in zip(_symbols(n_symbols), fv, lv):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": f, "label_fwd_ret": l})
    return _with_causal_columns(pd.DataFrame(rows))


def _spec(name="f1", family="momentum") -> FactorSpec:
    return FactorSpec(
        family=family,
        name=name,
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


def _eval_spec(factor_id: str) -> FactorEvaluationSpec:
    return FactorEvaluationSpec(
        factor_id=factor_id,
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_quantile_v1",
    )


def _succeed_factor(registry_db, factor_id: str, *, mean_ic: float = 0.1) -> str:
    """Open and finalize a real succeeded evaluation attempt for `factor_id`,
    returning its evaluation_id -- the only evaluation_id FactorPValueEvidence
    for this factor may legitimately cite."""
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(
            eval_id=evaluation_id, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={"mean_ic": mean_ic}
        ),
    )
    return evaluation_id


def _eval_kwargs():
    return dict(n_quantiles=3, min_cross_section=3)


# -- known BH examples match expected q/reject decisions -------------------

def test_known_bh_textbook_example():
    p_values = {"f1": 0.01, "f2": 0.02, "f3": 0.03, "f4": 0.04, "f5": 0.20}
    result = benjamini_hochberg(p_values, alpha=0.05)
    assert result["rejected_factor_ids"] == ["f1", "f2", "f3", "f4"]
    for fid in ("f1", "f2", "f3", "f4"):
        assert result["q_values"][fid] == pytest.approx(0.05)
    assert result["q_values"]["f5"] == pytest.approx(0.20)


def test_ordering_does_not_change_corrected_results():
    p_values = {"f1": 0.01, "f2": 0.02, "f3": 0.03, "f4": 0.04, "f5": 0.20}
    reordered = {"f5": 0.20, "f3": 0.03, "f1": 0.01, "f4": 0.04, "f2": 0.02}
    a = benjamini_hochberg(p_values, alpha=0.05)
    b = benjamini_hochberg(reordered, alpha=0.05)
    assert a == b


def test_bh_rejects_invalid_alpha_and_pvalues():
    with pytest.raises(ValueError):
        benjamini_hochberg({"f1": 0.05}, alpha=0.0)
    with pytest.raises(ValueError):
        benjamini_hochberg({"f1": 1.5}, alpha=0.05)
    with pytest.raises(ValueError):
        benjamini_hochberg({}, alpha=0.05)


# -- empirical p-value: real signal scores low, pure noise scores high ----

def test_empirical_pvalue_low_for_real_signal():
    df = _signal_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    assert real.status == EVAL_STATUS_SUCCEEDED
    eval_spec = _eval_spec("f" * 32)
    result = compute_empirical_pvalue(df, eval_spec, real, n_permutations=50, base_seed=0, **_eval_kwargs())
    assert result["p_value"] < 0.10
    assert result["n_permutations_used"] > 0


def test_empirical_pvalue_deterministic_on_repeat():
    df = _signal_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    eval_spec = _eval_spec("f" * 32)
    a = compute_empirical_pvalue(df, eval_spec, real, n_permutations=30, base_seed=7, **_eval_kwargs())
    b = compute_empirical_pvalue(df, eval_spec, real, n_permutations=30, base_seed=7, **_eval_kwargs())
    assert a == b


def test_empirical_pvalue_requires_succeeded_report():
    df = _signal_dataset()
    not_evaluable = evaluate_factor_ic_ir(
        _with_causal_columns(
            pd.DataFrame({"symbol": ["A"], "period_ts_utc": ["2024-01-01T00:00:00+00:00"], "factor_value": [1.0], "label_fwd_ret": [1.0]})
        ),
        n_quantiles=2,
        min_cross_section=2,
    )
    eval_spec = _eval_spec("f" * 32)
    with pytest.raises(ValueError):
        compute_empirical_pvalue(df, eval_spec, not_evaluable, n_permutations=10, **_eval_kwargs())


# -- registry-backed population: winner-only is INCOMPLETE, never a decision
# RESEARCH-FACTOR-FDR-FULL-POPULATION-AUTHORITY-01

def test_winner_only_population_is_incomplete_not_authoritative(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_a = register_factor(registry_db, _spec("winner"))
    factor_b = register_factor(registry_db, _spec("loser"))  # never evaluated at all
    evaluation_id_a = _succeed_factor(registry_db, factor_a)

    # Only bind p-value evidence for the "winner" -- simulates a caller
    # trying to sneak a winner-only population past the correction.
    report = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=factor_a, evaluation_id=evaluation_id_a, p_value=0.01)],
        alpha=0.05,
    )
    assert report["declared_population_count"] == 2
    assert report["status"] == FDR_POPULATION_STATUS_INCOMPLETE
    assert report["excluded_factor_ids_with_reasons"][factor_b] == "no_evaluation_attempted"
    assert report["included_factor_ids"] == [factor_a]
    # An incomplete population must never carry an authoritative BH decision.
    assert report["rejected_factor_ids"] is None
    assert report["q_values"] is None
    assert report["hypothesis_count"] is None


def test_succeeded_factor_missing_pvalue_makes_population_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_a = register_factor(registry_db, _spec("has_pvalue"))
    factor_b = register_factor(registry_db, _spec("missing_pvalue"))
    evaluation_id_a = _succeed_factor(registry_db, factor_a)
    _succeed_factor(registry_db, factor_b)  # succeeded, but no evidence supplied below

    report = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=factor_a, evaluation_id=evaluation_id_a, p_value=0.01)],
        alpha=0.05,
    )
    assert report["status"] == FDR_POPULATION_STATUS_INCOMPLETE
    assert report["excluded_factor_ids_with_reasons"][factor_b] == "p_value_not_supplied"
    assert report["rejected_factor_ids"] is None
    assert report["q_values"] is None


def test_pvalue_for_never_evaluated_factor_is_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec("never_evaluated"))
    with pytest.raises(ValueError, match="not a succeeded evaluation attempt"):
        build_fdr_population_report(
            registry_db,
            family="momentum",
            p_value_evidence=[FactorPValueEvidence(factor_id=factor_id, evaluation_id="f" * 32, p_value=0.01)],
            alpha=0.05,
        )


def test_pvalue_for_failed_only_factor_is_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec("failed_only"))
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(eval_id=evaluation_id, factor_id=factor_id, status="failed", reason="synthetic"),
    )
    with pytest.raises(ValueError, match="not a succeeded evaluation attempt"):
        build_fdr_population_report(
            registry_db,
            family="momentum",
            p_value_evidence=[FactorPValueEvidence(factor_id=factor_id, evaluation_id=evaluation_id, p_value=0.01)],
            alpha=0.05,
        )


def test_pvalue_for_factor_outside_declared_family_is_rejected(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    register_factor(registry_db, _spec("in_momentum"))
    value_factor = register_factor(registry_db, _spec("in_value", family="value"))
    evaluation_id = _succeed_factor(registry_db, value_factor)
    with pytest.raises(ValueError, match="outside the declared family"):
        build_fdr_population_report(
            registry_db,
            family="momentum",
            p_value_evidence=[FactorPValueEvidence(factor_id=value_factor, evaluation_id=evaluation_id, p_value=0.01)],
            alpha=0.05,
        )


def test_registry_population_distinguishes_failed_from_unsupplied(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    failed_factor_id = register_factor(registry_db, _spec("evaluated_but_failed"))
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(failed_factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(eval_id=evaluation_id, factor_id=failed_factor_id, status="failed", reason="synthetic"),
    )
    evaluable_factor_id = register_factor(registry_db, _spec("has_a_pvalue"))
    evaluation_id_ok = _succeed_factor(registry_db, evaluable_factor_id)
    report = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=evaluable_factor_id, evaluation_id=evaluation_id_ok, p_value=0.02)],
        alpha=0.05,
    )
    assert report["excluded_factor_ids_with_reasons"][failed_factor_id] == "no_succeeded_evaluation"
    assert report["included_factor_ids"] == [evaluable_factor_id]
    # A failed (attempted, terminal) factor is a typed exclusion, not an
    # incompleteness -- the population IS fully accounted for.
    assert report["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report["hypothesis_count"] == 1


def test_population_all_failed_is_not_evaluable_not_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec("only_failed"))
    attempt_id, evaluation_id, _ = begin_factor_evaluation(registry_db, _eval_spec(factor_id))
    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(eval_id=evaluation_id, factor_id=factor_id, status="failed", reason="synthetic"),
    )
    report = build_fdr_population_report(registry_db, family="momentum", p_value_evidence=[], alpha=0.05)
    assert report["status"] == FDR_POPULATION_STATUS_NOT_EVALUABLE
    assert report["rejected_factor_ids"] is None
    assert report["q_values"] is None


def test_registry_population_requires_registered_factors(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    with pytest.raises(ValueError, match="no registered factors"):
        build_fdr_population_report(registry_db, family="nonexistent", p_value_evidence=[], alpha=0.05)


# -- retry / evaluation slices do not increase hypothesis count -----------

def test_retries_do_not_increase_hypothesis_count(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec("retried"))
    eval_spec = _eval_spec(factor_id)

    attempt_1, eval_id_1, _ = begin_factor_evaluation(registry_db, eval_spec)
    finalize_factor_evaluation(
        registry_db,
        attempt_1,
        FactorEvaluationResult(eval_id=eval_id_1, factor_id=factor_id, status="failed", reason="transient"),
    )
    attempt_2, eval_id_2, _ = begin_factor_evaluation(registry_db, eval_spec)
    finalize_factor_evaluation(
        registry_db,
        attempt_2,
        FactorEvaluationResult(eval_id=eval_id_2, factor_id=factor_id, status=EVAL_STATUS_SUCCEEDED, metrics={"mean_ic": 0.1}),
    )

    report = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=factor_id, evaluation_id=eval_id_2, p_value=0.02)],
        alpha=0.05,
    )
    # Two attempts (one failed, one succeeded retry) under ONE factor_id --
    # still exactly one hypothesis.
    assert report["declared_population_count"] == 1
    assert report["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report["hypothesis_count"] == 1


# -- adding another true factor candidate changes correction population ---

def test_adding_another_factor_changes_population(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_a = register_factor(registry_db, _spec("a"))
    eval_id_a = _succeed_factor(registry_db, factor_a)
    report_before = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=factor_a, evaluation_id=eval_id_a, p_value=0.01)],
        alpha=0.05,
    )
    factor_b = register_factor(registry_db, _spec("b"))
    eval_id_b = _succeed_factor(registry_db, factor_b, mean_ic=0.5)
    report_after = build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[
            FactorPValueEvidence(factor_id=factor_a, evaluation_id=eval_id_a, p_value=0.01),
            FactorPValueEvidence(factor_id=factor_b, evaluation_id=eval_id_b, p_value=0.5),
        ],
        alpha=0.05,
    )
    assert report_before["declared_population_count"] == 1
    assert report_before["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report_after["declared_population_count"] == 2
    assert report_after["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report_after["hypothesis_count"] == 2


# -- result values do not alter factor identity ----------------------------

def test_fdr_result_does_not_alter_factor_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec("stable_identity")
    factor_id_before = spec.compute_factor_id()
    register_factor(registry_db, spec)
    evaluation_id = _succeed_factor(registry_db, factor_id_before)
    build_fdr_population_report(
        registry_db,
        family="momentum",
        p_value_evidence=[FactorPValueEvidence(factor_id=factor_id_before, evaluation_id=evaluation_id, p_value=0.5)],
        alpha=0.05,
    )
    assert spec.compute_factor_id() == factor_id_before
