"""Controlled implementation workflow: requests are inert, readiness is evidence-based, nothing is admitted by compiling."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory import implementation as impl
from mqk_research.strategy_factory import known_index as ki
from mqk_research.strategy_factory import pipeline
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row

REPO = Path(__file__).resolve().parents[2]
KNOWN = ki.build_index(REPO)


def records(rows, decisions=()):
    sheets = {"IDEAS": [HEADER, *rows], "VIEW": [["ID", "Note"], [rows[0][0], "x"]], "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    led = ci.import_catalog(make_xlsx(sheets), "c.xlsx", profile=ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": "impl"}))
    return pipeline.process([led], KNOWN, grammar_available=True, decisions=decisions)["records"]


def test_a_fully_specified_stateful_idea_gets_an_inert_deterministic_request():
    src = row("I-1", "RSI(3) pullback", rule="Buy when RSI(3) < 12 above the 150-day SMA, exit when RSI(3) > 65", direction="Long / flat")
    rec = records([src])[0]
    assert rec["disposition"] == "NEEDS_IMPLEMENTATION"
    req = impl.build_request(rec)
    assert req["template_id"] == "rsi_reversion" and req["stateful"] is True and req["executable_now"] is False
    assert req["status"] == "AWAITING_AUTHORIZED_IMPLEMENTATION" and "durable-restart" in req["stateful_requirements"]
    assert req["parameters"] == {"rsi_window": 3, "entry_below": 12, "exit_above": 65, "trend_window": 150}
    assert all(c == "EXPLICIT_SOURCE_RULE" for c in req["parameter_classes"].values())
    assert any("mutation proof" in p for p in req["required_proofs"]) and any("no downloaded or generated code" in p for p in req["required_proofs"])
    assert impl.build_request(rec) == req                                                 # deterministic, hash-identified
    assert req["request_sha256"] and "grants nothing" in req["authority"]


def test_a_request_is_refused_even_for_a_forged_disposition_when_parameters_are_missing():
    import copy
    rec = records([row("I-1", "RSI(3) pullback", rule="Buy when RSI(3) < 12 above the 150-day SMA, exit when RSI(3) > 65", direction="Long / flat")])[0]
    forged = copy.deepcopy(rec)
    forged["template"]["missing_params"] = ["exit_above"]
    forged["template"]["params"]["exit_above"]["value"] = None
    assert impl.build_request(forged) is None


def test_no_request_exists_for_ideas_that_are_not_recognized_fully_specified_and_engine_less():
    recs = records([row("I-2", "Fast/slow MA cross", rule="crossover"), row("I-3", "37-day SMA trend gate", rule="Hold above the 37-day SMA else cash"),
                    row("I-4", "Futures trend", rule="Hold above the 50-day SMA", assets="Futures")])
    by = {r["entry_id"]: r for r in recs}
    assert impl.build_request(by["I-2"]) is None and impl.build_request(by["I-3"]) is None and impl.build_request(by["I-4"]) is None


def test_a_native_strategy_is_not_ready_without_an_explicit_operator_authorization_record():
    rep = impl.admission_checklist(REPO, "trend_sma50")
    assert rep["checks"]["semantic_card"]["ok"] and rep["checks"]["registered_in_rust"]["ok"] and rep["checks"]["engine_has_deterministic_tests"]["ok"]
    assert rep["checks"]["explicit_operator_authorization"]["ok"] is False and rep["ready"] is False


def test_an_invented_or_unregistered_strategy_fails_every_check():
    rep = impl.admission_checklist(REPO, "llm_generated_alpha_v1")
    assert rep["ready"] is False and not any(c["ok"] for c in rep["checks"].values())


def test_readiness_needs_all_four_proofs_including_a_well_formed_authorization(tmp_path):
    for rel in (impl.ENGINES / "mod.rs", impl.ENGINES / "trend_sma50.rs", impl.ENGINES / "window.rs"):
        dst = tmp_path / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(REPO / rel, dst)
    for p in (REPO / impl.ENGINES).glob("*.rs"):                       # the registry parser reads every listed engine's NAME
        d = tmp_path / impl.ENGINES / p.name
        if not d.exists():
            shutil.copy(p, d)
    assert impl.admission_checklist(tmp_path, "trend_sma50")["ready"] is False
    auth = tmp_path / impl.AUTHORIZATION_DIR / "trend_sma50.json"
    auth.parent.mkdir(parents=True)
    auth.write_text(json.dumps({"operator": "op", "approval_ref": "AR-1", "strategy_id": "other"}), encoding="utf-8")
    assert impl.admission_checklist(tmp_path, "trend_sma50")["checks"]["explicit_operator_authorization"]["ok"] is False      # id must match
    auth.write_text("not json", encoding="utf-8")
    assert impl.admission_checklist(tmp_path, "trend_sma50")["ready"] is False
    auth.write_text(json.dumps({"operator": "op", "approval_ref": "AR-1", "strategy_id": "trend_sma50"}), encoding="utf-8")
    assert impl.admission_checklist(tmp_path, "trend_sma50")["ready"] is True
