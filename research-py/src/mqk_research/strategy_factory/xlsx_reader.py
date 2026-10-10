"""Read-only xlsx/csv reading for untrusted catalogs: bytes in, strings out.

No formula is evaluated (the cached value is the cell text and the formula text is recorded separately), no macro or
external link is followed, and cell text never becomes an instruction.
"""

from __future__ import annotations

import csv
import io
import re
import zipfile
from xml.etree import ElementTree as ET

_NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
       "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
       "pr": "http://schemas.openxmlformats.org/package/2006/relationships"}
MAX_MEMBER_BYTES = 32 * 1024 * 1024
MAX_MEMBERS = 256
MAX_CSV_BYTES = 16 * 1024 * 1024


class CatalogReadError(Exception):
    """The bytes are not a readable, bounded catalog container; nothing is repaired or guessed."""


def _col_index(ref: str) -> int:
    m = re.match(r"^([A-Z]+)\d+$", ref)
    if not m:
        raise CatalogReadError(f"bad cell reference {ref!r}")
    n = 0
    for ch in m.group(1):
        n = n * 26 + (ord(ch) - 64)
    return n - 1


def _member(zf: zipfile.ZipFile, name: str) -> bytes:
    if zf.getinfo(name).file_size > MAX_MEMBER_BYTES:
        raise CatalogReadError(f"member {name!r} exceeds the size bound")
    return zf.read(name)


def _text(si: ET.Element) -> str:
    return "".join(t.text or "" for t in si.iter(f"{{{_NS['m']}}}t"))


def read_xlsx(data: bytes) -> tuple[dict[str, list[list[str]]], dict[str, dict[str, str]]]:
    """({sheet name: rows of cell strings}, {sheet name: {cell ref: formula text}}) in workbook order."""
    try:
        zf = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile as exc:
        raise CatalogReadError("not an xlsx package") from exc
    with zf:
        if len(zf.infolist()) > MAX_MEMBERS:
            raise CatalogReadError("too many package members")
        names = set(zf.namelist())
        for need in ("xl/workbook.xml", "xl/_rels/workbook.xml.rels"):
            if need not in names:
                raise CatalogReadError(f"missing package part {need}")
        try:
            shared: list[str] = []
            if "xl/sharedStrings.xml" in names:
                shared = [_text(si) for si in ET.fromstring(_member(zf, "xl/sharedStrings.xml")).findall("m:si", _NS)]
            rels = {r.get("Id"): r.get("Target") for r in
                    ET.fromstring(_member(zf, "xl/_rels/workbook.xml.rels")).findall("pr:Relationship", _NS)}
            wb = ET.fromstring(_member(zf, "xl/workbook.xml"))
            sheets: dict[str, list[list[str]]] = {}
            formulas: dict[str, dict[str, str]] = {}
            for sheet in wb.findall("m:sheets/m:sheet", _NS):
                name = sheet.get("name")
                target = rels.get(sheet.get(f"{{{_NS['r']}}}id"))
                if not name or not target or name in sheets:
                    raise CatalogReadError("unresolvable or duplicate sheet entry")
                part = target.lstrip("/") if target.startswith("/") else f"xl/{target}"
                if part not in names:
                    raise CatalogReadError(f"sheet part {part!r} missing")
                sheets[name], found = _read_sheet(_member(zf, part), shared, name)
                if found:
                    formulas[name] = found
        except ET.ParseError as exc:
            raise CatalogReadError(f"malformed xml: {exc}") from exc
    return sheets, formulas


def _read_sheet(xml: bytes, shared: list[str], sheet: str):
    root = ET.fromstring(xml)
    rows: list[list[str]] = []
    formulas: dict[str, str] = {}
    for row in root.findall("m:sheetData/m:row", _NS):
        cells: dict[int, str] = {}
        for c in row.findall("m:c", _NS):
            f = c.find("m:f", _NS)
            if f is not None:
                formulas[c.get("r") or ""] = f.text or ""
            kind = c.get("t")
            if kind == "s":
                v = c.find("m:v", _NS)
                if v is None or v.text is None or not v.text.isdigit() or int(v.text) >= len(shared):
                    raise CatalogReadError(f"bad shared-string index in sheet {sheet!r}")
                text = shared[int(v.text)]
            elif kind == "inlineStr":
                is_ = c.find("m:is", _NS)
                text = _text(is_) if is_ is not None else ""
            elif kind in (None, "n", "str", "b"):
                v = c.find("m:v", _NS)
                text = v.text if v is not None and v.text is not None else ""
            else:
                raise CatalogReadError(f"unsupported cell type {kind!r} in sheet {sheet!r}")
            cells[_col_index(c.get("r") or "")] = text
        rows.append([cells.get(i, "") for i in range((max(cells) + 1) if cells else 0)])
    return rows, formulas


def read_csv(data: bytes) -> list[list[str]]:
    if len(data) > MAX_CSV_BYTES:
        raise CatalogReadError("csv exceeds the size bound")
    try:
        text = data.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise CatalogReadError("csv is not utf-8") from exc
    try:
        return [list(r) for r in csv.reader(io.StringIO(text, newline=""), strict=True)]
    except csv.Error as exc:
        raise CatalogReadError(f"malformed csv: {exc}") from exc
