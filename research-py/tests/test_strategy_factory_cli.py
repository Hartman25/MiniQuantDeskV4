"""The operator command line, exercised as real subprocesses (no native binary, provider or network needed)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from mqk_research.strategy_factory import status as status_mod
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row

SRC = str(Path(__file__).resolve().parents[1] / "src")


def cli(root, *args, expect=None):
    env = {**os.environ, "PYTHONPATH": SRC, "PYTHONIOENCODING": "utf-8", "PYTHONDONTWRITEBYTECODE": "1"}
    p = subprocess.run([sys.executable, "-m", "mqk_research.strategy_factory", "--root", str(root), *args], capture_output=True, text=True, env=env, timeout=300)
    if expect is not None:
        assert p.returncode == expect, (p.returncode, p.stdout[-600:], p.stderr[-600:])
    return p


@pytest.fixture
def workbook(tmp_path):
    sheets = {"IDEAS": [HEADER, row("C-1", "Fast/slow MA cross", rule="crossover"), row("C-2", "37-day SMA trend gate", rule="Hold above the 37-day SMA, else cash"),
                        row("C-3", "Futures trend", rule="Hold above the 50-day SMA", assets="Futures")],
              "VIEW": [["ID", "Note"], ["C-1", "x"]], "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    x = tmp_path / "cat.xlsx"
    x.write_bytes(make_xlsx(sheets))
    prof = tmp_path / "profile.json"
    prof.write_text(json.dumps({**TEST_PROFILE.to_json(), "catalog_family": "cli_fam"}), encoding="utf-8")
    return x, prof


def test_status_on_an_empty_root_reports_no_db_and_creates_nothing(tmp_path):
    root = tmp_path / "root"
    p = cli(root, "status")
    assert p.returncode == 3 and json.loads(p.stdout)["truth_state"] == "no_db" and not root.exists()


def test_import_intake_ideas_decide_and_export_status_end_to_end(tmp_path, workbook):
    x, prof = workbook
    root = tmp_path / "root"
    imp = json.loads(cli(root, "import-catalog", str(x), "--profile", str(prof), expect=0).stdout)
    assert imp[0]["entries"] == 3 and imp[0]["new"] is True
    assert json.loads(cli(root, "import-catalog", str(x), "--profile", str(prof), expect=0).stdout)[0]["new"] is False      # idempotent
    out = json.loads(cli(root, "intake", expect=0).stdout)
    assert out["result"]["unique_ideas"] == 3 and out["ai"] == {"configured": False, "conformant": None, "status_counts": {}}
    ideas = {i["entry_id"]: i for i in json.loads(cli(root, "ideas", expect=0).stdout)}
    assert ideas["C-1"]["disposition"] == "NEEDS_FORMALIZATION" and ideas["C-3"]["disposition"] == "DEFERRED_ASSET_CLASS"
    assert ideas["C-2"]["disposition"] == "NEEDS_IMPLEMENTATION"                      # no native binary configured: grammar unavailable, said so
    only = json.loads(cli(root, "ideas", "--disposition", "DEFERRED_ASSET_CLASS", expect=0).stdout)
    assert [i["entry_id"] for i in only] == ["C-3"]
    for param, val in (("fast", 30), ("slow", 120)):
        d = tmp_path / f"d_{param}.json"
        d.write_text(json.dumps({"intake_id": ideas["C-1"]["intake_id"], "param": param, "value": val, "decided_by": "op", "rationale": "policy",
                                 "decision_ref": f"D-{param}"}), encoding="utf-8")
        assert json.loads(cli(root, "decide", "parameter", str(d), expect=0).stdout) == {"recorded": True}
    cli(root, "intake", expect=0)
    after = {i["entry_id"]: i for i in json.loads(cli(root, "ideas", expect=0).stdout)}
    assert after["C-1"]["disposition"] == "NEEDS_IMPLEMENTATION"                      # fully specified now; executable path still absent here
    exp = tmp_path / "status.json"
    snap = json.loads(cli(root, "status", "--export", str(exp), expect=0).stdout)
    assert snap["truth_state"] == "active" and snap["ideas_by_disposition"] == {"DEFERRED_ASSET_CLASS": 1, "NEEDS_IMPLEMENTATION": 2}
    assert json.loads(exp.read_text(encoding="utf-8")) == snap and snap["authority"]["live"] == "NOT_TOUCHED_BY_FACTORY" and snap["authority"]["scope"].startswith("FACTORY_ACTIONS_ONLY")
    assert status_mod.build_status(root / "factory.sqlite3") == snap


def test_unsafe_or_malformed_input_is_refused_with_exit_2(tmp_path, workbook):
    x, prof = workbook
    root = tmp_path / "root"
    bad = tmp_path / "bad.xlsx"
    bad.write_bytes(b"not a workbook")
    p = cli(root, "import-catalog", str(bad), "--profile", str(prof))
    assert p.returncode == 2 and "REFUSED" in p.stderr
    spec = tmp_path / "spec.json"
    spec.write_text(json.dumps({"schema": "wrong"}), encoding="utf-8")
    p = cli(root, "campaign", "compile", str(spec))
    assert p.returncode == 2 and "REFUSED" in p.stderr


def test_ai_probe_labels_an_unavailable_model_blocked_dependency_never_functional(tmp_path):
    p = cli(tmp_path / "r", "ai", "probe", "--ollama-model", "no-such-model-xyz", "--ollama-url", "http://127.0.0.1:9")
    out = json.loads(p.stdout)
    assert p.returncode == 3 and out["label"] == "BLOCKED_DEPENDENCY" and out["conformant"] is False and out["available"] is False


def test_scout_with_the_default_empty_policy_fetches_nothing(tmp_path):
    pol = tmp_path / "policy.json"
    pol.write_text(json.dumps({"schema": "strategy_factory_source_policy_v1", "sources": []}), encoding="utf-8")
    out = json.loads(cli(tmp_path / "r", "scout", "--policy", str(pol), "https://example.com/a", expect=0).stdout)
    assert out["fetched"] == 0 and out["refused"][0]["reason"].startswith("example.com/a is not an operator-approved source")
