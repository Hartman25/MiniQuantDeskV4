"""Test support: an operator stand-in that signs a fresh authorization, with a synthetic key, for whatever
declaration a runner module currently holds. It exercises the REAL verification path (signature, identity,
class, window, incident acknowledgement); only the minting side is simulated. Never imported by production code."""

from __future__ import annotations

from datetime import datetime, timezone

import stage_authorization as sa

TEST_KEY = "t" * 40
RUNNER_CLASSES = [c for c in sa.AUTHORIZABLE if c not in sa.INCIDENT_BLOCKED]


def grant_runner_stages(monkeypatch, rb, classes=None, acknowledged=("HOA-KISS-EXT032-01",)) -> None:
    def fresh(_path):
        return sa.mint(rb.DECL, list(classes or RUNNER_CLASSES), operator="test-operator", approval_ref="TEST",
                       key=TEST_KEY, now=datetime.now(timezone.utc), acknowledged_incidents=list(acknowledged),
                       acknowledged_data_boundaries=list((rb.DECL.get("data") or {}).get("required_fetch_acknowledgements") or []))
    monkeypatch.setenv(sa.KEY_ENV, TEST_KEY)
    monkeypatch.setenv(sa.AUTH_FILE_ENV, "synthetic")
    monkeypatch.setattr(sa, "load_auth_file", fresh)


# ---- incident-ledger test support: a synthetic operator secret and signed adjudications (never real keys)
import holdout_incident as _hi  # noqa: E402

INCIDENT_TEST_KEY = "i" * 40


def adjudicated_entries(state: str = _hi.ADJUDICATED_PRESERVED, *, key: str = INCIDENT_TEST_KEY, now=None,
                        base=None) -> list[dict]:
    """The committed ledger plus one correctly signed ADJUDICATION entry (a test stand-in for the operator)."""
    base = base if base is not None else _hi.load_ledger()
    entry = _hi.make_adjudication(base, "HOA-KISS-EXT032-01", state, decision="test decision", operator="test-operator",
                                  approval_ref="TEST-APPROVAL", key=key, now=now or datetime.now(timezone.utc))
    return [*base, entry]


def use_incident_key(monkeypatch, key: str = INCIDENT_TEST_KEY) -> None:
    monkeypatch.setenv(_hi.KEY_ENV, key)
