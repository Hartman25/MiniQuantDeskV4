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
import shutil
import stat
import subprocess
import sys
import threading
import uuid
from pathlib import Path

GUARD_SITE = Path(__file__).resolve().parent / "_guard_site"
# Spawn paths the guard cannot follow into a child: refused outright (Popen is routed through the guarded wrapper).
UNGUARDED_SPAWN_EVENTS = frozenset({"os.system", "os.exec", "os.posix_spawn", "os.spawn"})
NETWORK_TOOLS = frozenset({"curl", "wget", "nc", "ncat", "netcat", "telnet", "ssh", "scp", "sftp", "ftp", "socat", "nmap"})
DENIED_EVENTS = frozenset({"socket.connect", "socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr",
                           "socket.getnameinfo", "socket.sendto", "socket.sendmsg"})
SECRET_BASENAMES = frozenset({".env", ".env.local", ".env.production", ".env.development"})
# Environment carried to a child: the root audit sink, optional diagnostic copies, the child's launch id and its parent's id.
LOG_ENV = "MQK_NETGUARD_LOG"
DIAG_ENV = "MQK_NETGUARD_DIAG"
LAUNCH_ENV = "MQK_NETGUARD_LAUNCH"
PARENT_ENV = "MQK_NETGUARD_PARENT"
# Sink rows that are bookkeeping, not attempts: a guard announcing itself, and a parent announcing a child it launched.
NON_ATTEMPT_KINDS = frozenset({"guard_ready", "child_launched"})
CHILD_SINK_EXIT = 97

# `root`/`root_id`: the immutable root audit sink (fixed by the first install; the authority the session audits).
# `diag`: diagnostic copies [(path, file id)]; they only ever receive copies of what the root also receives.
_state: dict = {"installed": False, "attempts": [], "expect_depth": 0, "allowed": {}, "root": None, "root_id": None,
                "owns_root": False, "diag": [], "ready": False, "token": uuid.uuid4().hex, "parent_token": None, "sink_errors": 0}
_in_guarded_popen = threading.local()
_in_sink_io = threading.local()
SINK_WRITE_FLAGS = os.O_WRONLY | os.O_RDWR | os.O_APPEND | os.O_TRUNC | os.O_CREAT
SINK_MUTATING_EVENTS = frozenset({"os.remove", "os.rename", "os.truncate", "os.replace", "shutil.move"})


class NetworkDenied(OSError):
    """An external network operation or secret-file read was attempted under the offline guard."""


def _is_local_unix(event: str, args: tuple) -> bool:
    return event == "socket.connect" and len(args) > 1 and isinstance(args[1], (str, bytes))


class SinkError(RuntimeError):
    """The audit sink cannot be established, rebound or relied on."""


class SinkRebindError(SinkError):
    """An attempt to replace the root audit sink after it was fixed."""


class SinkInvalid(SinkError):
    """A path that cannot be an audit sink (device, directory, missing, unusable)."""


class SinkCorrupt(SinkError):
    """An audit sink whose content cannot be trusted (malformed row, missing, truncated, replaced)."""


class SinkPartialRow(SinkCorrupt):
    """The sink ends mid-row: a writer is between bytes (retried briefly by the reader) or the tail was cut."""


def root_sink() -> str | None:
    return _state["root"]


def process_token() -> str:
    """This process's identity in the audit lineage (unique per process, unlike a pid)."""
    return _state["token"]


@contextlib.contextmanager
def _sink_io():
    _in_sink_io.depth = getattr(_in_sink_io, "depth", 0) + 1
    try:
        yield
    finally:
        _in_sink_io.depth -= 1


def _validated(path, *, create: bool) -> tuple[str, tuple[int, int]]:
    """(absolute real path, file id) of a path that is a regular file; created when asked. A device (`/dev/null`),
    directory, FIFO, socket, or a symlink to one of them is refused, so evidence cannot be pointed at a sink hole."""
    if path is None or not str(path):
        raise SinkInvalid("an empty audit sink path")
    real = os.path.realpath(os.fspath(path))
    if os.path.basename(real) in SECRET_BASENAMES:
        raise SinkInvalid(f"audit sink {path!r} is named like a secret file")
    with _sink_io():
        try:
            if create and not os.path.exists(real):
                with open(real, "a", encoding="utf-8"):
                    pass
            st = os.stat(real)
        except OSError as exc:
            raise SinkInvalid(f"audit sink {path!r} is not usable: {exc}") from exc
    if not stat.S_ISREG(st.st_mode):
        raise SinkInvalid(f"audit sink {path!r} is not a regular file")
    return real, (st.st_dev, st.st_ino)


