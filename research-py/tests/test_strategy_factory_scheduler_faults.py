"""Scheduler fault containment: no unexpected exception may leave a job silently claimed, report false idle, or touch another campaign."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
import threading
from pathlib import Path

import pytest

from mqk_research.strategy_factory import executor as X
from mqk_research.strategy_factory import scheduler as S
from mqk_research.strategy_factory.store import STAGES, FactoryStore, StoreError
from test_strategy_factory_campaign import FakeExecutor, store_with

SRC = str(Path(__file__).resolve().parents[1] / "src")


class Raising(FakeExecutor):
    def __init__(self, bad, exc=RuntimeError("executor bug"), **kw):
        super().__init__(**kw)
        self.bad, self.exc = bad, exc

    def execute(self, campaign, stage, attempt_no, heartbeat=None, heartbeat_every=60.0):
        if (campaign["campaign_id"], stage) in self.bad:
            with self.lock:
                self.calls.append((campaign["campaign_id"], stage, attempt_no))
            raise self.exc
        return super().execute(campaign, stage, attempt_no, heartbeat, heartbeat_every)


def running_jobs(st):
    return [j for j in st.list_jobs() if j["status"] == "running"]


# ------------------------------------------------------------------ during executor execution
def test_executor_exception_becomes_an_honest_terminal_failure_never_a_stuck_claim_or_false_idle(tmp_path):
    st = store_with(tmp_path, ("a", "b"))
    ex = Raising({("a", "register")})
    r = S.run_until_idle(st, ex, workers=2)
    assert not running_jobs(st), "a job was left claimed and RUNNING"
    assert r.ended == S.END_FAILED and r.ended != S.END_NO_WORK
    a = {j["stage"]: j for j in st.list_jobs("a")}
    assert a["register"]["status"] == "failed" and "executor_exception: RuntimeError: executor bug" in a["register"]["last_reason"]
    assert st.get_campaign("a")["state"] == "FAILED"
    att = st.attempts(a["register"]["job_id"])
    assert [x["status"] for x in att] == ["failed"] and att[0]["finished_at"] is not None
    assert a["trials"]["status"] == "queued" and a["trials"]["attempt_count"] == 0           # nothing downstream ran; no manufactured work
    assert st.get_campaign("b")["state"] == "COMPLETED"                                          # another campaign is unaffected
    assert all(j["status"] == "succeeded" and j["attempt_count"] == 1 for j in st.list_jobs("b"))


def test_a_base_exception_class_other_than_exception_in_the_executor_is_also_contained(tmp_path):
    st = store_with(tmp_path)
    r = S.run_pass(st, Raising({("a", "check")}, exc=SystemExit("fail-closed: boom")), workers=1)
    assert not running_jobs(st) and r.ended == S.END_FAILED
    assert "SystemExit" in st.list_jobs("a")[0]["last_reason"]


def test_the_operator_can_retry_an_executor_failure_without_creating_a_new_job_or_trial(tmp_path):
    st = store_with(tmp_path)
    S.run_until_idle(st, Raising({("a", "data")}), workers=1)
    before = [j["job_id"] for j in st.list_jobs("a")]
    st.retry_failed("a", "data", "executor bug fixed")
    r = S.run_until_idle(st, FakeExecutor(), workers=1)
    assert r.ended == S.END_NO_WORK and [j["job_id"] for j in st.list_jobs("a")] == before
    d = [j for j in st.list_jobs("a") if j["stage"] == "data"][0]
    assert [x["status"] for x in st.attempts(d["job_id"])] == ["failed", "succeeded"]
    assert len(st.campaign_trials("a")) == 1


# ------------------------------------------------------------------ before claim
def test_a_fault_before_any_claim_is_reported_and_leaves_nothing_claimed(tmp_path):
    st = store_with(tmp_path)
    real = st.claim_next
    boom = {"n": 0}

    def flaky(*a, **k):
        boom["n"] += 1
        if boom["n"] == 1:
            raise RuntimeError("database is locked")
        return real(*a, **k)
    st.claim_next = flaky
    r = S.run_pass(st, FakeExecutor(), workers=1)
    assert r.ended == S.END_ERROR and r.errors and "database is locked" in r.errors[0] and not running_jobs(st)
    assert all(j["status"] == "queued" and j["attempt_count"] == 0 for j in st.list_jobs("a"))
    again = S.run_until_idle(st, FakeExecutor(), workers=1)
    assert again.ended == S.END_NO_WORK and st.get_campaign("a")["state"] == "COMPLETED"


# ------------------------------------------------------------------ after claim, before execution
def test_a_fault_between_claim_and_execution_fails_the_claimed_job_honestly(tmp_path):
    st = store_with(tmp_path, ("a", "b"))
    real = st.get_campaign
    seen = {"a": 0}

    def flaky(cid):
        if cid == "a":
            seen["a"] += 1
            if seen["a"] == 1:
                raise RuntimeError("campaign row unreadable")
        return real(cid)
    st.get_campaign = flaky
    r = S.run_until_idle(st, FakeExecutor(), workers=2)
    assert not running_jobs(st) and r.ended == S.END_FAILED
    first = st.list_jobs("a")[0]
    assert first["status"] == "failed" and "pre_execution_error" in first["last_reason"]
    assert st.get_campaign("b")["state"] == "COMPLETED"


# ------------------------------------------------------------------ at finalization
def test_a_transient_finalization_fault_is_retried_with_the_same_claim_token(tmp_path):
    st = store_with(tmp_path)
    real = st.finish
    n = {"c": 0}

    def flaky(*a, **k):
        n["c"] += 1
        if n["c"] <= 2:
            raise sqlite3_error()
        return real(*a, **k)
    st.finish = flaky
    r = S.run_until_idle(st, FakeExecutor(), workers=1)
    assert r.ended == S.END_NO_WORK and not running_jobs(st) and st.get_campaign("a")["state"] == "COMPLETED"
    assert all(len(st.attempts(j["job_id"])) == 1 for j in st.list_jobs("a"))


def sqlite3_error():
    import sqlite3
    return sqlite3.OperationalError("database is locked")


def test_a_persistent_finalization_fault_is_never_reported_as_idle_and_is_recoverable_by_lease(tmp_path):
    st = store_with(tmp_path, ("a", "b"))
    real = st.finish

    def broken(job_id, token, **k):
        job = next(j for j in st.list_jobs() if j["job_id"] == job_id)
        if job["campaign_id"] == "a" and job["stage"] == "check":
            raise sqlite3_error()
        return real(job_id, token, **k)
    st.finish = broken
    r = S.run_pass(st, FakeExecutor(), workers=2, lease_seconds=900)
    assert r.ended == S.END_ERROR and r.unresolved_jobs and r.ended != S.END_NO_WORK
    stuck = running_jobs(st)
    assert [(j["campaign_id"], j["stage"]) for j in stuck] == [("a", "check")]
    assert st.get_campaign("b")["state"] == "COMPLETED"                                            # the other campaign finished
    st.finish = real
    import time
    assert st.recover_expired(now=time.time() + 100_000) == 1
    r2 = S.run_until_idle(st, FakeExecutor(), workers=1)
    assert r2.ended == S.END_NO_WORK and st.get_campaign("a")["state"] == "COMPLETED"
    chk = [j for j in st.list_jobs("a") if j["stage"] == "check"][0]
    assert [x["status"] for x in st.attempts(chk["job_id"])] == ["interrupted", "succeeded"]


def test_a_lost_claim_is_fenced_the_zombie_result_is_discarded_and_the_stage_is_redone_once(tmp_path):
    st = store_with(tmp_path)
    fired = {"done": False}

    class Zombie(FakeExecutor):
        def execute(self, campaign, stage, attempt_no, heartbeat=None, heartbeat_every=60.0):
            if stage == "check" and not fired["done"]:
                fired["done"] = True
                import time
                st.recover_expired(now=time.time() + 100_000)          # the lease is taken away while this worker is still "running"
            return super().execute(campaign, stage, attempt_no, heartbeat, heartbeat_every)
    r = S.run_until_idle(st, Zombie(), workers=1)
    assert r.lost_claims == 1 and r.ended == S.END_NO_WORK and not running_jobs(st)
    chk = [j for j in st.list_jobs("a") if j["stage"] == "check"][0]
    assert [x["status"] for x in st.attempts(chk["job_id"])] == ["interrupted", "succeeded"]
    assert st.get_campaign("a")["state"] == "COMPLETED"


def test_claim_loss_is_a_dedicated_error_type_and_invalid_outcomes_are_failures_not_lost_claims(tmp_path):
    st = store_with(tmp_path)
    j = st.claim_next("w", max_running=1, now=1.0)
    with pytest.raises(StoreError) as e:
        st.finish(j["job_id"], "forged", status="succeeded", exit_code=0, reason=None)
    assert type(e.value).__name__ == "ClaimLost"

    class Bad(FakeExecutor):
        def execute(self, *a, **k):
            return X.Outcome("exploded", 1, "x", "")
    st2 = store_with(tmp_path / "two")
    r = S.run_pass(st2, Bad(), workers=1)
    first = st2.list_jobs("a")[0]
    assert first["status"] == "failed" and "invalid executor outcome" in first["last_reason"] and r.ended == S.END_FAILED


# ------------------------------------------------------------------ cross-process
WORKER = textwrap.dedent("""
    import os, sys
    sys.path.insert(0, sys.argv[1]); sys.path.insert(0, sys.argv[2])
    from mqk_research.strategy_factory import scheduler as S
    from mqk_research.strategy_factory import executor as X
    from mqk_research.strategy_factory.store import FactoryStore
    st = FactoryStore(sys.argv[3])
    mode = sys.argv[4]

    class Ex:
        store = None
        def execute(self, campaign, stage, attempt_no, heartbeat=None, heartbeat_every=60.0):
            if campaign["campaign_id"] == "a" and stage == "trials":
                if mode == "crash":
                    os._exit(9)                      # hard crash mid-execution: no finally, no finish
                if mode == "raise":
                    raise RuntimeError("worker bug in trials")
            return X.Outcome("succeeded", 0, None, "")
    r = S.run_until_idle(st, Ex(), workers=2)
    print(r.ended)
    sys.exit({"NO_ELIGIBLE_WORK": 0, "BLOCKED": 3, "FAILED": 4, "ERROR": 4}.get(r.ended, 0))
