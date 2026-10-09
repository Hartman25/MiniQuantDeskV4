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
# Children inherit the guard: PYTHONPATH carries the sitecustomize directory and every guarded process writes its
# denied attempts, launches and `guard_ready` announcements to ONE root audit sink that the session audits. The root
# is fixed when the guard first installs and can never be rebound: a process that already inherited one (a pytest run
# started by a test) audits its own subtree of the inherited root, the outermost process owns a fresh file.
if _netguard.root_sink() is None:
    _root = Path(tempfile.mkdtemp(prefix="mqk_netguard_")) / "children.log"
    _netguard.install(sink=_root)
_netguard.install()
_SESSION_CHILD_LOG = Path(_netguard.root_sink())
os.environ.update(_netguard.guarded_env(os.environ))
os.environ.pop(_netguard.LAUNCH_ENV, None)  # an unwrapped child must not adopt a launch id nobody recorded
READY_GRACE_SECONDS = 5.0


def _rows() -> list[dict]:
    for _ in range(20):  # a child may be between bytes of one row; a row that stays partial is corruption
        try:
            return _netguard.sink_rows(_SESSION_CHILD_LOG)
        except _netguard.SinkPartialRow:
            time.sleep(0.05)
    return _netguard.sink_rows(_SESSION_CHILD_LOG)


def _audit(rows: list[dict] | None = None, start: int = 0, *, wait: bool = False) -> dict:
    """The lineage audit of this session's subtree (polled briefly for `guard_ready`: a child announces itself
    before it runs any of its own code). An unreadable or malformed root sink raises `SinkCorrupt`."""
    deadline = time.monotonic() + (READY_GRACE_SECONDS if wait else 0)
    while True:
        result = _netguard.audit(_rows() if rows is None else rows, _netguard.process_token(), start=start,
                                 owner=_netguard._state["owns_root"])
        if not result["uninitialized"] or time.monotonic() >= deadline:
            return result
        time.sleep(0.05)


@pytest.fixture(autouse=True)
def _no_external_attempt_in_this_test():
    before, rows_before = len(_netguard.unexpected_attempts()), len(_rows())
    yield
    new = _netguard.unexpected_attempts()[before:]
    assert not new, f"offline guard: external network/secret access attempted: {new}"
    found = _audit(start=rows_before, wait=True)
    bad = found["unexpected"] + found["orphans"]
    assert not bad, f"offline guard: a spawned child attempted external network/secret access: {bad}"
    assert not found["uninitialized"], (f"offline guard: spawned Python child launch(es) {found['uninitialized']} never "
                                        "initialized the guard (attempts invisible)")


def pytest_sessionfinish(session, exitstatus):
    errors = _netguard._state["sink_errors"]
    try:
        found = _audit(wait=True)
    except _netguard.SinkError:
        found, errors = {"unexpected": [], "orphans": [], "uninitialized": []}, errors + 1
    children = found["unexpected"] + found["orphans"]
    unexpected = _netguard.unexpected_attempts()
    summary = {"attempted_total": len(_netguard.attempts()),
               "unexpected_attempts": len(unexpected) + len(children) + len(found["uninitialized"]) + errors,
               "unexpected_child_attempts": len(children), "uninitialized_children": len(found["uninitialized"]),
               "sink_integrity_errors": errors}
    path = os.environ.get(SUMMARY_ENV)
    if path:
        Path(path).write_text(json.dumps(summary, sort_keys=True), encoding="utf-8")
    if (unexpected or children or found["uninitialized"] or errors) and session.exitstatus == 0:
        session.exitstatus = 1
