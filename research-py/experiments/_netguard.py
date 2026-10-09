"""Process-wide, non-removable denial of network access and secret-file reads for offline research tests.

Independent of environment variables, `.env.local`, flag state and any monkeypatching: it is a
`sys.addaudithook` hook, which fires for every real `socket.connect` / name resolution and for every
`open` of a secret file, whichever Python API reached it. Attempts are RECORDED (not only refused), so a
swallowed exception still fails the test and the session summary reports attempted == 0 or not.
"""

from __future__ import annotations

import contextlib
import json
import os
import subprocess
import sys
from pathlib import Path

DENIED_EVENTS = frozenset({"socket.connect", "socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr",
                           "socket.getnameinfo", "socket.sendto", "socket.sendmsg"})
SECRET_BASENAMES = frozenset({".env", ".env.local", ".env.production", ".env.development"})
LOG_ENV = "MQK_NETGUARD_LOG"

_state: dict = {"installed": False, "attempts": [], "expect_depth": 0}


class NetworkDenied(OSError):
    """An external network operation or secret-file read was attempted under the offline guard."""


def _is_local_unix(event: str, args: tuple) -> bool:
    return event == "socket.connect" and len(args) > 1 and isinstance(args[1], (str, bytes))


def _record(kind: str, detail: str) -> None:
    entry = {"kind": kind, "detail": detail, "expected": _state["expect_depth"] > 0}
    _state["attempts"].append(entry)
    log = os.environ.get(LOG_ENV)
    if log:
        with contextlib.suppress(OSError):
            with open(log, "a", encoding="utf-8") as fh:  # inside the hook: never re-enter via the open check
                fh.write(json.dumps(entry, sort_keys=True) + "\n")


def _hook(event: str, args: tuple) -> None:
    if event in DENIED_EVENTS:
        if _is_local_unix(event, args):
            return
        _record("network", f"{event} {args[1:] if event == 'socket.connect' else args[:1]}")
        raise NetworkDenied(f"offline guard: {event} refused")
    if event == "open" and args:
        target = args[0]
        if isinstance(target, (str, bytes, os.PathLike)):
            name = os.path.basename(os.fsdecode(target))
            if name in SECRET_BASENAMES:
                _record("secret_file_read", name)
                raise NetworkDenied(f"offline guard: reading {name} refused")


def install() -> None:
    """Idempotent. An audit hook cannot be removed for the life of the process."""
    if not _state["installed"]:
        sys.addaudithook(_hook)
        _state["installed"] = True


@contextlib.contextmanager
def expect_denied():
    """Attempts inside the block are the test's own deliberate probes: they are still recorded and still
    refused, but are not counted as unexpected."""
    install()
    _state["expect_depth"] += 1
    start = len(_state["attempts"])
    seen: list = []
    try:
        yield seen
    finally:
        _state["expect_depth"] -= 1
        seen.extend(_state["attempts"][start:])


def attempts() -> list[dict]:
    return list(_state["attempts"])


def unexpected_attempts() -> list[dict]:
    return [a for a in _state["attempts"] if not a["expected"]]


_BOOT = ("import runpy, sys; sys.path.insert(0, sys.argv[1]); import _netguard; _netguard.install(); "
         "script = sys.argv[2]; sys.argv = sys.argv[2:]; runpy.run_path(script, run_name='__main__')")


def run_guarded(script: Path, argv: list[str], *, env: dict, cwd: Path, log: Path):
    """Run a Python script in a child with the same guard installed and its attempts logged to `log`.
    Returns (CompletedProcess, [attempt dicts])."""
    env = {**env, LOG_ENV: str(log)}
    proc = subprocess.run([sys.executable, "-c", _BOOT, str(Path(__file__).resolve().parent), str(script), *argv],
                          capture_output=True, text=True, env=env, cwd=cwd)
    rows = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()] if log.exists() else []
    return proc, rows
