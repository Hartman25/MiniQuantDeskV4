"""The Final-Holdout access-incident ledger: the committed incident cannot be rewritten, an adjudication needs
authenticated operator authority, and a CONSUMED window never clears independence. Pure over committed content
and synthetic keys; no provider, registry or market data."""

from __future__ import annotations

import copy
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import holdout_incident as hi  # noqa: E402
import stage_auth_testkit as kit  # noqa: E402

KISS = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
B3 = json.loads((HERE / "PREDECLARED_BATCH_03.json").read_text(encoding="utf-8"))
GENESIS_ENTRY_SHA256 = "a0c48a20bb27882733c3c8fec20c79592c69792d86b768902c72bd22744b0d0e"
KEY = kit.INCIDENT_TEST_KEY
IID = "HOA-KISS-EXT032-01"


def rehash(entry: dict) -> dict:
    entry["entry_sha256"] = hi.entry_sha256(entry)
    return entry


@pytest.fixture
def keyed(monkeypatch):
    kit.use_incident_key(monkeypatch)


# ------------------------------------------------------------------ the committed record

def test_the_incident_is_durably_recorded_pending_and_the_first_entry_is_pinned():
    entries = hi.load_ledger()
    assert entries[0]["entry_sha256"] == GENESIS_ENTRY_SHA256 == hi.PINNED_GENESIS_SHA256
    assert hi.PINNED_OPENINGS == {IID: GENESIS_ENTRY_SHA256}
    assert hi.incident_states(entries) == {IID: hi.PENDING}


def test_event_classes_are_distinct_and_nothing_is_overstated():
    ev = hi.load_ledger()[0]["events"]
    assert set(ev) == set(hi.EVENT_CLASSES)
    assert ev["provider_access"]["status"] == ev["ohlcv_parsing"]["status"] == "OCCURRED"
    for cls in ("human_price_inspection", "strategy_evaluation", "trial_attempt_registration", "formal_holdout_consumption"):
        assert ev[cls]["status"] == "NOT_REPORTED", cls
    claims = " ".join(hi.load_ledger()[0]["not_asserted"])
    for forbidden in ("untouched", "statistically clean", "permanently consumed", "restored"):
        assert forbidden in claims


def test_a_missing_or_malformed_ledger_file_fails_closed(tmp_path):
    with pytest.raises(hi.IncidentLedgerError):
        hi.load_ledger(tmp_path / "absent.json")
    for body in ({"schema": hi.SCHEMA, "entries": []}, {"schema": "other", "entries": []}, [], "x",
                 {"schema": hi.SCHEMA, "entries": "no"}):
        bad = tmp_path / "bad.json"
        bad.write_text(json.dumps(body), encoding="utf-8")
        with pytest.raises(hi.IncidentLedgerError):
            hi.load_ledger(bad)
    corrupt = tmp_path / "corrupt.json"
    corrupt.write_text("{not json", encoding="utf-8")
    with pytest.raises(hi.IncidentLedgerError):
        hi.load_ledger(corrupt)


# ------------------------------------------------------------------ F1: the committed incident cannot be rewritten

def _opening_mutations():
    def facts(e):
        e["events"]["provider_access"]["status"] = "NOT_OCCURRED_PER_RECORD"

    def state(e):
        e["state"] = hi.ADJUDICATED_PRESERVED

    def window(e):
        e["affected"]["window_start_utc"] = "2030-01-01T00:00:00Z"

    def symbols(e):
        e["affected"]["symbols"] = ["XLE"]

    def not_asserted(e):
        e["not_asserted"] = []

    def unknown(e):
        e["unknown_field"] = 1
    return [("event fact rewritten", facts), ("state flipped to preserved", state), ("affected window moved", window),
            ("affected symbols replaced", symbols), ("disclaimers removed", not_asserted), ("field added", unknown)]


@pytest.mark.parametrize("label,mutate", _opening_mutations())
def test_a_rewritten_and_rehashed_opening_is_rejected_by_the_pin(label, mutate):
    entries = copy.deepcopy(hi.load_ledger())
    mutate(entries[0])
    rehash(entries[0])  # the attacker recomputes the public hash: the pin in source still refuses
    with pytest.raises(hi.IncidentLedgerError, match="pinned"):
        hi.verify_chain(entries)


@pytest.mark.parametrize("label,mutate", _opening_mutations())
def test_an_altered_opening_without_rehashing_is_rejected(label, mutate):
    entries = copy.deepcopy(hi.load_ledger())
    mutate(entries[0])
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain(entries)


def test_flipping_the_committed_state_without_a_new_entry_leaves_the_incident_pending_or_raises():
    entries = copy.deepcopy(hi.load_ledger())
    entries[0]["state"] = hi.ADJUDICATED_PRESERVED
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain(entries)
    assert hi.incident_states(hi.load_ledger()) == {IID: hi.PENDING}


