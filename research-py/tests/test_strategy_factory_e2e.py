"""Integrated Strategy Factory acceptance: the REAL entrypoints, stage-authorized runner, native Rust engine, registry,
judge, scanner/review and report - on SYNTHETIC bars (labelled as such; this proves the software path, never market
readiness). Skipped when the native binary is absent, unless MQK_FACTORY_REQUIRE_NATIVE=1 (the strategy-factory CI lane), where absence fails.
No provider, broker, Paper database or reserved holdout window is touched.
"""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory.service import FactoryService
from mqk_research.strategy_factory.store import STAGES
from support import factory_e2e as E

pytestmark = E.native_marks()
REPO = E.REPO
SMA = {"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": [20, 50]}}
NATIVE_MOM = {"kind": "native", "strategy_ids": ["absolute_momentum_252"]}
GRAMMAR_MOM = {"kind": "grammar_grid", "template": "abs_momentum_sessions", "grid": {"lookback": [252]}}


class World:
    def __init__(self, root: Path):
        self.root = root
        self.bars = E.make_bars_dir(root)
        self.env = {**os.environ, "MQK_M1_STAGE_AUTH_KEY": E.TEST_KEY}          # the operator-held secret (synthetic here)
        self.svc = FactoryService(root / "factory", REPO, cli_path=E.DEFAULT_CLI, env=self.env)

    def campaign(self, cid, sources, authorize=True, **kw):
        spec = E.make_spec(cid, self.bars, sources=sources, **kw)
        c = self.svc.compile_campaign(spec)
        if authorize:        # operator stand-in: release the gate and place a signed authorization beside the campaign
            E.operator_release_and_authorize(Path(c["declaration_path"]), E.DEFAULT_CLI, in_run_dir=True)
        return c


@pytest.fixture(scope="module")
def world(tmp_path_factory):
    return World(tmp_path_factory.mktemp("e2e"))


def run_one(world, cid, sources, workers=1, **kw):
    """Compile, release, authorize and run ONE campaign to idle with no intermediate commands."""
    c = world.campaign(cid, sources, **kw)
    return c, world.svc.run(workers=workers)


def report_of(world, cid):
    return json.loads((world.root / "factory" / "campaigns" / cid / "factory_report" / "report.json").read_text(encoding="utf-8"))


def registry_rows(world, cid, sql, args=()):
    db = world.root / "factory" / "campaigns" / cid / "registry" / "research.sqlite3"
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        return con.execute(sql, args).fetchall()
    finally:
        con.close()


@pytest.fixture(scope="module")
def campaign_a(world):
    """E2E-01/02/07/08/10: grammar SMA grid + the native absolute-momentum engine + its grammar twin."""
    c, res = run_one(world, "FC-E2E-A", [SMA, NATIVE_MOM, GRAMMAR_MOM])
    return c, res, report_of(world, "FC-E2E-A")


# ------------------------------------------------------------------ E2E-01 / E2E-10
def test_e2e_01_and_10_unattended_campaign_runs_every_stage_on_the_real_engine(world, campaign_a):
    c, res, rep = campaign_a
    assert res.ended == "NO_ELIGIBLE_WORK" and res.by_status == {"succeeded": len(STAGES)}
    camp = world.svc.store.get_campaign("FC-E2E-A")
    assert camp["state"] == "COMPLETED"
    assert [j["status"] for j in world.svc.store.list_jobs("FC-E2E-A")] == ["succeeded"] * len(STAGES)
    assert rep["trial_execution"]["data"]["status_counts"] == {"EVALUATED": 8}
    assert rep["registry"]["data"]["registered_trials"] == 8 and rep["registry"]["data"]["attempts_by_status"] == {"succeeded": 8}
    for k in ("registry", "trial_execution", "economic_and_benchmark_results", "statistical_judge", "data_identity", "oos_and_holdout", "candidate_review"):
        assert rep[k]["truth_state"] == "PRESENT", k
    assert rep["campaign"]["evidence_grade"] == "SYNTHETIC_DIAGNOSTIC" and rep["campaign"]["promotion_eligible"] is False and rep["campaign"]["promotion_readiness"] == "NOT_ELIGIBLE_SYNTHETIC"
    assert all(a["sha256"] for a in rep["artifacts"].values())


