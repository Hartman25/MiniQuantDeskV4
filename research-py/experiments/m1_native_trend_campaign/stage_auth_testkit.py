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
                       key=TEST_KEY, now=datetime.now(timezone.utc), acknowledged_incidents=list(acknowledged))
    monkeypatch.setenv(sa.KEY_ENV, TEST_KEY)
    monkeypatch.setenv(sa.AUTH_FILE_ENV, "synthetic")
    monkeypatch.setattr(sa, "load_auth_file", fresh)
