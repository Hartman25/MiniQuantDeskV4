"""Catalog import contract: profile-bound, fail-closed, every row retained verbatim."""

from __future__ import annotations

import glob
import os

import pytest

from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory import xlsx_reader
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, mini_workbook, row


def imp(data=None, profile=TEST_PROFILE):
    return ci.import_catalog(data if data is not None else mini_workbook(), "c.xlsx", profile=profile)


def test_every_row_is_an_entry_and_original_text_is_verbatim():
    led = imp()
    assert [e["entry_id"] for e in led["entries"]] == ["T-1", "T-2", "T-3", "C-1"]  # primary rows then additional
    t1 = led["entries"][0]
    assert t1["original"]["Idea"] == " 200-day SMA trend gate "          # verbatim, whitespace kept
    assert t1["fields"]["title"] == "200-day SMA trend gate"              # canonical copy is normalized
    assert t1["fields"]["source_refs"] == ["S1", "S2"] and t1["fields"]["unresolved_source_refs"] == []
    assert led["entries"][3]["entry_kind"] == "control"
    assert led["counts"] == {"entries": 4, "ideas": 3, "controls": 1, "sources": 2, "context_sheets": 1}
    assert led["trial_registered"] is False and led["economic_attempt"] is False
    assert led["entries"][0]["views"]["VIEW"]["original"]["Note"] == "screened"
    assert "README" in led["context_sheets"] and "README" not in {e["sheet"] for e in led["entries"]}


def test_identity_is_deterministic_and_content_bound():
    a, b = imp(), imp()
    assert a == b and a["ledger_sha256"] == b["ledger_sha256"]
    changed = mini_workbook(IDEAS=[HEADER, row("T-1", "200-day SMA trend gate", rule="x"), row("T-2", "q"), row("T-3", "r")])
    assert imp(changed)["ledger_sha256"] != a["ledger_sha256"]
    other = ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "profile_id": "other"})
    assert imp(profile=other)["ledger_sha256"] != a["ledger_sha256"]


def test_unresolved_source_reference_is_reported_not_dropped():
    led = imp()
    t3 = next(e for e in led["entries"] if e["entry_id"] == "T-3")
    assert t3["fields"]["unresolved_source_refs"] == ["S9"] and t3["fields"]["source_refs"] == ["S9"]


@pytest.mark.parametrize("mutate,needle", [
    (dict(VIEW=[["ID", "Note"], ["T-404", "x"]]), "absent from the primary sheet"),
    (dict(IDEAS=[HEADER, row("T-1", "a"), row("T-1", "b")]), "repeats ids"),
    (dict(IDEAS=[HEADER, ["", "no id here", "", "", "", "", "", "", "", ""]]), "no 'ID'"),
    (dict(IDEAS=[HEADER, row("T-1", "")]), "empty title"),
    (dict(CONTROLS=[HEADER, row("T-1", "same id on two entry sheets")]), "more than one entry sheet"),
    (dict(IDEAS=[HEADER + ["Idea"], row("T-1", "a")]), "unique non-empty"),
    (dict(IDEAS=[HEADER, row("T-1", "a") + ["overflow"]]), "wider than its header"),
])
def test_malformed_catalogs_are_refused(mutate, needle):
    with pytest.raises(ci.CatalogImportError, match=needle):
        imp(mini_workbook(**mutate))


def test_unknown_schema_is_refused_with_detected_sheets():
    with pytest.raises(ci.CatalogImportError, match="unknown schema refused"):
        ci.import_catalog(make_xlsx({"Sheet1": [["a", "b"], ["1", "2"]]}), "x.xlsx")


def test_profile_for_wrong_file_is_refused():
    with pytest.raises(ci.CatalogImportError, match="does not match profile"):
        ci.import_catalog(make_xlsx({"IDEAS": [["a"], ["1"]]}), "x.xlsx", profile=TEST_PROFILE)


def test_formula_in_entry_sheet_is_refused_but_context_formula_is_recorded_unevaluated():
    base = {"IDEAS": [HEADER, row("T-1", "a")], "VIEW": [["ID"], ["T-1"]], "CONTROLS": [HEADER],
            "SOURCES": [["SID", "Title"], ["S1", "x"]], "README": [["v"], ["3"]]}
    led = imp(make_xlsx(base, formulas={"README": {"A2": "SUM(1,2)"}}))
    assert led["context_sheets"]["README"]["formula_cells"] == {"A2": "SUM(1,2)"}
    with pytest.raises(ci.CatalogImportError, match="formula cell"):
        imp(make_xlsx(base, formulas={"IDEAS": {"B2": "1+1"}}))


def test_container_and_extension_refusals(monkeypatch):
    with pytest.raises(ci.CatalogImportError, match="not an xlsx"):
        ci.import_catalog(b"not a zip", "x.xlsx", profile=TEST_PROFILE)
    with pytest.raises(ci.CatalogImportError, match="unsupported catalog file type"):
        ci.import_catalog(b"x", "x.docx")
    with pytest.raises(ci.CatalogImportError, match="xlsx"):
        ci.import_catalog(mini_workbook(), "c.csv", profile=TEST_PROFILE)
    monkeypatch.setattr(xlsx_reader, "MAX_MEMBER_BYTES", 10)
    with pytest.raises(ci.CatalogImportError, match="size bound"):
        imp()


