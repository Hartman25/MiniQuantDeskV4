"""Append-only Final-Holdout access-incident ledger (`m1_holdout_access_incidents_v1`).

A per-run `research_holdout_ledger` row proves only what that run recorded. It cannot show that no
other process read the reserved window, so incident truth lives in a committed ledger that every
consequential entry point consults.

Trust model, stated plainly. A public SHA-256 chain only detects accidental edits: anyone can rewrite an
entry and recompute the hashes. Authority therefore comes from two anchors that a file edit cannot move:

* every OPEN entry must equal a hash PINNED IN THIS SOURCE (`PINNED_OPENINGS`), so the committed incident
  cannot be rewritten, reclassified or replaced without a reviewed code change; and
* an ADJUDICATION counts only when it carries an HMAC-SHA256 signature under the operator-held secret
  `MQK_M1_INCIDENT_ADJUDICATION_KEY`. Without the key an adjudication is UNVERIFIED and ignored (the incident
  stays pending); with the key a bad signature is tampering and raises. The HMAC is a shared-secret check, not
  non-repudiation.

Event classes are kept apart and never inferred from one another. An adjudication is a NEW entry; existing
entries are never edited. State transitions only move forward: PENDING -> PRESERVED -> CONSUMED, or
PENDING -> CONSUMED. A CONSUMED window is never untouched independent evidence again, so it blocks exactly
like a pending incident.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
INCIDENT_FILE = HERE / "HOLDOUT_ACCESS_INCIDENTS.json"
SCHEMA = "m1_holdout_access_incidents_v1"
GENESIS_PREV = "0" * 64
KEY_ENV = "MQK_M1_INCIDENT_ADJUDICATION_KEY"
MIN_KEY_CHARS = 32
CLOCK_SKEW = timedelta(minutes=5)
# No adjudication can predate the incident's own recording day.
ADJUDICATION_FLOOR_UTC = datetime(2026, 10, 9, tzinfo=timezone.utc)

PENDING = "ACCESS_INCIDENT_PENDING_ADJUDICATION"
ADJUDICATED_PRESERVED = "ADJUDICATED_HOLDOUT_STATUS_PRESERVED"
ADJUDICATED_CONSUMED = "ADJUDICATED_HOLDOUT_CONSUMED"
STATES = (PENDING, ADJUDICATED_PRESERVED, ADJUDICATED_CONSUMED)
# Only PRESERVED, by an authenticated operator decision, clears independence. PENDING and CONSUMED never do.
BLOCKING_STATES = (PENDING, ADJUDICATED_CONSUMED)
_TRANSITIONS = {PENDING: {ADJUDICATED_PRESERVED, ADJUDICATED_CONSUMED}, ADJUDICATED_PRESERVED: {ADJUDICATED_CONSUMED},
                ADJUDICATED_CONSUMED: set()}
EVENT_CLASSES = ("provider_access", "ohlcv_parsing", "human_price_inspection", "strategy_evaluation",
                 "trial_attempt_registration", "formal_holdout_consumption")
EVENT_VALUES = ("OCCURRED", "NOT_REPORTED", "NOT_OCCURRED_PER_RECORD")

# sha256 of each committed opening entry. Changing a recorded fact, or opening a new incident, is a code change.
PINNED_OPENINGS = {"HOA-KISS-EXT032-01": "a0c48a20bb27882733c3c8fec20c79592c69792d86b768902c72bd22744b0d0e"}
PINNED_GENESIS_SHA256 = PINNED_OPENINGS["HOA-KISS-EXT032-01"]


class IncidentLedgerError(RuntimeError):
    """The incident ledger is missing, malformed, rewritten or forged: fail closed."""


def _canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


def entry_sha256(entry: dict) -> str:
    body = {k: v for k, v in entry.items() if k != "entry_sha256"}
    return hashlib.sha256(_canonical(body).encode("utf-8")).hexdigest()


def _signature(entry: dict, key: str) -> str:
    body = {k: v for k, v in entry.items() if k not in ("entry_sha256", "signature")}
    return hmac.new(key.encode("utf-8"), _canonical(body).encode("utf-8"), hashlib.sha256).hexdigest()


def _key(key: str | None) -> str | None:
    k = key if key is not None else os.environ.get(KEY_ENV)
    return k if k and len(k) >= MIN_KEY_CHARS else None


def _parse_utc(value) -> datetime:
    try:
        t = datetime.fromisoformat(value)
    except (TypeError, ValueError) as exc:
        raise IncidentLedgerError(f"unreadable timestamp {value!r}") from exc
    if t.tzinfo is None:
        raise IncidentLedgerError(f"timestamp {value!r} is not timezone-aware")
    return t.astimezone(timezone.utc)


def make_adjudication(entries: list[dict], incident_id: str, state: str, *, decision: str, operator: str,
                      approval_ref: str, key: str, now: datetime) -> dict:
    """Operator tool (needs the secret): the next signed ADJUDICATION entry. The controller never calls it
    outside tests; appending it to the committed ledger is the operator's act."""
    if len(key) < MIN_KEY_CHARS:
        raise IncidentLedgerError(f"{KEY_ENV} must be at least {MIN_KEY_CHARS} characters")
    entry = {"sequence": len(entries) + 1, "kind": "ADJUDICATION", "incident_id": incident_id, "state": state,
             "decision": decision, "operator": operator, "approval_ref": approval_ref,
             "adjudicated_utc": now.astimezone(timezone.utc).isoformat(), "prev_entry_sha256": entries[-1]["entry_sha256"]}
    entry["signature"] = _signature(entry, key)
    entry["entry_sha256"] = entry_sha256(entry)
    return entry


