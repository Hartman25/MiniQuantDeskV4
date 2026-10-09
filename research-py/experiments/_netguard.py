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
import re
import subprocess
import sys
import threading
from pathlib import Path

GUARD_SITE = Path(__file__).resolve().parent / "_guard_site"
# Spawn paths the guard cannot follow into a child: refused outright (Popen is routed through the guarded wrapper).
UNGUARDED_SPAWN_EVENTS = frozenset({"os.system", "os.exec", "os.posix_spawn", "os.spawn"})
NETWORK_TOOLS = frozenset({"curl", "wget", "nc", "ncat", "netcat", "telnet", "ssh", "scp", "sftp", "ftp", "socat", "nmap"})
SHELLS = frozenset({"sh", "bash", "dash", "zsh", "ksh"})
DENIED_EVENTS = frozenset({"socket.connect", "socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr",
                           "socket.getnameinfo", "socket.sendto", "socket.sendmsg"})
SECRET_BASENAMES = frozenset({".env", ".env.local", ".env.production", ".env.development"})
LOG_ENV = "MQK_NETGUARD_LOG"

_state: dict = {"installed": False, "attempts": [], "expect_depth": 0}
_in_guarded_popen = threading.local()


class NetworkDenied(OSError):
    """An external network operation or secret-file read was attempted under the offline guard."""


def _is_local_unix(event: str, args: tuple) -> bool:
    return event == "socket.connect" and len(args) > 1 and isinstance(args[1], (str, bytes))


def _record(kind: str, detail: str) -> None:
    entry = {"kind": kind, "detail": detail, "expected": _state["expect_depth"] > 0, "pid": os.getpid()}
    _state["attempts"].append(entry)
    log = os.environ.get(LOG_ENV)
    if log:
        with contextlib.suppress(OSError):
            with open(log, "a", encoding="utf-8") as fh:  # inside the hook: never re-enter via the open check
                fh.write(json.dumps(entry, sort_keys=True) + "\n")


def _is_python(executable: str) -> bool:
    base = os.path.basename(str(executable))
    return base.startswith("python") or os.path.realpath(str(executable)) == os.path.realpath(sys.executable)


def _carries_guard(env) -> bool:
    paths = (env if env is not None else os.environ).get("PYTHONPATH", "")
    return str(GUARD_SITE) in paths.split(os.pathsep)


def _hook(event: str, args: tuple) -> None:
    if event in UNGUARDED_SPAWN_EVENTS:
        if event == "os.posix_spawn" and getattr(_in_guarded_popen, "depth", 0) > 0:
            return  # CPython's own Popen fast path, already screened and env-injected by the guarded wrapper
        _record("unguarded_spawn", event)
        raise NetworkDenied(f"offline guard: {event} cannot be followed into the child and is refused")
    if event == "subprocess.Popen" and args:
        # Backstop for any launch that bypassed the guarded Popen wrapper: a Python child must carry the guard.
        executable = args[0]
        env = args[3] if len(args) > 3 else None
        if _is_python(executable) and not _carries_guard(env):
            _record("unguarded_python_child", str(executable))
            raise NetworkDenied("offline guard: a Python child would start without the guard")
        return
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
    install_subprocess_guard()


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


def guarded_env(env) -> dict:
    """`env` (or the current environment when None) with the guard's `sitecustomize` directory first on
    PYTHONPATH and a log location, so a Python child installs the guard before running any of its own code even
    when the caller passed a minimal or hand-built environment."""
    out = dict(os.environ if env is None else env)
    paths = [p for p in out.get("PYTHONPATH", "").split(os.pathsep) if p]
    if str(GUARD_SITE) not in paths:
        out["PYTHONPATH"] = os.pathsep.join([str(GUARD_SITE), *paths])
    if LOG_ENV not in out and os.environ.get(LOG_ENV):
        out[LOG_ENV] = os.environ[LOG_ENV]
    return out