def test_replacing_the_ledger_with_a_different_incident_or_an_unpinned_opening_is_rejected():
    other = copy.deepcopy(hi.load_ledger()[0])
    other["incident_id"] = "HOA-SOMETHING-ELSE"
    rehash(other)
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain([other])
    second = copy.deepcopy(hi.load_ledger()[0])
    second.update(incident_id="HOA-SECOND", sequence=2, prev_entry_sha256=hi.load_ledger()[0]["entry_sha256"])
    rehash(second)
    with pytest.raises(hi.IncidentLedgerError, match="pinned"):
        hi.verify_chain([*hi.load_ledger(), second])
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain([hi.load_ledger()[0], hi.load_ledger()[0]])  # opened twice / bad chain


def test_the_first_entry_must_be_the_specific_committed_incident_even_if_another_opening_is_pinned(monkeypatch):
    other = copy.deepcopy(hi.load_ledger()[0])
    other["incident_id"] = "HOA-OTHER-PINNED"
    rehash(other)
    monkeypatch.setattr(hi, "PINNED_OPENINGS", {**hi.PINNED_OPENINGS, "HOA-OTHER-PINNED": other["entry_sha256"]})
    with pytest.raises(hi.IncidentLedgerError, match="first entry"):
        hi.verify_chain([other])  # a different, individually pinned incident cannot stand in for the committed one


def test_truncation_corruption_reordering_and_sequence_errors_are_rejected(keyed):
    full = kit.adjudicated_entries()
    for label, entries in {
        "empty": [], "not a list": "x", "dropped opening": full[1:],
        "reordered": [full[1], full[0]],
        "wrong sequence": [full[0], rehash({**full[1], "sequence": 5})],
        "wrong prev link": [full[0], rehash({**full[1], "prev_entry_sha256": "0" * 64})],
        "corrupt hash": [full[0], {**full[1], "entry_sha256": "f" * 64}],
        "unknown kind": [full[0], rehash({**full[1], "kind": "NOTE"})],
        "unknown state": [full[0], rehash({**full[1], "state": "CLEARED"})],
        "adjudicates a missing incident": [full[0], rehash({**full[1], "incident_id": "HOA-NOPE"})],
        "non-object entry": [full[0], "x"],
    }.items():
        with pytest.raises(hi.IncidentLedgerError):
            hi.verify_chain(entries)
    # truncating an adjudication away can only restore the blocking pending state
    assert hi.incident_states(full[:1]) == {IID: hi.PENDING}


# ------------------------------------------------------------------ F1: adjudication needs authenticated authority

def _forged(state=hi.ADJUDICATED_PRESERVED, **over):
    base = hi.load_ledger()
    e = {"sequence": 2, "kind": "ADJUDICATION", "incident_id": IID, "state": state, "decision": "fabricated",
         "operator": "x", "approval_ref": "y", "adjudicated_utc": datetime.now(timezone.utc).isoformat(),
         "prev_entry_sha256": base[0]["entry_sha256"], "signature": "0" * 64}
    e.update(over)
    return [base[0], rehash(e)]


def test_a_fabricated_adjudication_never_clears_the_incident(monkeypatch):
    forged = _forged()
    monkeypatch.delenv(hi.KEY_ENV, raising=False)
    # no operator secret in this process: the entry cannot be authenticated, so it is ignored (pending stays)
    assert hi.incident_states(forged) == {IID: hi.PENDING}
    assert hi.unverified_adjudications(forged) == [IID]
    assert hi.affecting_incidents(KISS, forged) == [IID]
    assert hi.truth_summary(KISS, forged)["unverified_adjudication_ids"] == [IID]
    kit.use_incident_key(monkeypatch)
    # with the secret available a bad signature is tampering and raises
    with pytest.raises(hi.IncidentLedgerError, match="signature"):
        hi.verify_chain(forged)
    with pytest.raises(hi.IncidentLedgerError, match="signature"):
        hi.verify_chain(_forged(signature=""))
    with pytest.raises(hi.IncidentLedgerError, match="signature"):
        hi.verify_chain(_forged(signature=None))


def test_a_signature_under_the_wrong_key_or_over_altered_content_is_rejected(monkeypatch):
    good = kit.adjudicated_entries(key="w" * 40)  # signed with a different secret
    kit.use_incident_key(monkeypatch)
    with pytest.raises(hi.IncidentLedgerError, match="signature"):
        hi.verify_chain(good)
    signed = kit.adjudicated_entries()
    altered = copy.deepcopy(signed)
    altered[1]["state"] = hi.ADJUDICATED_CONSUMED
    rehash(altered[1])
    with pytest.raises(hi.IncidentLedgerError, match="signature"):
        hi.verify_chain(altered)  # the attacker re-hashes; the HMAC still fails
    for field in ("decision", "operator", "approval_ref", "adjudicated_utc"):
        t = copy.deepcopy(signed)
        t[1][field] = (datetime.now(timezone.utc) - timedelta(seconds=7)).isoformat() if field == "adjudicated_utc" else "changed"
        rehash(t[1])
        with pytest.raises(hi.IncidentLedgerError, match="signature"):
            hi.verify_chain(t)


