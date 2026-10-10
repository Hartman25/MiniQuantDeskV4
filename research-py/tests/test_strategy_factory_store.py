"""Control-plane store: immutability, atomic claims, lease recovery, retry/blocked semantics, real multi-process races."""

from __future__ import annotations

import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from mqk_research.strategy_factory.store import STAGES, FactoryStore, StoreError

SRC = str(Path(__file__).resolve().parents[1] / "src")
TRIALS = [{"trial_key": "s1/SPY", "strategy_name": "s1", "symbol": "SPY"}, {"trial_key": "s1/QQQ", "strategy_name": "s1", "symbol": "QQQ"}]


@pytest.fixture
def store(tmp_path):
    return FactoryStore(tmp_path / "f.sqlite3")


def campaign(store, cid="c1", spec=None, decl="d" * 64, trials=TRIALS, stages=STAGES):
    return store.create_campaign(campaign_id=cid, spec=spec or {"k": cid}, declaration_sha256=decl, declaration_path="/x/d.json",
                                 run_dir="/x/run", evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=trials, stages=stages)


def drain(store, worker="w", max_running=1, now=100.0, status="succeeded"):
    done = []
    while (job := store.claim_next(worker, max_running=max_running, now=now)) is not None:
        store.finish(job["job_id"], job["claim_token"], status=status, exit_code=0, reason=None, now=now)
        done.append((job["campaign_id"], job["stage"]))
    return done


def test_schema_is_idempotent_and_versioned(tmp_path):
    a = FactoryStore(tmp_path / "x.sqlite3")
    campaign(a)
    b = FactoryStore(tmp_path / "x.sqlite3")
    assert b.get_campaign("c1")["state"] == "PREDECLARED"
    con = b._connect()
    con.execute("update factory_meta set v='99' where k='schema_version'")
    con.close()
    with pytest.raises(StoreError, match="schema"):
        FactoryStore(tmp_path / "x.sqlite3")


def test_predeclaration_is_immutable_and_idempotent(store):
    assert campaign(store) is True and campaign(store) is False
    with pytest.raises(StoreError, match="immutable"):
        campaign(store, spec={"k": "other"})
    with pytest.raises(StoreError, match="immutable"):
        campaign(store, decl="e" * 64)
    with pytest.raises(StoreError, match="population differs"):
        campaign(store, trials=TRIALS[:1])
    with pytest.raises(StoreError, match="non-empty population"):
        campaign(store, cid="c2", trials=[])
    with pytest.raises(StoreError, match="unique"):
        campaign(store, cid="c3", trials=[TRIALS[0], TRIALS[0]])
    with pytest.raises(StoreError, match="unknown stage"):
        campaign(store, cid="c4", stages=("nope",))
    assert [t["trial_key"] for t in store.campaign_trials("c1")] == ["s1/QQQ", "s1/SPY"]
    assert [j["stage"] for j in store.list_jobs("c1")] == list(STAGES)


def test_stage_order_and_per_campaign_serialization(store):
    campaign(store)
    j1 = store.claim_next("w1", max_running=5, now=1.0)
    assert j1["stage"] == "check"
    assert store.claim_next("w2", max_running=5, now=1.0) is None          # nothing else of this campaign may run alongside
    store.finish(j1["job_id"], j1["claim_token"], status="succeeded", exit_code=0, reason=None, now=2.0)
    assert [s for _, s in drain(store, now=3.0)] == list(STAGES[1:])
    assert store.get_campaign("c1")["state"] == "COMPLETED"
    assert store.claim_next("w", max_running=5, now=4.0) is None            # no eligible work: terminate truthfully
    with pytest.raises(StoreError, match="only a failed job"):              # a succeeded job can never be re-run
        store.retry_failed("c1", "check", "again please")


def test_independent_campaigns_run_concurrently_up_to_the_global_limit(store):
    campaign(store, "a"), campaign(store, "b"), campaign(store, "c")
    j = [store.claim_next(f"w{i}", max_running=2, now=1.0) for i in range(3)]
    assert [x["campaign_id"] if x else None for x in j] == ["a", "b", None]


