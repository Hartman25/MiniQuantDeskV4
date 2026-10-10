"""Reference knowledge base (educational glossary) integration, proposal kinds, composite source identity."""

from __future__ import annotations

import glob
import hashlib
import json
import os
import shutil
from pathlib import Path

import pytest

from mqk_research.strategy_factory import ai_normalize as ai
from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory import formalize, knowledge, pipeline
from mqk_research.strategy_factory import known_index as ki
from mqk_research.strategy_factory.contracts import COARSE_DISPOSITION, Disposition
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row
from test_strategy_factory_ai_normalize import FakeProvider, proposal

REPO = Path(__file__).resolve().parents[2]
KNOWN = ki.build_index(REPO)


def ledger(rows, family="kb_fam", sources=(("S1", "Paper"),)):
    sheets = {"IDEAS": [HEADER, *rows], "VIEW": [["ID", "Note"], [rows[0][0], "x"]], "CONTROLS": [HEADER],
              "SOURCES": [["SID", "Title"], *map(list, sources)]}
    prof = ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": family})
    return ci.import_catalog(make_xlsx(sheets), "c.xlsx", profile=prof)


def test_glossary_is_pinned_valid_and_nonexecutable():
    kb = knowledge.load(REPO)
    assert kb.sha256 == knowledge.KNOWLEDGE_SHA256 and len(kb.entries) == 166
    assert all(e["executable_rule"] is False and e["proof_status"] == knowledge.PROOF_STATUS for e in kb.entries)
    assert knowledge.try_load(REPO) is not None


@pytest.mark.parametrize("mutate,needle", [
    (lambda d: d.update(status="EXECUTABLE_STRATEGY_LIBRARY"), "educational-reference"),
    (lambda d: d["indicator_entries"][0].update(executable_rule=True), "executable authority"),
    (lambda d: d["indicator_entries"][0].update(default_parameters={"window": 14}), "default parameters"),
    (lambda d: d["indicator_entries"][1].update(id=d["indicator_entries"][0]["id"]), "unique"),
    (lambda d: d["indicator_entries"][0].update(proof_status="EDGE_PROVEN"), "evidence"),
    (lambda d: d.update(indicator_entries=[]), "no entries"),
])
def test_altered_or_authority_claiming_glossary_is_refused(tmp_path, mutate, needle):
    dst = tmp_path / knowledge.KNOWLEDGE_RELATIVE
    dst.parent.mkdir(parents=True)
    data = json.loads((REPO / knowledge.KNOWLEDGE_RELATIVE).read_text(encoding="utf-8"))
    mutate(data)
    raw = json.dumps(data).encode("utf-8")
    dst.write_bytes(raw)
    with pytest.raises(knowledge.KnowledgeError, match=needle):
        knowledge.load(tmp_path, expected_sha256=hashlib.sha256(raw).hexdigest())
    with pytest.raises(knowledge.KnowledgeError, match="sha256"):                # the pin catches any byte change
        knowledge.load(tmp_path)
    assert knowledge.try_load(tmp_path) is None


def test_missing_glossary_degrades_to_none(tmp_path):
    assert knowledge.try_load(tmp_path) is None
    with pytest.raises(knowledge.KnowledgeError, match="unreadable"):
        knowledge.load(tmp_path)


def test_retrieval_is_deterministic_bounded_and_whole_word():
    kb = knowledge.load(REPO)
    text = "Test OHLCV bars with a simple moving average and relative strength"
    a, b = kb.retrieve(text), kb.retrieve(text)
    assert a == b and a and all(e["name"] for e in a)
    assert len(kb.retrieve(text, limit=1)) == 1
    assert kb.retrieve("zzzz qqqq nothing here") == []
    block = kb.prompt_block(a)
    assert block.startswith(knowledge.BLOCK_HEADER) and "never rules" in block and len(block) < 2500


def test_glossary_context_reaches_the_prompt_and_provenance_but_cannot_fill_parameters():
    kb = knowledge.load(REPO)
    src = row("K-1", "Trend gate", rule="Own SPY while price exceeds its simple moving average of 50 days, otherwise cash")
    led = ledger([src])
    prov = FakeProvider(proposal(params={"window": {"value": 50, "evidence": "simple moving average of 50 days"}}))
    idea, rec = ai.normalize_entry(led["entries"][0], led, prov, knowledge=kb)
    assert knowledge.BLOCK_HEADER in prov.prompts[0] and rec["knowledge"]["glossary_sha256"] == kb.sha256 and rec["knowledge"]["entry_ids"]
    assert idea["ai"]["knowledge"] == rec["knowledge"]
    # the glossary has no defaults: a model "filling" a window from a glossary definition is a suggestion only
    idea2, _ = ai.normalize_entry(led["entries"][0], led, FakeProvider(proposal(params={"window": {"value": 14, "evidence": "simple moving average"}})), knowledge=kb)
    assert idea2["ai"]["suggestions"]["window"]["value"] == 14 and idea2["template"]["params"]["window"]["value"] is None
    plain, rec2 = ai.normalize_entry(led["entries"][0], led, FakeProvider(proposal()), knowledge=None)
    assert rec2["knowledge"] is None and rec2["prompt_sha256"] != rec["prompt_sha256"]