def _append(path: str, file_id: tuple[int, int], row: dict) -> bool:
    """Append one JSON row; refuses when the path no longer names the file that was validated."""
    data = (json.dumps(row, sort_keys=True) + "\n").encode("utf-8")
    with _sink_io():
        try:
            fd = os.open(path, os.O_WRONLY | os.O_APPEND)
        except OSError:
            return False
        try:
            st = os.fstat(fd)
            if (st.st_dev, st.st_ino) != file_id or not stat.S_ISREG(st.st_mode):
                return False
            return os.write(fd, data) == len(data)
        except OSError:
            return False
        finally:
            os.close(fd)


def _write_all(row: dict) -> bool:
    """Root first, then every diagnostic copy. False when any required destination could not take the row."""
    if _state["root"] is None:
        return False
    ok = _append(_state["root"], _state["root_id"], row)
    written = {_state["root_id"]}
    for path, file_id in list(_state["diag"]):
        if file_id not in written:
            written.add(file_id)
            ok = _append(path, file_id, row) and ok
    return ok


def _sinks_usable() -> bool:
    if _state["root"] is None:
        return True  # no audit root configured: nothing to collect (a plain process, not a guarded session)
    with _sink_io():
        try:
            for path, file_id in [(_state["root"], _state["root_id"]), *_state["diag"]]:
                st = os.stat(path)
                if (st.st_dev, st.st_ino) != file_id or not stat.S_ISREG(st.st_mode) or not os.access(path, os.W_OK):
                    return False
        except OSError:
            return False
    return True


def _is_sink(target) -> bool:
    if not isinstance(target, (str, bytes, os.PathLike)):
        return False
    real = os.path.realpath(os.fsdecode(target))
    return any(real == os.path.realpath(sk) for sk in [_state["root"], *[d[0] for d in _state["diag"]]] if sk)


def _record(kind: str, detail: str) -> None:
    entry = {"kind": kind, "detail": detail, "expected": _state["expect_depth"] > 0, "pid": os.getpid(),
             "proc": _state["token"]}
    _state["attempts"].append(entry)
    if _state["root"] is not None and not _write_all(entry):
        _state["sink_errors"] += 1
        if _state.get("child_fatal"):
            # The denial stands, but its evidence could not be collected: a child that cannot report must not go on.
            os.write(2, b"offline guard: audit sink unwritable; child terminated\n")
            os._exit(CHILD_SINK_EXIT)


def _is_python(executable: str) -> bool:
    """Only THIS interpreter (the same file as sys.executable or its base executable, whatever the path or name it
    is reached by). A binary merely named python, or a symlink called python to a shell, is not one."""
    real = os.path.realpath(str(executable))
    return any(real == os.path.realpath(ref) for ref in {sys.executable, getattr(sys, "_base_executable", sys.executable)})


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
    if event in SINK_MUTATING_EVENTS or (event == "open" and args and len(args) > 2 and isinstance(args[2], int) and args[2] & SINK_WRITE_FLAGS):
        # The audit sink is the parent's evidence: nothing else may write, truncate, move or delete it.
        if not getattr(_in_sink_io, "depth", 0) and any(_is_sink(a) for a in args[:2]):
            _record("sink_tamper", event)
            raise NetworkDenied(f"offline guard: {event} on the audit sink is refused")
    if event == "open" and args:
        target = args[0]
        if isinstance(target, (str, bytes, os.PathLike)):
            name = os.path.basename(os.fsdecode(target))
            if name in SECRET_BASENAMES:
                _record("secret_file_read", name)
                raise NetworkDenied(f"offline guard: reading {name} refused")