def test_finish_and_heartbeat_require_the_live_claim_token(store):
    campaign(store)
    job = store.claim_next("w", max_running=1, now=1.0)
    with pytest.raises(StoreError, match="claim is not held"):
        store.finish(job["job_id"], "forged", status="succeeded", exit_code=0, reason=None, now=2.0)
    with pytest.raises(StoreError, match="no longer held"):
        store.heartbeat(job["job_id"], "forged", now=2.0)
    store.heartbeat(job["job_id"], job["claim_token"], lease_seconds=500, now=2.0)
    store.finish(job["job_id"], job["claim_token"], status="succeeded", exit_code=0, reason=None, now=3.0)
    with pytest.raises(StoreError, match="claim is not held"):
        store.finish(job["job_id"], job["claim_token"], status="failed", exit_code=1, reason="late", now=4.0)
    with pytest.raises(StoreError, match="invalid terminal"):
        store.finish(job["job_id"], job["claim_token"], status="queued", exit_code=0, reason=None)


def test_expired_lease_is_recovered_truthfully_and_a_zombie_cannot_finish(store):
    campaign(store)
    job = store.claim_next("w1", max_running=1, lease_seconds=10, now=100.0)
    assert store.claim_next("w2", max_running=1, now=105.0) is None          # lease still held
    assert store.recover_expired(now=105.0) == 0
    assert store.recover_expired(now=111.0) == 1
    att = store.attempts(job["job_id"])
    assert [(a["attempt_no"], a["status"]) for a in att] == [(1, "interrupted")] and "lease expired" in att[0]["reason"]
    with pytest.raises(StoreError, match="claim is not held"):                # the zombie worker's late result is refused
        store.finish(job["job_id"], job["claim_token"], status="succeeded", exit_code=0, reason=None, now=112.0)
    again = store.claim_next("w2", max_running=1, now=112.0)
    assert again["job_id"] == job["job_id"] and again["attempt_count"] == 2
    assert [a["attempt_no"] for a in store.attempts(job["job_id"])] == [1, 2] and len(store.list_jobs("c1")) == len(STAGES)


def test_failure_stops_the_campaign_and_retry_appends_an_attempt(store):
    campaign(store)
    j = store.claim_next("w", max_running=1, now=1.0)
    store.finish(j["job_id"], j["claim_token"], status="failed", exit_code=2, reason="boom", output="tail", now=2.0)
    assert store.get_campaign("c1")["state"] == "FAILED" and store.claim_next("w", max_running=1, now=3.0) is None
    with pytest.raises(StoreError, match="needs a reason"):
        store.retry_failed("c1", "check", " ")
    with pytest.raises(StoreError, match="only a failed job"):
        store.retry_failed("c1", "data", "x")
    store.retry_failed("c1", "check", "disk was full")
    j2 = store.claim_next("w", max_running=1, now=4.0)
    assert j2["job_id"] == j["job_id"] and [a["status"] for a in store.attempts(j["job_id"])] == ["failed", "running"]
    assert store.attempts(j["job_id"])[0]["exit_code"] == 2 and store.attempts(j["job_id"])[0]["output_tail"] == "tail"


def test_blocked_is_not_failed_and_is_reevaluated_on_request(store):
    campaign(store)
    j = store.claim_next("w", max_running=1, now=1.0)
    store.finish(j["job_id"], j["claim_token"], status="blocked", exit_code=None, reason="no stage authorization", now=2.0)
    c = store.get_campaign("c1")
    assert c["state"] == "BLOCKED" and "blocked" in c["state_reason"]
    assert store.claim_next("w", max_running=1, now=3.0) is None
    with pytest.raises(StoreError, match="only a blocked job"):
        store.unblock("c1", "data", "x")
    assert store.requeue_blocked() == 1
    assert store.claim_next("w", max_running=1, now=4.0)["stage"] == "check"


