"""Declaration / registry ownership: the registry transaction is the single ordering point for a campaign's declaration files.

Races are forced with real processes and explicit file handshakes (no sleeps decide an outcome): each worker stops at the
moment it is about to register, so the parent chooses exactly which compiler registers first.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory import service as svc_mod
from mqk_research.strategy_factory.contracts import sha
from mqk_research.strategy_factory.service import FactoryService
from mqk_research.strategy_factory.store import STAGES, FactoryStore, StoreError
from support import factory_e2e as E

REPO = E.REPO
SRC = str(REPO / "research-py" / "src")
TESTS = str(Path(__file__).parent)

WORKER = r'''
import json, os, sys, time
from pathlib import Path
src, tests, repo, root, spec_path, bdir, name, mode = sys.argv[1:9]
sys.path[:0] = [src, tests]
from mqk_research.strategy_factory.service import FactoryService
svc = FactoryService(Path(root), Path(repo), grammar_available=True)
real = svc.store.create_campaign
def gated(**kw):
    b = Path(bdir)
    (b / (name + ".ready")).write_text("1")
    deadline = time.time() + 120
    while not (b / (name + ".go")).exists():
        if time.time() > deadline:
            sys.exit(5)
        time.sleep(0.01)
    pub = kw.get("publish")
    if mode == "crash_in_publish" and pub is not None:
        orig = pub
        def pub2():
            orig()
            os._exit(9)
        kw["publish"] = pub2
    out = real(**kw)
    if mode == "crash_after_commit":
        os._exit(9)
    return out
svc.store.create_campaign = gated
try:
    res = {"ok": True, "result": svc.compile_campaign(json.loads(Path(spec_path).read_text(encoding="utf-8")))}
except Exception as exc:
    res = {"ok": False, "type": type(exc).__name__, "msg": str(exc)}
(Path(bdir) / (name + ".out")).write_text(json.dumps(res), encoding="utf-8")
'''


@pytest.fixture(scope="module")
def bars(tmp_path_factory):
    return E.make_bars_dir(tmp_path_factory.mktemp("dcbars"))


def grid(*windows):
    return [{"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": list(windows)}}]


def wait_for(path: Path, proc=None, timeout=180):
    end = time.time() + timeout
    while not path.exists():
        assert time.time() < end, f"timed out waiting for {path.name}"
        if proc is not None and proc.poll() is not None and not path.exists():
            return False
        time.sleep(0.01)
    return True


class Race:
    """Workers are started one at a time and stop just before registering; `release` lets one proceed to completion."""

    def __init__(self, tmp_path: Path):
        self.root, self.bdir = tmp_path / "f", tmp_path / "barrier"
        self.bdir.mkdir()
        self.script = tmp_path / "worker.py"
        self.script.write_text(WORKER, encoding="utf-8")
        self.procs: dict[str, subprocess.Popen] = {}
        self.specs = tmp_path

    def start(self, name: str, spec: dict, mode: str = "normal") -> None:
        spec_path = self.specs / f"{name}.spec.json"
        spec_path.write_text(json.dumps(spec), encoding="utf-8")
        self.procs[name] = subprocess.Popen([sys.executable, str(self.script), SRC, TESTS, str(REPO), str(self.root), str(spec_path), str(self.bdir), name, mode],
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        assert wait_for(self.bdir / f"{name}.ready", self.procs[name]), self.procs[name].communicate()

    def release(self, name: str) -> dict | None:
        (self.bdir / f"{name}.go").write_text("1")
        p = self.procs[name]
        p.wait(timeout=300)
        out = self.bdir / f"{name}.out"
        return json.loads(out.read_text(encoding="utf-8")) if out.exists() else None


def frozen(root: Path, cid: str):
    """(registry row, declaration on disk, identity of that file) of a campaign."""
    row = FactoryStore(root / "factory.sqlite3").find_campaign(cid)
    path = root / "campaigns" / cid / "declaration.json"
    decl = json.loads(path.read_text(encoding="utf-8"))
    return row, decl, C.declaration_identity(decl)


def test_two_processes_same_id_different_specs_the_loser_cannot_touch_the_winners_frozen_files(bars, tmp_path):
    r = Race(tmp_path)
    spec_a, spec_b = E.make_spec("FC-DC-X", bars, sources=grid(37)), E.make_spec("FC-DC-X", bars, sources=grid(38))
    r.start("a", spec_a)
    r.start("b", spec_b)                                   # b has now also found no registered campaign
    out_a = r.release("a")
    assert out_a["ok"] and out_a["result"]["created"] is True
    snapshot = {n: (r.root / "campaigns" / "FC-DC-X" / n).read_bytes() for n in ("declaration.json", "spec.json")}
    out_b = r.release("b")
    assert out_b == {"ok": False, "type": "StoreError", "msg": out_b["msg"]} and "different predeclaration" in out_b["msg"]
    row, decl, ident = frozen(r.root, "FC-DC-X")
    assert row["spec_sha256"] == sha(spec_a) and row["declaration_sha256"] == ident == out_a["result"]["declaration_sha256"]
    for n, before in snapshot.items():
        assert (r.root / "campaigns" / "FC-DC-X" / n).read_bytes() == before            # byte-identical to what the winner froze
    assert json.loads((r.root / "campaigns" / "FC-DC-X" / "spec.json").read_text(encoding="utf-8")) == spec_a


def test_two_processes_same_id_same_spec_are_idempotent_without_duplicate_population(bars, tmp_path):
    r = Race(tmp_path)
    spec = E.make_spec("FC-DC-X", bars, sources=grid(37, 41))
    r.start("a", spec)
    r.start("b", spec)
    out_a, out_b = r.release("a"), r.release("b")
    assert out_a["ok"] and out_b["ok"] and [out_a["result"]["created"], out_b["result"]["created"]] == [True, False]
    assert out_a["result"]["declaration_sha256"] == out_b["result"]["declaration_sha256"]
    st = FactoryStore(r.root / "factory.sqlite3")
    row, decl, ident = frozen(r.root, "FC-DC-X")
    assert ident == row["declaration_sha256"] and len(st.campaign_trials("FC-DC-X")) == 2 * len(spec["population"]["symbols"])
    assert len(st.list_jobs("FC-DC-X")) == len(STAGES)


def test_concurrent_different_ids_stay_independent_and_a_stale_history_loser_leaves_no_files_then_retries(bars, tmp_path):
    r = Race(tmp_path)
    r.start("a", E.make_spec("FC-DC-A", bars, sources=grid(37)))
    r.start("b", E.make_spec("FC-DC-B", bars, sources=grid(38)))
    out_a = r.release("a")
    a_bytes = (r.root / "campaigns" / "FC-DC-A" / "declaration.json").read_bytes()
    out_b = r.release("b")
    assert out_a["ok"] and not out_b["ok"] and "history changed" in out_b["msg"]
    assert not (r.root / "campaigns" / "FC-DC-B" / "declaration.json").exists()           # a refused compile writes nothing
    assert (r.root / "campaigns" / "FC-DC-A" / "declaration.json").read_bytes() == a_bytes
    retry = FactoryService(r.root, REPO, grammar_available=True).compile_campaign(E.make_spec("FC-DC-B", bars, sources=grid(38)))
    assert retry["created"] is True
    for cid in ("FC-DC-A", "FC-DC-B"):
        row, decl, ident = frozen(r.root, cid)
        assert ident == row["declaration_sha256"] and decl["batch_id"] == cid
    assert frozen(r.root, "FC-DC-B")[1]["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == ["FC-DC-A"]


@pytest.mark.parametrize("junk", ["{not json", "", "[]", '{"batch_id": "x"}'])
def test_an_unregistered_corrupt_orphan_declaration_is_replaced_without_a_false_registration(bars, tmp_path, junk):
    root = tmp_path / "f"
    cdir = root / "campaigns" / "FC-DC-O"
    cdir.mkdir(parents=True)
    (cdir / "declaration.json").write_text(junk, encoding="utf-8")
    (cdir / "spec.json").write_text("garbage", encoding="utf-8")
    svc = FactoryService(root, REPO, grammar_available=True)
    assert svc.store.find_campaign("FC-DC-O") is None                                       # the orphan alone registers nothing
    spec = E.make_spec("FC-DC-O", bars, sources=grid(37))
    out = svc.compile_campaign(spec)
    assert out["created"] is True
    row, decl, ident = frozen(root, "FC-DC-O")
    assert ident == row["declaration_sha256"] and json.loads((cdir / "spec.json").read_text(encoding="utf-8")) == spec


@pytest.mark.parametrize("damage", ["corrupt", "tampered", "missing"])
def test_a_registered_damaged_declaration_fails_closed_and_is_never_silently_rewritten(bars, tmp_path, damage):
    svc = FactoryService(tmp_path / "f", REPO, grammar_available=True)
    spec = E.make_spec("FC-DC-R", bars, sources=grid(37))
    path = Path(svc.compile_campaign(spec)["declaration_path"])
    if damage == "corrupt":
        path.write_text("{not json", encoding="utf-8")
    elif damage == "tampered":
        d = json.loads(path.read_text(encoding="utf-8"))
        d["universe"]["symbols"] = ["TAMPERED"]
        path.write_text(json.dumps(d), encoding="utf-8")
    else:
        path.unlink()
    before = path.read_bytes() if path.exists() else None
    row_before = svc.store.find_campaign("FC-DC-R")
    with pytest.raises(StoreError):
        svc.compile_campaign(spec)
    with pytest.raises(StoreError):
        svc.release_gate("FC-DC-R", operator="op", approval_ref="ref")
    assert (path.read_bytes() if path.exists() else None) == before and svc.store.find_campaign("FC-DC-R") == row_before


def test_a_hard_crash_inside_registration_leaves_an_unregistered_recoverable_state(bars, tmp_path):
    r = Race(tmp_path)
    spec = E.make_spec("FC-DC-K", bars, sources=grid(37))
    r.start("a", spec, mode="crash_in_publish")
    assert r.release("a") is None and r.procs["a"].returncode == 9                          # died after writing files, before the commit
    assert FactoryStore(r.root / "factory.sqlite3").find_campaign("FC-DC-K") is None
    out = FactoryService(r.root, REPO, grammar_available=True).compile_campaign(spec)       # restart: safely recompiled
    assert out["created"] is True
    row, decl, ident = frozen(r.root, "FC-DC-K")
    assert ident == row["declaration_sha256"]


def test_a_hard_crash_after_the_commit_leaves_a_truthful_complete_registered_campaign(bars, tmp_path):
    r = Race(tmp_path)
    spec = E.make_spec("FC-DC-K", bars, sources=grid(37))
    r.start("a", spec, mode="crash_after_commit")
    assert r.release("a") is None and r.procs["a"].returncode == 9
    row, decl, ident = frozen(r.root, "FC-DC-K")
    assert row is not None and ident == row["declaration_sha256"]
    again = FactoryService(r.root, REPO, grammar_available=True).compile_campaign(spec)
    assert again["created"] is False and again["declaration_sha256"] == ident


def test_a_failure_while_publishing_rolls_back_the_registration(tmp_path):
    st = FactoryStore(tmp_path / "s.sqlite3")
    trials = [{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}]
    kw = dict(campaign_id="FC-DC-P", spec={"k": 1}, declaration_sha256="d" * 64, declaration_path="/x", run_dir="/x",
              evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=trials)

    def boom():
        raise OSError("disk full")

    with pytest.raises(OSError):
        st.create_campaign(publish=boom, **kw)
    assert st.find_campaign("FC-DC-P") is None and not st.list_jobs("FC-DC-P") and st.campaign_trials("FC-DC-P") == []
    calls = []
    assert st.create_campaign(publish=lambda: calls.append(1), **kw) is True and calls == [1]
    assert st.create_campaign(publish=lambda: calls.append(2), **kw) is False and calls == [1]       # identical re-registration publishes nothing
    with pytest.raises(StoreError):
        st.create_campaign(publish=lambda: calls.append(3), **{**kw, "declaration_sha256": "e" * 64})
    assert calls == [1]                                                                      # a refused registration publishes nothing


def test_atomic_write_survives_a_concurrent_writer_to_the_same_path(tmp_path, monkeypatch):
    target = tmp_path / "declaration.json"
    real_replace, nested = os.replace, []

    def replace_after_a_competitor_wrote(src, dst):
        if not nested:
            nested.append(1)
            svc_mod._atomic_write(target, '{"writer": "second"}')            # a second writer completes inside the first writer's window
        return real_replace(src, dst)

    monkeypatch.setattr(os, "replace", replace_after_a_competitor_wrote)
    svc_mod._atomic_write(target, '{"writer": "first"}')
    assert json.loads(target.read_text(encoding="utf-8"))["writer"] in ("first", "second")
    assert not [p for p in tmp_path.iterdir() if p.name != "declaration.json"]               # no temp file is left behind


@pytest.mark.parametrize("bad", ["FC-DC.", "con", "NUL", "aux", "Com1", "lpt9", "fc-dc-x.."])
def test_campaign_ids_that_alias_or_cannot_own_a_directory_are_refused(bars, bad):
    with pytest.raises(C.CampaignError, match="campaign_id"):
        C.validate_spec(E.make_spec(bad, bars, sources=grid(37)))


def test_campaign_ids_that_differ_only_by_case_cannot_share_a_directory(tmp_path):
    st = FactoryStore(tmp_path / "s.sqlite3")
    trials = [{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}]
    common = dict(spec={"k": 1}, declaration_sha256="d" * 64, declaration_path="/x", run_dir="/x", evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=trials)
    st.create_campaign(campaign_id="FC-Case-1", **common)
    with pytest.raises(StoreError, match="differs only by case"):
        st.create_campaign(campaign_id="fc-case-1", **{**common, "spec": {"k": 2}})
    assert st.find_campaign("fc-case-1") is None


def test_status_export_and_report_files_are_written_atomically_with_unique_temp_names(bars, tmp_path, monkeypatch):
    from mqk_research.strategy_factory import status as status_mod
    real_replace, seen = os.replace, []

    def spy(src, dst):
        seen.append((Path(src).name, Path(dst).name))
        return real_replace(src, dst)

    svc = FactoryService(tmp_path / "f", REPO, grammar_available=True)
    svc.compile_campaign(E.make_spec("FC-DC-S", bars, sources=grid(37)))
    monkeypatch.setattr(os, "replace", spy)
    status_mod.write_status(svc.root / "factory.sqlite3", tmp_path / "status.json")
    svc.report("FC-DC-S")
    targets = sorted(d for _, d in seen)
    assert targets == ["report.json", "report.md", "status.json"]
    assert all(src != dst + ".tmp" and src.startswith(dst + ".") and src.endswith(".tmp") for src, dst in seen)           # unique temp names
    assert json.loads((tmp_path / "status.json").read_text(encoding="utf-8"))["truth_state"] == "active"