def _set_root(path, *, owns: bool) -> None:
    real, file_id = _validated(path, create=owns)
    _state["root"], _state["root_id"], _state["owns_root"] = real, file_id, owns


def _publish_env() -> None:
    """Children that bypass the Popen wrapper (multiprocessing) inherit the root, the copies and the lineage."""
    os.environ[LOG_ENV] = _state["root"]
    os.environ[PARENT_ENV] = _state["token"]
    os.environ.pop(LAUNCH_ENV, None)
    if _state["diag"]:
        os.environ[DIAG_ENV] = os.pathsep.join(d[0] for d in _state["diag"])
    else:
        os.environ.pop(DIAG_ENV, None)


def install(sink=None) -> None:
    """Idempotent. An audit hook cannot be removed for the life of the process.

    The ROOT audit sink is fixed by the first call that names one (`sink`, else the environment, read once) and can
    never be replaced: a later call naming a different file raises `SinkRebindError`, naming the same file is a
    no-op. Diagnostic copies are separate (`sink_to`) and never stand in for the root."""
    if _state["root"] is not None:
        if sink is not None:
            try:
                same = _validated(sink, create=False)[1] == _state["root_id"]
            except SinkInvalid:
                same = False
            if not same:
                raise SinkRebindError(f"the root audit sink is fixed ({_state['root']}); it cannot be rebound to {sink!r}")
    elif sink is not None:
        _set_root(sink, owns=True)
        _publish_env()
    elif not _state["installed"] and os.environ.get(LOG_ENV):  # a guarded child: the parent chose the root
        _set_root(os.environ[LOG_ENV], owns=False)
        _state["parent_token"] = os.environ.get(PARENT_ENV)
        _state["token"] = os.environ.get(LAUNCH_ENV) or _state["token"]
        for d in [d for d in os.environ.get(DIAG_ENV, "").split(os.pathsep) if d]:
            _state["diag"].append(_validated(d, create=False))
        _publish_env()
    if not _state["installed"]:
        sys.addaudithook(_hook)
        _state["installed"] = True
    install_subprocess_guard()


def install_child() -> None:
    """Called by the guard's sitecustomize in every child. Installs the guard, then proves to the parent that it is
    running by writing `guard_ready` to the parent-fixed root sink (and every copy). A child that cannot report
    refuses to run."""
    _state["child_fatal"] = True
    try:
        install()
    except SinkError as exc:
        os.write(2, f"offline guard: {exc}; refusing to run\n".encode())
        os._exit(CHILD_SINK_EXIT)
    if _state["ready"]:
        return
    row = {"kind": "guard_ready", "proc": _state["token"], "parent_proc": _state["parent_token"],
           "pid": os.getpid(), "ppid": os.getppid()}
    if _state["root"] is None or not _write_all(row):
        os.write(2, b"offline guard: no writable audit sink in the child; refusing to run\n")
        os._exit(CHILD_SINK_EXIT)
    _state["ready"] = True


@contextlib.contextmanager
def sink_to(path):
    """Also write the audit rows of this process and its children launched inside the block to `path` (a regular
    file). A diagnostic COPY only: the root sink still receives every row, so a scope can neither hide nor replace
    evidence. A device, directory or unusable path raises `SinkInvalid`."""
    install()
    if _state["root"] is None:
        raise SinkError("no root audit sink is configured; there is nothing to copy")
    entry = _validated(path, create=True)
    _state["diag"].append(entry)
    try:
        yield
    finally:
        _state["diag"].remove(entry)


def sink_rows(path) -> list[dict]:
    """Every row of an audit sink. A missing file, a malformed or non-object row raises `SinkCorrupt`: an unreadable
    sink must never be reported as zero attempts."""
    p = Path(path)
    try:
        text = p.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise SinkCorrupt(f"audit sink {p} is unreadable: {exc}") from exc
    rows = []
    lines = text.split("\n")
    if lines.pop() != "":  # every row is one complete newline-terminated write
        raise SinkPartialRow(f"audit sink {p} ends in a partial row")
    for n, line in enumerate(lines, 1):
        try:
            row = json.loads(line)
        except ValueError as exc:
            raise SinkCorrupt(f"audit sink {p} line {n} is not valid JSON") from exc
        if (not isinstance(row, dict) or not isinstance(row.get("kind"), str)
                or any(not isinstance(row.get(k), (str, type(None))) for k in ("proc", "parent_proc", "launch"))):
            raise SinkCorrupt(f"audit sink {p} line {n} is not an audit row")
        rows.append(row)
    return rows


