"""Integrated Strategy Factory acceptance, part 2: unsupported ideas, whole-population accounting, concurrency across real
processes, kill/recovery, authority refusals and real-data readiness. Same real entrypoints and synthetic bars as part 1."""

from __future__ import annotations

import hashlib
import json
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory.store import STAGES
from support import factory_e2e as E
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row
from test_strategy_factory_e2e import SMA, World, registry_rows, report_of, run_one  # noqa: F401

pytestmark = E.native_marks()
REPO = E.REPO


@pytest.fixture(scope="module")
def world(tmp_path_factory):
    return World(tmp_path_factory.mktemp("e2e2"))


def sha(p: Path) -> str:
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


# ------------------------------------------------------------------ E2E-03
def test_e2e_03_unsupported_and_underspecified_ideas_are_dispositioned_never_executed(world):
    sheets = {"IDEAS": [HEADER, row("U-1", "Futures trend", rule="Hold the contract above its 50-day SMA", assets="Futures"),
                        row("U-2", "Headline reversal", rule="Buy after a bullish headline", data="News"),
                        row("U-3", "Fast/slow MA cross", rule="crossover"),
                        row("U-4", "Does value predict returns?", question="Do cheap stocks outperform?"),
                        row("U-5", "37-day SMA trend gate", rule="Hold above the 37-day SMA, else cash")],
              "VIEW": [["ID", "Note"], ["U-1", "x"]], "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    prof = ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": "e2e03"})
    path = world.root / "e2e03.xlsx"
    path.write_bytes(make_xlsx(sheets))
    world.svc.import_catalog(path, prof)
    out = world.svc.run_intake()
    assert out["result"]["unique_ideas"] == 5 and sum(out["result"]["disposition_counts"].values()) == 5
    ideas = {i["entry_id"]: i for i in world.svc.store.latest_ideas().values() if i["catalog_family"] == "e2e03"}
    assert [ideas[k]["disposition"] for k in ("U-1", "U-2", "U-3", "U-4", "U-5")] == [
        "DEFERRED_ASSET_CLASS", "UNSUPPORTED_DATA", "NEEDS_FORMALIZATION", "DIAGNOSTIC_NOT_STRATEGY", "ADMITTED_GRAMMAR"]
    assert all(ideas[k]["execution_path"] is None for k in ("U-1", "U-2", "U-3", "U-4"))
    for k in ("U-1", "U-2", "U-3", "U-4"):
        with pytest.raises(C.CampaignError):
            world.svc.compile_campaign(E.make_spec(f"FC-E2E-U-{k}", world.bars, sources=[{"kind": "admitted_ideas", "intake_ids": [ideas[k]["intake_id"]]}]))
    assert not (world.root / "factory" / "campaigns" / "FC-E2E-U-U-1").exists()          # nothing was registered or created
    ok = world.svc.compile_campaign(E.make_spec("FC-E2E-U-OK", world.bars, sources=[{"kind": "admitted_ideas", "intake_ids": [ideas["U-5"]["intake_id"]]}]))
    assert ok["trials"] == 2


