"""Promotion-readiness and authority wording are truthful for every evidence grade, and scoped to what the Factory did."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory import reporting, status
from mqk_research.strategy_factory.contracts import FACTORY_AUTHORITY, promotion_view
from mqk_research.strategy_factory.store import FactoryStore
from support import factory_e2e as E

REPO = E.REPO
SRC = REPO / "research-py" / "src" / "mqk_research" / "strategy_factory"
EXPECTED = {"SYNTHETIC_DIAGNOSTIC": "NOT_ELIGIBLE_SYNTHETIC", "EXPOSED_DEVELOPMENT": "NOT_ESTABLISHED"}


@pytest.fixture(scope="module")
def bars(tmp_path_factory):
    return E.make_bars_dir(tmp_path_factory.mktemp("authbars"))


def test_every_supported_grade_is_covered_and_none_is_promotion_eligible():
    assert set(C.GRADES) == set(EXPECTED)                       # a new grade needs this test (and operator approval) before it can ship
    for grade, readiness in EXPECTED.items():
        assert promotion_view(grade) == {"promotion_eligible": False, "promotion_readiness": readiness}
    assert promotion_view("SOMETHING_NEW")["promotion_eligible"] is False and promotion_view("")["promotion_readiness"] == "NOT_ESTABLISHED"


def test_exposed_development_is_not_independent_oos_and_the_declaration_says_so(bars, tmp_path):
    spec = E.make_spec("FC-AUTH-1", bars, sources=[{"kind": "native", "strategy_ids": ["absolute_momentum_252"]}])
    d = C.compile_campaign(spec, repo_root=REPO, run_root=tmp_path, ideas={}, grammar_available=False).declaration
    assert d["factory"]["promotion_eligible"] is False and d["factory"]["promotion_readiness"] == "NOT_ELIGIBLE_SYNTHETIC"
    assert d["evidence_grade"]["independent_confirmation"] is False
    assert "not independent confirmation" in C.GRADES["EXPOSED_DEVELOPMENT"]


@pytest.mark.parametrize("grade", sorted(EXPECTED))
def test_reports_and_status_derive_readiness_from_the_grade_never_from_a_stored_flag(bars, tmp_path, grade):
    spec = E.make_spec("FC-AUTH-2", bars, sources=[{"kind": "native", "strategy_ids": ["absolute_momentum_252"]}])
    d = C.compile_campaign(spec, repo_root=REPO, run_root=tmp_path, ideas={}, grammar_available=False).declaration
    d["evidence_grade"]["grade"] = grade                                        # exercise the report for each supported grade
    d["factory"]["promotion_eligible"] = True                                   # a legacy / tampered stored value must be ignored
    d["factory"]["promotion_readiness"] = "ELIGIBLE"
    path = tmp_path / "declaration.json"
    path.write_text(json.dumps(d), encoding="utf-8")
    campaign = {"campaign_id": "FC-AUTH-2", "state": "PREDECLARED", "declaration_path": str(path), "declaration_sha256": "0" * 64}
    rep = reporting.build_report(campaign, REPO, None, run_summary_stage=False)["campaign"]
    assert rep["promotion_eligible"] is False and rep["promotion_readiness"] == EXPECTED[grade]
    store = FactoryStore(tmp_path / "s.sqlite3")
    store.create_campaign(campaign_id="FC-AUTH-2", spec=spec, declaration_sha256="d" * 64, declaration_path=str(path), run_dir=str(tmp_path),
                          evidence_grade=grade, trials=[{"trial_key": "s/SPY", "strategy_name": "s", "symbol": "SPY"}])
    snap = status.build_status(tmp_path / "s.sqlite3")["campaigns"][0]
    assert snap["promotion_eligible"] is False and snap["promotion_readiness"] == EXPECTED[grade]


def test_authority_wording_is_scoped_to_factory_actions_not_the_live_mqd_runtime(bars, tmp_path):
    snap = status.build_status(_empty_store(tmp_path))
    for auth in (snap["authority"], FACTORY_AUTHORITY):
        assert auth["scope"].startswith("FACTORY_ACTIONS_ONLY") and "not read or asserted" in auth["scope"]
        assert auth["paper"] == "NOT_TOUCHED_BY_FACTORY" and auth["live"] == "NOT_TOUCHED_BY_FACTORY"
        assert "INACTIVE" not in json.dumps(auth)                                  # the Factory cannot know the real Paper runtime state
    text = "\n".join(p.read_text(encoding="utf-8") for p in SRC.glob("*.py"))
    assert not re.search(r"paper[\"']\s*:\s*[\"']INACTIVE", text)
    assert not re.search(r"promotion_eligible[^\n]*!=\s*[\"']SYNTHETIC_DIAGNOSTIC", text)         # "not synthetic" is never read as eligible


def _empty_store(tmp_path):
    FactoryStore(tmp_path / "e.sqlite3")
    return tmp_path / "e.sqlite3"