def verify_chain(entries: list[dict], *, key: str | None = None, now: datetime | None = None) -> list[dict]:
    """Structure, chain, pins, forward-only transitions, chronology and (when the secret is available)
    signatures. Raises IncidentLedgerError on any violation."""
    if not isinstance(entries, list) or not entries:
        raise IncidentLedgerError("incident ledger has no entries")
    key = _key(key)
    now = now or datetime.now(timezone.utc)
    prev, last_time = GENESIS_PREV, ADJUDICATION_FLOOR_UTC
    opened: set[str] = set()
    current: dict[str, str] = {}
    for i, e in enumerate(entries):
        if not isinstance(e, dict):
            raise IncidentLedgerError(f"entry {i} is not an object")
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
        kind = e.get("kind")
        if kind == "OPEN":
            if i == 0 and e["entry_sha256"] != PINNED_GENESIS_SHA256:
                raise IncidentLedgerError("the first entry is not the pinned committed incident")
            if PINNED_OPENINGS.get(iid) != e["entry_sha256"]:
                raise IncidentLedgerError(f"opening of {iid} does not match its pinned hash (rewritten or unpinned)")
            if iid in opened:
                raise IncidentLedgerError(f"incident {iid} opened twice")
            if e["state"] != PENDING:
                raise IncidentLedgerError(f"opening of {iid} must be {PENDING}")
            opened.add(iid)
            current[iid] = PENDING
            if set(e.get("events", {})) != set(EVENT_CLASSES) or any(
                    v.get("status") not in EVENT_VALUES for v in e["events"].values()):
                raise IncidentLedgerError(f"entry {i} event classes are incomplete or invalid")
        elif kind == "ADJUDICATION":
            if i == 0 or iid not in opened:
                raise IncidentLedgerError(f"adjudication {i} references an unopened incident")
            if e["state"] == PENDING or e["state"] not in _TRANSITIONS[current[iid]]:
                raise IncidentLedgerError(
                    f"adjudication {i}: {current[iid]} -> {e['state']} is not a permitted forward transition")
            if not e.get("operator") or not e.get("approval_ref") or not e.get("decision"):
                raise IncidentLedgerError(f"adjudication {i} names no operator, approval reference and decision")
            when = _parse_utc(e.get("adjudicated_utc"))
            if when < last_time:
                raise IncidentLedgerError(f"adjudication {i} is dated before its predecessor or the incident itself")
            if when > now + CLOCK_SKEW:
                raise IncidentLedgerError(f"adjudication {i} is dated in the future")
            last_time = when
            if key is not None and not hmac.compare_digest(str(e.get("signature", "")), _signature(e, key)):
                raise IncidentLedgerError(f"adjudication {i} signature does not verify")
            current[iid] = e["state"]
        else:
            raise IncidentLedgerError(f"entry {i} has unknown kind {kind!r}")
        prev = e["entry_sha256"]
    return entries


