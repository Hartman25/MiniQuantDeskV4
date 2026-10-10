"""Deterministic unit proof of the runner's opt-in `--resume` trial reconciliation (the crash windows a real kill rarely hits)."""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

from mqk_research.exp_distributed.storage import ResearchResultStore

HERE = Path(__file__).resolve().parents[1] / "experiments" / "m1_native_trend_campaign"


@pytest.fixture(scope="module")
def rb():
    sys.path.insert(0, str(HERE))
    os.environ.setdefault("MQK_M1_BATCH_DECLARATION", "PREDECLARED_BATCH_03.json")
    spec = importlib.util.spec_from_file_location("run_batch_resume_under_test", HERE / "run_batch.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture
def store(tmp_path):
    s = ResearchResultStore(tmp_path / "r.sqlite3")
    s.register_hypothesis(hypothesis_id="h", experiment_id="x", hypothesis_text="t")
    s.register_trial(trial_id="t1", experiment_id="x", hypothesis_id="h", strategy_id="s", protocol_id="p", identity={"k": 1})
    return s


def rec():
    return {"trial_id": "t1"}


def test_a_terminal_record_in_the_index_is_never_evaluated_again(rb, store):
    assert rb._reconcile_trial(store, {"trial_id": "t1", "economic_eval_id": "e"}) is True
    assert rb._reconcile_trial(store, {"trial_id": "t1", "failed": "no native signals inside fold 3"}) is True
    assert store.list_attempts("t1") == []                                       # nothing was started or rewritten


def test_a_succeeded_attempt_missing_from_the_index_is_recovered_from_durable_files_not_repeated(rb, store, tmp_path):
    econ = tmp_path / "economic.json"
    econ.write_text(json.dumps({"registry": {"execution_fidelity": 1.0}}), encoding="utf-8")
    aid, idx = store.begin_attempt(trial_id="t1", origin="o")
    store.finalize_attempt(aid, status="succeeded", result_id="eval-1", artifact_paths={"economic_walk_forward": str(econ)})
    r = rec()
    assert rb._reconcile_trial(store, r) is True
    assert r["economic_eval_id"] == "eval-1" and r["economic_path"] == str(econ) and r["attempt_index"] == idx and r["execution_fidelity"] == 1.0
    assert [a["status"] for a in store.list_attempts("t1")] == ["succeeded"]       # no second attempt


def test_a_succeeded_attempt_whose_artifact_vanished_fails_closed_instead_of_inventing_a_result(rb, store, tmp_path):
    aid, _ = store.begin_attempt(trial_id="t1", origin="o")
    store.finalize_attempt(aid, status="succeeded", result_id="eval-1", artifact_paths={"economic_walk_forward": str(tmp_path / "gone.json")})
    with pytest.raises(SystemExit, match="economic artifact is missing"):
        rb._reconcile_trial(store, rec())


def test_an_orphaned_started_attempt_is_finalized_failed_with_an_explicit_reason_and_the_trial_runs_again(rb, store):
    store.begin_attempt(trial_id="t1", origin="o")
    r = rec()
    assert rb._reconcile_trial(store, r) is False and "failed" not in r
    att = store.list_attempts("t1")
    assert [(a["status"], a["failure_reason"]) for a in att] == [("failed", rb.INTERRUPTED_REASON)]       # truthful, never fabricated as success


def test_a_genuine_failed_attempt_is_terminal_and_is_not_retried_by_outcome(rb, store):
    aid, _ = store.begin_attempt(trial_id="t1", origin="o")
    store.finalize_attempt(aid, status="failed", failure_reason="NativeSignalError: no native signals inside fold 4")
    r = rec()
    assert rb._reconcile_trial(store, r) is True and r["failed"].endswith("fold 4")


def test_a_previously_interrupted_attempt_does_not_make_the_trial_terminal(rb, store):
    aid, _ = store.begin_attempt(trial_id="t1", origin="o")
    store.finalize_attempt(aid, status="failed", failure_reason=rb.INTERRUPTED_REASON)
    assert rb._reconcile_trial(store, rec()) is False


def test_resume_is_opt_in_the_historical_default_keeps_register_once(rb):
    assert rb._resume(None) is False and rb._resume(type("A", (), {})()) is False
    assert rb._resume(type("A", (), {"resume": True})()) is True
