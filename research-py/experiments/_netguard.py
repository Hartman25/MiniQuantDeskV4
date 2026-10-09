"""Process-wide, non-removable denial of network access and secret-file reads for offline research tests.

Independent of environment variables, `.env.local`, flag state and any monkeypatching: it is a
`sys.addaudithook` hook, which fires for every real `socket.connect` / name resolution and for every
`open` of a secret file, whichever Python API reached it. Attempts are RECORDED (not only refused), so a
swallowed exception still fails the test and the session summary reports attempted == 0 or not.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path

GUARD_SITE = Path(__file__).resolve().parent / "_guard_site"
# Spawn paths the guard cannot follow into a child: refused outright (Popen is routed through the guarded wrapper).
UNGUARDED_SPAWN_EVENTS = frozenset({"os.system", "os.exec", "os.posix_spawn", "os.spawn"})
NETWORK_TOOLS = frozenset({"curl", "wget", "nc", "ncat", "netcat", "telnet", "ssh", "scp", "sftp", "ftp", "socat", "nmap"})
PYTHON_NAME = re.compile(r"^python[0-9.]*(\.exe)?$")
DENIED_EVENTS = frozenset({"socket.connect", "socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr",
                           "socket.getnameinfo", "socket.sendto", "socket.sendmsg"})
SECRET_BASENAMES = frozenset({".env", ".env.local", ".env.production", ".env.development"})
LOG_ENV = "MQK_NETGUARD_LOG"

_state: dict = {"installed": False, "attempts": [], "expect_depth": 0, "allowed": {}}
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
    """A real Python interpreter: this interpreter (by resolved path), or a binary named python[N.N] that is not a
    script. A shell script named `python` is not one: it would run without the guard."""
    real = os.path.realpath(str(executable))
    if real == os.path.realpath(sys.executable):
        return True
    if not (PYTHON_NAME.match(os.path.basename(str(executable))) or PYTHON_NAME.match(os.path.basename(real))):
        return False
    try:
        with open(real, "rb") as fh:  # `open` of a non-secret name: not screened by the audit hook
            return fh.read(2) != b"#!"
    except OSError:
        return False


def _carries_guard(env) -> bool:
    """The guard's sitecustomize directory must come FIRST on PYTHONPATH, so no other `sitecustomize` wins."""
    paths = [p for p in (env if env is not None else os.environ).get("PYTHONPATH", "").split(os.pathsep) if p]
    return bool(paths) and paths[0] == str(GUARD_SITE)


def _resolve(executable, env, cwd) -> str:
    """The file Popen will exec: a bare name is looked up on the child's PATH, a relative path from its cwd."""
    exe = os.fsdecode(executable)
    if os.sep in exe:
        return exe if os.path.isabs(exe) or cwd is None else os.path.join(os.fsdecode(cwd), exe)
    path = (env if env is not None else os.environ).get("PATH", os.defpath)
    return shutil.which(exe, path=path) or exe


