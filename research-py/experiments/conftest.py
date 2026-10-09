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
# one session file the parent audits (attempts made in the parent are excluded by pid).
os.environ.update(_netguard.guarded_env(os.environ))
_SESSION_CHILD_LOG = Path(tempfile.mkdtemp(prefix="mqk_netguard_")) / "children.log"
os.environ[_netguard.LOG_ENV] = str(_SESSION_CHILD_LOG)
_netguard.install()


def _child_attempts() -> list[dict]:
    if not _SESSION_CHILD_LOG.exists():
        return []
    rows = []
    for line in _SESSION_CHILD_LOG.read_text(encoding="utf-8").splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row.get("pid") != os.getpid() and not row.get("expected"):
            rows.append(row)
    return rows


@pytest.fixture(autouse=True)
def _no_external_attempt_in_this_test():
    before, children_before = len(_netguard.unexpected_attempts()), len(_child_attempts())
    yield
    new = _netguard.unexpected_attempts()[before:]
    assert not new, f"offline guard: external network/secret access attempted: {new}"
    new_children = _child_attempts()[children_before:]
    assert not new_children, f"offline guard: a spawned child attempted external network/secret access: {new_children}"


def pytest_sessionfinish(session, exitstatus):
    children = _child_attempts()
    unexpected = _netguard.unexpected_attempts() + children
    summary = {"attempted_total": len(_netguard.attempts()), "unexpected_attempts": len(unexpected),
               "unexpected_child_attempts": len(children)}
    path = os.environ.get(SUMMARY_ENV)
    if path:
        Path(path).write_text(json.dumps(summary, sort_keys=True), encoding="utf-8")
    if unexpected and session.exitstatus == 0:
        session.exitstatus = 1