def audit(rows: list[dict], me: str, *, start: int = 0, owner: bool = False) -> dict:
    """What the session owned by process `me` must account for in `rows[start:]`.

    Lineage is rebuilt from the sink itself (launch ids, not pids): an edge `parent -> child` comes from each
    `child_launched` row (expected when the parent launched it inside `expect_denied`) and each `guard_ready` row.
    A descendant is expected only if every path to it passed through an expected launch. Returns
    `unexpected` (attempt rows by descendants), `uninitialized` (launch ids of guarded children that never wrote
    `guard_ready`) and, for the owner of the root, `orphans` (attempts by processes outside any known lineage)."""
    pairs: dict[tuple, bool] = {}  # (parent, child) -> launched inside expect_denied; the same edge is seen twice
    ready = set()
    for r in rows:
        if r["kind"] == "child_launched":
            key = (r.get("proc"), r.get("launch"))
            pairs[key] = pairs.get(key, False) or bool(r.get("expected"))
        elif r["kind"] == "guard_ready":
            ready.add(r.get("proc"))
            if r.get("parent_proc"):
                pairs.setdefault((r["parent_proc"], r.get("proc")), False)
    edges: dict[str, list[tuple[str, bool]]] = {}
    for (parent, child), expected in pairs.items():
        edges.setdefault(parent, []).append((child, expected))
    reach = {me: False}
    changed = True
    while changed:
        changed = False
        for parent in list(reach):
            for child, expected in edges.get(parent, []):
                via = reach[parent] or expected
                if child not in reach or (reach[child] and not via):
                    reach[child], changed = via, True
    new = rows[start:]
    # A row's own `expected` flag is the writer's claim and is ignored: only a launch made inside the owner's
    # `expect_denied` (an edge above) can mark a descendant's attempts as deliberate.
    attempts = [r for r in new if r["kind"] not in NON_ATTEMPT_KINDS and r.get("proc") != me]
    return {"unexpected": [r for r in attempts if r.get("proc") in reach and not reach[r["proc"]]],
            "orphans": [r for r in attempts if r.get("proc") not in reach] if owner else [],
            "uninitialized": sorted({str(r.get("launch")) for r in new if r["kind"] == "child_launched" and r.get("guarded")
                                     and r.get("proc") in reach and r.get("launch") not in ready})}


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


def guarded_env(env, launch: str | None = None) -> dict:
    """`env` (or the current environment when None) with the guard's `sitecustomize` directory first on
    PYTHONPATH and the audit lineage: the ROOT sink (a child-supplied value is replaced, never kept), the
    diagnostic copies, this process as the parent and a fresh launch id for the child. A Python child then installs
    the guard before running any of its own code even when the caller passed a minimal or hand-built environment."""
    out = dict(os.environ if env is None else env)
    paths = [p for p in out.get("PYTHONPATH", "").split(os.pathsep) if p and p != str(GUARD_SITE)]
    out["PYTHONPATH"] = os.pathsep.join([str(GUARD_SITE), *paths])  # first: no other sitecustomize may win
    for name in (LOG_ENV, DIAG_ENV, LAUNCH_ENV, PARENT_ENV):
        out.pop(name, None)
    if _state["root"] is not None:
        out[LOG_ENV], out[PARENT_ENV] = _state["root"], _state["token"]
        out[LAUNCH_ENV] = launch or uuid.uuid4().hex
        if _state["diag"]:
            out[DIAG_ENV] = os.pathsep.join(d[0] for d in _state["diag"])
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