def test_imports_ideas_and_decisions_are_append_only_and_idempotent(store):
    led = {"ledger_sha256": "a" * 64, "catalog_family": "f", "profile_id": "p", "source": {"filename": "x.xlsx", "sha256": "b" * 64},
           "counts": {"entries": 3}}
    assert store.record_import(led) is True and store.record_import(led) is False and len(store.list_imports()) == 1
    v1, v2 = {"intake_id": "idea_1", "x": 1}, {"intake_id": "idea_1", "x": 2}
    assert store.record_ideas([v1]) == 1 and store.record_ideas([v1]) == 0 and store.record_ideas([v2]) == 1
    assert store.latest_ideas()["idea_1"]["x"] == 2 and [h["x"] for h in store.idea_history("idea_1")] == [1, 2]
    d = {"intake_id": "idea_1", "param": "fast", "value": 3, "decided_by": "op", "rationale": "r", "decision_ref": "D1"}
    assert store.record_decision("parameter", d) is True and store.record_decision("parameter", d) is False
    with pytest.raises(StoreError, match="different content"):
        store.record_decision("parameter", {**d, "value": 4})
    with pytest.raises(StoreError, match="needs"):
        store.record_decision("parameter", {**d, "decision_ref": ""})
    with pytest.raises(StoreError, match="unknown decision kind"):
        store.record_decision("bogus", d)
    assert store.decisions("parameter") == [d]
    # refs that look like SQL wildcards are exact strings, never patterns
    assert store.record_decision("parameter", {**d, "decision_ref": "DA1", "value": 5}) is True
    assert store.record_decision("parameter", {**d, "decision_ref": "D_1", "value": 6}) is True
    assert store.record_decision("parameter", {**d, "decision_ref": "D%1", "value": 7}) is True
    with pytest.raises(StoreError, match="different content"):
        store.record_decision("parameter", {**d, "decision_ref": "D_1", "value": 8})
    n = {"intake_id": "idea_1", "status": "APPLIED", "provider": {"model": "m"}}
    assert store.record_normalization(n) is True and store.record_normalization(n) is False and store.normalizations("idea_1") == [n]
    store.cache_embedding("h", "nomic", [0.1, 0.2])
    store.cache_embedding("h", "nomic", [9, 9])
    assert store.cached_embedding("h", "nomic") == [0.1, 0.2] and store.cached_embedding("h", "other") is None


WORKER = textwrap.dedent("""
    import json, sys
    sys.path.insert(0, sys.argv[1])
    from mqk_research.strategy_factory.store import FactoryStore
    store = FactoryStore(sys.argv[2])
    claimed = []
    while True:
        job = store.claim_next(sys.argv[3], max_running=int(sys.argv[4]), lease_seconds=600)
        if job is None:
            break
        claimed.append([job["campaign_id"], job["stage"], job["attempt_count"]])
        store.finish(job["job_id"], job["claim_token"], status="succeeded", exit_code=0, reason=None)
    print(json.dumps(claimed))
""")


def test_real_processes_never_double_claim(tmp_path):
    db = tmp_path / "race.sqlite3"
    st = FactoryStore(db)
    for i in range(6):
        campaign(st, f"c{i}")
    script = tmp_path / "worker.py"
    script.write_text(WORKER, encoding="utf-8")
    procs = [subprocess.Popen([sys.executable, str(script), SRC, str(db), f"w{i}", "4"], stdout=subprocess.PIPE, text=True) for i in range(4)]
    claimed = [tuple(x) for p in procs for x in json.loads(p.communicate(timeout=300)[0])]
    assert len(claimed) == len(set(claimed)) == 6 * len(STAGES)               # every job exactly once, none twice
    assert all(c[2] == 1 for c in claimed)                                     # no job was retried
    assert all(c["state"] == "COMPLETED" for c in st.list_campaigns())
    for j in st.list_jobs():
        assert len(st.attempts(j["job_id"])) == 1


def test_snapshot_reports_states(store):
    campaign(store, "a")
    snap = store.snapshot()
    assert snap["campaigns"][0]["state"] == "PREDECLARED" and snap["job_counts"] == {"queued": len(STAGES)}
