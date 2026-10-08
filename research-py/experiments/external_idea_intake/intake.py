"""Hash-bound, read-only intake of the external strategy-idea catalog (untrusted data).

The workbook is opened as bytes only: no formula is evaluated, no macro or external link is followed,
no price data or provider is touched, and cell text never becomes an instruction. `freeze` verifies the
exact SHA-256 BEFORE parsing, then the structure, and returns a canonical ledger that keeps every
original cell string verbatim and separate from any later normalization. It registers nothing: an
intake row is an idea, not a hypothesis, trial, edge claim or Promotion authority.

    python intake.py <workbook.xlsx> --out <dir>
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

SCHEMA = "external_idea_intake_ledger_v1"
STATUS = "UNTRUSTED_IDEA_INTAKE"
EXPECTED_SHA256 = "6fc945a873733cda6a1552049923a153ed2f7213c488c6d5f07ffeb8077a37f3"
EXPECTED_SHEETS = ("Strategy_Catalog", "Summary", "Source_Index", "Asset_Test_Matrix",
                   "Testing_Guardrails", "README")
CATALOG_SHEET = "Strategy_Catalog"
EXPECTED_ROWS = 200
EXPECTED_COLUMNS = 26
ID_RE = re.compile(r"^EXT-(\d{3})$")
LEDGER_NAME = "external_idea_catalog_ledger_v1.json"

_NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
       "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
       "pr": "http://schemas.openxmlformats.org/package/2006/relationships"}
_MAX_MEMBER_BYTES = 32 * 1024 * 1024
_MAX_MEMBERS = 256


class IntakeError(Exception):
    """Any reason the workbook is refused; the intake never repairs or completes a source."""


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _col_index(ref: str) -> int:
    letters = re.match(r"^([A-Z]+)\d+$", ref)
    if not letters:
        raise IntakeError(f"bad cell reference {ref!r}")
    n = 0
    for ch in letters.group(1):
        n = n * 26 + (ord(ch) - 64)
    return n - 1


def _read_member(zf: zipfile.ZipFile, name: str) -> bytes:
    info = zf.getinfo(name)
    if info.file_size > _MAX_MEMBER_BYTES:
        raise IntakeError(f"member {name!r} exceeds the size bound")
    return zf.read(name)


def _text(si: ET.Element) -> str:
    return "".join(t.text or "" for t in si.iter(f"{{{_NS['m']}}}t"))


def read_workbook(data: bytes) -> dict[str, list[list[str]]]:
    """Return {sheet name: rows of cell strings} in workbook order. Formula cells are refused."""
    import io
    try:
        zf = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile as exc:
        raise IntakeError("not an xlsx package") from exc
    with zf:
        if len(zf.infolist()) > _MAX_MEMBERS:
            raise IntakeError("too many package members")
        names = set(zf.namelist())
        for need in ("xl/workbook.xml", "xl/_rels/workbook.xml.rels"):
            if need not in names:
                raise IntakeError(f"missing package part {need}")
        shared: list[str] = []
        if "xl/sharedStrings.xml" in names:
            root = ET.fromstring(_read_member(zf, "xl/sharedStrings.xml"))
            shared = [_text(si) for si in root.findall("m:si", _NS)]
        rels = {r.get("Id"): r.get("Target")
                for r in ET.fromstring(_read_member(zf, "xl/_rels/workbook.xml.rels")).findall("pr:Relationship", _NS)}
        wb = ET.fromstring(_read_member(zf, "xl/workbook.xml"))
        out: dict[str, list[list[str]]] = {}
        for sheet in wb.findall("m:sheets/m:sheet", _NS):
            name = sheet.get("name")
            target = rels.get(sheet.get(f"{{{_NS['r']}}}id"))
            if not name or not target or name in out:
                raise IntakeError("unresolvable or duplicate sheet entry")
            part = target.lstrip("/") if target.startswith("/") else f"xl/{target}"
            if part not in names:
                raise IntakeError(f"sheet part {part!r} missing")
            out[name] = _read_sheet(_read_member(zf, part), shared, name)
        return out


def _read_sheet(xml: bytes, shared: list[str], sheet: str) -> list[list[str]]:
    root = ET.fromstring(xml)
    rows: list[list[str]] = []
    for row in root.findall("m:sheetData/m:row", _NS):
        cells: dict[int, str] = {}
        for c in row.findall("m:c", _NS):
            if c.find("m:f", _NS) is not None:
                raise IntakeError(f"formula cell in sheet {sheet!r}: refused")
            kind = c.get("t")
            if kind == "s":
                v = c.find("m:v", _NS)
                if v is None or v.text is None or not v.text.isdigit() or int(v.text) >= len(shared):
                    raise IntakeError(f"bad shared-string index in sheet {sheet!r}")
                text = shared[int(v.text)]
            elif kind == "inlineStr":
                is_ = c.find("m:is", _NS)
                text = _text(is_) if is_ is not None else ""
            elif kind in (None, "n", "str", "b"):
                v = c.find("m:v", _NS)
                text = v.text if v is not None and v.text is not None else ""
            else:
                raise IntakeError(f"unsupported cell type {kind!r} in sheet {sheet!r}")
            cells[_col_index(c.get("r") or "")] = text
        width = max(cells) + 1 if cells else 0
        rows.append([cells.get(i, "") for i in range(width)])
    return rows


def _catalog_table(rows: list[list[str]], expected_rows: int, expected_columns: int):
    if not rows:
        raise IntakeError("empty catalog sheet")
    header, body = rows[0], rows[1:]
    if len(header) != expected_columns or any(not h.strip() for h in header) or len(set(header)) != len(header):
        raise IntakeError(f"catalog header must be {expected_columns} unique non-empty columns")
    if len(body) != expected_rows:
        raise IntakeError(f"catalog must have exactly {expected_rows} rows, found {len(body)}")
    padded = [r + [""] * (expected_columns - len(r)) for r in body]
    if any(len(r) != expected_columns for r in padded):
        raise IntakeError("catalog row wider than the header")
    id_cols = [i for i in range(expected_columns) if all(ID_RE.match(r[i]) for r in padded)]
    if len(id_cols) != 1:
        raise IntakeError("exactly one column must hold EXT-nnn identifiers for every row")
    ids = [r[id_cols[0]] for r in padded]
    if ids != [f"EXT-{i:03d}" for i in range(1, expected_rows + 1)]:
        raise IntakeError(f"identifiers must be unique, contiguous and ordered EXT-001..EXT-{expected_rows:03d}")
    return header, padded, id_cols[0]


def freeze(data: bytes, *, expected_sha256: str = EXPECTED_SHA256, expected_sheets=EXPECTED_SHEETS,
           expected_rows: int = EXPECTED_ROWS, expected_columns: int = EXPECTED_COLUMNS) -> dict:
    """Verify the bytes, then the structure; return the canonical ledger. Hash first, parse second."""
    digest = sha256_hex(data)
    if digest != expected_sha256:
        raise IntakeError(f"sha256 mismatch: expected {expected_sha256}, got {digest}")
    sheets = read_workbook(data)
    if tuple(sheets) != tuple(expected_sheets):
        raise IntakeError(f"sheet set/order mismatch: {list(sheets)}")
    header, body, id_col = _catalog_table(sheets[CATALOG_SHEET], expected_rows, expected_columns)
    return {
        "schema": SCHEMA,
        "status": STATUS,
        "trial_registered": False,
        "economic_attempt": False,
        "source_sha256": digest,
        "source_bytes": len(data),
        "id_column": header[id_col],
        "columns": header,
        "rows": [{"ext_id": r[id_col], "original": dict(zip(header, r))} for r in body],
        "other_sheets": {n: sheets[n] for n in expected_sheets if n != CATALOG_SHEET},
    }


def canonical_json(ledger: dict) -> bytes:
    return (json.dumps(ledger, sort_keys=False, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")


def main(argv=None, *, expected_sha256: str = EXPECTED_SHA256) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("workbook", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args(argv)
    try:
        ledger = freeze(args.workbook.read_bytes(), expected_sha256=expected_sha256)
    except (IntakeError, OSError, ET.ParseError) as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    body = canonical_json(ledger)
    (args.out / LEDGER_NAME).write_bytes(body)
    print(f"{LEDGER_NAME} sha256={sha256_hex(body)} rows={len(ledger['rows'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
