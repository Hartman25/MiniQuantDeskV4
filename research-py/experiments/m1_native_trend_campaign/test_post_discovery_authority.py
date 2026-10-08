"""Pins the facts that `docs/research/M1_POST_DISCOVERY_CANDIDATE_AUTHORITY.md` rests on to committed sources.

Reads committed files only (no run directory, no provider, no reserved rows). If one of these facts changes, the
authority document is stale and must be re-derived; the tests fail rather than let the document drift.
"""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
EXP = HERE.parent
DOC = REPO / "docs" / "research" / "M1_POST_DISCOVERY_CANDIDATE_AUTHORITY.md"
CENSUS02 = EXP / "alpha_edge_census_02"
RESULT01 = CENSUS02 / "results" / "discovery_result_01"

CONFIRMATION_SYMBOLS = ("DIA", "MDY", "XLF", "XLI", "XLV", "XLP")
FINAL_HOLDOUT_START = date(2026, 3, 1)
CONSUMED_CONFIRMATION_START = date(2025, 1, 1)


def _json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def _add_months(d: date, months: int) -> date:
    index = d.year * 12 + (d.month - 1) + months
    return date(index // 12, index % 12 + 1, d.day)


def test_batch03_confirmation_is_prepared_never_registered_and_no_confirmation_declaration_exists() -> None:
    decl = _json(HERE / "PREDECLARED_BATCH_03.json")
    prep = decl["confirmation_preparation"]
    assert prep["status"] == "PREPARED_NOT_REGISTERED"
    assert tuple(prep["universe"]) == ("DIA", "MDY", "XLF", "XLI", "XLV", "XLP")
    assert prep["future_trials"] == 18
    for path in sorted(HERE.glob("PREDECLARED_*.json")):
        body = _json(path)
        ident = f"{path.name} {body.get('batch_id')} {(body.get('experiment') or {}).get('real_experiment_id')}"
        assert "confirmation" not in ident.lower(), f"a Confirmation declaration exists: {ident}"
    assert decl["universe"]["max_trials"] == 60 and not set(prep["universe"]) & set(decl["universe"]["symbols"])


def test_confirmation_universe_exposure_matches_the_authority_table() -> None:
    class_c = set(_json(CENSUS02 / "CENSUS02_PREDECLARATION.json")["strategy_population"]["class_c_scope"])
    census_universe = set(_json(RESULT01 / "universe_dispositions.json")["dispositions"])
    confirmation01 = {r["symbol"] for r in _json(
        EXP / "alpha_edge_confirmation_01" / "results" / "confirmation_data_provenance.json")["symbols"]}
    registry = {r["symbol"] for r in _json(REPO / "config" / "instruments" / "equities.json")}
    observed = {s: (s in class_c, s in census_universe, s in confirmation01, s in registry) for s in CONFIRMATION_SYMBOLS}
    previously_exposed = (True, True, True, True)
    never_seen = (False, False, False, False)
    assert observed == {"DIA": previously_exposed, "XLF": previously_exposed, "XLI": previously_exposed,
                        "XLP": previously_exposed, "XLV": previously_exposed, "MDY": never_seen}


@pytest.mark.parametrize("name", sorted(p.name for p in HERE.glob("PREDECLARED_*.json")
                                        if "partition" in json.loads(p.read_text(encoding="utf-8"))))
def test_every_native_walk_forward_scores_a_fold_inside_the_globally_consumed_confirmation_window(name: str) -> None:
    part = _json(HERE / name)["partition"]
    start = date.fromisoformat(part["evaluation_start_utc"][:10])
    last_fold_start = _add_months(start, part["test_months"] * (part["expected_folds"] - 1))
    last_fold_end = _add_months(start, part["test_months"] * part["expected_folds"])
    assert last_fold_end == FINAL_HOLDOUT_START
    assert CONSUMED_CONFIRMATION_START <= last_fold_start < FINAL_HOLDOUT_START


def test_final_holdout_start_is_one_authority_across_census02_and_batch03() -> None:
    truth = _json(CENSUS02 / "CENSUS02_PREDECLARATION.json")["structural_protocol"]["partitions"]
    assert truth["final_holdout"]["start_inclusive"] == FINAL_HOLDOUT_START.isoformat()
    assert truth["final_holdout"]["status"] == "RESERVED_UNCONSUMED"
    assert truth["confirmation_window"]["status"] == "CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01"
    assert truth["confirmation_window"]["start_inclusive"] == CONSUMED_CONFIRMATION_START.isoformat()
    batch = _json(HERE / "PREDECLARED_BATCH_03.json")
    assert FINAL_HOLDOUT_START.isoformat() in batch["partition"]["holdout_rule"]
    assert batch["holdout"]["status"] == "RESERVED / UNCONSUMED"


def test_census02_result_is_discovery_only_with_the_counts_the_document_cites() -> None:
    summary = _json(RESULT01 / "strategy_campaign_summary.json")
    assert summary["outcomes"] == {"INSUFFICIENT_CLOSED_ROUND_TRIPS": 1712, "NOT_QUALIFIED": 7409, "QUALIFIED": 279}
    assert summary["trials"] == 9400
    manifest = _json(RESULT01 / "RUN_MANIFEST.json")
    assert (manifest["PROMOTION_AUTHORITY"], manifest["VALIDATION_STATUS"]) == ("NONE", "NOT_VALIDATED")
    # No native engine implements a Census-02 family: the shorts live only in the Python simulator.
    engines = (REPO / "core-rs" / "crates" / "mqk-strategy" / "src" / "engines" / "mod.rs").read_text(encoding="utf-8")
    assert not re.search(r"\b(SH|LS)\d{2}\b", engines)


def test_monthly_family_calendar_horizon_is_the_one_the_document_blocks_on() -> None:
    cal = _json(HERE / "PREDECLARED_BATCH_03.json")["calendar"]
    assert (cal["contract_id"], cal["coverage_end_date"]) == ("us_equity_regular_sessions_v1", "2026-12-31")
    sessions = (REPO / "core-rs" / "crates" / "mqk-integrity" / "src" / "sessions.rs").read_text(encoding="utf-8")
    assert "pub const COVERAGE_END: (i32, u32, u32) = (2026, 12, 31);" in sessions


def test_document_records_the_verdict_and_every_operator_decision() -> None:
    text = DOC.read_text(encoding="utf-8")
    assert "NO_ELIGIBLE_CANDIDATE_EXISTS" in text and "NOT_EXECUTABLE" in text
    for n in range(1, 9):
        assert f"OD-{n}" in text, f"operator decision OD-{n} missing"


def test_external_catalog_is_recorded_as_untrusted_unread_intake() -> None:
    text = DOC.read_text(encoding="utf-8")
    for needle in ("UNTRUSTED IDEA INTAKE", "V4-M1-EXTERNAL-IDEA-INTAKE-DEDUP-01", "6fc945a873733cda6a1552049923a153ed2f7213c488c6d5f07ffeb8077a37f3", "not yet committed",
                   "EXT-032", "EXT-024", "EXT-045", "EXT-070", "EXT-141", "BACKWARD_TEMPORAL"):
        assert needle in text, needle
    # Intake must not create a trial or declaration: no declaration names an external idea.
    for path in HERE.glob("PREDECLARED_*.json"):
        assert "EXT-" not in path.read_text(encoding="utf-8"), path.name
