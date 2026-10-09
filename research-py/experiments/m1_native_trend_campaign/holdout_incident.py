"""Append-only Final-Holdout access-incident ledger (`m1_holdout_access_incidents_v1`).

A per-run `research_holdout_ledger` row proves only what that run recorded. It cannot show that no
other process read the reserved window, so incident truth lives in a committed, hash-chained ledger
that every consequential entry point consults.

Event classes are kept apart and never inferred from one another: provider access, OHLCV parsing,
human price inspection, strategy evaluation, trial/attempt registration, formal holdout consumption.
An adjudication is a NEW entry referencing the incident; existing entries are never edited (the chain
and the pinned genesis hash make an edit detectable).
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
INCIDENT_FILE = HERE / "HOLDOUT_ACCESS_INCIDENTS.json"
SCHEMA = "m1_holdout_access_incidents_v1"
GENESIS_PREV = "0" * 64

PENDING = "ACCESS_INCIDENT_PENDING_ADJUDICATION"
ADJUDICATED_PRESERVED = "ADJUDICATED_HOLDOUT_STATUS_PRESERVED"
ADJUDICATED_CONSUMED = "ADJUDICATED_HOLDOUT_CONSUMED"
STATES = (PENDING, ADJUDICATED_PRESERVED, ADJUDICATED_CONSUMED)
EVENT_CLASSES = ("provider_access", "ohlcv_parsing", "human_price_inspection", "strategy_evaluation",
                 "trial_attempt_registration", "formal_holdout_consumption")
EVENT_VALUES = ("OCCURRED", "NOT_REPORTED", "NOT_OCCURRED_PER_RECORD")


class IncidentLedgerError(RuntimeError):
    """The incident ledger is missing, malformed, or fails its hash chain: fail closed."""


def entry_sha256(entry: dict) -> str:
    body = {k: v for k, v in entry.items() if k != "entry_sha256"}
    return hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()


def verify_chain(entries: list[dict]) -> list[dict]:
    if not entries:
        raise IncidentLedgerError("incident ledger has no entries")
    prev = GENESIS_PREV
    seen = set()
    for i, e in enumerate(entries):
        if e.get("prev_entry_sha256") != prev:
            raise IncidentLedgerError(f"entry {i} does not chain to its predecessor")
        if e.get("entry_sha256") != entry_sha256(e):
            raise IncidentLedgerError(f"entry {i} content does not match its recorded hash")
        if e.get("sequence") != i + 1:
            raise IncidentLedgerError(f"entry {i} has sequence {e.get('sequence')!r}")
        if e.get("state") not in STATES:
            raise IncidentLedgerError(f"entry {i} has unknown state {e.get('state')!r}")
        iid = e.get("incident_id")
        if not iid:
            raise IncidentLedgerError(f"entry {i} has no incident_id")
        if e.get("kind") == "OPEN":
            if iid in seen:
                raise IncidentLedgerError(f"incident {iid} opened twice")
            seen.add(iid)
            if set(e.get("events", {})) != set(EVENT_CLASSES) or any(
                    v.get("status") not in EVENT_VALUES for v in e["events"].values()):
                raise IncidentLedgerError(f"entry {i} event classes are incomplete or invalid")
        elif e.get("kind") == "ADJUDICATION":
            if iid not in seen:
                raise IncidentLedgerError(f"adjudication {i} references an unopened incident")
            if e["state"] == PENDING:
                raise IncidentLedgerError(f"adjudication {i} cannot leave the incident pending")
        else:
            raise IncidentLedgerError(f"entry {i} has unknown kind {e.get('kind')!r}")
        prev = e["entry_sha256"]
    return entries


def load_ledger(path: Path = INCIDENT_FILE) -> list[dict]:
    try:
        doc = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise IncidentLedgerError(f"incident ledger unreadable: {exc}") from exc
    if doc.get("schema") != SCHEMA or not isinstance(doc.get("entries"), list):
        raise IncidentLedgerError("incident ledger schema mismatch")
    return verify_chain(doc["entries"])


def incident_states(entries: list[dict]) -> dict[str, str]:
    """Latest state per incident (a later adjudication entry supersedes, never edits, the opening)."""
    return {e["incident_id"]: e["state"] for e in entries}


def _opening(entries: list[dict], incident_id: str) -> dict:
    return next(e for e in entries if e["incident_id"] == incident_id and e["kind"] == "OPEN")


def reserved_window(decl: dict) -> tuple[pd.Timestamp, pd.Timestamp] | None:
    """The declaration's reserved window [start, end): the fixed partition boundary when declared, else
    the month-aligned derivation the historical bridge uses (end of the requested range minus the holdout
    months, ending at the month-aligned range end). None when there is no partition/holdout to protect."""
    part = decl.get("partition") or {}
    fixed = part.get("holdout_boundary")
    if isinstance(fixed, dict) and fixed.get("holdout_start_utc") and fixed.get("holdout_end_utc"):
        return pd.Timestamp(fixed["holdout_start_utc"]).tz_convert("UTC"), pd.Timestamp(fixed["holdout_end_utc"]).tz_convert("UTC")
    end = (decl.get("data") or {}).get("end_utc")
    months = part.get("holdout_months")
    if end is None or months is None:
        return None
    e = pd.Timestamp(end)
    e = e.tz_localize("UTC") if e.tzinfo is None else e.tz_convert("UTC")
    last = e - pd.Timedelta(seconds=1)
    month_end = pd.Timestamp(year=last.year, month=last.month, day=1, tz="UTC") + pd.DateOffset(months=1)
    return month_end - pd.DateOffset(months=int(months)), month_end


def affecting_incidents(decl: dict, entries: list[dict] | None = None, *, states: tuple = (PENDING,)) -> list[str]:
    """Incident ids in `states` whose affected symbols intersect the declaration's universe and whose
    affected window intersects the declaration's reserved holdout window."""
    entries = load_ledger() if entries is None else verify_chain(entries)
    window = reserved_window(decl)
    if window is None:
        return []
    start, end = window
    symbols = set((decl.get("universe") or {}).get("symbols") or [])
    out = []
    for iid, state in incident_states(entries).items():
        if state not in states:
            continue
        aff = _opening(entries, iid)["affected"]
        if not symbols.intersection(aff["symbols"]):
            continue
        if pd.Timestamp(aff["window_start_utc"]) < end and pd.Timestamp(aff["window_end_utc"]) > start:
            out.append(iid)
    return sorted(out)


def require_no_pending_incident(decl: dict, purpose: str, entries: list[dict] | None = None) -> None:
    pending = affecting_incidents(decl, entries)
    if pending:
        raise SystemExit(f"fail-closed: {purpose} is refused while Final-Holdout access incident(s) {pending} are "
                         f"{PENDING}; the holdout is not independently cleared")


def truth_summary(decl: dict, entries: list[dict] | None = None) -> dict:
    """What a report may state about the Final Holdout. This ledger can only block a clearance; it never
    grants one (an empty incident set is absence of a recorded incident, not proof of independence)."""
    pending = affecting_incidents(decl, entries)
    return {"access_incident_status": PENDING if pending else "NO_PENDING_INCIDENT_RECORDED",
            "pending_incident_ids": pending,
            "independence_certification_blocked": bool(pending)}