def test_e2e_01_evidence_lineage_is_reproducible_from_the_frozen_declaration(world, campaign_a):
    c, _, rep = campaign_a
    again = world.svc.compile_campaign(json.loads((world.root / "factory" / "campaigns" / "FC-E2E-A" / "spec.json").read_text(encoding="utf-8")))
    assert again["created"] is False and again["declaration_sha256"] == c["declaration_sha256"] == rep["campaign"]["declaration_sha256"]
    rows = rep["economic_and_benchmark_results"]["data"]
    assert len(rows) == 8 and all(r["semantic_fingerprint"] and r["trial_id"] and r["economic_eval_id"] for r in rows)
    assert len({r["trial_id"] for r in rows}) == 8


# ------------------------------------------------------------------ E2E-02
def test_e2e_02_losing_candidates_are_honestly_rejected_and_nothing_is_promoted(world, campaign_a):
    _, _, rep = campaign_a
    states = rep["candidate_review"]["data"]
    assert "paper_candidate" not in states and sum(states.values()) == 8 and states.get("rejected", 0) >= 1
    run_dir = world.root / "factory" / "campaigns" / "FC-E2E-A"
    assert not [p for p in run_dir.rglob("*") if p.is_file() and "promotion" in p.name.lower()]
    assert rep["authority"]["promotion"].startswith("NOT_REQUESTED_BY_FACTORY") and rep["authority"]["paper"] == "NOT_TOUCHED_BY_FACTORY"


# ------------------------------------------------------------------ E2E-07
def test_e2e_07_python_rust_parity_native_engine_vs_its_grammar_twin_and_declared_fingerprints(world, campaign_a):
    _, _, rep = campaign_a
    rows = {(r["strategy"], r["symbol"]): r for r in rep["economic_and_benchmark_results"]["data"]}
    for sym in world.bars["symbols"]:
        native = rows[("absolute_momentum_252", sym)]
        twin = rows[("grammar_v1__abs_momentum_sessions__lookback_252", sym)]
        for k in ("net_return", "gross_return", "rust_total_return_pct", "rust_trade_count", "rust_exposure_time_pct", "turnover"):
            assert native[k] == twin[k], (sym, k, native[k], twin[k])             # same rule, two implementations, same economics
        assert native["semantic_fingerprint"] != twin["semantic_fingerprint"]   # distinct identities, never confused
    for (strategy, sym), r in rows.items():                                       # the fingerprint evaluated is the one the CLI declares
        out = subprocess.run([str(E.DEFAULT_CLI), "backtest", "native-fingerprint", "--strategy", strategy, "--symbol", sym,
                              "--sizing-policy", "fixed_initial_capital_fraction_v1", "--allocation-fraction-bps", "1000",
                              "--initial-cash-micros", "100000000000", "--max-position-notional-usd", "50000"], capture_output=True, text=True)
        assert f"semantic_fingerprint={r['semantic_fingerprint']}" in out.stdout, (strategy, sym, out.stdout[-200:])
    twin_h = next(s for s in rep["idea_sources_and_hypotheses"]["data"] if s["strategy_name"].endswith("lookback_252"))
    assert twin_h["required_history_bars"] == 253 and twin_h["relationship_to_known"]["relationship"] == "EXACT_DUPLICATE"


# ------------------------------------------------------------------ E2E-08
def test_e2e_08_full_population_judged_with_preserved_denominator(world, campaign_a):
    _, _, rep = campaign_a
    j = rep["statistical_judge"]["data"]
    assert j["judge_status"] == "evaluated" and j["registry_population"]["registered_unique_trials"] == 8
    assert j["included"] + len(j["excluded"]) == 8 and j["dsr_trial_accounting"]
    assert rep["registry"]["data"]["judge_artifacts"] and rep["population_declaration"]["data"]["trial_count"] == 8


# ------------------------------------------------------------------ E2E-11
def test_e2e_11_scheduled_pass_with_no_eligible_work_does_nothing_then_runs_only_new_work(world, campaign_a):
    before = {j["job_id"]: j["attempt_count"] for j in world.svc.store.list_jobs("FC-E2E-A")}
    res = world.svc.run(workers=2)
    assert res.ended == "NO_ELIGIBLE_WORK" and res.jobs_run == 0
    assert {j["job_id"]: j["attempt_count"] for j in world.svc.store.list_jobs("FC-E2E-A")} == before