def _allowed_executable(path: str) -> bool:
    """True for an executable a test vouched for with `allow_executable`, byte-identical to what it vouched for."""
    real = os.path.realpath(path)
    digest = _state["allowed"].get(real)
    if digest is None or not os.path.isfile(real):
        return False
    with open(real, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest() == digest


def _hook(event: str, args: tuple) -> None:
    if event in UNGUARDED_SPAWN_EVENTS:
        if event == "os.posix_spawn" and getattr(_in_guarded_popen, "depth", 0) > 0:
            return  # CPython's own Popen fast path, already screened and env-injected by the guarded wrapper
        _record("unguarded_spawn", event)
        raise NetworkDenied(f"offline guard: {event} cannot be followed into the child and is refused")
    if event == "subprocess.Popen" and args:
        # Backstop for any launch that bypassed the guarded Popen wrapper: the same launch policy applies.
        executable, argv, cwd, env = (list(args) + [None] * 4)[:4]
        refusal = launch_refusal(executable, argv, cwd, env)
        if refusal:
            _record(refusal[0], refusal[1])
            raise NetworkDenied(f"offline guard: {refusal[2]}")
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
    paths = [p for p in out.get("PYTHONPATH", "").split(os.pathsep) if p and p != str(GUARD_SITE)]
    out["PYTHONPATH"] = os.pathsep.join([str(GUARD_SITE), *paths])  # first: no other sitecustomize may win
    if LOG_ENV not in out and os.environ.get(LOG_ENV):
        out[LOG_ENV] = os.environ[LOG_ENV]
    return out


def _python_skips_guard(argv: list) -> bool:
    """True when a Python command line disables the environment, the path or `site` (`-I`, `-S`, `-E`, alone or
    in a cluster): the child would then never load the guard's sitecustomize. Option arguments (`-W x`, `-Xdev`)
    are skipped, and parsing stops where Python stops reading options (`-c`, `-m`, a script, `-`)."""
    it = iter(argv[1:])
    for tok in it:
        if tok in ("-", "--") or not tok.startswith("-"):
            return False
        if tok.startswith("--"):
            if tok == "--check-hash-based-pycs":
                next(it, None)
            continue
        for i, ch in enumerate(tok[1:], 1):
            if ch in "ISE":
                return True
            if ch in "cm":
                return False
            if ch in "WX":
                if i == len(tok) - 1:
                    next(it, None)  # the option argument is the next token
                break
    return False


def launch_refusal(executable, argv, cwd, env):
    """None when the guard can prove the child is isolated; otherwise (kind, detail, reason). Supported:
    this interpreter / a real Python interpreter started so that it loads the guard, and an executable a test
    explicitly vouched for. Everything else is refused: a shell, `env`/`nice`/`timeout`/... wrappers, any other
    interpreter or program is an intermediary the guard cannot instrument, whatever its command line says."""
    exe = _resolve(executable, env, cwd)
    base = os.path.basename(exe)
    argv = [os.fsdecode(a) for a in (argv if isinstance(argv, (list, tuple)) else [argv])]
    if _is_python(exe):
        if _python_skips_guard(argv):
            return ("guard_bypass_spawn", " ".join(argv[:4]), "a Python child in isolated mode (-I/-S/-E) would skip the guard")
        if not _carries_guard(env):
            return ("unguarded_python_child", base, "a Python child would start without the guard")
        return None
    if _allowed_executable(exe):
        return None
    if base in NETWORK_TOOLS:
        return ("network_tool_spawn", base, f"launching a network client ({base}) is refused")
    return ("unsupported_launcher", base, f"{base!r} is not a Python interpreter the guard can instrument; "
                                          "shells, env/nice/timeout wrappers and other programs are refused")


@contextlib.contextmanager
def allow_executable(path):
    """A test vouches for one exact executable (a stub it wrote) so the guard lets it run. Matched by resolved
    path AND content hash, so a substituted file at the same path is still refused."""
    install()
    real = os.path.realpath(path)
    with open(real, "rb") as fh:
        _state["allowed"][real] = hashlib.sha256(fh.read()).hexdigest()
    try:
        yield
    finally:
        _state["allowed"].pop(real, None)


def screen_spawn(args, shell: bool, executable=None, env=None, cwd=None) -> None:
    """Refuse a launch the guard cannot protect (see `launch_refusal`). `shell=True` always launches a shell."""
    if shell:
        executable = executable or "/bin/sh"
        argv = [executable, "-c", os.fsdecode(args) if isinstance(args, (str, bytes, os.PathLike)) else " ".join(map(os.fsdecode, args))]
    else:
        argv = [os.fsdecode(args)] if isinstance(args, (str, bytes, os.PathLike)) else [os.fsdecode(a) for a in args]
        executable = executable or (argv[0] if argv else None)
    if executable is None:
        return
    refusal = launch_refusal(executable, argv, cwd, guarded_env(env))
    if refusal:
        _record(refusal[0], refusal[1])
        raise NetworkDenied(f"offline guard: {refusal[2]}")


def install_subprocess_guard() -> None:
    """Route every `subprocess.Popen` through a wrapper that screens the launch and injects the guard into the
    child's environment, wherever the caller built that environment. Idempotent."""
    if _state.get("subprocess_guard"):
        return
    original = subprocess.Popen.__init__
    _state["original_popen_init"] = original  # exposed only so a test can prove the audit backstop

    def guarded_init(self, args, *pos, **kw):
        shell = kw.get("shell", pos[7] if len(pos) > 7 else False)
        env = kw.get("env", pos[9] if len(pos) > 9 else None)
        cwd = kw.get("cwd", pos[8] if len(pos) > 8 else None)
        screen_spawn(args, bool(shell), kw.get("executable", pos[1] if len(pos) > 1 else None), env, cwd)
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
