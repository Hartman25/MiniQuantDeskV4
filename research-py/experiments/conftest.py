"""Offline guard for every test under research-py/experiments.

* Provider credentials are removed from the process environment and the hermetic flag is set, so the
  provider boundary itself refuses (src/mqk_research/data/alpaca_historical.py).
* An audit-hook guard (`_netguard`) independently refuses any real socket connect / name resolution and any
  read of a `.env*` secret file, and records each attempt. A test (or a mutated test) that reaches the
  network fails even if it swallows the exception, and the session summary reports the count.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import _netguard  # noqa: E402

HERMETIC_FLAG = "MQK_HERMETIC_NO_PROVIDER"
SUMMARY_ENV = "MQK_NETGUARD_SUMMARY"
_CREDENTIAL_PREFIXES = ("ALPACA", "APCA", "TIINGO", "TWELVEDATA", "POLYGON", "FINNHUB")

for _k in [k for k in os.environ if k.upper().startswith(_CREDENTIAL_PREFIXES)]:
    del os.environ[_k]
os.environ[HERMETIC_FLAG] = "1"
_netguard.install()


@pytest.fixture(autouse=True)
def _no_external_attempt_in_this_test():
    before = len(_netguard.unexpected_attempts())
    yield
    new = _netguard.unexpected_attempts()[before:]
    assert not new, f"offline guard: external network/secret access attempted: {new}"


def pytest_sessionfinish(session, exitstatus):
    unexpected = _netguard.unexpected_attempts()
    summary = {"attempted_total": len(_netguard.attempts()), "unexpected_attempts": len(unexpected)}
    path = os.environ.get(SUMMARY_ENV)
    if path:
        Path(path).write_text(json.dumps(summary, sort_keys=True), encoding="utf-8")
    if unexpected and session.exitstatus == 0:
        session.exitstatus = 1
