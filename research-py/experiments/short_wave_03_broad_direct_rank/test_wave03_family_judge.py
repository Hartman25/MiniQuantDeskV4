"""WAVE03-FAMILY-JUDGE-01 -- focused, network-free tests for
run_family_judge(). Uses the real ResearchResultStore/build_multiple_
testing_judge production seams against a local SQLite fixture registry
(same convention as tests/test_multiple_testing_judge.py's own direct-
fixture helpers, adapted to wave03's REAL_EXPERIMENT_ID/
PLACEBO_EXPERIMENT_ID/hypothesis IDs) -- no network, no ML training, no
research-py/src modification. Does not re-derive DSR/PBO numeric behavior
(already proven by tests/test_multiple_testing_judge.py); proves only
wave03's own registry/experiment-id WIRING is correct.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

import pandas as pd
import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402
from run_wave import DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS, REAL_CANDIDATE_HYPOTHESIS_IDS  # noqa: E402

sys.path.insert(0, str(EXPERIMENT_ROOT.parents[2] / "src"))
from mqk_research.exp_distributed.hashing import short_hash  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID  # noqa: E402
from mqk_research.ml.util_hash import file_record, sha256_json  # noqa: E402


# ---------------------------------------------------------------------------
# Fixture helpers -- adapted from tests/test_multiple_testing_judge.py's own
# direct-fixture convention, scoped to wave03's identity fields.
# ---------------------------------------------------------------------------


def _identity(*, experiment_id: str, hypothesis_id: str, strategy_id: str, data_salt: str = "") -> tuple[str, Dict[str, Any]]:
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "protocol_id": ECONOMIC_PROTOCOL_ID,
        "data_identity": {
            "features_csv": {"sha256": f"feat-{strategy_id}{data_salt}", "bytes": 1},
            "targets_csv": {"sha256": f"targ-{strategy_id}{data_salt}", "bytes": 1},
            "feature_schema": {"sha256": f"schema-{strategy_id}", "bytes": 1},
            "bars_provenance": {
                "schema_version": "bars_provenance_manifest_v1",
                "provider_ids_observed": ["alpaca"],
                "resolved_close_column": "close_micros",
                "price_adjustment_convention": "raw_unadjusted",
                "corporate_action_policy": "forbid_affected_periods",
                "corporate_action_evidence_id": "evidence-fixture",
                "forbidden_periods": [],
                "timeframe": "1D",
                "start_utc": "2016-01-01T00:00:00+00:00",
                "end_utc": "2024-01-01T00:00:00+00:00",
                "symbol_universe": ["AAA"],
                "universe_mode": "fixed_ex_ante",
                "canonical_semantic_bars_hash": "bars-fixture",
            },
        },
        "evaluation_spec": {
            "label_col": "target", "end_ts_col": "end_ts", "train_years": 3, "test_months": 3,
            "step_months": 3, "min_rows_per_fold": 300, "purge_enabled": True,
            "label_end_ts_col": "label_end_ts", "embargo_seconds": 0, "holdout_months": 6,
        },
        "model_spec": {"l2": 1e-3, "lr": 0.05, "steps": 300, "standardize": True, "clip_z": 8.0},
        "economic_protocol": {
            "protocol_id": ECONOMIC_PROTOCOL_ID,
            "signal_policy": {
                "direction_policy": "cross_sectional_rank_long_only_v1", "long_only": True,
                "rank_side_count": 5, "sizing": "equal_weight_active", "max_gross_exposure": 1.0,
                "fold_end_policy": "force_flat_last_bar", "capacity_policy": "reduce_first_defer_increase_batch_v1",
            },
            "cost_model": {"commission_bps_per_side": 10.0, "slippage_bps_per_side": 0.0, "diagnostic_zero_cost": False},
            "annualization": {"annualization_days": 252, "risk_free_rate_annual": 0.0},
        },
    }
    trial_id = short_hash(identity, length=32)
    return trial_id, identity


def _dates(n: int, start: str = "2021-01-01") -> List[str]:
    return [d.strftime("%Y-%m-%d") for d in pd.date_range(start, periods=n, freq="D")]


def _write_economic_artifact(eval_dir: Path, *, dates: Sequence[str], net_returns: Sequence[float], salt: str) -> Path:
    eval_dir.mkdir(parents=True, exist_ok=True)
    daily_df = pd.DataFrame({"date": list(dates), "net_daily_return": [float(x) for x in net_returns]})
    daily_path = eval_dir / "economic_daily_returns.csv"
    daily_df.to_csv(daily_path, index=False)
    out: Dict[str, Any] = {
        "schema_version": "economic_walk_forward_v1",
        "protocol": {"protocol_id": ECONOMIC_PROTOCOL_ID},
        "holdout": {"status": "reserved_not_evaluated"},
        "outputs": {"economic_daily_returns_csv": file_record(daily_path)},
    }
    out["ids"] = {"economic_eval_id": sha256_json({"dates": list(dates), "returns": list(net_returns), "salt": salt})}
    out_path = eval_dir / "economic_walk_forward.json"
    out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path


def _register_trial_with_result(
    store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str, strategy_id: str,
    run_dir: Path, dates: Sequence[str], net_returns: Sequence[float],
    data_salt: str = "", origin: str = "test",
) -> tuple[str, str]:
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    trial_id, identity = _identity(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, strategy_id=strategy_id, data_salt=data_salt,
    )
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id, origin=origin)
    econ_path = _write_economic_artifact(run_dir / "eval", dates=dates, net_returns=net_returns, salt=str(run_dir))
    econ_out = json.loads(econ_path.read_text(encoding="utf-8"))
    store.finalize_attempt(
        attempt_id, status="succeeded", result_id=econ_out["ids"]["economic_eval_id"],
        artifact_paths={"economic_walk_forward": str(econ_path)},
        result_summary={"net_total_return": float(sum(net_returns))},
    )
    return trial_id, attempt_id


def _register_failed_attempt(store: ResearchResultStore, trial_id: str, *, origin: str = "test-retry") -> str:
    attempt_id, _ = store.begin_attempt(trial_id=trial_id, origin=origin)
    store.finalize_attempt(attempt_id, status="failed", failure_reason="synthetic failure for test")
    return attempt_id


def _register_all_six_real_candidates(store: ResearchResultStore, tmp_path: Path, *, n_dates: int = 20) -> None:
    dates = _dates(n_dates)
    for i, hyp_id in enumerate(REAL_CANDIDATE_HYPOTHESIS_IDS):
        _register_trial_with_result(
            store, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=hyp_id,
            strategy_id=f"strategy-{i}", run_dir=tmp_path / f"real_{i}",
            dates=dates, net_returns=[0.001 * ((i % 3) + 1)] * n_dates,
        )


def _register_all_three_placebos(store: ResearchResultStore, tmp_path: Path, *, n_dates: int = 20) -> None:
    dates = _dates(n_dates)
    for i, hyp_id in enumerate(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS):
        _register_trial_with_result(
            store, experiment_id=run_wave.PLACEBO_EXPERIMENT_ID, hypothesis_id=hyp_id,
            strategy_id=f"placebo-strategy-{i}", run_dir=tmp_path / f"placebo_{i}",
            dates=dates, net_returns=[0.0001] * n_dates,
        )


# ---------------------------------------------------------------------------
# Structural routing: run_family_judge() calls build_multiple_testing_judge
# with experiment_id=REAL_EXPERIMENT_ID, hypothesis_id=None, registry_db=
# REGISTRY_DB, and persists judge_artifact.json under RUN_ROOT.
# ---------------------------------------------------------------------------


def test_run_family_judge_calls_build_multiple_testing_judge_with_real_experiment_id_only(monkeypatch, tmp_path: Path) -> None:
    captured = {}

    def fake_build_judge(*, experiment_id, hypothesis_id=None, registry_db, spec=None):
        captured["experiment_id"] = experiment_id
        captured["hypothesis_id"] = hypothesis_id
        captured["registry_db"] = registry_db
        return {"judge_status": "not_evaluable", "registry_population": {}, "included_trial_ids": [], "excluded_trial_ids": []}

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", fake_run_root / "registry" / "research.sqlite3")
    monkeypatch.setattr(run_wave, "build_multiple_testing_judge", fake_build_judge)

    judge = run_wave.run_family_judge()

    assert captured["experiment_id"] == run_wave.REAL_EXPERIMENT_ID
    assert captured["experiment_id"] != run_wave.PLACEBO_EXPERIMENT_ID
    assert captured["hypothesis_id"] is None
    assert captured["registry_db"] == fake_run_root / "registry" / "research.sqlite3"
    assert (fake_run_root / "judge_artifact.json").exists()
    assert judge["judge_status"] == "not_evaluable"


# ---------------------------------------------------------------------------
# "candidate omission from frozen population fails" / "exact judge population
# is derived from the frozen real experiment"
# ---------------------------------------------------------------------------


def test_full_six_candidate_population_matches_frozen_set_exactly(tmp_path: Path) -> None:
    db_path = tmp_path / "research.sqlite3"
    store = ResearchResultStore(db_path)
    _register_all_six_real_candidates(store, tmp_path)

    registered = {t["hypothesis_id"] for t in store.list_trials(experiment_id=run_wave.REAL_EXPERIMENT_ID)}
    assert registered == set(REAL_CANDIDATE_HYPOTHESIS_IDS)
    assert len(registered) == 6


def test_candidate_omission_from_frozen_population_is_detectable(tmp_path: Path) -> None:
    """Mutation/negative proof: omitting one of the six frozen real
    candidates from registration is exactly what the positive test above
    would catch -- proven here by registering only 5 of 6 and showing the
    equality check the positive test relies on now fails."""
    db_path = tmp_path / "research.sqlite3"
    store = ResearchResultStore(db_path)
    dates = _dates(20)
    for i, hyp_id in enumerate(REAL_CANDIDATE_HYPOTHESIS_IDS[:5]):  # omit the 6th
        _register_trial_with_result(
            store, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=hyp_id,
            strategy_id=f"strategy-{i}", run_dir=tmp_path / f"real_{i}",
            dates=dates, net_returns=[0.001] * 20,
        )
    registered = {t["hypothesis_id"] for t in store.list_trials(experiment_id=run_wave.REAL_EXPERIMENT_ID)}
    assert registered != set(REAL_CANDIDATE_HYPOTHESIS_IDS)
    assert len(registered) == 5
    with pytest.raises(AssertionError):
        assert registered == set(REAL_CANDIDATE_HYPOTHESIS_IDS)


# ---------------------------------------------------------------------------
# "placebos MUST remain under the diagnostic-placebo experiment and cannot
# enter real PBO/DSR" -- proven through the REAL run_family_judge() wiring.
# ---------------------------------------------------------------------------


def test_run_family_judge_never_includes_placebo_population(monkeypatch, tmp_path: Path) -> None:
    db_path = tmp_path / "research.sqlite3"
    store = ResearchResultStore(db_path)
    _register_all_six_real_candidates(store, tmp_path)
    _register_all_three_placebos(store, tmp_path)

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", db_path)

    judge = run_wave.run_family_judge()
    assert judge["registry_population"]["registered_unique_trials"] == 6  # not 9

    # Wrong-experiment-id mutation proof: pointing the judge at
    # PLACEBO_EXPERIMENT_ID instead surfaces the 3 placebos, never the 6
    # real candidates -- experiment_id selection is load-bearing.
    from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge

    placebo_judge = build_multiple_testing_judge(experiment_id=run_wave.PLACEBO_EXPERIMENT_ID, registry_db=db_path)
    assert placebo_judge["registry_population"]["registered_unique_trials"] == 3


# ---------------------------------------------------------------------------
# "retries do not create new independent trials"
# ---------------------------------------------------------------------------


def test_retry_does_not_inflate_registered_trial_count(monkeypatch, tmp_path: Path) -> None:
    db_path = tmp_path / "research.sqlite3"
    store = ResearchResultStore(db_path)
    trial_id, _ = _register_trial_with_result(
        store, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=REAL_CANDIDATE_HYPOTHESIS_IDS[0],
        strategy_id="strategy-0", run_dir=tmp_path / "real_0", dates=_dates(20), net_returns=[0.001] * 20,
    )
    _register_failed_attempt(store, trial_id)  # a retry -- same trial_id, new attempt

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", db_path)

    judge = run_wave.run_family_judge()
    assert judge["registry_population"]["registered_unique_trials"] == 1
    assert judge["registry_population"]["attempt_count"] == 2


# ---------------------------------------------------------------------------
# "return-series date mismatch is handled truthfully, never silently
# aligned/interpolated" -- excluded with an honest reason.
# ---------------------------------------------------------------------------


def test_return_series_date_misalignment_is_surfaced_not_silently_aligned(monkeypatch, tmp_path: Path) -> None:
    db_path = tmp_path / "research.sqlite3"
    store = ResearchResultStore(db_path)
    dates_a = _dates(20, start="2021-01-01")
    dates_b = _dates(20, start="2021-06-01")  # disjoint calendar range
    _register_trial_with_result(
        store, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=REAL_CANDIDATE_HYPOTHESIS_IDS[0],
        strategy_id="strategy-0", run_dir=tmp_path / "real_0", dates=dates_a, net_returns=[0.001] * 20,
    )
    _register_trial_with_result(
        store, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=REAL_CANDIDATE_HYPOTHESIS_IDS[1],
        strategy_id="strategy-1", run_dir=tmp_path / "real_1", dates=dates_b, net_returns=[0.001] * 20,
    )

    fake_run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", db_path)

    judge = run_wave.run_family_judge()
    reasons = {e["reason"] for e in judge["excluded_trial_ids"]}
    assert "return_series_date_misalignment" in reasons
    assert judge["registry_population"]["registered_unique_trials"] == 2


# ---------------------------------------------------------------------------
# "result-order changes do not alter candidate identity"
# ---------------------------------------------------------------------------


def test_registration_order_does_not_change_judge_identity(monkeypatch, tmp_path: Path) -> None:
    dates = _dates(20)
    ids_forward = REAL_CANDIDATE_HYPOTHESIS_IDS
    ids_reversed = list(reversed(REAL_CANDIDATE_HYPOTHESIS_IDS))

    db_forward = tmp_path / "forward.sqlite3"
    store_forward = ResearchResultStore(db_forward)
    for i, hyp_id in enumerate(ids_forward):
        _register_trial_with_result(
            store_forward, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=hyp_id,
            strategy_id=f"strategy-{i}", run_dir=tmp_path / "fwd" / f"real_{i}", dates=dates, net_returns=[0.001] * 20,
        )

    db_reversed = tmp_path / "reversed.sqlite3"
    store_reversed = ResearchResultStore(db_reversed)
    for i, hyp_id in enumerate(ids_reversed):
        strategy_id = f"strategy-{ids_forward.index(hyp_id)}"
        _register_trial_with_result(
            store_reversed, experiment_id=run_wave.REAL_EXPERIMENT_ID, hypothesis_id=hyp_id,
            strategy_id=strategy_id, run_dir=tmp_path / "rev" / f"real_{i}", dates=dates, net_returns=[0.001] * 20,
        )

    fake_run_root_a = tmp_path / "runs_a"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root_a)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", db_forward)
    judge_forward = run_wave.run_family_judge()

    fake_run_root_b = tmp_path / "runs_b"
    monkeypatch.setattr(run_wave, "RUN_ROOT", fake_run_root_b)
    monkeypatch.setattr(run_wave, "REGISTRY_DB", db_reversed)
    judge_reversed = run_wave.run_family_judge()

    assert judge_forward["included_trial_ids"] == judge_reversed["included_trial_ids"]
    assert judge_forward["registry_population"] == judge_reversed["registry_population"]