def test_an_adjudication_without_operator_approval_or_decision_is_rejected(keyed):
    for field in ("operator", "approval_ref", "decision"):
        t = copy.deepcopy(kit.adjudicated_entries())
        t[1][field] = ""
        t[1]["signature"] = hi._signature(t[1], KEY)
        rehash(t[1])
        with pytest.raises(hi.IncidentLedgerError, match="operator"):
            hi.verify_chain(t)


def test_adjudication_chronology_is_enforced(keyed):
    now = datetime.now(timezone.utc)
    hi.verify_chain(kit.adjudicated_entries(now=now))
    for label, when in {"before the incident": hi.ADJUDICATION_FLOOR_UTC - timedelta(seconds=1),
                        "in the future": now + timedelta(hours=1)}.items():
        with pytest.raises(hi.IncidentLedgerError, match="dated"):
            hi.verify_chain(kit.adjudicated_entries(now=when))
    naive = kit.adjudicated_entries()
    naive[1]["adjudicated_utc"] = "2026-10-10T00:00:00"
    naive[1]["signature"] = hi._signature(naive[1], KEY)
    rehash(naive[1])
    with pytest.raises(hi.IncidentLedgerError, match="timezone"):
        hi.verify_chain(naive)
    first = kit.adjudicated_entries(now=now)
    second = hi.make_adjudication(first, IID, hi.ADJUDICATED_CONSUMED, decision="d", operator="o", approval_ref="a", key=KEY,
                                  now=now - timedelta(hours=2))  # earlier than its predecessor
    with pytest.raises(hi.IncidentLedgerError, match="dated before"):
        hi.verify_chain([*first, second])
    ok = hi.make_adjudication(first, IID, hi.ADJUDICATED_CONSUMED, decision="d", operator="o", approval_ref="a", key=KEY, now=now)
    hi.verify_chain([*first, ok])


def test_a_legitimate_signed_append_is_accepted_and_changes_nothing_else(keyed):
    entries = kit.adjudicated_entries()
    assert hi.verify_chain(entries) is entries
    assert entries[0]["entry_sha256"] == GENESIS_ENTRY_SHA256, "the opening is untouched; an adjudication is an append"
    assert hi.incident_states(entries) == {IID: hi.ADJUDICATED_PRESERVED}
    assert hi.unverified_adjudications(entries) == []
    with pytest.raises(hi.IncidentLedgerError, match="at least"):
        hi.make_adjudication(hi.load_ledger(), IID, hi.ADJUDICATED_PRESERVED, decision="d", operator="o",
                             approval_ref="a", key="short", now=datetime.now(timezone.utc))


def test_adjudications_only_move_forward(keyed):
    now = datetime.now(timezone.utc)
    preserved = kit.adjudicated_entries(hi.ADJUDICATED_PRESERVED, now=now)
    consumed_later = [*preserved, hi.make_adjudication(preserved, IID, hi.ADJUDICATED_CONSUMED, decision="d", operator="o",
                                                       approval_ref="a", key=KEY, now=now)]
    assert hi.incident_states(hi.verify_chain(consumed_later)) == {IID: hi.ADJUDICATED_CONSUMED}
    consumed = kit.adjudicated_entries(hi.ADJUDICATED_CONSUMED, now=now)
    back = [*consumed, hi.make_adjudication(consumed, IID, hi.ADJUDICATED_PRESERVED, decision="d", operator="o",
                                            approval_ref="a", key=KEY, now=now)]
    with pytest.raises(hi.IncidentLedgerError, match="forward transition"):
        hi.verify_chain(back)
    again = [*preserved, hi.make_adjudication(preserved, IID, hi.ADJUDICATED_PRESERVED, decision="d", operator="o",
                                              approval_ref="a", key=KEY, now=now)]
    with pytest.raises(hi.IncidentLedgerError, match="forward transition"):
        hi.verify_chain(again)
    to_pending = hi.make_adjudication(hi.load_ledger(), IID, hi.PENDING, decision="d", operator="o", approval_ref="a",
                                      key=KEY, now=now)
    with pytest.raises(hi.IncidentLedgerError, match="forward transition"):
        hi.verify_chain([hi.load_ledger()[0], to_pending])


# ------------------------------------------------------------------ F2: consumed never clears independence