CSV_PROFILE = ci.CatalogProfile.from_json({
    "schema": ci.PROFILE_SCHEMA, "profile_id": "csv_t", "catalog_family": "fam", "file_format": "csv",
    "sheets": [{"sheet": "csv", "role": "primary", "id_column": "id", "entry_kind": "idea"}],
    "field_map": {"title": ["name"], "rule_text": ["seed"]}, "label_columns": ["status"]})


def test_csv_with_bom_quotes_and_embedded_newline():
    data = ('﻿id,name,seed,status\nR-1,"Gate, 200d","Hold above\nMA",IDEA ONLY\nR-2,Other,x,y\n').encode("utf-8")
    led = ci.import_catalog(data, "r.csv", profile=CSV_PROFILE)
    e = led["entries"][0]
    assert e["fields"]["title"] == "Gate, 200d" and e["fields"]["rule_text"] == "Hold above\nMA"
    assert e["labels"] == {"status": "IDEA ONLY"} and led["counts"]["entries"] == 2


def test_csv_copy_reconciliation_reports_differences_explicitly():
    a = ci.import_catalog(b"id,name,seed,status\nR-1,A,x,s\nR-2,B,y,s\n", "a.csv", profile=CSV_PROFILE)
    b = ci.import_catalog(b"id,name,seed,status\nR-1,A,x,s\nR-2,B,CHANGED,s\nR-3,C,z,s\n", "b.csv", profile=CSV_PROFILE)
    rec = ci.reconcile_copies(a, b)
    assert rec["identical"] == ["R-1"] and rec["different"] == {"R-2": {"rule_text": ["y", "CHANGED"]}}
    assert rec["only_in_b"] == ["R-3"] and rec["only_in_a"] == []


def test_profile_validation_rejects_unsafe_profiles():
    base = TEST_PROFILE.to_json()
    with pytest.raises(ci.CatalogImportError, match="exactly one primary"):
        ci.CatalogProfile.from_json({**base, "sheets": base["sheets"][1:]})
    with pytest.raises(ci.CatalogImportError, match="only canonical fields"):
        ci.CatalogProfile.from_json({**base, "field_map": {**base["field_map"], "pnl": ["x"]}})
    with pytest.raises(ci.CatalogImportError, match="schema"):
        ci.CatalogProfile.from_json({**base, "schema": "nope"})
    assert ci.CatalogProfile.from_json(base) == TEST_PROFILE


def test_builtin_profiles_are_valid_and_distinct():
    ids = [p.profile_id for p in ci.BUILTIN_PROFILES]
    assert len(set(ids)) == len(ids) == 8 and len({p.identity() for p in ci.BUILTIN_PROFILES}) == 8
    for p in ci.BUILTIN_PROFILES:
        assert ci.CatalogProfile.from_json(p.to_json()) == p


DOWNLOADS = os.path.join(os.path.expanduser("~"), "Downloads")
REAL = sorted(glob.glob(os.path.join(DOWNLOADS, "MQD_*_2026-10-09.*")))


@pytest.mark.skipif(len(REAL) != 8, reason="operator catalogs not present on this machine")
def test_operator_catalogs_import_every_row():
    counts = {}
    for p in REAL:
        with open(p, "rb") as fh:
            led = ci.import_catalog(fh.read(), os.path.basename(p))
        counts[led["profile_id"]] = led["counts"]["entries"]
        assert all(e["original"] for e in led["entries"]) and led["trial_registered"] is False
    assert counts == {"academic_xlsx_v1": 121, "shocks_xlsx_v1": 86, "mechanisms_xlsx_v1": 36, "fourarea_xlsx_v1": 48,
                      "psychology_xlsx_v1": 26, "reddit_csv_v1": 147, "reddit_xlsx_v1": 147, "technical_xlsx_v1": 57}


def test_zip_bomb_total_size_and_dtd_entities_are_refused(monkeypatch):
    monkeypatch.setattr(xlsx_reader, "MAX_TOTAL_BYTES", 200)
    with pytest.raises(ci.CatalogImportError, match="total uncompressed"):
        imp()
    monkeypatch.undo()
    import io
    import zipfile
    raw = mini_workbook()
    zin = zipfile.ZipFile(io.BytesIO(raw))
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zout:
        for item in zin.infolist():
            data = zin.read(item.filename)
            if item.filename == "xl/workbook.xml":
                data = data.replace(b"<workbook", b'<!DOCTYPE x [<!ENTITY a "aaaa">]><workbook', 1)
            zout.writestr(item.filename, data)
    with pytest.raises(ci.CatalogImportError, match="DTD"):
        ci.import_catalog(buf.getvalue(), "c.xlsx", profile=TEST_PROFILE)


def test_the_xlsx_fixture_bytes_do_not_depend_on_the_wall_clock(monkeypatch):
    sheets = {"IDEAS": [HEADER, row("D-1", "Deterministic", rule="Hold above the 50-day SMA, else cash")], "VIEW": [["ID", "Note"], ["D-1", "x"]],
              "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    import time
    monkeypatch.setattr(time, "time", lambda: 1_000_000_000.0)
    first = make_xlsx(sheets)
    monkeypatch.setattr(time, "time", lambda: 1_100_000_000.0)
    assert make_xlsx(sheets) == first
