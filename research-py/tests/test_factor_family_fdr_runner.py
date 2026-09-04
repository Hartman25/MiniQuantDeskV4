"""
RESEARCH-FACTOR-FDR-01 -- production FDR driver negative controls.

Registers a real family of factors via
`run_registered_factor_diagnostics` (RESEARCH-FACTOR-IC-IR-QUANTILE-
BENCH-01's runner), then proves `run_registered_factor_family_fdr` --
the first real, non-test caller connecting `compute_empirical_pvalue` +
`FactorPValueEvidence` + `build_fdr_population_report` -- acts on the
FULL durable registered population, never a caller-assembled winners
subset, never inflates hypothesis count from retries, and never accepts
p-value evidence that isn't provably bound to the EXACT registered
attempt it claims.

RESEARCH-FACTOR-FDR-RESULT-INDEPENDENT-ATTEMPT-AUTHORITY-01: a caller
declares only `factor_id`/`evaluation_id` (pure identity, fixed before
any result is known) -- never an `attempt_id`, which would let outcome
knowledge become authority by naming a favorable succeeded attempt. The
driver itself picks the authoritative attempt for that slice
deterministically (latest attempt_index), so a succeeded attempt
followed by a later failed/not_evaluable retry of the SAME evaluation_id
can never be cited -- the factor simply receives no p-value and
`build_fdr_population_report`'s own typed exclusion accounting reports
the truthful reason.
"""
from __future__ import annotations

import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_SUCCEEDED,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.fdr import (
    EMPIRICAL_PVALUE_PROTOCOL_VERSION,
    FDR_POPULATION_STATUS_COMPLETE,
    FDR_POPULATION_STATUS_INCOMPLETE,
    FDR_REASON_NOT_EVALUABLE,
    FDR_REASON_PVALUE_NOT_SUPPLIED,
    FactorFDREvidenceInput,
    run_registered_factor_family_fdr,
)
from mqk_research.factors.registry import list_factor_evaluation_attempts
from mqk_research.factors.runner import UNIVERSE_MODE_FIXED_EX_ANTE, run_registered_factor_diagnostics

N_SYMBOLS = 6
N_PERIODS = 8
_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"
_N_PERMUTATIONS = 20  # small but >=1; test speed only, never asserted as accepted policy
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