def load_ledger(path: Path = INCIDENT_FILE, *, key: str | None = None, now: datetime | None = None) -> list[dict]:
    try:
        doc = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise IncidentLedgerError(f"incident ledger unreadable: {exc}") from exc
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA or not isinstance(doc.get("entries"), list):
        raise IncidentLedgerError("incident ledger schema mismatch")
    return verify_chain(doc["entries"], key=key, now=now)


def incident_states(entries: list[dict], *, key: str | None = None) -> dict[str, str]:
    """Effective state per incident. An adjudication counts ONLY when authenticated with the operator secret;
    without it the incident stays PENDING (fail closed)."""
    return _effective(entries, key)[0]


def unverified_adjudications(entries: list[dict], *, key: str | None = None) -> list[str]:
    """Incident ids that carry an adjudication this process could not authenticate (no secret available)."""
    return _effective(entries, key)[1]


def _effective(entries: list[dict], key: str | None) -> tuple[dict[str, str], list[str]]:
    key = _key(key)
    states: dict[str, str] = {}
    unverified: list[str] = []
    for e in entries:
        iid = e["incident_id"]
        if e["kind"] == "OPEN":
            states[iid] = PENDING
        elif key is not None:  # verify_chain already refused a bad signature when the key was available
            states[iid] = e["state"]
        else:
            unverified.append(iid)
    return states, sorted(set(unverified))


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


def affecting_incidents(decl: dict, entries: list[dict] | None = None, *, states: tuple = BLOCKING_STATES,
                       key: str | None = None) -> list[str]:
    """Incident ids in `states` (default: every state that blocks independence, i.e. pending or consumed) whose
    affected symbols intersect the declaration's universe and whose affected window intersects its reserved
    holdout window."""
    entries = load_ledger(key=key) if entries is None else verify_chain(entries, key=key)
    window = reserved_window(decl)
    if window is None:
        return []
    start, end = window
    symbols = set((decl.get("universe") or {}).get("symbols") or [])
    out = []
    for iid, state in incident_states(entries, key=key).items():
        if state not in states:
            continue
        aff = _opening(entries, iid)["affected"]
        if not symbols.intersection(aff["symbols"]):
            continue
        if pd.Timestamp(aff["window_start_utc"]) < end and pd.Timestamp(aff["window_end_utc"]) > start:
            out.append(iid)
    return sorted(out)


def require_independence_clear(decl: dict, purpose: str, entries: list[dict] | None = None) -> None:
    """Refuse `purpose` while an affecting incident is pending OR the window was adjudicated consumed."""
    blocking = affecting_incidents(decl, entries)
    if blocking:
        states = incident_states(load_ledger() if entries is None else entries)
        raise SystemExit(f"fail-closed: {purpose} is refused while Final-Holdout access incident(s) {blocking} are "
                         f"{sorted({states[i] for i in blocking})}; the holdout is not independently cleared")


def truth_summary(decl: dict, entries: list[dict] | None = None, *, key: str | None = None) -> dict:
    """What a report may state about the Final Holdout. This ledger can only block a clearance; it never
    grants one (an empty set is absence of a recorded incident, not proof of independence). A consumed window
    stays blocked: it is never untouched independent evidence again."""
    entries = load_ledger(key=key) if entries is None else verify_chain(entries, key=key)
    blocking = affecting_incidents(decl, entries, key=key)
    states = incident_states(entries, key=key)
    pending = [i for i in blocking if states[i] == PENDING]
    consumed = [i for i in blocking if states[i] == ADJUDICATED_CONSUMED]
    status = PENDING if pending else ADJUDICATED_CONSUMED if consumed else "NO_BLOCKING_INCIDENT_RECORDED"
    return {"access_incident_status": status, "pending_incident_ids": pending, "consumed_incident_ids": consumed,
            "blocking_incident_ids": blocking, "independence_certification_blocked": bool(blocking),
            "unverified_adjudication_ids": unverified_adjudications(entries, key=key)}
