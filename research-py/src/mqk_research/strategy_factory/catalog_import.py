"""Generic, hash-bound, read-only import of operator-supplied strategy-idea catalogs (untrusted data).

A catalog is accepted only through a `CatalogProfile` whose header signature matches the submitted file exactly;
an unknown schema is refused (never guessed). Every row of every declared entry sheet becomes an entry that keeps
the original cell strings verbatim next to the canonical fields. Subset sheets must be subsets of the primary sheet
(their extra columns are kept as views); sheets that are not declared as entry or source sheets are recorded
verbatim as context and never become entries. Nothing here registers a hypothesis, trial or attempt.

Operator contract for a new catalog: write a profile JSON (see `CatalogProfile.from_json`) and pass it explicitly;
the profile is part of the ledger identity, so the same file under a different profile is a different ledger.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from typing import Any, Mapping

from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes
from mqk_research.strategy_factory.xlsx_reader import CatalogReadError, read_csv, read_xlsx

LEDGER_SCHEMA = "strategy_factory_catalog_ledger_v1"
PROFILE_SCHEMA = "strategy_factory_catalog_profile_v1"

# Canonical fields an entry may carry. Anything else in a profile field_map is refused.
CANONICAL_FIELDS = (
    "title", "hypothesis", "mechanism", "rule_text", "assets", "horizon", "direction", "data_needed", "family",
    "complexity", "risk", "negative_control", "as_of", "source_refs", "source_urls", "source_finding",
    "source_limitation",
)
ROLES = ("primary", "subset", "additional")
ENTRY_KINDS = ("idea", "control")


class CatalogImportError(Exception):
    """Any reason a catalog is refused. The import never repairs, completes or partially accepts a source."""


@dataclass(frozen=True)
class SheetSpec:
    sheet: str
    role: str = "primary"          # primary: the entry list; subset: ids must be in primary; additional: more entries
    id_column: str = "ID"
    entry_kind: str = "idea"        # idea | control (a governance/control row is not a strategy idea)

    def validate(self) -> None:
        if self.role not in ROLES or self.entry_kind not in ENTRY_KINDS or not self.sheet or not self.id_column:
            raise CatalogImportError(f"invalid sheet spec {self!r}")


@dataclass(frozen=True)
class CatalogProfile:
    profile_id: str
    catalog_family: str                      # entries with the same (family, entry id) are the same idea across files
    file_format: str                         # xlsx | csv
    sheets: tuple[SheetSpec, ...]
    field_map: Mapping[str, tuple[str, ...]]  # canonical field -> candidate source columns (first non-empty wins)
    label_columns: tuple[str, ...] = ()      # the catalog's own classification columns, kept verbatim, never authority
    source_sheet: str | None = None          # optional sources table, joined on source_refs
    source_id_column: str = "ID"
    source_columns: tuple[str, ...] = ()
    required_sheets: tuple[str, ...] = ()

    def validate(self) -> None:
        if not self.profile_id or not self.catalog_family or self.file_format not in ("xlsx", "csv"):
            raise CatalogImportError("profile_id, catalog_family and a supported file_format are required")
        if sum(1 for s in self.sheets if s.role == "primary") != 1:
            raise CatalogImportError("exactly one primary sheet is required")
        for s in self.sheets:
            s.validate()
        unknown = set(self.field_map) - set(CANONICAL_FIELDS)
        if unknown or "title" not in self.field_map:
            raise CatalogImportError(f"field_map must map 'title' and only canonical fields (bad: {sorted(unknown)})")
        if self.file_format == "csv" and len(self.sheets) != 1:
            raise CatalogImportError("a csv profile has exactly one (primary) sheet")

    def to_json(self) -> dict[str, Any]:
        return {"schema": PROFILE_SCHEMA, "profile_id": self.profile_id, "catalog_family": self.catalog_family,
                "file_format": self.file_format,
                "sheets": [{"sheet": s.sheet, "role": s.role, "id_column": s.id_column, "entry_kind": s.entry_kind}
                           for s in self.sheets],
                "field_map": {k: list(v) for k, v in self.field_map.items()},
                "label_columns": list(self.label_columns), "source_sheet": self.source_sheet,
                "source_id_column": self.source_id_column, "source_columns": list(self.source_columns),
                "required_sheets": list(self.required_sheets)}

    @staticmethod
    def from_json(obj: Mapping[str, Any]) -> "CatalogProfile":
        if obj.get("schema") != PROFILE_SCHEMA:
            raise CatalogImportError(f"profile schema must be {PROFILE_SCHEMA!r}")
        prof = CatalogProfile(
            profile_id=obj["profile_id"], catalog_family=obj["catalog_family"], file_format=obj["file_format"],
            sheets=tuple(SheetSpec(**s) for s in obj["sheets"]),
            field_map={k: tuple(v) for k, v in obj["field_map"].items()},
            label_columns=tuple(obj.get("label_columns", ())), source_sheet=obj.get("source_sheet"),
            source_id_column=obj.get("source_id_column", "ID"), source_columns=tuple(obj.get("source_columns", ())),
            required_sheets=tuple(obj.get("required_sheets", ())))
        prof.validate()
        return prof

    def identity(self) -> str:
        return sha256_bytes(canonical_json(self.to_json()).encode("utf-8"))


def _prof(profile_id, family, fmt, sheets, field_map, labels=(), source_sheet=None, source_id_column="ID",
          source_columns=()):
    p = CatalogProfile(profile_id, family, fmt, tuple(sheets), {k: tuple(v) for k, v in field_map.items()},
                       tuple(labels), source_sheet, source_id_column, tuple(source_columns),
                       tuple(s.sheet for s in sheets) + ((source_sheet,) if source_sheet else ()))
    p.validate()
    return p


S = SheetSpec
BUILTIN_PROFILES: tuple[CatalogProfile, ...] = (
    _prof("reddit_xlsx_v1", "reddit_m1_2026-10-09", "xlsx",
          [S("ALL_147_IDEAS", "primary", "ID"), S("M1_EQUITY_ETF", "subset", "ID"), S("DEFERRED_ASSETS", "subset", "ID")],
          {"title": ["Idea / hypothesis"], "mechanism": ["Proposed market mechanism"], "rule_text": ["Minimal test seed"],
           "assets": ["Asset coverage"], "horizon": ["Holding horizon"], "direction": ["Direction"],
           "data_needed": ["Data dependency"], "family": ["Family"], "complexity": ["Complexity"],
           "risk": ["Main failure mode / caveat"], "source_urls": ["Direct source URL"]},
          labels=["MQD M1 disposition", "Idea provenance", "Proof status", "Subreddit", "Source post / thread"],
          source_sheet="SOURCES_64", source_id_column="Source ID",
          source_columns=["Subreddit", "Thread title", "Direct source URL", "Ideas linked"]),
    _prof("reddit_csv_v1", "reddit_m1_2026-10-09", "csv", [S("csv", "primary", "id")],
          {"title": ["name"], "mechanism": ["mechanism"], "rule_text": ["seed"], "assets": ["assets"],
           "horizon": ["horizon"], "direction": ["direction"], "data_needed": ["data"], "family": ["family"],
           "complexity": ["tier"], "risk": ["risk"], "source_urls": ["source_url"]},
          labels=["m1", "provenance", "status", "subreddit", "source_title"]),
    _prof("academic_xlsx_v1", "academic_official_2026-10-09", "xlsx",
          [S("IDEAS", "primary", "ID"), S("M1 SCREENS", "subset", "ID"), S("LATER ASSETS", "subset", "ID"),
           S("VALIDATION", "subset", "ID")],
          {"title": ["Idea / Question"], "hypothesis": ["Falsifiable Hypothesis"], "mechanism": ["Mechanism Family"],
           "assets": ["Assets"], "horizon": ["Horizon"], "direction": ["Long/Short"], "data_needed": ["Data Needed"],
           "family": ["Family Key"], "complexity": ["Tier"], "risk": ["Primary Risk"],
           "negative_control": ["Negative Control"], "source_refs": ["Source ID"], "source_urls": ["Source URL"],
           "source_finding": ["Actual Source Finding"], "source_limitation": ["Source Limitation"]},
          labels=["MQD M1 Classification", "Dedup Status", "Authority Status", "Connection to Paper", "Source Type"],
          source_sheet="SOURCES", source_id_column="ID",
          source_columns=["Title", "Authors", "Year", "Source Class", "Official/Publisher Link", "Limitations"]),
    _prof("shocks_xlsx_v1", "deep_dive_shocks_2026-10-09", "xlsx",
          [S("All Proposals", "primary", "ID"), S("A Shock Dynamics", "subset", "ID"), S("B Attention News", "subset", "ID"),
           S("C Institutional", "subset", "ID"), S("D Guardrails", "subset", "ID"), S("M1 Diagnostics", "subset", "ID")],
          {"title": ["Idea / diagnostic"], "hypothesis": ["Falsifiable research question"],
           "mechanism": ["Proposed mechanism"], "rule_text": ["Recommended falsification"],
           "data_needed": ["Data needed"], "horizon": ["Horizon contract"], "family": ["Group"],
           "as_of": ["Point-in-time / causal execution"], "negative_control": ["Rival explanation"],
           "risk": ["Principal caveat"], "source_refs": ["Source IDs"], "source_urls": ["Primary source links"]},
          labels=["Provisional M1 / later fit", "Preliminary priority", "Novelty status", "Authority", "Source relation"],
          source_sheet="Sources", source_id_column="ID",
          source_columns=["Title / organization", "URL", "What source actually supports", "Evidence type"]),
    _prof("mechanisms_xlsx_v1", "deep_mechanisms_behavior_news_2026-10-09", "xlsx",
          [S("Research Hypotheses", "primary", "ID")],
          {"title": ["Idea / test"], "hypothesis": ["Predeclared question"], "mechanism": ["Mechanism"],
           "rule_text": ["Observable proxy"], "data_needed": ["Minimum data"], "family": ["Family"],
           "as_of": ["Point-in-time / chronology"], "negative_control": ["Falsification / negative control"],
           "complexity": ["Complexity 1-5"], "source_refs": ["Primary source IDs"],
           "source_urls": ["Primary source URL(s)"]},
          labels=["M1 eligibility (provisional)", "Evidence category", "Overlap / caution", "Current status"],
          source_sheet="Primary Sources", source_id_column="Source ID",
          source_columns=["Title", "Authors / Organization", "Year", "Type", "Primary URL", "Limit"]),
    _prof("fourarea_xlsx_v1", "four_area_deep_dive_2026-10-09", "xlsx",
          [S("Four Research Areas", "primary", "ID"), S("M1 Triage", "subset", "ID"),
           S("Research Controls", "additional", "ID", "control")],
          {"title": ["Hypothesis / diagnostic"], "hypothesis": ["Falsifiable question"],
           "mechanism": ["Proposed mechanism"], "rule_text": ["Observed features / study design"],
           "data_needed": ["Minimum data"], "family": ["Area"], "as_of": ["Point-in-time / execution constraint"],
           "negative_control": ["Competing explanations / negative controls"], "complexity": ["Complexity 1-5"],
           "source_refs": ["Primary source IDs"], "source_urls": ["Source URL(s)"]},
          labels=["M1 classification", "Novelty status", "Research status"],
          source_sheet="Primary Sources", source_id_column="Source ID",
          source_columns=["Author / authority", "Work", "Year", "URL", "What the source supports"]),
    _prof("psychology_xlsx_v1", "investor_psychology_order_flow_2026-10-09", "xlsx",
          [S("Mechanism Probes", "primary", "id"), S("M1 First Look", "subset", "id")],
          {"title": ["title"], "hypothesis": ["question"], "mechanism": ["mechanism"], "data_needed": ["data"],
           "family": ["group"], "as_of": ["asof"], "negative_control": ["control"], "complexity": ["tier"],
           "source_refs": ["source_ids"], "source_urls": ["source_urls"]},
          labels=["fit", "kind", "novelty", "notes", "status"],
          source_sheet="Primary Sources", source_id_column="source_id",
          source_columns=["authors_year", "title", "type", "source_url", "source_supported_point"]),
    _prof("technical_xlsx_v1", "technical_market_mechanism_2026-10-09", "xlsx",
          [S("ALL 57 IDEAS", "primary", "ID"), S("M1 TRIAGE", "subset", "ID")],
          {"title": ["Research idea"], "hypothesis": ["Falsifiable question"], "mechanism": ["Why it might exist"],
           "rule_text": ["Initial test (not strategy)"], "data_needed": ["Data needed"], "family": ["Area"],
           "as_of": ["As-of rule"], "negative_control": ["Negative control"], "risk": ["Main failure risk"],
           "assets": ["Asset"], "horizon": ["Horizon"], "complexity": ["Level"], "source_refs": ["Source IDs"],
           "source_urls": ["Public source URLs"]},
          labels=["M1 status", "Source connection", "Novelty verdict", "Status", "Evidence status"],
          source_sheet="PRIMARY SOURCES", source_id_column="Source ID",
          source_columns=["Document or site", "Author / issuer", "Source category", "Verified public URL"]),
)


def profile_by_id(profile_id: str) -> CatalogProfile:
    for p in BUILTIN_PROFILES:
        if p.profile_id == profile_id:
            return p
    raise CatalogImportError(f"unknown built-in profile {profile_id!r}")


def _primary(profile: CatalogProfile) -> SheetSpec:
    return next(s for s in profile.sheets if s.role == "primary")


def _header_ok(profile: CatalogProfile, sheets: Mapping[str, list[list[str]]]) -> bool:
    for name in profile.required_sheets:
        if name not in sheets or not sheets[name]:
            return False
    for spec in profile.sheets:
        header = sheets[spec.sheet][0]
        if spec.id_column not in header:
            return False
        if spec.role != "subset":
            wanted = {c for cols in profile.field_map.values() for c in cols}
            # every mapped column must exist on the sheets that carry full entries, except optional extras
            if spec.role == "primary" and not wanted <= set(header):
                return False
    return True


def detect_profile(sheets: Mapping[str, list[list[str]]] | None, csv_rows: list[list[str]] | None,
                   candidates=BUILTIN_PROFILES) -> CatalogProfile:
    matches = []
    for p in candidates:
        if p.file_format == "csv" and csv_rows:
            wanted = {c for cols in p.field_map.values() for c in cols} | {_primary(p).id_column}
            if wanted <= set(csv_rows[0]):
                matches.append(p)
        elif p.file_format == "xlsx" and sheets and _header_ok(p, sheets):
            matches.append(p)
    if len(matches) != 1:
        found = (sorted(sheets) if sheets else csv_rows[0] if csv_rows else [])
        raise CatalogImportError(
            f"{len(matches)} profiles match this file (need exactly one); unknown schema refused. Detected: {found}")
    return matches[0]


def _split_refs(value: str) -> list[str]:
    return [t for t in re.split(r"[,;\s|]+", value.strip()) if t]


def _split_urls(value: str) -> list[str]:
    return [t for t in re.split(r"[|;\s]+", value.strip()) if t.startswith(("http://", "https://", "doi:", "10."))]


def _table(rows: list[list[str]], spec: SheetSpec, sheet_label: str):
    if not rows:
        raise CatalogImportError(f"sheet {sheet_label!r} is empty")
    header = rows[0]
    if any(not h.strip() for h in header) or len(set(header)) != len(header):
        raise CatalogImportError(f"sheet {sheet_label!r} header must be unique non-empty columns")
    idx = header.index(spec.id_column) if spec.id_column in header else -1
    if idx < 0:
        raise CatalogImportError(f"sheet {sheet_label!r} has no id column {spec.id_column!r}")
    body = []
    for n, raw in enumerate(rows[1:], start=2):
        if len(raw) > len(header) and any(c.strip() for c in raw[len(header):]):
            raise CatalogImportError(f"sheet {sheet_label!r} row {n} is wider than its header")
        row = (raw + [""] * len(header))[:len(header)]
        if not any(c.strip() for c in row):
            continue                        # a fully empty row carries nothing
        if not row[idx].strip():
            raise CatalogImportError(f"sheet {sheet_label!r} row {n} has content but no {spec.id_column!r}")
        body.append((n, dict(zip(header, row)), row[idx].strip()))
    ids = [e[2] for e in body]
    if len(set(ids)) != len(ids):
        dup = sorted({i for i in ids if ids.count(i) > 1})
        raise CatalogImportError(f"sheet {sheet_label!r} repeats ids {dup[:5]}")
    return header, body


def _canonical_fields(profile: CatalogProfile, original: Mapping[str, str]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for fname, cols in profile.field_map.items():
        val = next((original[c].strip() for c in cols if c in original and original[c].strip()), "")
        out[fname] = val
    out["source_refs"] = _split_refs(out.get("source_refs", "")) if "source_refs" in out else []
    out["source_urls"] = _split_urls(out.get("source_urls", "")) if "source_urls" in out else []
    return out


def import_catalog(data: bytes, filename: str, *, profile: CatalogProfile | None = None) -> dict[str, Any]:
    """Return the canonical, hash-identified catalog ledger. Raises CatalogImportError on any refusal."""
    if filename.lower().endswith(".csv"):
        fmt = "csv"
    elif filename.lower().endswith(".xlsx"):
        fmt = "xlsx"
    else:
        raise CatalogImportError(f"unsupported catalog file type: {filename!r}")
    if profile is not None:
        profile.validate()
        if profile.file_format != fmt:
            raise CatalogImportError(f"profile {profile.profile_id!r} is {profile.file_format}, file is {fmt}")
    try:
        if fmt == "xlsx":
            sheets, formulas = read_xlsx(data)
            csv_rows = None
        else:
            sheets, formulas, csv_rows = None, {}, read_csv(data)
    except CatalogReadError as exc:
        raise CatalogImportError(str(exc)) from exc
    if profile is None:
        profile = detect_profile(sheets, csv_rows)
    else:
        if fmt == "xlsx" and not _header_ok(profile, sheets):
            raise CatalogImportError(f"file does not match profile {profile.profile_id!r}")
    if fmt == "csv":
        sheets = {profile.sheets[0].sheet: csv_rows}
    primary = _primary(profile)
    entries: dict[str, dict[str, Any]] = {}
    order: list[str] = []
    views: dict[str, dict[str, Any]] = {}
    declared = {s.sheet for s in profile.sheets} | ({profile.source_sheet} if profile.source_sheet else set())
    for spec in sorted(profile.sheets, key=lambda s: {"primary": 0, "additional": 1, "subset": 2}[s.role]):
        header, body = _table(sheets[spec.sheet], spec, spec.sheet)
        if spec.role in ("primary", "additional"):
            for n, original, eid in body:
                if eid in entries:
                    raise CatalogImportError(f"id {eid!r} appears on more than one entry sheet")
                fields = _canonical_fields(profile, original)
                if not fields.get("title") and spec.entry_kind == "idea":
                    raise CatalogImportError(f"{eid}: empty title on sheet {spec.sheet!r}")
                entries[eid] = {"entry_id": eid, "sheet": spec.sheet, "row_number": n, "entry_kind": spec.entry_kind,
                                "fields": fields, "original": original,
                                "labels": {c: original[c].strip() for c in profile.label_columns if c in original},
                                "views": {}}
                order.append(eid)
        else:
            for n, original, eid in body:
                if eid not in entries:
                    raise CatalogImportError(f"subset sheet {spec.sheet!r} lists {eid!r}, absent from the primary sheet")
                views.setdefault(eid, {})[spec.sheet] = {"row_number": n, "original": original}
    for eid, v in views.items():
        entries[eid]["views"] = v
    # entry order is primary-sheet order first, then additional sheets, each in source row order
    ordered = [entries[e] for e in sorted(order, key=lambda e: (entries[e]["sheet"] != primary.sheet,
                                                                [s.sheet for s in profile.sheets].index(entries[e]["sheet"]),
                                                                entries[e]["row_number"]))]
    sources: dict[str, Any] = {}
    if profile.source_sheet:
        sspec = SheetSpec(profile.source_sheet, "additional", profile.source_id_column)
        _, sbody = _table(sheets[profile.source_sheet], sspec, profile.source_sheet)
        for n, original, sid in sbody:
            sources[sid] = {"row_number": n, **{c: original.get(c, "") for c in profile.source_columns}}
    for e in ordered:
        e["fields"]["unresolved_source_refs"] = sorted(r for r in e["fields"].get("source_refs", []) if sources and r not in sources)
        e["content_hash"] = sha256_bytes(canonical_json({"f": e["fields"], "o": e["original"]}).encode("utf-8"))
        e["canonical_hash"] = sha256_bytes(canonical_json(
            {k: e["fields"].get(k, "") for k in CANONICAL_FIELDS if k not in ("source_refs", "source_urls")}).encode("utf-8"))
    context = {name: {"rows": rows, "formula_cells": formulas.get(name, {})}
               for name, rows in sheets.items() if name not in declared}
    ledger = {
        "schema": LEDGER_SCHEMA,
        "catalog_family": profile.catalog_family,
        "profile_id": profile.profile_id,
        "profile_identity": profile.identity(),
        "source": {"filename": filename, "format": fmt, "sha256": sha256_bytes(data), "bytes": len(data)},
        "entries": ordered,
        "sources": sources,
        "context_sheets": context,
        "formula_cells_in_entry_sheets": {n: f for n, f in formulas.items() if n in declared},
        "counts": {"entries": len(ordered), "ideas": sum(1 for e in ordered if e["entry_kind"] == "idea"),
                   "controls": sum(1 for e in ordered if e["entry_kind"] == "control"), "sources": len(sources),
                   "context_sheets": len(context)},
        "trial_registered": False,
        "economic_attempt": False,
        "authority": "UNTRUSTED_IDEA_INTAKE",
    }
    if ledger["formula_cells_in_entry_sheets"]:
        raise CatalogImportError("formula cell in an entry/source sheet: refused")
    ledger["ledger_sha256"] = sha256_bytes(canonical_json({k: v for k, v in ledger.items() if k != "ledger_sha256"}).encode("utf-8"))
    return ledger


def reconcile_copies(a: Mapping[str, Any], b: Mapping[str, Any]) -> dict[str, Any]:
    """Two ledgers of one catalog family (for example the xlsx and csv copies): per shared entry id, are the canonical
    fields identical? A difference is reported explicitly and both copies stay authoritative sources of record."""
    if a["catalog_family"] != b["catalog_family"]:
        raise CatalogImportError("ledgers belong to different catalog families")
    ea, eb = {e["entry_id"]: e for e in a["entries"]}, {e["entry_id"]: e for e in b["entries"]}
    same, differ = [], {}
    for eid in sorted(set(ea) & set(eb)):
        fa, fb = ea[eid]["fields"], eb[eid]["fields"]
        keys = set(fa) & set(fb) - {"unresolved_source_refs", "source_refs", "source_urls"}
        diff = {k: [fa[k], fb[k]] for k in sorted(keys) if fa[k] != fb[k]}
        (differ.__setitem__(eid, diff) if diff else same.append(eid))
    return {"identical": same, "different": differ, "only_in_a": sorted(set(ea) - set(eb)),
            "only_in_b": sorted(set(eb) - set(ea))}