def _monotonic_dataset(*, scale: float = 1.0) -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            rows.append(
                {
                    "symbol": sym,
                    "period_ts_utc": period,
                    "factor_value": float(i) * scale,
                    "label_fwd_ret": float(i),
                }
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


def _broken_dataset() -> pd.DataFrame:
    """Missing a required column -- run_registered_factor_diagnostics
    raises and finalizes the attempt as EVAL_STATUS_FAILED."""
    return _monotonic_dataset().drop(columns=["label_fwd_ret"])


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


def _run(registry_db, out_dir, *, observations, spec):
    return run_registered_factor_diagnostics(
        registry_db,
        out_dir,
        factor_spec=spec,
        observations=observations,
        universe_identity=_UNIVERSE_IDENTITY,
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        origin="test_family_fdr_runner",
    )


def test_full_population_with_evidence_for_everyone_reaches_complete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec_a = _spec(name="12m_1m_lag")
    spec_b = _spec(name="6m_1m_lag")
    obs_a = _monotonic_dataset()
    obs_b = _monotonic_dataset(scale=0.37)

    result_a = _run(registry_db, tmp_path / "a", observations=obs_a, spec=spec_a)
    result_b = _run(registry_db, tmp_path / "b", observations=obs_b, spec=spec_b)
    assert result_a.status == EVAL_STATUS_SUCCEEDED
    assert result_b.status == EVAL_STATUS_SUCCEEDED

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(
                factor_id=result_a.factor_id, evaluation_id=result_a.evaluation_id, observations=obs_a
            ),
            FactorFDREvidenceInput(
                factor_id=result_b.factor_id, evaluation_id=result_b.evaluation_id, observations=obs_b
            ),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    assert report["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report["declared_population_count"] == 2
    assert report["hypothesis_count"] == 2
    assert set(report["q_values"].keys()) == {result_a.factor_id, result_b.factor_id}
    assert isinstance(report["rejected_factor_ids"], list)
    assert report["pvalue_method"] == {
        "protocol_version": EMPIRICAL_PVALUE_PROTOCOL_VERSION,
        "n_permutations": _N_PERMUTATIONS,
        "base_seed": 0,
    }


def test_missing_observations_for_one_factor_keeps_report_incomplete(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec_a = _spec(name="12m_1m_lag")
    spec_b = _spec(name="6m_1m_lag")
    obs_a = _monotonic_dataset()
    obs_b = _monotonic_dataset(scale=0.37)

    result_a = _run(registry_db, tmp_path / "a", observations=obs_a, spec=spec_a)
    result_b = _run(registry_db, tmp_path / "b", observations=obs_b, spec=spec_b)

    # Only "winning" candidate A's evidence supplied -- a caller cannot
    # manufacture a complete/authoritative report by only feeding evidence
    # for a chosen subset.
    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(
                factor_id=result_a.factor_id, evaluation_id=result_a.evaluation_id, observations=obs_a
            ),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    assert report["status"] == FDR_POPULATION_STATUS_INCOMPLETE
    assert report["excluded_factor_ids_with_reasons"][result_b.factor_id] == FDR_REASON_PVALUE_NOT_SUPPLIED
    assert report["rejected_factor_ids"] is None
    assert report["q_values"] is None


def test_not_evaluable_factor_is_excluded_without_needing_observations(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec_ok = _spec(name="12m_1m_lag")
    spec_degenerate = _spec(name="degenerate_variant")
    obs_ok = _monotonic_dataset()

    result_ok = _run(registry_db, tmp_path / "ok", observations=obs_ok, spec=spec_ok)
    result_degenerate = _run(
        registry_db, tmp_path / "degenerate", observations=_constant_factor_dataset(), spec=spec_degenerate
    )
    assert result_degenerate.status != EVAL_STATUS_SUCCEEDED

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(
                factor_id=result_ok.factor_id, evaluation_id=result_ok.evaluation_id, observations=obs_ok
            ),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    assert report["status"] == FDR_POPULATION_STATUS_COMPLETE
    assert report["excluded_factor_ids_with_reasons"][result_degenerate.factor_id] == FDR_REASON_NOT_EVALUABLE
    assert result_ok.factor_id in report["q_values"]


def test_evidence_citing_a_not_succeeded_authoritative_attempt_is_silently_withheld(tmp_path):
    """Citing evidence for an evaluation_id whose authoritative attempt did
    not succeed must never raise -- it truthfully withholds a p-value for
    that factor rather than treating a not_evaluable/failed outcome as an
    error. build_fdr_population_report's own accounting then reports the
    correct typed exclusion reason from the factor's real attempt
    history."""
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="degenerate_variant")
    degenerate_obs = _constant_factor_dataset()
    result = _run(registry_db, tmp_path / "a", observations=degenerate_obs, spec=spec)
    assert result.status != EVAL_STATUS_SUCCEEDED

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(
                factor_id=result.factor_id, evaluation_id=result.evaluation_id, observations=degenerate_obs
            ),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    assert result.factor_id not in (report["q_values"] or {})
    assert report["excluded_factor_ids_with_reasons"][result.factor_id] == FDR_REASON_NOT_EVALUABLE


def test_success_then_later_failed_retry_of_same_evaluation_cannot_cite_the_older_success(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()

    first = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)
    assert first.status == EVAL_STATUS_SUCCEEDED
    with pytest.raises(ValueError):
        _run(registry_db, tmp_path / "b", observations=_broken_dataset(), spec=spec)

    attempts = list_factor_evaluation_attempts(registry_db, first.factor_id)
    assert len(attempts) == 2
    assert attempts[-1]["status"] == "failed"

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(factor_id=first.factor_id, evaluation_id=first.evaluation_id, observations=obs),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    # build_fdr_population_report's own (frozen, pre-existing) population
    # accounting sees a real succeeded attempt somewhere in this factor's
    # history and no bound p-value evidence -- PVALUE_NOT_SUPPLIED, not a
    # blanket FAILED/NOT_EVALUABLE label. The load-bearing proof is that no
    # p-value was ever computed from the stale success: this driver refused
    # to bind evidence to it once a later attempt of the SAME evaluation_id
    # became authoritative and failed.
    assert first.factor_id not in (report["q_values"] or {})
    assert report["excluded_factor_ids_with_reasons"][first.factor_id] == FDR_REASON_PVALUE_NOT_SUPPLIED


def test_success_then_later_not_evaluable_retry_of_same_evaluation_cannot_cite_the_older_success(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()

    first = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)
    assert first.status == EVAL_STATUS_SUCCEEDED
    second = _run(registry_db, tmp_path / "b", observations=_constant_factor_dataset(), spec=spec)
    assert second.status != EVAL_STATUS_SUCCEEDED

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(factor_id=first.factor_id, evaluation_id=first.evaluation_id, observations=obs),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    # Same reasoning as the failed-retry case above: a real succeeded
    # attempt exists in this factor's history, so build_fdr_population_
    # report's own accounting reports PVALUE_NOT_SUPPLIED rather than a
    # blanket terminal label. What matters is that no p-value was ever
    # produced from the stale success once the later same-evaluation_id
    # retry became authoritative and came back not_evaluable.
    assert first.factor_id not in (report["q_values"] or {})
    assert report["excluded_factor_ids_with_reasons"][first.factor_id] == FDR_REASON_PVALUE_NOT_SUPPLIED


def test_retry_does_not_inflate_hypothesis_count(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()

    first = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)
    second = _run(registry_db, tmp_path / "b", observations=obs, spec=spec)
    assert first.factor_id == second.factor_id  # same semantic factor, two attempts
    assert first.evaluation_id == second.evaluation_id

    report = run_registered_factor_family_fdr(
        registry_db,
        family="momentum",
        evidence_inputs=[
            FactorFDREvidenceInput(factor_id=first.factor_id, evaluation_id=first.evaluation_id, observations=obs),
        ],
        alpha=0.10,
        n_permutations=_N_PERMUTATIONS,
    )

    assert report["declared_population_count"] == 1
    assert report["hypothesis_count"] == 1


def test_mutated_observations_fail_content_binding(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()
    result = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)

    mutated_obs = obs.copy()
    mutated_obs.loc[mutated_obs.index[0], "factor_value"] += 100.0

    with pytest.raises(ValueError, match="content hash"):
        run_registered_factor_family_fdr(
            registry_db,
            family="momentum",
            evidence_inputs=[
                FactorFDREvidenceInput(
                    factor_id=result.factor_id, evaluation_id=result.evaluation_id, observations=mutated_obs
                ),
            ],
            alpha=0.10,
            n_permutations=_N_PERMUTATIONS,
        )


def test_wrong_factor_evaluation_id_for_a_real_evaluation_fails_closed(tmp_path):
    """Supplying factor B's real evaluation_id under factor A's label must
    never be silently accepted -- evaluation_id is looked up scoped to the
    caller-declared factor_id's own attempts, so a foreign evaluation_id is
    simply unmatched there (evaluation_id already binds factor_id into its
    own identity, so this can never coincidentally collide)."""
    registry_db = tmp_path / "registry.sqlite3"
    spec_a = _spec(name="12m_1m_lag")
    spec_b = _spec(name="6m_1m_lag")
    obs_a = _monotonic_dataset()
    obs_b = _monotonic_dataset(scale=0.37)

    result_a = _run(registry_db, tmp_path / "a", observations=obs_a, spec=spec_a)
    result_b = _run(registry_db, tmp_path / "b", observations=obs_b, spec=spec_b)

    with pytest.raises(ValueError, match="does not match any durable evaluation attempt"):
        run_registered_factor_family_fdr(
            registry_db,
            family="momentum",
            evidence_inputs=[
                FactorFDREvidenceInput(
                    factor_id=result_a.factor_id, evaluation_id=result_b.evaluation_id, observations=obs_a
                ),
            ],
            alpha=0.10,
            n_permutations=_N_PERMUTATIONS,
        )


def test_unknown_evaluation_id_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()
    result = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)

    with pytest.raises(ValueError, match="does not match any durable evaluation attempt"):
        run_registered_factor_family_fdr(
            registry_db,
            family="momentum",
            evidence_inputs=[
                FactorFDREvidenceInput(
                    factor_id=result.factor_id, evaluation_id="never-attempted-evaluation-id", observations=obs
                ),
            ],
            alpha=0.10,
            n_permutations=_N_PERMUTATIONS,
        )


def test_evidence_for_a_factor_outside_the_family_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(family="volatility", name="realized_vol_20d")
    obs = _monotonic_dataset()
    result = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)

    with pytest.raises(ValueError, match="outside the declared family"):
        run_registered_factor_family_fdr(
            registry_db,
            family="momentum",
            evidence_inputs=[
                FactorFDREvidenceInput(
                    factor_id=result.factor_id, evaluation_id=result.evaluation_id, observations=obs
                ),
            ],
            alpha=0.10,
            n_permutations=_N_PERMUTATIONS,
        )


def test_pvalue_method_evidence_is_deterministic_and_never_folded_into_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    spec = _spec(name="12m_1m_lag")
    obs = _monotonic_dataset()
    result = _run(registry_db, tmp_path / "a", observations=obs, spec=spec)

    def _report():
        return run_registered_factor_family_fdr(
            registry_db,
            family="momentum",
            evidence_inputs=[
                FactorFDREvidenceInput(
                    factor_id=result.factor_id, evaluation_id=result.evaluation_id, observations=obs
                ),
            ],
            alpha=0.10,
            n_permutations=_N_PERMUTATIONS,
            base_seed=7,
        )

    first = _report()
    second = _report()
    assert first["pvalue_method"] == {
        "protocol_version": EMPIRICAL_PVALUE_PROTOCOL_VERSION,
        "n_permutations": _N_PERMUTATIONS,
        "base_seed": 7,
    }
    assert first["q_values"] == second["q_values"]  # deterministic given the same seed/protocol
    assert result.factor_id == list(first["q_values"].keys())[0]  # pvalue_method never touches identity
