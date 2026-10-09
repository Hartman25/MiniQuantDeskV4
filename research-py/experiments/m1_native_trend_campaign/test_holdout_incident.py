"""The Final-Holdout access-incident ledger: durable, append-only, and consulted by the holdout guard.
Pure over committed content; no provider, registry or market data."""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import holdout_incident as hi  # noqa: E402

KISS = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
B3 = json.loads((HERE / "PREDECLARED_BATCH_03.json").read_text(encoding="utf-8"))
# The first entry is pinned: editing any recorded fact changes this hash.
GENESIS_ENTRY_SHA256 = "a0c48a20bb27882733c3c8fec20c79592c69792d86b768902c72bd22744b0d0e"


def _adjudication(entries, state=hi.ADJUDICATED_PRESERVED):
    e = {"sequence": len(entries) + 1, "kind": "ADJUDICATION", "incident_id": "HOA-KISS-EXT032-01", "state": state,
         "decision": "test", "prev_entry_sha256": entries[-1]["entry_sha256"]}
    e["entry_sha256"] = hi.entry_sha256(e)
    return [*entries, e]


def test_the_incident_is_durably_recorded_pending_and_the_first_entry_is_pinned():
    entries = hi.load_ledger()
    assert entries[0]["entry_sha256"] == GENESIS_ENTRY_SHA256
    assert hi.incident_states(entries) == {"HOA-KISS-EXT032-01": hi.PENDING}


def test_event_classes_are_distinct_and_nothing_is_overstated():
    ev = hi.load_ledger()[0]["events"]
    assert set(ev) == set(hi.EVENT_CLASSES)
    assert ev["provider_access"]["status"] == ev["ohlcv_parsing"]["status"] == "OCCURRED"
    for cls in ("human_price_inspection", "strategy_evaluation", "trial_attempt_registration", "formal_holdout_consumption"):
        assert ev[cls]["status"] == "NOT_REPORTED", cls  # unreported is not asserted-absent
    claims = " ".join(hi.load_ledger()[0]["not_asserted"])
    for forbidden in ("untouched", "statistically clean", "permanently consumed", "restored"):
        assert forbidden in claims


def test_editing_a_recorded_fact_breaks_the_chain():
    entries = copy.deepcopy(hi.load_ledger())
    entries[0]["events"]["provider_access"]["status"] = "NOT_OCCURRED_PER_RECORD"
    with pytest.raises(hi.IncidentLedgerError, match="hash"):
        hi.verify_chain(entries)
    entries = copy.deepcopy(hi.load_ledger())
    entries[0]["state"] = hi.ADJUDICATED_PRESERVED  # a silent flip also changes the hash
    with pytest.raises(hi.IncidentLedgerError, match="hash"):
        hi.verify_chain(entries)


def test_a_missing_or_malformed_ledger_fails_closed(tmp_path):
    with pytest.raises(hi.IncidentLedgerError):
        hi.load_ledger(tmp_path / "absent.json")
    bad = tmp_path / "bad.json"
    bad.write_text(json.dumps({"schema": hi.SCHEMA, "entries": []}), encoding="utf-8")
    with pytest.raises(hi.IncidentLedgerError):
        hi.load_ledger(bad)


def test_the_incident_affects_the_new_declaration_and_every_declaration_sharing_the_window():
    assert hi.affecting_incidents(KISS) == ["HOA-KISS-EXT032-01"]
    assert hi.affecting_incidents(B3) == ["HOA-KISS-EXT032-01"]  # same 2026-03-01 reserved window, same symbols
    assert hi.affecting_incidents({"partition": {"holdout_months": 6}, "data": {"end_utc": "2020-01-01T00:00:00Z"},
                                   "universe": {"symbols": ["SPY"]}}) == []
    assert hi.affecting_incidents({"partition": {"holdout_months": 6}, "data": {"end_utc": "2026-09-01T00:00:00Z"},
                                   "universe": {"symbols": ["XLE"]}}) == []


def test_pending_blocks_and_only_an_appended_adjudication_unblocks():
    with pytest.raises(SystemExit, match="ACCESS_INCIDENT_PENDING_ADJUDICATION"):
        hi.require_no_pending_incident(KISS, "Promotion")
    adjudicated = _adjudication(hi.load_ledger())
    hi.require_no_pending_incident(KISS, "Promotion", adjudicated)  # no raise
    # an adjudication is an append: the opening entry (and the pinned hash) is untouched
    assert adjudicated[0]["entry_sha256"] == GENESIS_ENTRY_SHA256
    with pytest.raises(hi.IncidentLedgerError):
        hi.verify_chain(_adjudication(hi.load_ledger(), hi.PENDING))  # cannot "adjudicate" back to pending


def test_truth_summary_can_only_block_never_certify():
    s = hi.truth_summary(KISS)
    assert s["independence_certification_blocked"] is True and s["pending_incident_ids"] == ["HOA-KISS-EXT032-01"]
    clear = hi.truth_summary({"universe": {"symbols": ["XLE"]}, "partition": {"holdout_months": 6},
                              "data": {"end_utc": "2026-09-01T00:00:00Z"}})
    assert "independence_certified" not in json.dumps(clear) and clear["access_incident_status"] == "NO_PENDING_INCIDENT_RECORDED"


def test_the_declaration_no_longer_says_unconsumed_without_the_incident():
    h = KISS["holdout"]
    assert h["status"].startswith("RESERVED_NOT_FORMALLY_CONSUMED") and "ACCESS_INCIDENT_PENDING_ADJUDICATION" in h["status"]
    assert h["access_incident"]["incident_ids"] == ["HOA-KISS-EXT032-01"]
    assert (HERE.parents[2] / h["access_incident"]["record"]).exists()
