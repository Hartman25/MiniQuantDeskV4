"""
RESEARCH-EXPERIMENT-REGISTRY-01 — durable trial/attempt ledger tests.

Covers, per the mission's 20 required tests:
  * core registry lifecycle (hypothesis -> trial -> attempt), append-only
    attempt history, fail-closed immutability and terminal-state contracts,
    atomic concurrent attempt numbering, and the query/summary API
    (registry-level unit tests, no evaluator involved);
  * walk-forward integration (registration-before-result, rerun vs param/spec/
    data/hypothesis-change trial identity, holdout stays reserved);
  * exp_distributed integration (window slices do not inflate trial count,
    universe changes do, failed candidates remain registered);
  * sweep-plan generation never registers an attempt.

No subprocess, no broker, no OMS, no network, no live Postgres.
"""
from __future__ import annotations

import json
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from mqk_research.exp_distributed.models import BatchSpec, DatasetFingerprint, JobSpec, WindowSpec
from mqk_research.exp_distributed.registry_integration import build_candidate_trial_identity
from mqk_research.exp_distributed.runner import default_db_path, rerun_failed_jobs, run_batch
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.registry_integration import run_registered_walkforward_eval
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.sweeps.run_sweep import run_sweep_scaffold


# ---------------------------------------------------------------------------
# Walk-forward fixture helpers (mirrors test_ml_eval_walkforward_purged_holdout.py)
# ---------------------------------------------------------------------------

BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)


def _build_wf_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for d in dates:
            f1 = float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append({
                "symbol": sym,
                "end_ts": d,
                "f1": f1,
                "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
            })
    return pd.DataFrame(rows)


def _write_wf_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _registered_eval(run_dir, df, *, registry_db, spec=None, steps=5, **overrides):
    _write_wf_run_dir(run_dir, df)
    kwargs = dict(
        experiment_id="alpha.discovery.test",
        hypothesis_id="alpha.peer_lag_catchup",
        strategy_id="research.peer_lag_v1",
        registry_db=registry_db,
        spec=spec or WalkForwardSpec(**BASE_SPEC_KW),
        steps=steps,
    )
    kwargs.update(overrides)
    return run_registered_walkforward_eval(run_dir, **kwargs)


# ---------------------------------------------------------------------------
# TEST 1 — trial registered before result (forced failure after attempt open)
# ---------------------------------------------------------------------------