def test_composite_source_identity_prevents_cross_workbook_id_collisions():
    a = ledger([row("K-1", "Idea A", src="P01")], "fam_a", sources=(("P01", "Paper A"),))
    b = ledger([row("K-1", "Idea B", src="P01")], "fam_b", sources=(("P01", "Paper B"),))
    ra, rb = a["entries"][0]["fields"]["source_composite_refs"], b["entries"][0]["fields"]["source_composite_refs"]
    assert ra != rb and ra[0].startswith("fam_a:") and rb[0].startswith("fam_b:") and ra[0].endswith(":P01")
    ideas = [formalize.formalize_entry(a["entries"][0], a), formalize.formalize_entry(b["entries"][0], b)]
    assert ideas[0]["provenance"]["source_composite_refs"] == ra and ideas[0]["intake_id"] != ideas[1]["intake_id"]


def test_byte_identical_file_copies_never_double_the_population():
    led = ledger([row("K-1", "Idea A"), row("K-2", "Idea B")])
    again = ledger([row("K-1", "Idea A"), row("K-2", "Idea B")])
    out = pipeline.process([led, again], KNOWN, grammar_available=True)
    assert led["ledger_sha256"] == again["ledger_sha256"] and out["unique_ideas"] == 2 and len(out["source_copies"]) == 2


def test_benchmarks_controls_and_diagnostics_are_never_candidate_hypotheses():
    rows = [row("B-1", "Buy and hold broad ETF", rule="Compare any active candidate against causal SPY"),
            row("B-2", "Equal-weight ETF basket", rule="Rebalance a fixed ETF list on a declared calendar"),
            row("D-1", "Overnight reversal", question="Does the overnight return reverse after a down close?"),
            row("F-1", "Futures trend", rule="Hold the contract above its 50-day SMA", assets="Futures"),
            row("S-1", "50-day SMA trend gate", rule="Hold above the 50-day SMA, else cash")]
    out = pipeline.process([ledger(rows)], KNOWN, grammar_available=True)
    by = {r["entry_id"]: r for r in out["records"]}
    assert by["B-1"]["disposition"] == by["B-2"]["disposition"] == Disposition.BENCHMARK_NOT_STRATEGY.value
    assert by["B-1"]["proposal_kind"] == "BENCHMARK" and by["D-1"]["proposal_kind"] == "MECHANISM_DIAGNOSTIC"
    assert by["F-1"]["proposal_kind"] == "FUTURE_ASSET" and by["S-1"]["proposal_kind"] == "STRATEGY_HYPOTHESIS"
    assert set(COARSE_DISPOSITION.values()) <= {"REFERENCE_ONLY", "DUPLICATE_CANDIDATE", "NEEDS_FORMALIZATION", "REQUIRES_UNAVAILABLE_DATA",
                                                "REQUIRES_NEW_NATIVE_IMPLEMENTATION", "RESEARCH_ELIGIBLE_AWAITING_PREDECLARATION", "REJECTED"}
    assert set(COARSE_DISPOSITION) == {d.value for d in Disposition}
    assert out["proposal_kind_counts"]["BENCHMARK"] == 2 and sum(out["coarse_disposition_counts"].values()) == out["unique_ideas"]
    candidates = [r for r in out["records"] if r["proposal_kind"] == "STRATEGY_HYPOTHESIS"]
    assert [r["entry_id"] for r in candidates] == ["S-1"]


def test_research_controls_are_not_candidate_hypotheses():
    sheets = {"IDEAS": [HEADER, row("K-1", "50-day SMA trend gate", rule="Hold above the 50-day SMA")], "VIEW": [["ID", "Note"], ["K-1", "x"]],
              "CONTROLS": [HEADER, row("C-1", "Matched-benchmark placebo control")], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    prof = ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": "kb_ctl"})
    out = pipeline.process([ci.import_catalog(make_xlsx(sheets), "c.xlsx", profile=prof)], KNOWN, grammar_available=True)
    by = {r["entry_id"]: r for r in out["records"]}
    assert by["C-1"]["proposal_kind"] == "RESEARCH_CONTROL" and by["C-1"]["disposition"] == Disposition.GOVERNANCE_CONTROL.value
    assert by["K-1"]["proposal_kind"] == "STRATEGY_HYPOTHESIS"


DOWNLOADS = os.path.join(os.path.expanduser("~"), "Downloads")
REAL = sorted(glob.glob(os.path.join(DOWNLOADS, "MQD_*_2026-10-09.*")))


@pytest.mark.skipif(len(REAL) != 8, reason="operator catalogs not present on this machine")
def test_reference_review_cardinalities_hold_for_the_real_catalogs():
    leds = []
    for p in REAL:
        with open(p, "rb") as fh:
            leds.append(ci.import_catalog(fh.read(), os.path.basename(p)))
    counts = {l["profile_id"]: l["counts"]["ideas"] for l in leds}
    assert [counts[k] for k in ("reddit_xlsx_v1", "academic_xlsx_v1", "mechanisms_xlsx_v1", "psychology_xlsx_v1", "shocks_xlsx_v1", "fourarea_xlsx_v1")] \
        == [147, 121, 36, 26, 86, 40]                                                    # 456 raw rows, not 456 trials
    assert counts["reddit_csv_v1"] == 147
    out = pipeline.process(leds, KNOWN, grammar_available=True)
    assert out["submitted_entries"] == 668 and out["unique_ideas"] == 521 and len(out["source_copies"]) == 147
    assert out["proposal_kind_counts"]["STRATEGY_HYPOTHESIS"] < 30                      # almost nothing is an executable candidate
    assert out["disposition_counts"].get("ADMITTED_GRAMMAR", 0) == 0                    # no submitted idea is fully specified and new
    assert sum(out["disposition_counts"].values()) == 521