# ------------------------------------------------------------------ E2E-04
def test_e2e_04_whole_declared_population_is_accounted_including_invalid_and_extreme_members(world):
    sources = [{"kind": "grammar_grid", "template": "dual_sma_cross", "grid": {"fast": [10, 60], "slow": [50, 120]}},
               {"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [30, 1000]}}]
    c, res = run_one(world, "FC-E2E-POP", sources)
    rep = report_of(world, "FC-E2E-POP")
    declared = rep["population_declaration"]["data"]
    assert [(e["params"]["fast"], e["params"]["slow"]) for e in declared["excluded_invalid_combinations"]] == [(60, 50)]     # visible exclusion
    n = declared["trial_count"]
    assert n == c["trials"] == 2 * (3 + 2)
    assert rep["registry"]["data"]["registered_trials"] == n
    assert sum(rep["registry"]["data"]["attempts_by_status"].values()) == n and not rep["registry"]["data"]["retried_trials"]
    assert sum(rep["trial_execution"]["data"]["status_counts"].values()) == n
    j = rep["statistical_judge"]["data"]
    assert j["registry_population"]["registered_unique_trials"] == n and j["included"] + len(j["excluded"]) == n
    keys = [t["trial_key"] for t in rep["trial_execution"]["data"]["trials"]]
    assert len(keys) == len(set(keys)) == n
    ids = [r[0] for r in registry_rows(world, "FC-E2E-POP", "select trial_id from research_trials")]
    assert len(ids) == len(set(ids)) == n


# ------------------------------------------------------------------ E2E-05
def test_e2e_05_concurrent_campaigns_and_real_worker_processes_never_cross_contaminate(tmp_path):
    world = World(tmp_path)                                    # a store of its own: only these three campaigns exist
    cids = ("FC-E2E-P1", "FC-E2E-P2", "FC-E2E-P3")
    for cid in cids:
        world.campaign(cid, [SMA])
    env = {**world.env, "PYTHONPATH": str(REPO / "research-py" / "src"), "PYTHONIOENCODING": "utf-8"}
    procs = [subprocess.Popen([sys.executable, "-m", "mqk_research.strategy_factory", "--root", str(world.root / "factory"), "--cli", str(E.DEFAULT_CLI),
                               "run", "--workers", "2"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(2)]
    outs = [p.communicate(timeout=3000) for p in procs]
    assert all(p.returncode == 0 for p in procs), outs
    st = world.svc.store
    jobs = [j for c in cids for j in st.list_jobs(c)]
    assert len(jobs) == 3 * len(STAGES) and all(j["status"] == "succeeded" and j["attempt_count"] == 1 for j in jobs)     # exactly once each
    assert all(len(st.attempts(j["job_id"])) == 1 for j in jobs)
    spans = {c: [(a["started_at"], a["finished_at"]) for j in st.list_jobs(c) for a in st.attempts(j["job_id"])] for c in cids}
    overlap = any(a0 < b1 and b0 < a1 for c1 in cids for c2 in cids if c1 < c2 for a0, a1 in spans[c1] for b0, b1 in spans[c2])
    assert overlap, "independent campaigns should have overlapped in time"
    reps = {c: report_of(world, c) for c in cids}
    tables = {c: {(r["strategy"], r["symbol"]): (r["net_return"], r["rust_total_return_pct"], r["rust_trade_count"], r["sharpe"])
                  for r in reps[c]["economic_and_benchmark_results"]["data"]} for c in cids}
    assert tables[cids[0]] == tables[cids[1]] == tables[cids[2]] and len(tables[cids[0]]) == 4                 # deterministic, no leakage
    tid = {c: {r["trial_id"] for r in reps[c]["economic_and_benchmark_results"]["data"]} for c in cids}
    assert not (tid[cids[0]] & tid[cids[1]]) and not (tid[cids[1]] & tid[cids[2]]) and not (tid[cids[0]] & tid[cids[2]])
    dbs = {str(world.root / "factory" / "campaigns" / c / "registry" / "research.sqlite3") for c in cids}
    assert len(dbs) == 3 and all(Path(d).is_file() for d in dbs)


# ------------------------------------------------------------------ E2E-06
def test_e2e_06_killed_mid_trials_recovers_truthfully_without_rewriting_prior_evidence(tmp_path):
    world = World(tmp_path)
    cid = "FC-E2E-KILL"
    world.campaign(cid, [SMA])
    run_dir = world.root / "factory" / "campaigns" / cid
    env = {**world.env, "PYTHONPATH": str(REPO / "research-py" / "src"), "PYTHONIOENCODING": "utf-8"}
    proc = subprocess.Popen([sys.executable, "-m", "mqk_research.strategy_factory", "--root", str(world.root / "factory"), "--cli", str(E.DEFAULT_CLI),
                             "run", "--workers", "1"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **E.popen_group())
    deadline, durable = time.time() + 900, None
    reg = run_dir / "registry" / "research.sqlite3"
    while time.time() < deadline and durable is None:
        running = [j for j in world.svc.store.list_jobs(cid) if j["stage"] == "trials" and j["status"] == "running"]
        if running and reg.is_file():
            con = sqlite3.connect(f"file:{reg}?mode=ro", uri=True)
            try:
                rows = con.execute("select artifact_paths_json from research_attempts where status='succeeded'").fetchall()
            except sqlite3.Error:
                rows = []
            finally:
                con.close()
            if rows and len(rows) < 4:                                  # at least one trial is durably complete, not all
                durable = rows
        time.sleep(0.05)
    assert durable is not None, "the trials stage never had a durably completed trial mid-stage"
    # Only DURABLY completed trials (registry says succeeded) carry the immutability guarantee: a trial whose attempt had
    # not reached a terminal state is legitimately re-run, and its same-path partial artifacts are replaced.
    before = {path: sha(Path(path)) for row in durable for path in json.loads(row[0]).values() if Path(path).is_file()}
    assert before
    E.kill_tree(proc)
    proc.wait(timeout=60)
    st = world.svc.store
    assert [j for j in st.list_jobs(cid) if j["stage"] == "trials"][0]["status"] == "running"      # a dead worker still holds the lease
    assert st.recover_expired(now=time.time() + 100_000) == 1
    res = world.svc.run(workers=1)
    assert res.ended == "NO_ELIGIBLE_WORK" and st.get_campaign(cid)["state"] == "COMPLETED"
    t = [j for j in st.list_jobs(cid) if j["stage"] == "trials"][0]
    assert [a["status"] for a in st.attempts(t["job_id"])] == ["interrupted", "succeeded"]
    rep = report_of(world, cid)
    assert rep["registry"]["data"]["registered_trials"] == 4 and rep["registry"]["data"]["attempts_by_status"].get("succeeded") == 4
    succ = registry_rows(world, cid, "select trial_id, count(*) from research_attempts where status='succeeded' group by trial_id")
    assert len(succ) == 4 and all(n == 1 for _, n in succ)                                        # no duplicated economic outcome
    for path, digest in before.items():
        assert sha(Path(path)) == digest                                                           # prior evidence is immutable
    assert rep["trial_execution"]["data"]["status_counts"] == {"EVALUATED": 4}
    orphan = registry_rows(world, cid, "select count(*) from research_attempts where status='started'")
    assert orphan == [(0,)]                                                                        # an orphaned 'started' attempt is never left


# ------------------------------------------------------------------ E2E-09
def test_e2e_09_an_unreleased_gate_blocks_every_stage_before_any_data_or_registry_access(world):
    world.campaign("FC-E2E-NOGATE", [SMA], authorize=False)
    res = world.svc.run(workers=1)
    assert res.ended == "BLOCKED"
    first_blocked = next(j for j in world.svc.store.list_jobs("FC-E2E-NOGATE") if j["status"] == "blocked")
    assert first_blocked["stage"] == "check" and "BLOCKED_GATE" in first_blocked["last_reason"]
    run_dir = world.root / "factory" / "campaigns" / "FC-E2E-NOGATE"
    assert not (run_dir / "registry").exists() and not (run_dir / "data").exists()


def test_e2e_09_a_released_gate_without_a_signed_authorization_still_runs_nothing_effectful(world):
    world.campaign("FC-E2E-GATEONLY", [SMA], authorize=False)
    world.svc.release_gate("FC-E2E-GATEONLY", operator="op", approval_ref="REF-1")
    res = world.svc.run(workers=1)
    assert res.ended == "BLOCKED"
    jobs = {j["stage"]: j for j in world.svc.store.list_jobs("FC-E2E-GATEONLY")}
    assert jobs["check"]["status"] == "succeeded" and jobs["data"]["status"] == "blocked" and "BLOCKED_AUTHORIZATION" in jobs["data"]["last_reason"]
    run_dir = world.root / "factory" / "campaigns" / "FC-E2E-GATEONLY"
    assert not (run_dir / "registry").exists() and not (run_dir / "data").exists()


def test_e2e_09_bars_reaching_the_reserved_holdout_are_refused_before_any_attempt(world):
    breach = E.make_bars_dir(world.root, end="2021-12-31", name="bars_breach")
    c = world.svc.compile_campaign(E.make_spec("FC-E2E-BREACH", breach, sources=[SMA]))
    E.operator_release_and_authorize(Path(c["declaration_path"]), E.DEFAULT_CLI, in_run_dir=True)
    res = world.svc.run(workers=1)
    assert res.ended == "FAILED"
    failed = [j for j in world.svc.store.list_jobs("FC-E2E-BREACH") if j["status"] == "failed"]
    assert [j["stage"] for j in failed] == ["holdout_pre"]
    assert registry_rows(world, "FC-E2E-BREACH", "select count(*) from research_attempts") == [(0,)]
    assert registry_rows(world, "FC-E2E-BREACH", "select count(*) from research_holdout_ledger") == [(0,)]


def test_e2e_09_synthetic_data_can_never_be_graded_as_market_evidence(world):
    with pytest.raises(C.CampaignError, match="inconsistent"):
        world.svc.compile_campaign(E.make_spec("FC-E2E-MASQ", world.bars, sources=[SMA], grade="EXPOSED_DEVELOPMENT"))


# ------------------------------------------------------------------ E2E-12
REAL_RUN = Path(r"C:\Users\Zacha\Desktop\MiniQuantDeskV4\research-py\experiments\m1_native_trend_campaign\runs\run_batch_03")


@pytest.mark.skipif(not (REAL_RUN / "data" / "research_bars_provenance.json").is_file(), reason="no verified local historical bars on this machine")
def test_e2e_12_real_data_readiness_explains_exactly_why_a_real_campaign_cannot_start(tmp_path):
    world = World(tmp_path)                                    # own store: earlier scenarios leave FAILED/BLOCKED campaigns in the shared one
    man = json.loads((REAL_RUN / "data" / "research_bars_provenance.json").read_text(encoding="utf-8"))
    data_dir = REAL_RUN / "data"
    before = {p.name: sha(p) for p in data_dir.iterdir() if p.is_file()}
    spec = {"schema": C.SPEC_SCHEMA, "campaign_id": "FC-REAL-READINESS-01", "evidence_grade": "EXPOSED_DEVELOPMENT", "protocol_profile": "m1_batch03_v1",
            "predeclared_utc_date": "2026-10-10",
            "population": {"sources": [{"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [90, 110]}}],
                           "symbols": sorted(man["symbol_universe"]), "max_trials": 60},
            "data": {"mode": "reuse", "reuse_from": str(data_dir).replace("\\", "/"), "feed": "sip", "adjustment": "all",
                     "start_utc": "2016-01-01T00:00:00Z", "end_utc": "2026-03-01T00:00:00Z", "asof": "2026-10-10",
                     "expected_artifact_sha256": man["artifact_sha256"], "expected_row_count": man["row_count"],
                     "expected_canonical_semantic_bars_hash": man["canonical_semantic_bars_hash"],
                     "expected_source_attestation_id": man["source_attestation_id"]},
            "partition": {"evaluation_start_utc": "2016-03-01T00:00:00Z", "test_months": 12, "holdout_months": 6, "expected_folds": 10,
                          "holdout_boundary": {"version": "fixed_holdout_boundary_v1", "holdout_start_utc": "2026-03-01T00:00:00Z",
                                               "holdout_end_utc": "2026-09-01T00:00:00Z"}}}
    world.svc.compile_campaign(spec)
    rep = world.svc.readiness("FC-REAL-READINESS-01")
    pre = rep["prerequisites"]
    assert rep["ready"] is False and pre["declaration"]["status"] == "OK" and pre["data_pins"]["status"] == "OK"
    assert pre["data_authority"]["status"] == "OK" and "official_provider" in pre["data_authority"]["detail"]          # real attested bars
    assert pre["execution_gate"]["status"] == "BLOCKED" and pre["authorization:registry_registration"]["status"] == "BLOCKED"
    assert "no valid stage authorization" in pre["authorization:attempt_execution"]["detail"]
    assert "ACCESS_INCIDENT_PENDING_ADJUDICATION" in pre["holdout_incidents"]["detail"]                                   # the open incident is surfaced
    res = world.svc.run(workers=1)
    assert res.ended == "BLOCKED"
    rd = world.root / "factory" / "campaigns" / "FC-REAL-READINESS-01"
    assert not (rd / "registry").exists() and not (rd / "data").exists()
    assert {p.name: sha(p) for p in data_dir.iterdir() if p.is_file()} == before                                       # real data untouched