def _isolates_python(argv: list) -> bool:
    """True when a Python command line disables the environment, the path or `site` (`-I`, `-S`, `-E`): those
    flags would drop the guard's sitecustomize, so such a child cannot be guarded and is refused."""
    for tok in argv[1:]:
        if tok in ("-c", "-m") or not tok.startswith("-"):
            return False
        if not tok.startswith("--") and set(tok[1:]) & set("ISE"):
            return True
    return False


def screen_spawn(args, shell: bool, executable=None) -> None:
    """Refuse a launch the guard cannot protect: a network client by name, or a Python interpreter started in
    an isolation mode that skips the guard."""
    if isinstance(args, (str, bytes, os.PathLike)):
        argv = [os.fsdecode(args)] if not shell else os.fsdecode(args).split()
    else:
        argv = [os.fsdecode(a) for a in args]
    if executable is not None:
        argv = [os.fsdecode(executable), *argv[1:]]
    if not argv:
        return
    base = os.path.basename(argv[0])
    words = {os.path.basename(w) for w in argv}
    if base in SHELLS:  # a shell command line is one argument: look at the words inside it
        words |= {os.path.basename(w) for tok in argv[1:] for w in re.split(r"[\s;|&()`$]+", tok) if w}
    if base in NETWORK_TOOLS or (base in SHELLS and words & NETWORK_TOOLS):
        _record("network_tool_spawn", base)
        raise NetworkDenied(f"offline guard: launching a network client ({sorted(words & NETWORK_TOOLS) or base}) is refused")
    if not _is_python(argv[0]):  # a Python child is guarded and reads files through the audit hook; others are not
        secret = sorted(w for w in words if w in SECRET_BASENAMES)
        if secret:
            _record("secret_file_spawn", f"{base} {secret}")
            raise NetworkDenied(f"offline guard: a non-Python child may not be handed a secret file ({secret})")
    if _is_python(argv[0]) and _isolates_python(argv):
        _record("guard_bypass_spawn", " ".join(argv[:4]))
        raise NetworkDenied("offline guard: a Python child in isolated mode (-I/-S/-E) would skip the guard")


def install_subprocess_guard() -> None:
    """Route every `subprocess.Popen` through a wrapper that screens the launch and injects the guard into the
    child's environment, wherever the caller built that environment. Idempotent."""
    if _state.get("subprocess_guard"):
        return
    original = subprocess.Popen.__init__
    _state["original_popen_init"] = original  # exposed only so a test can prove the audit backstop

    def guarded_init(self, args, *pos, **kw):
        shell = kw.get("shell", pos[7] if len(pos) > 7 else False)
        screen_spawn(args, bool(shell), kw.get("executable", pos[1] if len(pos) > 1 else None))
        if len(pos) > 9:  # env given positionally
            pos = list(pos)
            pos[9] = guarded_env(pos[9])
        else:
            kw["env"] = guarded_env(kw.get("env"))
        _in_guarded_popen.depth = getattr(_in_guarded_popen, "depth", 0) + 1
        try:
            return original(self, args, *pos, **kw)
        finally:
            _in_guarded_popen.depth -= 1

    subprocess.Popen.__init__ = guarded_init
    _state["subprocess_guard"] = True


def attempts() -> list[dict]:
    return list(_state["attempts"])


def unexpected_attempts() -> list[dict]:
    return [a for a in _state["attempts"] if not a["expected"]]


_BOOT = ("import runpy, sys; sys.path.insert(0, sys.argv[1]); import _netguard; _netguard.install(); "
         "script = sys.argv[2]; sys.argv = sys.argv[2:]; runpy.run_path(script, run_name='__main__')")


def run_guarded(script: Path, argv: list[str], *, env: dict, cwd: Path, log: Path):
    """Run a Python script in a child with the same guard installed and its attempts logged to `log`.
    Returns (CompletedProcess, [attempt dicts])."""
    install_subprocess_guard()
    env = {**env, LOG_ENV: str(log)}
    proc = subprocess.run([sys.executable, "-c", _BOOT, str(Path(__file__).resolve().parent), str(script), *argv],
                          capture_output=True, text=True, env=env, cwd=cwd)
    rows = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()] if log.exists() else []
    return proc, rows
