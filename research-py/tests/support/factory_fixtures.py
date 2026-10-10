"""Synthetic catalog fixtures for Strategy Factory tests: a minimal xlsx writer (inline strings, optional formula cells)
and a tiny operator-style profile. No third-party spreadsheet library and no real catalog content."""

from __future__ import annotations

import io
import zipfile
from xml.sax.saxutils import escape

from mqk_research.strategy_factory.catalog_import import CatalogProfile, SheetSpec


def _col(i: int) -> str:
    s = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        s = chr(65 + r) + s
    return s


def _put(z: zipfile.ZipFile, name: str, data: str) -> None:
    """Fixed entry timestamp: the workbook bytes (and so the catalog file hash) must not depend on the wall clock."""
    z.writestr(zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0)), data, compress_type=zipfile.ZIP_DEFLATED)


def make_xlsx(sheets: dict[str, list[list[str]]], formulas: dict[str, dict[str, str]] | None = None) -> bytes:
    formulas = formulas or {}
    names = list(sheets)
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        _put(z, "[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>')
        wb = "".join(f'<sheet name="{escape(n)}" sheetId="{i+1}" r:id="rId{i+1}"/>' for i, n in enumerate(names))
        _put(z, "xl/workbook.xml", '<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
                   f'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{wb}</sheets></workbook>')
        rels = "".join(f'<Relationship Id="rId{i+1}" Type="x" Target="worksheets/sheet{i+1}.xml"/>' for i in range(len(names)))
        _put(z, "xl/_rels/workbook.xml.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + rels + "</Relationships>")
        for i, n in enumerate(names):
            rows = []
            for r, row_ in enumerate(sheets[n], start=1):
                cells = []
                for c, val in enumerate(row_):
                    ref = f"{_col(c)}{r}"
                    f = formulas.get(n, {}).get(ref)
                    fx = f"<f>{escape(f)}</f>" if f is not None else ""
                    cells.append(f'<c r="{ref}" t="inlineStr">{fx}<is><t xml:space="preserve">{escape(val)}</t></is></c>')
                rows.append(f'<row r="{r}">{"".join(cells)}</row>')
            _put(z, f"xl/worksheets/sheet{i+1}.xml", '<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
                       f'<sheetData>{"".join(rows)}</sheetData></worksheet>')
    return buf.getvalue()


HEADER = ["ID", "Idea", "Question", "Rule", "Assets", "Horizon", "Dir", "Data", "Area", "Src"]


def row(eid: str, idea: str, *, question="", rule="", assets="US ETFs", horizon="daily", direction="Long / flat",
        data="OHLCV", area="trend", src="S1") -> list[str]:
    return [eid, idea, question, rule, assets, horizon, direction, data, area, src]


TEST_PROFILE = CatalogProfile(
    profile_id="test_profile_v1", catalog_family="test_family", file_format="xlsx",
    sheets=(SheetSpec("IDEAS", "primary", "ID"), SheetSpec("VIEW", "subset", "ID"),
            SheetSpec("CONTROLS", "additional", "ID", "control")),
    field_map={"title": ("Idea",), "hypothesis": ("Question",), "rule_text": ("Rule",), "assets": ("Assets",),
               "horizon": ("Horizon",), "direction": ("Dir",), "data_needed": ("Data",), "family": ("Area",),
               "source_refs": ("Src",)},
    label_columns=("Area",), source_sheet="SOURCES", source_id_column="SID", source_columns=("Title",),
    required_sheets=("IDEAS", "VIEW", "CONTROLS", "SOURCES"))


def mini_workbook(**over) -> bytes:
    sheets = {
        "IDEAS": [HEADER, row("T-1", " 200-day SMA trend gate ", rule="Hold above the 200-day SMA, else cash", src="S1, S2"),
                  row("T-2", "Does value predict returns?", question="Do cheap stocks outperform?", assets="Futures"),
                  ["", "", "", "", "", "", "", "", "", ""],
                  row("T-3", "RSI(2) pullback", rule="Buy when RSI(2) < 10 inside an uptrend", src="S9")],
        "VIEW": [["ID", "Note"], ["T-1", "screened"]],
        "CONTROLS": [HEADER, row("C-1", "Novelty gate control")],
        "SOURCES": [["SID", "Title"], ["S1", "Paper one"], ["S2", "Paper two"]],
        "README": [["notes"], ["not an entry"]],
    }
    sheets.update(over)
    return make_xlsx(sheets)
