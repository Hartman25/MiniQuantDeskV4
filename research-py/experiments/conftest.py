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
import tempfile
import time
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
# Children inherit the guard: PYTHONPATH carries the sitecustomize directory and every child attempt is logged to
# one session file the parent audits (attempts made in the parent are excluded by pid). The sink is fixed here and
# re-imposed on every child environment; a child cannot redirect it, and every guarded Python child must announce
# `guard_ready` there or the session fails.
_SESSION_CHILD_LOG = Path(tempfile.mkdtemp(prefix="mqk_netguard_")) / "children.log"
_SESSION_CHILD_LOG.touch()
_netguard.install(sink=_SESSION_CHILD_LOG)
os.environ.update(_netguard.guarded_env(os.environ))
READY_GRACE_SECONDS = 5.0


def _rows() -> list[dict]:
    return _netguard.sink_rows(_SESSION_CHILD_LOG)


def _child_attempts(rows: list[dict] | None = None) -> list[dict]:
    return [r for r in (_rows() if rows is None else rows)
            if r.get("kind") not in _netguard.NON_ATTEMPT_KINDS and r.get("pid") != os.getpid() and not r.get("expected")]


def _uninitialized_children(rows: list[dict] | None = None, *, wait: bool = False) -> list[int]:
    """Guarded Python children that never announced `guard_ready` (polled briefly: a child announces itself
    before it runs any of its own code)."""
    deadline = time.monotonic() + (READY_GRACE_SECONDS if wait else 0)
    while True:
        missing = _netguard.uninitialized_children(_rows() if rows is None else rows)
        if not missing or time.monotonic() >= deadline:
            return missing
        time.sleep(0.05)


@pytest.fixture(autouse=True)
def _no_external_attempt_in_this_test():
    before, children_before, rows_before = len(_netguard.unexpected_attempts()), len(_child_attempts()), len(_rows())
    yield
    new = _netguard.unexpected_attempts()[before:]
    assert not new, f"offline guard: external network/secret access attempted: {new}"
    new_children = _child_attempts()[children_before:]
    assert not new_children, f"offline guard: a spawned child attempted external network/secret access: {new_children}"
    missing = _uninitialized_children(_rows()[rows_before:], wait=True)
    assert not missing, f"offline guard: spawned Python child pid(s) {missing} never initialized the guard (attempts invisible)"


def pytest_sessionfinish(session, exitstatus):
    rows = _rows()
    children, uninitialized = _child_attempts(rows), _uninitialized_children(rows, wait=True)
    unexpected = _netguard.unexpected_attempts() + children
    summary = {"attempted_total": len(_netguard.attempts()), "unexpected_attempts": len(unexpected) + len(uninitialized),
               "unexpected_child_attempts": len(children), "uninitialized_children": len(uninitialized)}
    path = os.environ.get(SUMMARY_ENV)
    if path:
        Path(path).write_text(json.dumps(summary, sort_keys=True), encoding="utf-8")
    if (unexpected or uninitialized) and session.exitstatus == 0:
        session.exitstatus = 1