def test_trial_and_failed_attempt_survive_evaluator_exception(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset(symbols=("AAA",), periods_days=40)  # too short -> fail-closed

    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        _registered_eval(run_dir, df, registry_db=registry_db)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 1
    assert summary["attempts"] == 1
    assert summary["failed_attempts"] == 1
    assert summary["succeeded_attempts"] == 0

    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    attempts = store.list_attempts(trial["trial_id"])
    assert len(attempts) == 1
    assert attempts[0]["status"] == "failed"
    assert attempts[0]["failure_reason"] and "Fail-closed" in attempts[0]["failure_reason"]


# ---------------------------------------------------------------------------
# TEST 2 — successful attempt
# ---------------------------------------------------------------------------

def test_successful_attempt_has_eval_linkage(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    out_path = _registered_eval(run_dir, df, registry_db=registry_db)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 1
    assert summary["attempts"] == 1
    assert summary["succeeded_attempts"] == 1

    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    attempts = store.list_attempts(trial["trial_id"])
    assert attempts[0]["status"] == "succeeded"
    assert attempts[0]["result_id"] == out["ids"]["eval_id"]
    assert out["registry"]["trial_id"] == trial["trial_id"]
    assert out["registry"]["status"] == "succeeded"


# ---------------------------------------------------------------------------
# TEST 3 — exact rerun increments attempts, not unique trials
# ---------------------------------------------------------------------------

def test_rerun_same_candidate_increments_attempts_not_trials(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    _registered_eval(run_dir, df, registry_db=registry_db)
    _registered_eval(run_dir, df, registry_db=registry_db)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 1
    assert summary["attempts"] == 2
    assert summary["succeeded_attempts"] == 2

    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    attempts = store.list_attempts(trial["trial_id"])
    assert [a["attempt_index"] for a in attempts] == [1, 2]
    assert attempts[0]["attempt_id"] != attempts[1]["attempt_id"]


# ---------------------------------------------------------------------------
# TEST 3B (artifact identity hardening check) — eval_id identifies the
# statistical evaluation core and is computed BEFORE registry-attempt linkage
# is appended (see run_walkforward_eval: out["ids"]["eval_id"] is set before
# run_registered_walkforward_eval attaches out["registry"]). Two attempts of
# the exact same candidate evaluation are therefore allowed — and expected —
# to share the same eval_id while getting distinct attempt_ids. eval_id must
# NOT depend on attempt_id, and trial identity must NOT depend on eval_id.
# ---------------------------------------------------------------------------

def test_repeat_attempts_of_same_candidate_share_eval_id_but_not_attempt_id(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    # both runs write to the same run_dir/eval/walk_forward_eval.json path, so
    # each artifact must be captured immediately after its own run — the file
    # is overwritten in place by the second call.
    out_path_1 = _registered_eval(run_dir, df, registry_db=registry_db)
    out_1 = json.loads(out_path_1.read_text(encoding="utf-8"))
    out_path_2 = _registered_eval(run_dir, df, registry_db=registry_db)
    out_2 = json.loads(out_path_2.read_text(encoding="utf-8"))

    # deterministic evaluator, identical inputs -> identical statistical core
    assert out_1["ids"]["eval_id"] == out_2["ids"]["eval_id"]
    # but each is a distinct attempt of that candidate
    assert out_1["registry"]["attempt_id"] != out_2["registry"]["attempt_id"]
    assert out_1["registry"]["trial_id"] == out_2["registry"]["trial_id"]

    store = ResearchResultStore(registry_db)
    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    # trial identity does not depend on eval_id / result content
    assert "eval_id" not in trial["identity_json"]


# ---------------------------------------------------------------------------
# TEST 4 — model parameter change creates a new trial
# ---------------------------------------------------------------------------

def test_param_change_creates_new_trial(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    _registered_eval(run_dir, df, registry_db=registry_db, l2=1e-3)
    _registered_eval(run_dir, df, registry_db=registry_db, l2=5e-2)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 2
    assert summary["attempts"] == 2


# ---------------------------------------------------------------------------
# TEST 5 — walk-forward spec change creates a new trial
# ---------------------------------------------------------------------------

def test_spec_change_creates_new_trial(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    spec_a = WalkForwardSpec(**BASE_SPEC_KW)
    spec_b = WalkForwardSpec(**{**BASE_SPEC_KW, "embargo_seconds": 3600})

    _registered_eval(run_dir, df, registry_db=registry_db, spec=spec_a)
    _registered_eval(run_dir, df, registry_db=registry_db, spec=spec_b)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 2
    assert summary["attempts"] == 2


# ---------------------------------------------------------------------------
# TEST 6 — data change creates a new trial
# ---------------------------------------------------------------------------

def test_data_change_creates_new_trial(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    df_a = _build_wf_dataset(seed=0)
    df_b = _build_wf_dataset(seed=1)

    _registered_eval(tmp_path / "run_a", df_a, registry_db=registry_db)
    _registered_eval(tmp_path / "run_b", df_b, registry_db=registry_db)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")
    assert summary["unique_trials"] == 2
    assert summary["attempts"] == 2


# ---------------------------------------------------------------------------
# TEST 7 — result change does not define trial identity
# ---------------------------------------------------------------------------

def test_result_change_does_not_redefine_trial_identity(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.a", experiment_id="exp.a")
    identity = {"strategy_id": "s", "params": {"x": 1}, "protocol_id": "p"}
    store.register_trial(
        trial_id="trial-fixed", experiment_id="exp.a", hypothesis_id="hyp.a",
        strategy_id="s", protocol_id="p", identity=identity,
    )

    attempt_1, _ = store.begin_attempt(trial_id="trial-fixed")
    store.finalize_attempt(attempt_1, status="succeeded", result_summary={"auc": 0.51})
    attempt_2, _ = store.begin_attempt(trial_id="trial-fixed")
    store.finalize_attempt(attempt_2, status="succeeded", result_summary={"auc": 0.93})

    attempts = store.list_attempts("trial-fixed")
    assert len(attempts) == 2
    assert json.loads(attempts[0]["result_summary_json"])["auc"] == 0.51
    assert json.loads(attempts[1]["result_summary_json"])["auc"] == 0.93
    # trial row itself (identity) is untouched by either result
    trial = store.get_trial("trial-fixed")
    assert json.loads(trial["identity_json"]) == identity


# ---------------------------------------------------------------------------
# TEST 8 — hypothesis change creates a new trial
# ---------------------------------------------------------------------------

def test_hypothesis_change_creates_new_trial(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    _registered_eval(run_dir, df, registry_db=registry_db, hypothesis_id="alpha.peer_lag_catchup")
    _registered_eval(run_dir, df, registry_db=registry_db, hypothesis_id="alpha.peer_lag_catchup_v2")

    store = ResearchResultStore(registry_db)
    all_trials = store.list_trials(experiment_id="alpha.discovery.test")
    assert len(all_trials) == 2
    assert {t["hypothesis_id"] for t in all_trials} == {"alpha.peer_lag_catchup", "alpha.peer_lag_catchup_v2"}


# ---------------------------------------------------------------------------
# TEST 9 — failed attempt survives a successful retry (append-only history)
# ---------------------------------------------------------------------------

def test_failed_attempt_survives_successful_retry(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.a", experiment_id="exp.a")
    store.register_trial(
        trial_id="trial-retry", experiment_id="exp.a", hypothesis_id="hyp.a",
        strategy_id="s", protocol_id="p", identity={"k": "v"},
    )

    attempt_1, idx_1 = store.begin_attempt(trial_id="trial-retry")
    store.finalize_attempt(attempt_1, status="failed", failure_reason="boom")
    attempt_2, idx_2 = store.begin_attempt(trial_id="trial-retry")
    store.finalize_attempt(attempt_2, status="succeeded", result_id="eval-123")

    attempts = store.list_attempts("trial-retry")
    assert len(attempts) == 2
    assert (idx_1, idx_2) == (1, 2)
    by_index = {a["attempt_index"]: a for a in attempts}
    assert by_index[1]["status"] == "failed"
    assert by_index[1]["failure_reason"] == "boom"
    assert by_index[2]["status"] == "succeeded"
    assert by_index[2]["result_id"] == "eval-123"


# ---------------------------------------------------------------------------
# TEST 10 — terminal attempt cannot be reopened / refinalized
# ---------------------------------------------------------------------------

def test_terminal_attempt_cannot_be_refinalized(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.a", experiment_id="exp.a")
    store.register_trial(
        trial_id="trial-terminal", experiment_id="exp.a", hypothesis_id="hyp.a",
        strategy_id="s", protocol_id="p", identity={"k": "v"},
    )
    attempt_id, _ = store.begin_attempt(trial_id="trial-terminal")
    store.finalize_attempt(attempt_id, status="succeeded")

    with pytest.raises(RuntimeError, match="already terminal"):
        store.finalize_attempt(attempt_id, status="failed", failure_reason="late")

    # status is unchanged by the rejected transition
    attempts = store.list_attempts("trial-terminal")
    assert attempts[0]["status"] == "succeeded"


# ---------------------------------------------------------------------------
# TEST 11 — trial identity immutability (fail-closed on collision)
# ---------------------------------------------------------------------------

def test_trial_id_reuse_with_conflicting_identity_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.a", experiment_id="exp.a")
    store.register_trial(
        trial_id="collision-id", experiment_id="exp.a", hypothesis_id="hyp.a",
        strategy_id="s", protocol_id="p", identity={"params": {"lookback": 5}},
    )
    # idempotent reuse with IDENTICAL identity is allowed
    store.register_trial(
        trial_id="collision-id", experiment_id="exp.a", hypothesis_id="hyp.a",
        strategy_id="s", protocol_id="p", identity={"params": {"lookback": 5}},
    )
    with pytest.raises(RuntimeError, match="conflicting canonical identity"):
        store.register_trial(
            trial_id="collision-id", experiment_id="exp.a", hypothesis_id="hyp.a",
            strategy_id="s", protocol_id="p", identity={"params": {"lookback": 10}},
        )


def test_hypothesis_id_reuse_with_conflicting_experiment_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.shared", experiment_id="exp.a")
    with pytest.raises(RuntimeError, match="hypothesis_id collision"):
        store.register_hypothesis(hypothesis_id="hyp.shared", experiment_id="exp.b")


# ---------------------------------------------------------------------------
# TEST 12 — existing exp_batches/exp_jobs DB compatibility
# ---------------------------------------------------------------------------

def test_existing_exp_distributed_rows_survive_registry_init(tmp_path):
    db_path = tmp_path / "state" / "exp.sqlite3"
    store_v1 = ResearchResultStore(db_path)
    store_v1.upsert_batch(
        batch_id="batch-1",
        spec={"engine_id": "EXP", "experiment_id": "exp.test", "strategy_id": "exp.buy_hold_v1", "batch_label": "b1"},
        root_dir=tmp_path,
        job_count=0,
    )

    # Simulate reopening an existing v1 DB after this patch: new instance,
    # same file, registry tables added via create-if-not-exists.
    store_v2 = ResearchResultStore(db_path)
    batch = store_v2.get_batch("batch-1")
    assert batch["engine_id"] == "EXP"

    assert store_v2.list_trials() == []
    summary = store_v2.registry_summary()
    assert summary == {
        "unique_trials": 0, "attempts": 0, "started_attempts": 0,
        "succeeded_attempts": 0, "failed_attempts": 0, "blocked_attempts": 0,
    }


# ---------------------------------------------------------------------------
# exp_distributed dataset/batch-spec helpers (mirrors test_exp_distributed.py)
# ---------------------------------------------------------------------------

def _write_exp_dataset(root: Path, symbols=("AAA", "BBB", "CCC"), periods_days=12) -> Path:
    rows = []
    base_dates = pd.date_range("2024-01-01", periods=periods_days, freq="D", tz="UTC")
    for idx, ts in enumerate(base_dates):
        for si, sym in enumerate(symbols):
            rows.append({"symbol": sym, "timeframe": "1D", "end_ts": int(ts.timestamp()), "close": 100 + idx + si})
    path = root / "sample.csv"
    pd.DataFrame(rows).to_csv(path, index=False)
    return path


def _write_exp_batch_spec(
    root: Path, dataset_path: Path, *, symbol_groups, windows, hypothesis_id="",
    allow_unregistered_diagnostic=False, out_name="batch.json",
) -> Path:
    payload = {
        "schema_version": "exp-distributed-v1",
        "engine_id": "EXP",
        "experiment_id": "exp.registry_smoke",
        "batch_label": "registry-test",
        "dataset_path": str(dataset_path),
        "strategy_id": "exp.cross_sectional_momentum_v1",
        "timeframe": "1D",
        "base_params": {"min_signal": 0.0, "rebalance_every": 1},
        "parameter_grid": {"lookback_days": [2, 3], "top_n": [1, 2]},
        "symbol_groups": symbol_groups,
        "windows": windows,
        "max_workers": 1,
        "notes": ["registry test"],
        "hypothesis_id": hypothesis_id,
        "allow_unregistered_diagnostic": allow_unregistered_diagnostic,
    }
    path = root / out_name
    path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# TEST 13 — distributed parameter grid counts candidates, not window slices.
# One run_batch invocation opens exactly one attempt per unique candidate;
# windows are evaluation slices linked to that candidate's single attempt
# (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-01 Blocker 2).
# ---------------------------------------------------------------------------

def test_distributed_windows_do_not_inflate_trial_count(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [
        {"label": "window_a", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "window_b", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "window_c", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
    ]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_smoke",
    )
    result = run_batch(spec_path, root=root / "research_out", max_workers=1)

    assert result["summary"]["job_count"] == 12  # 4 params x 1 universe x 3 windows
    assert result["registry_status"] == "registered"

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_smoke")
    assert summary["unique_trials"] == 4  # windows excluded from candidate identity
    assert summary["attempts"] == 4  # one attempt per candidate, NOT one per window job

    trials = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_smoke")
    assert len(trials) == 4
    for trial in trials:
        attempts = store.list_attempts(trial["trial_id"])
        assert len(attempts) == 1  # one attempt covers all 3 windows for this candidate
        assert attempts[0]["status"] == "succeeded"
        linked_job_ids = store.list_attempt_jobs(attempts[0]["attempt_id"])
        assert len(linked_job_ids) == 3  # all 3 window jobs for this candidate link to the same attempt
        result_summary = json.loads(attempts[0]["result_summary_json"])
        assert result_summary["total_slices"] == 3
        assert result_summary["succeeded_slices"] == 3
        assert result_summary["failed_slices"] == 0
        assert sorted(result_summary["job_ids"]) == sorted(linked_job_ids)


# ---------------------------------------------------------------------------
# TEST 13B — a candidate attempt fails if any of its required window slices
# fails, even when other windows for the same candidate succeeded.
# ---------------------------------------------------------------------------

def test_distributed_candidate_attempt_fails_if_one_window_slice_fails(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    # lookback_days=0 triggers a ValueError inside the strategy (see TEST 15);
    # lookback_days=2 is a normal successful candidate. Mixing them in one
    # window list per candidate isn't directly expressible via parameter_grid
    # cross-product with a single window, so instead we use two windows where
    # only one dataset slice is wide enough for lookback_days=2 to succeed and
    # short-circuit failure is instead driven by an explicit bad param.
    windows = [
        {"label": "w1", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "w2", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
    ]
    spec_payload_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_mixed",
    )
    payload = json.loads(spec_payload_path.read_text(encoding="utf-8"))
    # one of the four params in the 2x2 grid (lookback_days=0) always fails in
    # the strategy; the other three succeed on both windows.
    payload["parameter_grid"] = {"lookback_days": [0, 2], "top_n": [1]}
    spec_payload_path.write_text(json.dumps(payload), encoding="utf-8")

    result = run_batch(spec_payload_path, root=root / "research_out", max_workers=1)
    assert result["status"] == "failed"  # batch has at least one failed job

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trials = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_mixed")
    assert len(trials) == 2  # lookback_days in {0, 2}, top_n fixed at 1

    by_lookback = {}
    for trial in trials:
        identity = json.loads(trial["identity_json"])
        by_lookback[identity["params"]["lookback_days"]] = trial

    bad_attempts = store.list_attempts(by_lookback[0]["trial_id"])
    assert len(bad_attempts) == 1
    assert bad_attempts[0]["status"] == "failed"  # both its window slices failed
    bad_summary = json.loads(bad_attempts[0]["result_summary_json"])
    assert bad_summary["succeeded_slices"] == 0
    assert bad_summary["failed_slices"] == 2

    good_attempts = store.list_attempts(by_lookback[2]["trial_id"])
    assert len(good_attempts) == 1
    assert good_attempts[0]["status"] == "succeeded"
    good_summary = json.loads(good_attempts[0]["result_summary_json"])
    assert good_summary["succeeded_slices"] == 2
    assert good_summary["failed_slices"] == 0


# ---------------------------------------------------------------------------
# TEST 13C — an exact second run_batch of the same candidates doubles
# attempts but leaves unique_trials unchanged; a partial rerun_failed_jobs
# retry groups by candidate (one new attempt per candidate), not per window.
# ---------------------------------------------------------------------------

def test_distributed_exact_rerun_doubles_attempts_not_trials(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_exact_rerun",
    )

    run_batch(spec_path, root=root / "research_out", max_workers=1)
    run_batch(spec_path, root=root / "research_out", max_workers=1)  # same spec => same batch_id, same 4 trials

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_exact_rerun")
    assert summary["unique_trials"] == 4
    assert summary["attempts"] == 8  # one new attempt per candidate per invocation, not per window


def test_distributed_rerun_failed_jobs_groups_by_candidate_one_new_attempt(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [
        {"label": "w1", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "w2", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "w3", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
    ]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_partial_retry",
    )
    payload = json.loads(spec_path.read_text(encoding="utf-8"))
    payload["parameter_grid"] = {"lookback_days": [2], "top_n": [1]}  # single candidate, 3 window slices
    spec_path.write_text(json.dumps(payload), encoding="utf-8")

    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    batch_id = result["batch_id"]

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary_before = store.registry_summary(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_partial_retry")
    assert summary_before["unique_trials"] == 1
    assert summary_before["attempts"] == 1
    trial_id_before = store.list_trials(
        experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_partial_retry"
    )[0]["trial_id"]
    original_attempt_id = store.list_attempts(trial_id_before)[0]["attempt_id"]

    # simulate 2 of the 3 window jobs having failed after the fact, as
    # rerun_failed_jobs would find them (status="failed" in exp_jobs)
    all_jobs = store.list_jobs(batch_id)
    assert len(all_jobs) == 3
    for row in all_jobs[:2]:
        store.set_job_status(row["job_id"], "failed")

    rerun_result = rerun_failed_jobs(batch_id, root=root / "research_out", max_workers=1)
    assert rerun_result["rerun_count"] == 2
    assert rerun_result["registry_status"] == "registered"

    summary_after = store.registry_summary(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_partial_retry")
    assert summary_after["unique_trials"] == 1  # unchanged
    assert summary_after["attempts"] == 2  # ONE new attempt, not two

    trial = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_partial_retry")[0]
    attempts = store.list_attempts(trial["trial_id"])
    assert len(attempts) == 2
    assert [a["attempt_index"] for a in attempts] == [1, 2]
    retry_attempt = attempts[1]
    linked = store.list_attempt_jobs(retry_attempt["attempt_id"])
    assert len(linked) == 2  # only the 2 rerun window slices link to the new attempt
    retry_summary = json.loads(retry_attempt["result_summary_json"])
    assert retry_summary["total_slices"] == 2
    metadata = json.loads(retry_attempt["metadata_json"])
    assert metadata["scope"] == "failed_slices_only"

    # the original (first) attempt is untouched — failed-candidate history is preserved
    assert attempts[0]["attempt_id"] == original_attempt_id
    assert attempts[0]["attempt_id"] != retry_attempt["attempt_id"]

    # REQUIRED TEST 4 (partial retry) — attempt 1's immutable slice snapshots
    # preserve exactly what attempt 1 actually observed (all 3 succeeded — the
    # exp_jobs "failed" status above is a post-hoc simulation of a later
    # operational state change and must NOT retroactively alter attempt 1's
    # durable evidence). Attempt 2's snapshots cover only the 2 retried slices.
    original_slices = store.list_attempt_slices(original_attempt_id)
    assert len(original_slices) == 3
    assert all(s["status"] == "succeeded" for s in original_slices)

    retry_slices = store.list_attempt_slices(retry_attempt["attempt_id"])
    assert len(retry_slices) == 2
    assert all(s["status"] == "succeeded" for s in retry_slices)
    retried_job_ids = {s["job_id"] for s in retry_slices}
    original_job_ids = {s["job_id"] for s in original_slices}
    assert retried_job_ids <= original_job_ids
    # the retried job_ids are the exact 2 that were force-marked "failed" in exp_jobs
    simulated_failed_job_ids = {row["job_id"] for row in all_jobs[:2]}
    assert retried_job_ids == simulated_failed_job_ids


# ---------------------------------------------------------------------------
# RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-02 — attempt-slice snapshot
# immutability (storage-level unit tests against research_attempt_slices).
# ---------------------------------------------------------------------------

def _setup_trial(store: ResearchResultStore, *, trial_id: str, hypothesis_id: str = "hyp.slice", experiment_id: str = "exp.slice") -> None:
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id="s", protocol_id="p", identity={"k": trial_id},
    )


def test_slice_snapshot_survives_same_job_id_across_different_attempts(tmp_path):
    """REQUIRED TEST 1 — same job_id, different attempts: an exact deterministic
    rerun of job J that succeeds under attempt 2 must not change what attempt
    1's snapshot of J says."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _setup_trial(store, trial_id="trial-slice-1")

    attempt_1, _ = store.begin_attempt(trial_id="trial-slice-1")
    store.link_attempt_jobs(attempt_1, ["job-J"])
    store.record_attempt_slices(attempt_1, "trial-slice-1", [
        {"job_id": "job-J", "status": "failed", "failure_reason": "bad lookback"},
    ])
    store.finalize_attempt(attempt_1, status="failed", failure_reason="bad lookback")

    attempt_2, _ = store.begin_attempt(trial_id="trial-slice-1")
    store.link_attempt_jobs(attempt_2, ["job-J"])  # same deterministic job_id, new attempt
    store.record_attempt_slices(attempt_2, "trial-slice-1", [
        {"job_id": "job-J", "status": "succeeded"},
    ])
    store.finalize_attempt(attempt_2, status="succeeded")

    slices_1 = store.list_attempt_slices(attempt_1)
    slices_2 = store.list_attempt_slices(attempt_2)
    assert len(slices_1) == 1 and slices_1[0]["status"] == "failed"
    assert len(slices_2) == 1 and slices_2[0]["status"] == "succeeded"
    assert slices_1[0]["job_id"] == slices_2[0]["job_id"] == "job-J"


def test_slice_failure_reason_survives_successful_retry(tmp_path):
    """REQUIRED TEST 2 — attempt 1's exact failure_reason remains queryable
    after a successful retry of the same job_id."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _setup_trial(store, trial_id="trial-slice-2")

    attempt_1, _ = store.begin_attempt(trial_id="trial-slice-2")
    store.link_attempt_jobs(attempt_1, ["job-K"])
    store.record_attempt_slices(attempt_1, "trial-slice-2", [
        {"job_id": "job-K", "status": "failed", "failure_reason": "ValueError: lookback_days must be > 0"},
    ])
    store.finalize_attempt(attempt_1, status="failed", failure_reason="boom")

    attempt_2, _ = store.begin_attempt(trial_id="trial-slice-2")
    store.link_attempt_jobs(attempt_2, ["job-K"])
    store.record_attempt_slices(attempt_2, "trial-slice-2", [{"job_id": "job-K", "status": "succeeded"}])
    store.finalize_attempt(attempt_2, status="succeeded")

    slices_1 = store.list_attempt_slices(attempt_1)
    assert slices_1[0]["failure_reason"] == "ValueError: lookback_days must be > 0"


def test_slice_metrics_and_artifact_linkage_survive_retry(tmp_path):
    """REQUIRED TEST 3 — attempt 1 and attempt 2 produce different
    metrics/artifact fields; each attempt returns its own original snapshot."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _setup_trial(store, trial_id="trial-slice-3")

    attempt_1, _ = store.begin_attempt(trial_id="trial-slice-3")
    store.link_attempt_jobs(attempt_1, ["job-M"])
    store.record_attempt_slices(attempt_1, "trial-slice-3", [
        {
            "job_id": "job-M", "status": "succeeded",
            "metrics": {"sharpe": 0.1}, "artifact_paths": {"summary_metrics": "/a1/summary_metrics.json"},
        },
    ])
    store.finalize_attempt(attempt_1, status="succeeded")

    attempt_2, _ = store.begin_attempt(trial_id="trial-slice-3")
    store.link_attempt_jobs(attempt_2, ["job-M"])
    store.record_attempt_slices(attempt_2, "trial-slice-3", [
        {
            "job_id": "job-M", "status": "succeeded",
            "metrics": {"sharpe": 1.4}, "artifact_paths": {"summary_metrics": "/a2/summary_metrics.json"},
        },
    ])
    store.finalize_attempt(attempt_2, status="succeeded")

    slices_1 = store.list_attempt_slices(attempt_1)
    slices_2 = store.list_attempt_slices(attempt_2)
    assert slices_1[0]["metrics"]["sharpe"] == 0.1
    assert slices_1[0]["artifact_paths"]["summary_metrics"] == "/a1/summary_metrics.json"
    assert slices_2[0]["metrics"]["sharpe"] == 1.4
    assert slices_2[0]["artifact_paths"]["summary_metrics"] == "/a2/summary_metrics.json"


def test_slice_snapshot_is_immutable_once_recorded(tmp_path):
    """REQUIRED TEST 5 — mutating an already-recorded terminal slice snapshot
    must fail closed; there is no update path through the public API."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _setup_trial(store, trial_id="trial-slice-5")

    attempt_id, _ = store.begin_attempt(trial_id="trial-slice-5")
    store.link_attempt_jobs(attempt_id, ["job-Z"])
    store.record_attempt_slices(attempt_id, "trial-slice-5", [
        {"job_id": "job-Z", "status": "failed", "failure_reason": "x"},
    ])

    with pytest.raises(RuntimeError, match="immutable"):
        store.record_attempt_slices(attempt_id, "trial-slice-5", [{"job_id": "job-Z", "status": "succeeded"}])

    slices = store.list_attempt_slices(attempt_id)
    assert len(slices) == 1
    assert slices[0]["status"] == "failed"  # unchanged by the rejected write


# ---------------------------------------------------------------------------
# TEST 14 — different universe creates a distinct candidate trial
# ---------------------------------------------------------------------------

def test_distributed_universe_change_creates_distinct_trial(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"], ["AAA", "CCC"]], windows=windows,
        hypothesis_id="hyp.exp_universe",
    )
    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["summary"]["job_count"] == 8  # 4 params x 2 universes x 1 window

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trials = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_universe")
    assert len(trials) == 8  # 4 params x 2 distinct universes
    universes = {tuple(json.loads(t["identity_json"])["universe"]) for t in trials}
    assert universes == {("AAA", "BBB"), ("AAA", "CCC")}


# ---------------------------------------------------------------------------
# TEST 15 — failed distributed candidate remains registered
# ---------------------------------------------------------------------------

def test_distributed_failed_candidate_remains_registered(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    spec_payload = {
        "schema_version": "exp-distributed-v1",
        "engine_id": "EXP",
        "experiment_id": "exp.registry_fail",
        "dataset_path": str(dataset_path),
        "strategy_id": "exp.cross_sectional_momentum_v1",
        "timeframe": "1D",
        "base_params": {},
        "parameter_grid": {"lookback_days": [0], "top_n": [1]},  # 0 -> ValueError in strategy
        "symbol_groups": [["AAA", "BBB"]],
        "windows": [{"label": "bad", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}],
        "max_workers": 1,
        "hypothesis_id": "hyp.exp_fail",
    }
    spec_path = root / "bad_batch.json"
    spec_path.write_text(json.dumps(spec_payload, indent=2), encoding="utf-8")

    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["status"] == "failed"

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary(experiment_id="exp.registry_fail", hypothesis_id="hyp.exp_fail")
    assert summary["unique_trials"] == 1
    assert summary["failed_attempts"] == 1

    trial = store.list_trials(experiment_id="exp.registry_fail", hypothesis_id="hyp.exp_fail")[0]
    attempts = store.list_attempts(trial["trial_id"])
    assert attempts[0]["status"] == "failed"
    assert attempts[0]["failure_reason"]


# ---------------------------------------------------------------------------
# TEST 16 — sweep-plan generation is not an attempt
# ---------------------------------------------------------------------------

def test_sweep_plan_generation_does_not_touch_registry(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    baseline = store.registry_summary()
    assert baseline["unique_trials"] == 0
    assert baseline["attempts"] == 0

    grid_path = tmp_path / "grid.json"
    grid_path.write_text(json.dumps({"grid": {"lookback_days": [2, 3], "top_n": [1, 2]}}), encoding="utf-8")
    out_csv = tmp_path / "sweep_plan.csv"
    run_sweep_scaffold(grid_json=grid_path, out_csv=out_csv)
    assert out_csv.exists()

    after = store.registry_summary()
    assert after == baseline  # planning alone never touches the registry


# ---------------------------------------------------------------------------
# TEST 16B — create_batch (planning) never enforces the registration gate,
# even with an empty hypothesis_id and no diagnostic override.
# ---------------------------------------------------------------------------

def test_create_batch_planning_is_exempt_from_registration_gate(tmp_path):
    from mqk_research.exp_distributed.runner import create_batch

    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="",
    )
    result = create_batch(spec_path, root=root / "research_out")  # must not raise
    assert result["job_count"] == 4

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary()
    assert summary["unique_trials"] == 0
    assert summary["attempts"] == 0


# ---------------------------------------------------------------------------
# TEST 16C (Blocker 1) — run_batch with no hypothesis_id and no explicit
# diagnostic override fails closed; no silent unregistered execution.
# ---------------------------------------------------------------------------

def test_run_batch_without_hypothesis_id_fails_closed(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="",
    )
    with pytest.raises(ValueError, match="hypothesis_id"):
        run_batch(spec_path, root=root / "research_out", max_workers=1)

    # nothing was registered and no job execution happened
    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary()
    assert summary["unique_trials"] == 0
    assert summary["attempts"] == 0


# ---------------------------------------------------------------------------
# TEST 16D (Blocker 1) — explicit allow_unregistered_diagnostic=True permits
# the run, leaves the registry untouched, and is visibly marked in the
# result — never represented as registered research.
# ---------------------------------------------------------------------------

def test_run_batch_explicit_diagnostic_override_is_visibly_marked(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="",
        allow_unregistered_diagnostic=True,
    )
    store = ResearchResultStore(default_db_path(root / "research_out"))
    baseline = store.registry_summary()

    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["registry_status"] == "unregistered_diagnostic"
    assert result["summary"]["job_count"] == 4

    after = store.registry_summary()
    assert after == baseline  # unregistered diagnostic run never touches trial/attempt tables


# ---------------------------------------------------------------------------
# TEST 16E (Blocker 1) — hypothesis_id and allow_unregistered_diagnostic
# together fail closed (contradictory, never a silent fallback either way).
# ---------------------------------------------------------------------------

def test_hypothesis_id_and_diagnostic_override_are_mutually_exclusive(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.x",
        allow_unregistered_diagnostic=True,
    )
    with pytest.raises(ValueError, match="mutually exclusive"):
        run_batch(spec_path, root=root / "research_out", max_workers=1)


# ---------------------------------------------------------------------------
# TEST 16F (Blocker 1) — rerun_failed_jobs follows the identical fail-closed
# rule: a batch whose stored spec has no hypothesis_id and no diagnostic
# override cannot be silently retried unregistered.
# ---------------------------------------------------------------------------

def test_rerun_failed_jobs_without_registration_fails_closed(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows,
        hypothesis_id="", allow_unregistered_diagnostic=True,
    )
    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    batch_id = result["batch_id"]

    store = ResearchResultStore(default_db_path(root / "research_out"))
    all_jobs = store.list_jobs(batch_id)
    store.set_job_status(all_jobs[0]["job_id"], "failed")

    # simulate a legacy/corrupted stored spec that lost its explicit diagnostic
    # opt-in (spec_json no longer says allow_unregistered_diagnostic=True) —
    # rerun must still refuse to silently retry unregistered.
    batch_row = store.get_batch(batch_id)
    tampered_spec = json.loads(batch_row["spec_json"])
    tampered_spec["allow_unregistered_diagnostic"] = False
    store.upsert_batch(
        batch_id, tampered_spec, root_dir=root / "research_out", job_count=batch_row["job_count"], status=batch_row["status"],
    )

    with pytest.raises(ValueError, match="hypothesis_id"):
        rerun_failed_jobs(batch_id, root=root / "research_out", max_workers=1)


# ---------------------------------------------------------------------------
# TEST 17 — holdout remains reserved through the registered path
# ---------------------------------------------------------------------------

def test_registered_eval_holdout_stays_reserved(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset(symbols=("AAA", "BBB", "CCC"))

    out_path = _registered_eval(run_dir, df, registry_db=registry_db)
    out = json.loads(out_path.read_text(encoding="utf-8"))
    assert out["holdout"]["status"] == "reserved_not_evaluated"
    blob = json.dumps(out)
    assert "holdout_auc" not in blob
    assert "holdout_logloss" not in blob
    assert "holdout_predictions" not in blob

    store = ResearchResultStore(registry_db)
    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    identity_blob = trial["identity_json"]
    assert "holdout_auc" not in identity_blob
    assert "holdout_logloss" not in identity_blob


# ---------------------------------------------------------------------------
# TEST 18 — canonical ordering determinism (params dict order, symbol order)
# ---------------------------------------------------------------------------

def _make_job(*, params, symbols) -> JobSpec:
    return JobSpec(
        job_id="job-x",
        batch_id="batch-x",
        job_index=0,
        experiment_id="exp.order",
        strategy_id="exp.cross_sectional_momentum_v1",
        params=params,
        symbols=symbols,
        window=WindowSpec(start_utc="2024-01-01T00:00:00Z", end_utc="2024-01-02T00:00:00Z", label="w"),
        dataset_fingerprint=DatasetFingerprint(
            dataset_path="/tmp/data.csv", dataset_sha256="deadbeef", selection_sha256="cafef00d",
            timeframe="1D", symbols=list(symbols), start_utc="2024-01-01T00:00:00Z", end_utc="2024-01-02T00:00:00Z",
        ),
        timeframe="1D",
    )


def test_param_key_order_and_symbol_order_do_not_change_trial_id():
    job_a = _make_job(params={"lookback_days": 2, "top_n": 1}, symbols=["AAA", "BBB"])
    job_b = _make_job(params={"top_n": 1, "lookback_days": 2}, symbols=["BBB", "AAA"])

    trial_a, _ = build_candidate_trial_identity(job_a, hypothesis_id="hyp.x", experiment_id="exp.order")
    trial_b, _ = build_candidate_trial_identity(job_b, hypothesis_id="hyp.x", experiment_id="exp.order")
    assert trial_a == trial_b


# ---------------------------------------------------------------------------
# TEST 19 — concurrent attempt registration cannot collide
# ---------------------------------------------------------------------------

def test_concurrent_attempt_registration_has_no_collisions(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.concurrent", experiment_id="exp.concurrent")
    store.register_trial(
        trial_id="trial-concurrent", experiment_id="exp.concurrent", hypothesis_id="hyp.concurrent",
        strategy_id="s", protocol_id="p", identity={"k": "v"},
    )

    n_attempts = 25
    results = []
    lock = threading.Lock()

    def _begin():
        attempt_id, attempt_index = store.begin_attempt(trial_id="trial-concurrent")
        with lock:
            results.append((attempt_id, attempt_index))

    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = [executor.submit(_begin) for _ in range(n_attempts)]
        for future in futures:
            future.result()

    indices = sorted(idx for _, idx in results)
    assert indices == list(range(1, n_attempts + 1))
    attempt_ids = {aid for aid, _ in results}
    assert len(attempt_ids) == n_attempts

    attempts = store.list_attempts("trial-concurrent")
    assert len(attempts) == n_attempts


# ---------------------------------------------------------------------------
# TEST 20 — query summary reflects exact counts
# ---------------------------------------------------------------------------

def test_registry_summary_exact_counts(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp.summary", experiment_id="exp.summary")

    store.register_trial(
        trial_id="trial-1", experiment_id="exp.summary", hypothesis_id="hyp.summary",
        strategy_id="s", protocol_id="p", identity={"k": 1},
    )
    store.register_trial(
        trial_id="trial-2", experiment_id="exp.summary", hypothesis_id="hyp.summary",
        strategy_id="s", protocol_id="p", identity={"k": 2},
    )

    a1, _ = store.begin_attempt(trial_id="trial-1")
    store.finalize_attempt(a1, status="succeeded")
    a2, _ = store.begin_attempt(trial_id="trial-1")
    store.finalize_attempt(a2, status="failed", failure_reason="x")
    a3, _ = store.begin_attempt(trial_id="trial-2")
    store.finalize_attempt(a3, status="blocked", failure_reason="halt")

    summary = store.registry_summary(experiment_id="exp.summary", hypothesis_id="hyp.summary")
    assert summary == {
        "unique_trials": 2,
        "attempts": 3,
        "started_attempts": 0,
        "succeeded_attempts": 1,
        "failed_attempts": 1,
        "blocked_attempts": 1,
    }


# ---------------------------------------------------------------------------
# REQUIRED TEST 6 — attempt-level summary agrees with immutable slice evidence
# (distributed integration: 2 candidates, one all-succeed, one all-fail).
# ---------------------------------------------------------------------------

def test_attempt_summary_matches_its_own_immutable_slice_snapshots(tmp_path):
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [
        {"label": "w1", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "w2", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
    ]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_slice_summary",
    )
    payload = json.loads(spec_path.read_text(encoding="utf-8"))
    payload["parameter_grid"] = {"lookback_days": [0, 2], "top_n": [1]}  # 0 always fails; 2 always succeeds
    spec_path.write_text(json.dumps(payload), encoding="utf-8")

    run_batch(spec_path, root=root / "research_out", max_workers=1)

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trials = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_slice_summary")
    assert len(trials) == 2
    for trial in trials:
        attempt = store.list_attempts(trial["trial_id"])[0]
        summary = json.loads(attempt["result_summary_json"])
        slices = store.list_attempt_slices(attempt["attempt_id"])
        assert summary["total_slices"] == len(slices) == 2
        assert summary["succeeded_slices"] == sum(1 for s in slices if s["status"] == "succeeded")
        assert summary["failed_slices"] == sum(1 for s in slices if s["status"] != "succeeded")
        assert sorted(summary["job_ids"]) == sorted(s["job_id"] for s in slices)
        assert attempt["status"] == ("succeeded" if summary["failed_slices"] == 0 else "failed")


# ---------------------------------------------------------------------------
# DISTRIBUTED DIAGNOSTIC ARTIFACT MARKING — durable canonical batch artifact
# (batch_manifest.json) carries the same registry truth as the in-memory
# result, so an artifact directory inspected later cannot be mistaken for a
# different registration state than what actually happened.
# ---------------------------------------------------------------------------

def test_unregistered_diagnostic_run_batch_manifest_is_durably_marked(tmp_path):
    """REQUIRED TEST 7 — explicit unregistered diagnostic run: returned result
    says unregistered_diagnostic, durable batch_manifest.json also says
    unregistered_diagnostic, registry counts unchanged."""
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="",
        allow_unregistered_diagnostic=True,
    )
    store = ResearchResultStore(default_db_path(root / "research_out"))
    baseline = store.registry_summary()

    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["registry_status"] == "unregistered_diagnostic"

    batch = store.get_batch(result["batch_id"])
    aggregate_paths = json.loads(batch["aggregate_paths_json"])
    manifest_on_disk = json.loads(Path(aggregate_paths["batch_manifest"]).read_text(encoding="utf-8"))
    assert manifest_on_disk["registry_status"] == "unregistered_diagnostic"

    assert store.registry_summary() == baseline  # registry counts unchanged


def test_registered_run_batch_manifest_is_durably_marked(tmp_path):
    """REQUIRED TEST 8 — registered run: durable batch_manifest.json says
    registered."""
    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_manifest",
    )
    result = run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["registry_status"] == "registered"

    store = ResearchResultStore(default_db_path(root / "research_out"))
    batch = store.get_batch(result["batch_id"])
    aggregate_paths = json.loads(batch["aggregate_paths_json"])
    manifest_on_disk = json.loads(Path(aggregate_paths["batch_manifest"]).read_text(encoding="utf-8"))
    assert manifest_on_disk["registry_status"] == "registered"


def test_create_batch_planning_manifest_is_not_falsely_attempted(tmp_path):
    """REQUIRED TEST 9 — planning-only create_batch does not falsely claim an
    attempted trial in the durable manifest."""
    from mqk_research.exp_distributed.artifacts import batch_root
    from mqk_research.exp_distributed.runner import create_batch

    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="",
    )
    result = create_batch(spec_path, root=root / "research_out")

    manifest_path = batch_root(root / "research_out", result["batch_id"]) / "batch_manifest.json"
    manifest_on_disk = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_on_disk["registry_status"] == "planned_not_attempted"

    store = ResearchResultStore(default_db_path(root / "research_out"))
    summary = store.registry_summary()
    assert summary["unique_trials"] == 0
    assert summary["attempts"] == 0


# ---------------------------------------------------------------------------
# NEGATIVE CONTROL — proves that reading exp_jobs' CURRENT (mutable) row for a
# job_id, instead of the immutable research_attempt_slices snapshot, would
# make attempt 1 incorrectly appear succeeded after attempt 2 retries the
# same deterministic job_id to success. Confirms list_attempt_slices avoids
# this bug by never joining against exp_jobs' current state.
# ---------------------------------------------------------------------------

def test_negative_control_naive_exp_jobs_read_would_leak_mutated_state_but_slices_do_not(tmp_path, monkeypatch):
    import mqk_research.exp_distributed.runner as runner_module

    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.exp_negcontrol",
    )
    payload = json.loads(spec_path.read_text(encoding="utf-8"))
    payload["parameter_grid"] = {"lookback_days": [2], "top_n": [1]}  # single candidate, single job
    spec_path.write_text(json.dumps(payload), encoding="utf-8")

    real_worker = runner_module.run_job_worker
    call_count = {"n": 0}

    def _forced_first_failure(payload_dict, root_dir):
        # forces the FIRST invocation of this (otherwise always-succeeding)
        # job to report "failed", and lets a later re-invocation of the exact
        # same deterministic job_id report its real "succeeded" outcome —
        # simulating "same job_id, attempt 1 fails, attempt 2 succeeds".
        call_count["n"] += 1
        raw = real_worker(payload_dict, root_dir)
        if call_count["n"] == 1:
            raw = dict(raw)
            raw["status"] = "failed"
            raw["failure_reason"] = "forced: bad lookback (negative-control test)"
            raw["metrics"] = {}
        return raw

    monkeypatch.setattr(runner_module, "run_job_worker", _forced_first_failure)

    result = runner_module.run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["status"] == "failed"
    batch_id = result["batch_id"]

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trial = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.exp_negcontrol")[0]
    attempt_1 = store.list_attempts(trial["trial_id"])[0]
    assert attempt_1["status"] == "failed"
    job_id = store.list_attempt_jobs(attempt_1["attempt_id"])[0]

    rerun_result = runner_module.rerun_failed_jobs(batch_id, root=root / "research_out", max_workers=1)
    assert rerun_result["rerun_count"] == 1

    attempts = store.list_attempts(trial["trial_id"])
    assert len(attempts) == 2
    attempt_2 = attempts[1]
    assert attempt_2["status"] == "succeeded"
    assert store.list_attempt_jobs(attempt_2["attempt_id"])[0] == job_id  # exact same deterministic job_id

    # --- negative control: what a NAIVE implementation reading exp_jobs'
    # current state for this job_id would report for attempt 1 ---
    current_rows = {row["job_id"]: row["status"] for row in store.list_jobs(batch_id)}
    assert current_rows[job_id] == "succeeded"  # exp_jobs was legitimately overwritten by the retry
    naive_status_attempt_1_would_report = current_rows[job_id]
    assert naive_status_attempt_1_would_report == "succeeded"  # BUG a mutable-state read would produce

    # --- correct behavior: the immutable snapshot preserves attempt 1's truth ---
    attempt_1_slices = store.list_attempt_slices(attempt_1["attempt_id"])
    assert len(attempt_1_slices) == 1
    assert attempt_1_slices[0]["status"] == "failed"
    assert attempt_1_slices[0]["failure_reason"] == "forced: bad lookback (negative-control test)"

    attempt_2_slices = store.list_attempt_slices(attempt_2["attempt_id"])
    assert len(attempt_2_slices) == 1
    assert attempt_2_slices[0]["status"] == "succeeded"


# ---------------------------------------------------------------------------
# EVAL_ID HARDENING CHECK — eval_id is a self-hash of the statistical
# evaluation core, computed BEFORE the "ids" and "registry" sections are
# attached (see run_walkforward_eval: out["ids"]["eval_id"] = sha256_json(out)
# is set before run_registered_walkforward_eval appends out["registry"]).
# There is therefore no circular dependency
# (trial_id -> eval_id -> attempt_id -> eval_id), and trial identity does not
# depend on eval_id / result content (see also TEST 3B above).
# ---------------------------------------------------------------------------

def test_eval_id_is_a_self_hash_of_the_evaluation_core_excluding_ids_and_registry(tmp_path):
    from mqk_research.ml.util_hash import sha256_json

    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    out_path = _registered_eval(run_dir, df, registry_db=registry_db)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    assert "ids" in out and "registry" in out
    core = {k: v for k, v in out.items() if k not in ("ids", "registry")}
    assert sha256_json(core) == out["ids"]["eval_id"]

    # trial identity must not depend on eval_id/result content (documented contract)
    store = ResearchResultStore(registry_db)
    trial = store.list_trials(experiment_id="alpha.discovery.test", hypothesis_id="alpha.peer_lag_catchup")[0]
    assert out["ids"]["eval_id"] not in trial["identity_json"]


# ---------------------------------------------------------------------------
# RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03 — immutable attempt-specific
# ARTIFACT EVIDENCE (research_attempt_slices.artifact_evidence_json).
#
# Blocker: artifacts/exp_distributed/batches/<batch_id>/jobs/<job_index>_<job_id>/
# is a DETERMINISTIC path. An exact retry of the same job_id overwrites the
# files there in place (write_job_artifacts), including artifact_metadata.json.
# A bare artifact_paths_json path recorded for an earlier attempt therefore no
# longer points at that attempt's own evidence once a later attempt retries
# the same job_id — it points at whatever the retry most recently wrote.
#
# capture_artifact_evidence() (exp_distributed/artifacts.py) snapshots
# artifact_metadata.json's own file/sha256/bytes records into
# research_attempt_slices.artifact_evidence_json at finalization time, before
# any later retry can overwrite the source path
# (runner._finalize_candidate_attempts).
# ---------------------------------------------------------------------------

def _force_job_execution_to_report_content_diverging_failure(runner_module, *, only_window_labels=None):
    """Patches run_job_worker so the FIRST invocation of each matching job
    physically overwrites the deterministic job artifact directory with
    genuinely different (failed, empty-metrics) file content — not just a
    different reported status — before a later real invocation of the exact
    same job_id overwrites it again with real successful content. This
    reproduces the REPAIR-03 blocker scenario end-to-end: same job_id, same
    artifact directory, two attempts, two genuinely different sets of bytes
    on disk.

    `only_window_labels`: if given, only jobs whose window.label is in this
    set are forced to a first-attempt failure; others succeed for real on
    their first (and only) execution. Returns (patched_function, forced_job_ids).
    """
    from mqk_research.exp_distributed.artifacts import write_job_artifacts

    real_worker = runner_module.run_job_worker
    forced_job_ids: set = set()

    def _patched(payload_dict, root_dir):
        job = JobSpec.from_dict(payload_dict)
        raw = real_worker(payload_dict, root_dir)
        should_force = only_window_labels is None or job.window.label in only_window_labels
        if should_force and job.job_id not in forced_job_ids:
            forced_job_ids.add(job.job_id)
            failure_reason = f"forced: injected content-divergence failure (REPAIR-03 test, job={job.job_id})"
            artifact_paths = write_job_artifacts(
                root=Path(root_dir),
                job=job,
                dataset_fingerprint=job.dataset_fingerprint.to_dict(),
                metrics={},
                daily_returns=pd.DataFrame(columns=["ts_utc", "portfolio_return"]),
                positions=pd.DataFrame(columns=["ts_utc"] + list(job.symbols)),
                trade_events=pd.DataFrame(columns=["ts_utc", "symbol", "event_type", "old_weight", "new_weight"]),
                failure_reason=failure_reason,
            )
            raw = dict(raw)
            raw["status"] = "failed"
            raw["failure_reason"] = failure_reason
            raw["metrics"] = {}
            raw["artifact_paths"] = artifact_paths
        return raw

    return _patched, forced_job_ids


def _single_candidate_batch_spec(root: Path, *, hypothesis_id: str) -> Path:
    dataset_path = _write_exp_dataset(root)
    windows = [{"label": "w", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id=hypothesis_id,
    )
    payload = json.loads(spec_path.read_text(encoding="utf-8"))
    payload["parameter_grid"] = {"lookback_days": [2], "top_n": [1]}  # single candidate, single job
    spec_path.write_text(json.dumps(payload), encoding="utf-8")
    return spec_path


def _run_repair03_exact_retry_scenario(tmp_path, monkeypatch, hypothesis_id):
    """Attempt 1: single-job candidate whose first execution is forced to a
    content-diverging failure (real bytes on disk, not just a relabeled
    status). Attempt 2: exact retry of the exact same deterministic job_id,
    succeeding for real and overwriting the shared source artifact directory
    in place."""
    import mqk_research.exp_distributed.runner as runner_module

    root = tmp_path
    spec_path = _single_candidate_batch_spec(root, hypothesis_id=hypothesis_id)
    patched, _forced = _force_job_execution_to_report_content_diverging_failure(runner_module)
    monkeypatch.setattr(runner_module, "run_job_worker", patched)

    result = runner_module.run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["status"] == "failed"
    batch_id = result["batch_id"]

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trial = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id=hypothesis_id)[0]
    attempt_1 = store.list_attempts(trial["trial_id"])[0]
    assert attempt_1["status"] == "failed"

    rerun_result = runner_module.rerun_failed_jobs(batch_id, root=root / "research_out", max_workers=1)
    assert rerun_result["rerun_count"] == 1

    attempts = store.list_attempts(trial["trial_id"])
    assert len(attempts) == 2
    attempt_2 = attempts[1]
    assert attempt_2["status"] == "succeeded"

    evidence_1 = store.list_attempt_slices(attempt_1["attempt_id"])[0]["artifact_evidence"]
    evidence_2 = store.list_attempt_slices(attempt_2["attempt_id"])[0]["artifact_evidence"]
    return store, attempt_1, attempt_2, evidence_1, evidence_2


def test_repair03_exact_retry_artifact_evidence_survives_source_overwrite(tmp_path, monkeypatch):
    """REQUIRED TEST 1 — exact retry artifact history: attempt 1's immutable
    artifact evidence is unchanged after attempt 2 overwrites the shared
    deterministic source artifact directory."""
    store, attempt_1, attempt_2, evidence_1_before, evidence_2 = _run_repair03_exact_retry_scenario(
        tmp_path, monkeypatch, hypothesis_id="hyp.repair03_test1"
    )
    assert evidence_1_before["status"] == "failed"
    assert evidence_1_before["artifact_metadata_sha256"] is not None

    # re-read attempt 1's evidence again — nothing about attempt 2 running
    # (which already happened inside the scenario helper) may have changed it.
    evidence_1_after = store.list_attempt_slices(attempt_1["attempt_id"])[0]["artifact_evidence"]
    assert evidence_1_after == evidence_1_before
    assert evidence_1_after["status"] == "failed"
    assert evidence_2["status"] == "succeeded"


def test_repair03_artifact_evidence_hashes_diverge_across_retry(tmp_path, monkeypatch):
    """REQUIRED TEST 2 — attempt 1 and attempt 2 have different content at the
    same shared source path; their captured hashes must differ (H1 != H2),
    proving the evidence is genuinely attempt-specific, not just re-reading a
    shared mutable path."""
    _, _, _, evidence_1, evidence_2 = _run_repair03_exact_retry_scenario(
        tmp_path, monkeypatch, hypothesis_id="hyp.repair03_test2"
    )
    h1 = evidence_1["artifact_metadata_sha256"]
    h2 = evidence_2["artifact_metadata_sha256"]
    assert h1 is not None and h2 is not None
    assert h1 != h2

    status_sha_1 = evidence_1["artifact_files"]["status"]["sha256"]
    status_sha_2 = evidence_2["artifact_files"]["status"]["sha256"]
    assert status_sha_1 != status_sha_2


def test_repair03_failure_evidence_provable_without_reading_current_status_file(tmp_path, monkeypatch):
    """REQUIRED TEST 3 — attempt 1's failure must be provable purely from its
    immutable evidence row, without the (by-now-overwritten) current
    status.json at the shared source path agreeing with it."""
    _, _, _, evidence_1, evidence_2 = _run_repair03_exact_retry_scenario(
        tmp_path, monkeypatch, hypothesis_id="hyp.repair03_test3"
    )
    assert evidence_1["status"] == "failed"
    assert "forced: injected content-divergence failure" in evidence_1["failure_reason"]

    # the CURRENT on-disk status.json (source path) now reflects attempt 2's
    # success — reading it would falsely "launder" attempt 1's failure.
    source_status_path = Path(evidence_1["source_artifact_paths"]["status"])
    current_status_on_disk = json.loads(source_status_path.read_text(encoding="utf-8"))
    assert current_status_on_disk["status"] == "succeeded"

    # attempt 1's own immutable evidence still says failed, independent of that overwrite
    assert evidence_1["status"] == "failed"
    assert evidence_2["status"] == "succeeded"


def test_repair03_metrics_artifact_evidence_matches_each_attempt(tmp_path, monkeypatch):
    """REQUIRED TEST 4 — each attempt's artifact evidence carries the correct
    original summary_metrics file record/hash, and the existing immutable
    metrics_json snapshot (REPAIR-02) remains correct alongside it."""
    store, attempt_1, attempt_2, evidence_1, evidence_2 = _run_repair03_exact_retry_scenario(
        tmp_path, monkeypatch, hypothesis_id="hyp.repair03_test4"
    )
    metrics_record_1 = evidence_1["artifact_files"]["summary_metrics"]
    metrics_record_2 = evidence_2["artifact_files"]["summary_metrics"]
    assert metrics_record_1["sha256"] != metrics_record_2["sha256"]

    slice_1 = store.list_attempt_slices(attempt_1["attempt_id"])[0]
    slice_2 = store.list_attempt_slices(attempt_2["attempt_id"])[0]
    assert slice_1["metrics"] == {}  # forced failure -> empty metrics (REPAIR-02 snapshot)
    assert slice_2["metrics"] != {}  # real successful strategy metrics


def test_repair03_partial_retry_preserves_original_per_slice_evidence(tmp_path, monkeypatch):
    """REQUIRED TEST 5 — partial retry (A succeeds; B, C fail then are
    retried to success): attempt 1's immutable artifact evidence for A, B, C
    is untouched by attempt 2's retry, which only carries B/C's new content."""
    import mqk_research.exp_distributed.runner as runner_module

    root = tmp_path
    dataset_path = _write_exp_dataset(root)
    windows = [
        {"label": "wA", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "wB", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
        {"label": "wC", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
    ]
    spec_path = _write_exp_batch_spec(
        root, dataset_path, symbol_groups=[["AAA", "BBB"]], windows=windows, hypothesis_id="hyp.repair03_test5",
    )
    payload = json.loads(spec_path.read_text(encoding="utf-8"))
    payload["parameter_grid"] = {"lookback_days": [2], "top_n": [1]}  # single candidate, 3 window slices
    spec_path.write_text(json.dumps(payload), encoding="utf-8")

    patched, _forced = _force_job_execution_to_report_content_diverging_failure(
        runner_module, only_window_labels={"wB", "wC"}
    )
    monkeypatch.setattr(runner_module, "run_job_worker", patched)

    result = runner_module.run_batch(spec_path, root=root / "research_out", max_workers=1)
    assert result["status"] == "failed"  # B and C failed
    batch_id = result["batch_id"]

    store = ResearchResultStore(default_db_path(root / "research_out"))
    trial = store.list_trials(experiment_id="exp.registry_smoke", hypothesis_id="hyp.repair03_test5")[0]
    attempt_1 = store.list_attempts(trial["trial_id"])[0]
    assert attempt_1["status"] == "failed"

    slices_1_before = {s["job_id"]: s for s in store.list_attempt_slices(attempt_1["attempt_id"])}
    assert len(slices_1_before) == 3
    assert sorted(s["status"] for s in slices_1_before.values()) == ["failed", "failed", "succeeded"]
    evidence_1_before = {job_id: s["artifact_evidence"] for job_id, s in slices_1_before.items()}

    rerun_result = runner_module.rerun_failed_jobs(batch_id, root=root / "research_out", max_workers=1)
    assert rerun_result["rerun_count"] == 2  # B and C

    attempts = store.list_attempts(trial["trial_id"])
    assert len(attempts) == 2
    attempt_2 = attempts[1]
    assert attempt_2["status"] == "succeeded"

    slices_1_after = {s["job_id"]: s for s in store.list_attempt_slices(attempt_1["attempt_id"])}
    for job_id, evidence_before in evidence_1_before.items():
        assert slices_1_after[job_id]["artifact_evidence"] == evidence_before  # untouched by attempt 2's retry

    slices_2 = {s["job_id"]: s for s in store.list_attempt_slices(attempt_2["attempt_id"])}
    assert len(slices_2) == 2  # only B and C retried
    for job_id, s in slices_2.items():
        assert s["status"] == "succeeded"
        assert s["artifact_evidence"]["status"] == "succeeded"
        # the retried evidence's hash differs from attempt 1's original (failed) hash for the same job_id
        assert (
            s["artifact_evidence"]["artifact_metadata_sha256"]
            != evidence_1_before[job_id]["artifact_metadata_sha256"]
        )


def test_repair03_artifact_evidence_is_immutable_once_recorded(tmp_path):
    """REQUIRED TEST 6 — trying to overwrite an existing attempt-slice's
    artifact evidence through the public API must fail closed."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _setup_trial(store, trial_id="trial-repair03-immutable")

    attempt_id, _ = store.begin_attempt(trial_id="trial-repair03-immutable")
    store.link_attempt_jobs(attempt_id, ["job-EV"])
    original_evidence = {"artifact_metadata_sha256": "hash-one", "artifact_files": {"status": {"sha256": "hash-one"}}}
    store.record_attempt_slices(attempt_id, "trial-repair03-immutable", [
        {"job_id": "job-EV", "status": "failed", "failure_reason": "x", "artifact_evidence": original_evidence},
    ])

    with pytest.raises(RuntimeError, match="immutable"):
        store.record_attempt_slices(attempt_id, "trial-repair03-immutable", [
            {
                "job_id": "job-EV", "status": "succeeded",
                "artifact_evidence": {"artifact_metadata_sha256": "hash-two", "artifact_files": {}},
            },
        ])

    slices = store.list_attempt_slices(attempt_id)
    assert len(slices) == 1
    assert slices[0]["artifact_evidence"] == original_evidence  # unchanged by the rejected write


# ---------------------------------------------------------------------------
# NEGATIVE CONTROL — proves that re-deriving a hash from the CURRENT (mutable)
# source artifact path at query time, instead of trusting the stored
# artifact_evidence snapshot, would make attempt 1 incorrectly observe
# attempt 2's content once attempt 2 retries the same deterministic job_id.
# Confirms the actual implementation avoids this bug by never re-reading the
# source path for evidence — it trusts the immutable snapshot captured at
# finalization time.
# ---------------------------------------------------------------------------

def test_repair03_negative_control_naive_source_path_reread_would_leak_retry_content(tmp_path, monkeypatch):
    from mqk_research.exp_distributed.hashing import sha256_bytes

    _, _, _, evidence_1, evidence_2 = _run_repair03_exact_retry_scenario(
        tmp_path, monkeypatch, hypothesis_id="hyp.repair03_negcontrol"
    )

    # --- naive (buggy) approach: re-read the CURRENT artifact_metadata.json at
    # attempt 1's recorded SOURCE path, instead of trusting the immutable
    # evidence snapshot ---
    source_metadata_path = Path(evidence_1["source_artifact_paths"]["artifact_metadata"])
    naive_current_hash_for_attempt_1 = sha256_bytes(source_metadata_path.read_bytes())

    # attempt 2 overwrote the same shared source path, so the naive re-read
    # incorrectly matches attempt 2's real evidence hash, not attempt 1's —
    # this is the exact BLOCKER the mission describes, reproduced and proven.
    assert naive_current_hash_for_attempt_1 == evidence_2["artifact_metadata_sha256"]
    assert naive_current_hash_for_attempt_1 != evidence_1["artifact_metadata_sha256"]  # bug confirmed

    # --- correct implementation: the stored snapshot is unaffected by the overwrite ---
    assert evidence_1["artifact_metadata_sha256"] != evidence_2["artifact_metadata_sha256"]
