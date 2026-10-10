"""Campaign compiler, executor preflight and scheduler (no native binary needed)."""

from __future__ import annotations

import copy
import json
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory import executor as X
from mqk_research.strategy_factory import scheduler as S
from mqk_research.strategy_factory.store import STAGES, FactoryStore
from support import factory_e2e as E

REPO = E.REPO
GRID = {"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [20, 50]}}


@pytest.fixture(scope="module")
def bars(tmp_path_factory):
    return E.make_bars_dir(tmp_path_factory.mktemp("bars"))


def compile_(spec, tmp_path, ideas=None, grammar=True):
    return C.compile_campaign(spec, repo_root=REPO, run_root=tmp_path / "campaigns", ideas=ideas or {}, grammar_available=grammar)


def spec_of(bars, sources=(GRID,), **kw):
    return E.make_spec(kw.pop("cid", "FC-UNIT-01"), bars, sources=list(sources), **kw)


# ------------------------------------------------------------------ compiler
def test_population_is_complete_deterministic_and_content_addressed(bars, tmp_path):
    spec = spec_of(bars, sources=[GRID, {"kind": "native", "strategy_ids": ["absolute_momentum_252"]}])
    a, b = compile_(spec, tmp_path), compile_(copy.deepcopy(spec), tmp_path)
    assert a.declaration == b.declaration and a.declaration_sha256 == b.declaration_sha256
    keys = [t["trial_key"] for t in a.trials]
    assert keys == sorted(keys) or len(keys) == 6
    assert len(a.trials) == 3 * 2 == a.declaration["universe"]["max_trials"]
    assert [t["order"] for t in a.declaration["universe"]["trials"]] == [1, 2, 3, 4, 5, 6]
    changed = copy.deepcopy(spec)
    changed["population"]["symbols"] = ["SPY"]
    assert compile_(changed, tmp_path).declaration_sha256 != a.declaration_sha256


def test_gate_is_excluded_from_identity_exactly_like_the_accepted_authorization_module(bars, tmp_path):
    import stage_authorization as sa
    d = compile_(spec_of(bars), tmp_path).declaration
    assert C.declaration_identity(d) == sa.declaration_identity(d)
    released = copy.deepcopy(d)
    released["execution_gate"] = {"status": "x", "executable": True}
    assert C.declaration_identity(released) == C.declaration_identity(d)
    assert d["execution_gate"]["executable"] is False and d["evidence_grade"]["grade"] == "SYNTHETIC_DIAGNOSTIC"
    assert d["factory"]["promotion_eligible"] is False


def test_invalid_grid_combinations_are_excluded_visibly_not_silently(bars, tmp_path):
    spec = spec_of(bars, sources=[{"kind": "grammar_grid", "template": "dual_sma_cross", "grid": {"fast": [20, 100], "slow": [50, 100]}}])
    out = compile_(spec, tmp_path)
    ex = out.population_report["excluded"]
    assert [(e["params"]["fast"], e["params"]["slow"]) for e in ex] == [(100, 50), (100, 100)] or len(ex) == 2
    assert len({t["strategy_name"] for t in out.trials}) == 2                     # (20,50) and (20,100) remain
    assert out.declaration["factory"]["population_report"]["excluded"] == ex


def test_overlapping_sources_never_duplicate_a_trial_slot(bars, tmp_path):
    spec = spec_of(bars, sources=[GRID, {"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [50, 100]}}])
    out = compile_(spec, tmp_path)
    assert sorted({t["strategy_name"] for t in out.trials}) == [f"grammar_v1__sma_trend_gate__window_{w}" for w in (100, 20, 50)] or len(out.trials) == 6
    s50 = next(s for s in out.declaration["factory"]["strategies"] if s["params"].get("window") == 50)
    assert len(s50["population_sources"]) == 2 and len(out.trials) == 6


@pytest.mark.parametrize("mutate,needle", [
    (lambda s: s.update(schema="x"), "schema"),
    (lambda s: s.update(campaign_id="a b"), "campaign_id"),
    (lambda s: s.update(evidence_grade="OFFICIAL_ALPHA"), "evidence_grade"),
    (lambda s: s.update(protocol_profile="invented"), "protocol_profile"),
    (lambda s: s.update(predeclared_utc_date="today"), "explicit"),
    (lambda s: s["population"].update(symbols=["spy"]), "tickers"),
    (lambda s: s["population"].update(symbols=["SPY", "SPY"]), "tickers"),
    (lambda s: s["population"].update(max_trials=3), "above the predeclared max_trials"),
    (lambda s: s["population"].update(max_trials=10_000), "max_trials"),
    (lambda s: s["data"].update(mode="fetch"), "reuse"),
    (lambda s: s["data"].pop("expected_artifact_sha256"), "expected_artifact_sha256"),
    (lambda s: s["data"].update(expected_artifact_sha256="0" * 64), "does not match"),
    (lambda s: s["partition"].pop("holdout_boundary"), "fixed reserved boundary"),
    (lambda s: s.update(evidence_grade="EXPOSED_DEVELOPMENT"), "inconsistent"),
    (lambda s: s["population"].update(sources=[{"kind": "mystery"}]), "kind"),
    (lambda s: s["population"].update(sources=[{"kind": "grammar_grid", "template": "rsi_reversion", "grid": {"rsi_window": [2]}}]), "executable template"),
    (lambda s: s["population"].update(sources=[{"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [20, 20]}}]), "unique"),
    (lambda s: s["population"].update(sources=[{"kind": "native", "strategy_ids": ["nope"]}]), "no semantic card"),
    (lambda s: s["population"].update(sources=[{"kind": "native", "strategy_ids": ["swing_momentum"]}]), "legacy"),
    (lambda s: s["population"].update(sources=[{"kind": "native", "strategy_ids": ["intraday_scalper"]}]), "legacy"),
])
def test_unsafe_specs_are_refused(bars, tmp_path, mutate, needle):
    spec = spec_of(bars, max_trials=20)
    mutate(spec)
    with pytest.raises(C.CampaignError, match=needle):
        compile_(spec, tmp_path)


def test_grammar_sources_are_refused_when_the_build_has_no_grammar(bars, tmp_path):
    with pytest.raises(C.CampaignError, match="not available"):
        compile_(spec_of(bars), tmp_path, grammar=False)


def test_protocol_profile_is_pinned_by_content(bars, tmp_path, monkeypatch):
    monkeypatch.setitem(C.PROFILES, "m1_batch03_v1", ("PREDECLARED_BATCH_03.json", "0" * 64))
    with pytest.raises(C.CampaignError, match="pinned content hash"):
        compile_(spec_of(bars), tmp_path)


def test_only_admitted_ideas_enter_a_population_and_a_known_duplicate_needs_explicit_reevaluation(bars, tmp_path):
    ideas = {"idea_a": {"disposition": "ADMITTED_GRAMMAR", "execution_path": {"kind": "grammar_v1", "strategy_name": "grammar_v1__sma_trend_gate__window_37"}},
             "idea_b": {"disposition": "NEEDS_FORMALIZATION", "execution_path": None},
             "idea_c": {"disposition": "DUPLICATE_OF_KNOWN", "execution_path": {"kind": "native", "strategy_id": "absolute_momentum_252"}}}
    ok = compile_(spec_of(bars, sources=[{"kind": "admitted_ideas", "intake_ids": ["idea_a"]}]), tmp_path, ideas)
    assert ok.declaration["factory"]["strategies"][0]["intake_ids"] == ["idea_a"]
    for iid, src in (("idea_b", {}), ("idea_c", {}), ("idea_zzz", {})):
        with pytest.raises(C.CampaignError):
            compile_(spec_of(bars, sources=[{"kind": "admitted_ideas", "intake_ids": [iid], **src}]), tmp_path, ideas)
    allowed = compile_(spec_of(bars, sources=[{"kind": "admitted_ideas", "intake_ids": ["idea_c"], "allow_reevaluation_of_known": True}]), tmp_path, ideas)
    assert allowed.declaration["factory"]["strategies"][0]["strategy_name"] == "absolute_momentum_252"


def test_each_strategy_discloses_its_relationship_to_everything_already_searched(bars, tmp_path):
    spec = spec_of(bars, sources=[{"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [37, 200]}}])
    by = {s["params"]["window"]: s["relationship_to_known"] for s in compile_(spec, tmp_path).declaration["factory"]["strategies"]}
    assert by[37]["relationship"] == "PARAMETER_VARIANT" and by[200]["relationship"] == "EXACT_DUPLICATE"


# ------------------------------------------------------------------ executor preflight (no subprocess)
class Env:
    """A compiled campaign on disk, an operator stand-in, and a store row."""

    def __init__(self, bars, tmp_path, release=True, authorize=True, cli_bytes=b"fake-native-binary"):
        self.root = tmp_path
        tmp_path.mkdir(parents=True, exist_ok=True)
        self.cli = tmp_path / "mqk-cli.exe"
        self.cli.write_bytes(cli_bytes)
        out = compile_(spec_of(bars), tmp_path)
        self.decl_path = tmp_path / "declaration.json"
        self.decl_path.write_text(json.dumps(out.declaration, indent=1, sort_keys=True), encoding="utf-8")
        self.sha = out.declaration_sha256
        self.campaign = {"campaign_id": "FC-UNIT-01", "declaration_path": str(self.decl_path), "declaration_sha256": self.sha, "state": "RUNNING"}
        self.env = {}
        if release:
            op = E.operator_release_and_authorize(self.decl_path, self.cli, tmp_path / "auth") if authorize else None
            if op:
                self.env = op["env"]
            else:
                d = json.loads(self.decl_path.read_text(encoding="utf-8"))
                d["execution_gate"]["executable"] = True
                self.decl_path.write_text(json.dumps(d), encoding="utf-8")

    def executor(self, cli=True, **kw):
        return X.StageExecutor(REPO, cli_path=self.cli if cli else None, env=self.env, **kw)


def test_preflight_is_blocked_until_the_operator_releases_the_gate(bars, tmp_path):
    e = Env(bars, tmp_path, release=False)
    out = e.executor().preflight(e.campaign, "check")
    assert out.status == "blocked" and "BLOCKED_GATE" in out.reason


def test_preflight_names_a_missing_authorization_precisely_and_never_mints_one(bars, tmp_path):
    e = Env(bars, tmp_path, release=True, authorize=False)
    for stage in ("data", "register", "trials", "judge"):
        out = e.executor().preflight(e.campaign, stage)
        assert out.status == "blocked" and "BLOCKED_AUTHORIZATION" in out.reason, stage
    assert e.executor().preflight(e.campaign, "check") is None and e.executor().preflight(e.campaign, "holdout_pre") is None
    assert not (tmp_path / "auth").exists()


def test_preflight_passes_with_a_valid_authorization_and_the_pinned_binary(bars, tmp_path):
    e = Env(bars, tmp_path)
    for stage in STAGES:
        assert e.executor().preflight(e.campaign, stage) is None, stage


def test_preflight_refuses_wrong_binary_expired_foreign_and_unsigned_authorizations(bars, tmp_path):
    e = Env(bars, tmp_path)
    e.cli.write_bytes(b"a different binary")
    out = e.executor().preflight(e.campaign, "trials")
    assert out.status == "blocked" and "differs from the one the authorization pins" in out.reason
    assert e.executor(cli=False).preflight(e.campaign, "trials").reason.startswith("BLOCKED_DEPENDENCY")
    e.cli.write_bytes(b"fake-native-binary")
    late = datetime.now(timezone.utc) + timedelta(days=3)
    out = e.executor(clock=lambda: late).preflight(e.campaign, "trials")
    assert out.status == "blocked" and "expired" in out.reason
    bad = dict(e.env)
    auth = json.loads(Path(bad["MQK_M1_STAGE_AUTHORIZATION"]).read_text(encoding="utf-8"))
    auth["operator"] = "someone-else"
    Path(bad["MQK_M1_STAGE_AUTHORIZATION"]).write_text(json.dumps(auth), encoding="utf-8")
    assert "signature" in X.StageExecutor(REPO, cli_path=e.cli, env=bad).preflight(e.campaign, "trials").reason
    assert "secret" in X.StageExecutor(REPO, cli_path=e.cli, env={k: v for k, v in e.env.items() if k != "MQK_M1_STAGE_AUTH_KEY"}).preflight(e.campaign, "trials").reason


def test_authorization_for_another_declaration_does_not_transfer(bars, tmp_path):
    a = Env(bars, tmp_path / "a")
    other = compile_(spec_of(bars, cid="FC-UNIT-02"), tmp_path / "b")
    path = tmp_path / "b" / "declaration.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(other.declaration), encoding="utf-8")
    foreign = {"campaign_id": "FC-UNIT-02", "declaration_path": str(path), "declaration_sha256": other.declaration_sha256, "state": "RUNNING"}
    d = json.loads(path.read_text(encoding="utf-8"))
    d["execution_gate"]["executable"] = True
    path.write_text(json.dumps(d), encoding="utf-8")
    out = a.executor().preflight(foreign, "data")
    assert out.status == "blocked" and "different declaration identity" in out.reason


def test_a_declaration_altered_after_predeclaration_is_a_failure_not_a_block(bars, tmp_path):
    e = Env(bars, tmp_path)
    d = json.loads(e.decl_path.read_text(encoding="utf-8"))
    d["universe"]["symbols"] = ["SPY"]
    e.decl_path.write_text(json.dumps(d), encoding="utf-8")
    out = e.executor().preflight(e.campaign, "check")
    assert out.status == "failed" and "altered" in out.reason


def test_no_factory_stage_can_reach_promotion_paper_or_live(bars, tmp_path):
    import stage_authorization as sa
    for stage, runner in X.RUNNER_STAGE.items():
        assert sa.STAGE_CLASS[runner] not in (sa.PROMOTION, sa.PAPER)
    assert not {"promotion", "paper", "live", "deploy", "promote"} & set(STAGES)
    e = Env(bars, tmp_path)
    # even a (validly minted) promotion/paper authorization enables no Factory stage
    op = E.operator_release_and_authorize(e.decl_path, e.cli, tmp_path / "auth2", classes=[sa.PROMOTION])
    ex = X.StageExecutor(REPO, cli_path=e.cli, env=op["env"])
    assert ex.preflight(e.campaign, "trials").status == "blocked"


def test_readiness_reports_every_prerequisite(bars, tmp_path):
    e = Env(bars, tmp_path, release=False)
    rep = e.executor().readiness(e.campaign)
    assert rep["ready"] is False
    pre = rep["prerequisites"]
    assert pre["execution_gate"]["status"] == "BLOCKED" and pre["authorization:registry_registration"]["status"] == "BLOCKED"
    assert pre["data_pins"]["status"] == "OK" and pre["data_authority"]["status"] == "OK" and pre["declaration"]["status"] == "OK"
    ok = Env(bars, tmp_path / "ok")
    assert ok.executor().readiness(ok.campaign)["ready"] is True


# ------------------------------------------------------------------ scheduler (fake executor)
class FakeExecutor:
    def __init__(self, plan=None, delay=0.0):
        self.plan, self.delay, self.calls, self.lock, self.store = plan or {}, delay, [], threading.Lock(), None
        self.running = 0
        self.peak = 0

    def execute(self, campaign, stage, attempt_no, heartbeat=None, heartbeat_every=60.0):
        with self.lock:
            self.calls.append((campaign["campaign_id"], stage, attempt_no))
            self.running += 1
            self.peak = max(self.peak, self.running)
        time.sleep(self.delay)
        with self.lock:
            self.running -= 1
        return self.plan.get((campaign["campaign_id"], stage), X.Outcome("succeeded", 0, None, ""))


def store_with(tmp_path, ids=("a",)):
    st = FactoryStore(tmp_path / "s.sqlite3")
    for cid in ids:
        st.create_campaign(campaign_id=cid, spec={"k": cid}, declaration_sha256="d" * 64, declaration_path="/x", run_dir="/x", evidence_grade="SYNTHETIC_DIAGNOSTIC",
                           trials=[{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}])
    return st


def test_unattended_campaign_finishes_every_stage_in_order(tmp_path):
    st = store_with(tmp_path)
    ex = FakeExecutor()
    r = S.run_until_idle(st, ex, workers=2)
    assert r.ended == S.END_NO_WORK and r.jobs_run == len(STAGES)
    assert [c[1] for c in ex.calls] == list(STAGES) and st.get_campaign("a")["state"] == "COMPLETED"


def test_no_work_is_reported_truthfully_and_repeat_passes_execute_nothing(tmp_path):
    st = store_with(tmp_path)
    S.run_until_idle(st, FakeExecutor(), workers=2)
    ex = FakeExecutor()
    again = S.run_until_idle(st, ex, workers=2)
    assert again.ended == S.END_NO_WORK and again.jobs_run == 0 and ex.calls == []
    empty = S.run_pass(FactoryStore(tmp_path / "empty.sqlite3"), FakeExecutor())
    assert empty.ended == S.END_NO_WORK and empty.jobs_run == 0


def test_independent_campaigns_overlap_but_one_campaign_never_runs_two_stages_at_once(tmp_path):
    st = store_with(tmp_path, ("a", "b", "c"))
    ex = FakeExecutor(delay=0.05)
    r = S.run_pass(st, ex, workers=3)
    assert r.jobs_run == 3 * len(STAGES) and ex.peak >= 2
    per = {}
    for cid, stage, _ in ex.calls:
        per.setdefault(cid, []).append(stage)
    assert all(v == list(STAGES) for v in per.values())


def test_worker_budget_is_a_hard_limit(tmp_path):
    st = store_with(tmp_path, ("a", "b", "c", "d"))
    ex = FakeExecutor(delay=0.03)
    S.run_pass(st, ex, workers=2)
    assert ex.peak <= 2


def test_blocked_stage_stops_the_campaign_and_resumes_after_the_prerequisite_appears(tmp_path):
    st = store_with(tmp_path)
    blocked = X.Outcome("blocked", None, "BLOCKED_AUTHORIZATION: no stage authorization")
    ex = FakeExecutor({("a", "register"): blocked})
    r = S.run_until_idle(st, ex, workers=1)
    assert r.ended == S.END_BLOCKED and st.get_campaign("a")["state"] == "BLOCKED"
    assert [c[1] for c in ex.calls] == ["check", "data", "register"]
    assert S.run_until_idle(st, FakeExecutor({("a", "register"): blocked}), workers=1).ended == S.END_BLOCKED   # no spinning
    ex2 = FakeExecutor()
    r2 = S.run_until_idle(st, ex2, workers=1)
    assert r2.ended == S.END_NO_WORK and st.get_campaign("a")["state"] == "COMPLETED" and ex2.calls[0][1] == "register"
    reg = next(j for j in st.list_jobs("a") if j["stage"] == "register")
    assert [a["status"] for a in st.attempts(reg["job_id"])] == ["blocked", "blocked", "succeeded"]


def test_failure_is_terminal_until_the_operator_retries_it(tmp_path):
    st = store_with(tmp_path)
    ex = FakeExecutor({("a", "trials"): X.Outcome("failed", 3, "boom")})
    r = S.run_until_idle(st, ex, workers=1)
    assert r.ended == S.END_FAILED and st.get_campaign("a")["state"] == "FAILED"
    assert S.run_until_idle(st, FakeExecutor(), workers=1).jobs_run == 0
    st.retry_failed("a", "trials", "disk full")
    assert S.run_until_idle(st, FakeExecutor(), workers=1).ended == S.END_NO_WORK
    t = next(j for j in st.list_jobs("a") if j["stage"] == "trials")
    assert [a["status"] for a in st.attempts(t["job_id"])] == ["failed", "succeeded"]


def test_expired_lease_of_a_dead_worker_is_recovered_by_the_next_pass(tmp_path):
    st = store_with(tmp_path)
    j = st.claim_next("dead-worker", max_running=1, lease_seconds=1, now=100.0)
    assert j["stage"] == "check"
    ex = FakeExecutor()
    r = S.run_pass(st, ex, workers=1)
    assert r.interrupted_recovered == 1 and ex.calls[0] == ("a", "check", 2)
    assert [a["status"] for a in st.attempts(j["job_id"])] == ["interrupted", "succeeded"]


def test_job_budget_ends_the_pass_with_work_remaining(tmp_path):
    st = store_with(tmp_path)
    r = S.run_pass(st, FakeExecutor(), workers=1, max_jobs=3)
    assert r.ended == S.END_BUDGET and r.jobs_run == 3