""")


def run_worker(tmp_path, db, mode):
    script = tmp_path / f"w_{mode}.py"
    script.write_text(WORKER, encoding="utf-8")
    return subprocess.run([sys.executable, str(script), SRC, str(Path(__file__).parent), str(db), mode], capture_output=True, text=True, timeout=300)


def test_a_hard_crashed_worker_process_is_recovered_by_another_process_without_touching_other_campaigns(tmp_path):
    db = tmp_path / "x.sqlite3"
    st = FactoryStore(db)
    for cid in ("a", "b"):
        st.create_campaign(campaign_id=cid, spec={"k": cid}, declaration_sha256="d" * 64, declaration_path="/x", run_dir="/x",
                           evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=[{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}])
    p = run_worker(tmp_path, db, "crash")
    assert p.returncode == 9
    stuck = running_jobs(st)
    assert ("a", "trials") in [(j["campaign_id"], j["stage"]) for j in stuck]                      # the dead worker's claim is visible, not hidden
    trials_before = {c: st.campaign_trials(c) for c in ("a", "b")}
    import time
    assert st.recover_expired(now=time.time() + 100_000) == len(stuck)
    p2 = run_worker(tmp_path, db, "none")
    assert p2.returncode == 0 and p2.stdout.strip() == "NO_ELIGIBLE_WORK"
    for c in ("a", "b"):
        assert st.get_campaign(c)["state"] == "COMPLETED" and st.campaign_trials(c) == trials_before[c]
    t = [j for j in st.list_jobs("a") if j["stage"] == "trials"][0]
    assert [x["status"] for x in st.attempts(t["job_id"])] == ["interrupted", "succeeded"]
    for j in st.list_jobs("b"):                                      # b's only extra attempt, if any, is the truthful interruption of its in-flight stage
        assert [x["status"] for x in st.attempts(j["job_id"])] in (["succeeded"], ["interrupted", "succeeded"])


def test_an_exception_inside_a_worker_process_fails_only_that_job_and_exits_nonzero(tmp_path):
    db = tmp_path / "y.sqlite3"
    st = FactoryStore(db)
    for cid in ("a", "b"):
        st.create_campaign(campaign_id=cid, spec={"k": cid}, declaration_sha256="d" * 64, declaration_path="/x", run_dir="/x",
                           evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=[{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}])
    p = run_worker(tmp_path, db, "raise")
    assert p.returncode == 4 and p.stdout.strip() == "FAILED"
    assert not running_jobs(st) and st.get_campaign("a")["state"] == "FAILED" and st.get_campaign("b")["state"] == "COMPLETED"