def screen_spawn(args, shell: bool, executable=None, env=None, cwd=None) -> bool:
    """Refuse a launch the guard cannot protect (see `launch_refusal`). `shell=True` always launches a shell."""
    if shell:
        executable = executable or "/bin/sh"
        argv = [executable, "-c", os.fsdecode(args) if isinstance(args, (str, bytes, os.PathLike)) else " ".join(map(os.fsdecode, args))]
    else:
        argv = [os.fsdecode(args)] if isinstance(args, (str, bytes, os.PathLike)) else [os.fsdecode(a) for a in args]
        executable = executable or (argv[0] if argv else None)
    if executable is None:
        return False
    env = guarded_env(env) if env is None else env  # the wrapper passes the final (already guarded) child environment
    refusal = launch_refusal(executable, argv, cwd, env)
    if refusal:
        _record(refusal[0], refusal[1])
        raise NetworkDenied(f"offline guard: {refusal[2]}")
    return _is_python(_resolve(executable, env, cwd))  # True: a guarded Python child that must announce itself


def install_subprocess_guard() -> None:
    """Route every `subprocess.Popen` through a wrapper that screens the launch and injects the guard into the
    child's environment, wherever the caller built that environment. Idempotent."""
    if _state.get("subprocess_guard"):
        return
    original = subprocess.Popen.__init__
    _state["original_popen_init"] = original  # exposed only so a test can prove the audit backstop

    def guarded_init(self, args, *pos, **kw):
        shell = kw.get("shell", pos[7] if len(pos) > 7 else False)
        cwd = kw.get("cwd", pos[8] if len(pos) > 8 else None)
        launch = uuid.uuid4().hex
        env_positional = len(pos) > 9  # env given positionally
        child_env = guarded_env(pos[9] if env_positional else kw.get("env"), launch)
        if not _sinks_usable():  # evidence from this child could not be collected: do not start it
            _record("audit_sink_unavailable", str(_state["root"]))
            raise NetworkDenied("offline guard: the audit sink is not writable; a child launch would be unobservable")
        python_child = screen_spawn(args, bool(shell), kw.get("executable", pos[1] if len(pos) > 1 else None), child_env, cwd)
        if env_positional:
            pos = list(pos)
            pos[9] = child_env
        else:
            kw["env"] = child_env
        _in_guarded_popen.depth = getattr(_in_guarded_popen, "depth", 0) + 1
        try:
            result = original(self, args, *pos, **kw)
        finally:
            _in_guarded_popen.depth -= 1
        if _state["root"] is not None:  # the child must later announce `guard_ready`; the session audit enforces it
            row = {"kind": "child_launched", "proc": _state["token"], "launch": launch, "child_pid": self.pid,
                   "guarded": python_child, "expected": _state["expect_depth"] > 0, "pid": os.getpid()}
            if not _write_all(row):
                self.kill()
                self.wait()
                raise NetworkDenied("offline guard: the child launch could not be recorded; the child was stopped")
        return result

    subprocess.Popen.__init__ = guarded_init
    _state["subprocess_guard"] = True


def attempts() -> list[dict]:
    return list(_state["attempts"])


def unexpected_attempts() -> list[dict]:
    return [a for a in _state["attempts"] if not a["expected"]]


_BOOT = ("import runpy, sys; sys.path.insert(0, sys.argv[1]); import _netguard; _netguard.install(); "
         "script = sys.argv[2]; sys.argv = sys.argv[2:]; runpy.run_path(script, run_name='__main__')")


def run_guarded(script: Path, argv: list[str], *, env: dict, cwd: Path, log: Path):
    """Run a Python script in a child with the same guard installed. The root sink receives every row; `log` also
    receives a copy. Returns (CompletedProcess, [attempt rows from `log`]). A child that never initialized the guard
    is an error. Deliberate probes need an explicit `expect_denied()` around the call."""
    install_subprocess_guard()
    log = Path(log)
    with sink_to(log):
        proc = subprocess.run([sys.executable, "-c", _BOOT, str(Path(__file__).resolve().parent), str(script), *argv],
                              capture_output=True, text=True, env=env, cwd=cwd)
    all_rows = sink_rows(log)
    missing = audit(all_rows, _state["token"])["uninitialized"]
    if missing:
        raise NetworkDenied(f"offline guard: child launch(es) {missing} never initialized the guard")
    return proc, [r for r in all_rows if r.get("kind") not in NON_ATTEMPT_KINDS]