def test_the_incident_affects_the_new_declaration_and_every_declaration_sharing_the_window():
    assert hi.affecting_incidents(KISS) == [IID]
    assert hi.affecting_incidents(B3) == [IID]
    assert hi.affecting_incidents({"partition": {"holdout_months": 6}, "data": {"end_utc": "2020-01-01T00:00:00Z"},
                                   "universe": {"symbols": ["SPY"]}}) == []
    assert hi.affecting_incidents({"partition": {"holdout_months": 6}, "data": {"end_utc": "2026-09-01T00:00:00Z"},
                                   "universe": {"symbols": ["XLE"]}}) == []


def test_pending_blocks_a_preserved_adjudication_clears_and_a_consumed_one_keeps_blocking(keyed):
    with pytest.raises(SystemExit, match="ACCESS_INCIDENT_PENDING_ADJUDICATION"):
        hi.require_independence_clear(KISS, "Promotion")
    preserved = kit.adjudicated_entries(hi.ADJUDICATED_PRESERVED)
    hi.require_independence_clear(KISS, "Promotion", preserved)  # explicit authenticated operator decision
    assert hi.affecting_incidents(KISS, preserved) == []
    consumed = kit.adjudicated_entries(hi.ADJUDICATED_CONSUMED)
    with pytest.raises(SystemExit, match="ADJUDICATED_HOLDOUT_CONSUMED"):
        hi.require_independence_clear(KISS, "Paper deployment", consumed)
    assert hi.affecting_incidents(KISS, consumed) == [IID]
    assert hi.affecting_incidents(B3, consumed) == [IID], "every campaign sharing the consumed window is blocked"
    unrelated = {"partition": {"holdout_months": 6}, "data": {"end_utc": "2020-01-01T00:00:00Z"}, "universe": {"symbols": ["SPY"]}}
    hi.require_independence_clear(unrelated, "Promotion", consumed)


def test_truth_summary_distinguishes_pending_consumed_and_unverified_and_never_certifies(keyed, monkeypatch):
    s = hi.truth_summary(KISS)
    assert s["access_incident_status"] == hi.PENDING and s["pending_incident_ids"] == [IID] and s["consumed_incident_ids"] == []
    assert s["independence_certification_blocked"] is True
    c = hi.truth_summary(KISS, kit.adjudicated_entries(hi.ADJUDICATED_CONSUMED))
    assert c["access_incident_status"] == hi.ADJUDICATED_CONSUMED and c["consumed_incident_ids"] == [IID]
    assert c["pending_incident_ids"] == [] and c["independence_certification_blocked"] is True
    p = hi.truth_summary(KISS, kit.adjudicated_entries(hi.ADJUDICATED_PRESERVED))
    assert p["independence_certification_blocked"] is False and p["blocking_incident_ids"] == []
    assert "independence_certified" not in json.dumps([s, c, p]), "the ledger blocks; it never certifies"
    clear = hi.truth_summary({"universe": {"symbols": ["XLE"]}, "partition": {"holdout_months": 6},
                              "data": {"end_utc": "2026-09-01T00:00:00Z"}})
    assert clear["access_incident_status"] == "NO_BLOCKING_INCIDENT_RECORDED"
    monkeypatch.delenv(hi.KEY_ENV)
    unverified = hi.truth_summary(KISS, kit.adjudicated_entries(hi.ADJUDICATED_PRESERVED))
    assert unverified["access_incident_status"] == hi.PENDING and unverified["unverified_adjudication_ids"] == [IID]


def test_the_exact_defects_the_review_demonstrated_are_closed(monkeypatch):
    monkeypatch.delenv(hi.KEY_ENV, raising=False)
    entries = copy.deepcopy(hi.load_ledger())
    rewritten = copy.deepcopy(entries[0])
    rewritten["events"]["provider_access"]["status"] = "NOT_OCCURRED_PER_RECORD"
    rewritten["state"] = hi.ADJUDICATED_PRESERVED
    rehash(rewritten)
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain([rewritten])
    unsigned = {"sequence": 2, "kind": "ADJUDICATION", "incident_id": IID, "state": hi.ADJUDICATED_PRESERVED,
                "decision": "fabricated", "prev_entry_sha256": entries[0]["entry_sha256"]}
    rehash(unsigned)
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain([entries[0], unsigned])
    consumed = {**unsigned, "state": hi.ADJUDICATED_CONSUMED}
    rehash(consumed)
    with pytest.raises(hi.IncidentLedgerError):
        hi.affecting_incidents(KISS, [entries[0], consumed])


def test_the_declaration_no_longer_says_unconsumed_without_the_incident():
    h = KISS["holdout"]
    assert h["status"].startswith("RESERVED_NOT_FORMALLY_CONSUMED") and "ACCESS_INCIDENT_PENDING_ADJUDICATION" in h["status"]
    assert h["access_incident"]["incident_ids"] == [IID]
    assert (HERE.parents[2] / h["access_incident"]["record"]).exists()
