"""Durable Factory history feeds cross-campaign novelty and the previous-search disclosure, independent of results."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from mqk_research.strategy_factory import campaign as C
from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory.dedup import classify_template
from mqk_research.strategy_factory.known_index import build_index, factory_prior_entries
from mqk_research.strategy_factory.service import FactoryService
from mqk_research.strategy_factory.store import FactoryStore, StoreError
from support import factory_e2e as E
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row

REPO = E.REPO
W37, W38 = "grammar_v1__sma_trend_gate__window_37", "grammar_v1__sma_trend_gate__window_38"


def grid(*windows):
    return [{"kind": "grammar_grid", "template": "sma_trend_gate", "grid": {"window": list(windows)}}]


@pytest.fixture(scope="module")
def bars(tmp_path_factory):
    return E.make_bars_dir(tmp_path_factory.mktemp("psbars"))


def service(root):
    return FactoryService(root, REPO, grammar_available=True)


def rel_of(declaration, name):
    return next(s["relationship_to_known"] for s in declaration["factory"]["strategies"] if s["strategy_name"] == name)


def load(result):
    return json.loads(Path(result["declaration_path"]).read_text(encoding="utf-8"))


def test_first_campaign_discloses_no_history_and_a_later_one_recognises_the_exact_duplicate_and_the_adjacent_variant(bars, tmp_path):
    svc = service(tmp_path / "f")
    a = svc.compile_campaign(E.make_spec("FC-PS-A", bars, sources=grid(37, 41)))
    da = load(a)
    assert da["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == []
    assert rel_of(da, W37)["relationship"] == "PARAMETER_VARIANT" and not any("factory:" in m for m in rel_of(da, W37)["matches"])
    b = svc.compile_campaign(E.make_spec("FC-PS-B", bars, sources=grid(37, 38)))
    db = load(b)
    assert db["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == ["FC-PS-A"]
    dup = rel_of(db, W37)
    assert dup["relationship"] == "EXACT_DUPLICATE" and "factory:FC-PS-A:" + W37 in dup["matches"] and "FC-PS-A" in dup["previously_tested_in"]
    adj = rel_of(db, W38)                                           # adjacent: same template, new parameter, searched neighbours exist
    assert adj["relationship"] == "PARAMETER_VARIANT" and "FC-PS-A" in adj["previously_tested_in"]
    assert adj["same_template_known"] == rel_of(da, W37)["same_template_known"] + 2        # the two predeclared strategies of FC-PS-A
    assert da["factory"]["prior_search_disclosure"] != db["factory"]["prior_search_disclosure"]


def test_history_survives_a_restart_and_a_frozen_campaign_is_not_recomputed_against_newer_history(bars, tmp_path):
    root = tmp_path / "f"
    svc = service(root)
    spec_a = E.make_spec("FC-PS-A", bars, sources=grid(37))
    a = svc.compile_campaign(spec_a)
    before = Path(a["declaration_path"]).read_bytes()
    svc.compile_campaign(E.make_spec("FC-PS-B", bars, sources=grid(38)))
    restarted = service(root)                                        # a new process: history comes from the control-plane file only
    again = restarted.compile_campaign(spec_a)
    assert again["created"] is False and again["declaration_sha256"] == a["declaration_sha256"]
    assert Path(a["declaration_path"]).read_bytes() == before
    assert load(again)["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == []
    c = restarted.compile_campaign(E.make_spec("FC-PS-C", bars, sources=grid(39)))
    assert load(c)["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == ["FC-PS-A", "FC-PS-B"]
    with pytest.raises(StoreError, match="different spec"):
        restarted.compile_campaign(E.make_spec("FC-PS-A", bars, sources=grid(37, 99)))


def test_prior_search_accounting_does_not_depend_on_outcomes(bars, tmp_path):
    store = FactoryStore(tmp_path / "s.sqlite3")
    for cid in ("FC-PS-A", "FC-PS-B"):
        store.create_campaign(campaign_id=cid, spec={"campaign_id": cid}, declaration_sha256=cid.lower().ljust(64, "0"), declaration_path=str(tmp_path / cid),
                              run_dir=str(tmp_path), evidence_grade="SYNTHETIC_DIAGNOSTIC",
                              trials=[{"trial_key": f"{W37}/SPY", "strategy_name": W37, "symbol": "SPY"}])
    pending = store.prior_campaign_strategies()
    job = store.claim_next("w", max_running=1, lease_seconds=60.0)
    store.finish(job["job_id"], job["claim_token"], status="failed", exit_code=1, reason="boom", output="")
    assert store.prior_campaign_strategies() == pending                                   # a failed attempt neither adds nor removes history
    spec = E.make_spec("FC-PS-N", bars, sources=grid(37))
    kw = dict(repo_root=REPO, run_root=tmp_path / "c", ideas={}, grammar_available=True)
    ids, rows = pending
    one = C.compile_campaign(spec, factory_prior=factory_prior_entries(rows), prior_campaigns=ids, **kw)
    ids2, rows2 = store.prior_campaign_strategies()
    two = C.compile_campaign(spec, factory_prior=factory_prior_entries(rows2), prior_campaigns=ids2, **kw)
    assert one.declaration_sha256 == two.declaration_sha256


def test_the_prior_history_is_part_of_the_declared_identity(bars, tmp_path):
    spec = E.make_spec("FC-PS-N", bars, sources=grid(37))
    kw = dict(repo_root=REPO, run_root=tmp_path / "c", ideas={}, grammar_available=True)
    rows = [("FC-PS-A", W37)]
    plain = C.compile_campaign(spec, **kw).declaration_sha256
    with_history = C.compile_campaign(spec, factory_prior=factory_prior_entries(rows), prior_campaigns=["FC-PS-A"], **kw).declaration_sha256
    assert plain != with_history


def test_a_campaign_compiled_against_stale_history_is_refused(bars, tmp_path):
    store = FactoryStore(tmp_path / "s.sqlite3")
    spec = E.make_spec("FC-PS-LATE", bars, sources=grid(37))
    trials = [{"trial_key": f"{W37}/SPY", "strategy_name": W37, "symbol": "SPY"}]
    common = dict(declaration_path=str(tmp_path / "d"), run_dir=str(tmp_path), evidence_grade="SYNTHETIC_DIAGNOSTIC", trials=trials)
    store.create_campaign(campaign_id="FC-PS-A", spec={"campaign_id": "FC-PS-A"}, declaration_sha256="a" * 64, **common)       # arrives mid-compile
    with pytest.raises(StoreError, match="history changed"):
        store.create_campaign(campaign_id="FC-PS-LATE", spec=spec, declaration_sha256="b" * 64, expected_prior_campaigns=[], **common)
    assert store.find_campaign("FC-PS-LATE") is None
    assert store.create_campaign(campaign_id="FC-PS-LATE", spec=spec, declaration_sha256="b" * 64, expected_prior_campaigns=["FC-PS-A"], **common)


def test_prior_entries_carry_semantic_identity_only_for_recognised_names():
    rows = [("FC-X", W37), ("FC-X", "absolute_momentum_252"), ("FC-X", "not_a_known_strategy")]
    entries = factory_prior_entries(rows)
    assert [e.known_id for e in entries] == [f"factory:FC-X:{W37}", "factory:FC-X:absolute_momentum_252"]
    assert all(e.source == "factory_prior" and e.tested_in == ("FC-X",) for e in entries)
    sig = {"template_id": "sma_trend_gate", "direction": "long_only", "params": {"window": 37}, "complete": True}
    assert classify_template(sig, build_index(REPO))["relationship"] == "PARAMETER_VARIANT"
    hit = classify_template(sig, build_index(REPO, entries))
    assert hit["relationship"] == "EXACT_DUPLICATE" and "FC-X" in hit["previously_tested_in"]


def test_intake_dedup_uses_prior_factory_campaigns(bars, tmp_path):
    svc = service(tmp_path / "f")
    sheets = {"IDEAS": [HEADER, row("P-1", "37-day SMA trend gate", rule="Hold above the 37-day SMA, else cash")],
              "VIEW": [["ID", "Note"], ["P-1", "x"]], "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    path = tmp_path / "ps.xlsx"
    path.write_bytes(make_xlsx(sheets))
    svc.import_catalog(path, ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": "ps_fam"}))
    svc.run_intake()
    idea = next(iter(svc.store.latest_ideas().values()))
    assert idea["disposition"] == "ADMITTED_GRAMMAR"
    svc.compile_campaign(E.make_spec("FC-PS-A", bars, sources=grid(37)))
    svc.run_intake()
    idea = next(iter(svc.store.latest_ideas().values()))
    assert idea["disposition"] == "DUPLICATE_OF_KNOWN" and "FC-PS-A" in idea["dedup"]["previously_tested_in"]


def test_a_compile_that_lost_the_history_race_can_be_retried_and_then_discloses_the_interloper(bars, tmp_path):
    svc = service(tmp_path / "f")
    real = svc.store.create_campaign
    state = {"armed": True}

    def racing(**kw):
        if state["armed"]:                                  # another process predeclares a campaign between compile and commit
            state["armed"] = False
            FactoryStore(tmp_path / "f" / "factory.sqlite3").create_campaign(
                campaign_id="FC-PS-OTHER", spec={"campaign_id": "FC-PS-OTHER"}, declaration_sha256="c" * 64, declaration_path=str(tmp_path / "o"),
                run_dir=str(tmp_path), evidence_grade="SYNTHETIC_DIAGNOSTIC",
                trials=[{"trial_key": f"{W37}/SPY", "strategy_name": W37, "symbol": "SPY"}])
        return real(**kw)

    svc.store.create_campaign = racing
    spec = E.make_spec("FC-PS-LATE", bars, sources=grid(37))
    with pytest.raises(StoreError, match="history changed"):
        svc.compile_campaign(spec)
    assert svc.store.find_campaign("FC-PS-LATE") is None
    out = svc.compile_campaign(spec)                          # the orphan declaration of the refused attempt must not block the retry
    assert out["created"] is True
    d = load(out)
    assert d["factory"]["prior_search_disclosure"]["prior_factory_campaigns"] == ["FC-PS-OTHER"]
    assert rel_of(d, W37)["relationship"] == "EXACT_DUPLICATE"


@pytest.mark.parametrize("damage", ["missing", "corrupt", "tampered"])
def test_a_damaged_frozen_declaration_is_refused_not_silently_returned_or_recomputed(bars, tmp_path, damage):
    svc = service(tmp_path / "f")
    spec = E.make_spec("FC-PS-A", bars, sources=grid(37))
    out = svc.compile_campaign(spec)
    path = Path(out["declaration_path"])
    if damage == "missing":
        path.unlink()
    elif damage == "corrupt":
        path.write_text("{not json", encoding="utf-8")
    else:
        d = json.loads(path.read_text(encoding="utf-8"))
        d["universe"]["symbols"] = ["TAMPERED"]
        path.write_text(json.dumps(d), encoding="utf-8")
    with pytest.raises(StoreError, match="frozen declaration|frozen identity"):
        svc.compile_campaign(spec)
