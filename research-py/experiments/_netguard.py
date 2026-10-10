"""Process-wide, non-removable denial of network access and secret-file reads for offline research tests.

Independent of environment variables, `.env.local`, flag state and any monkeypatching: it is a
`sys.addaudithook` hook, which fires for every real `socket.connect` / name resolution and for every
`open` of a secret file, whichever Python API reached it. Attempts are RECORDED (not only refused), so a
swallowed exception still fails the test and the session summary reports attempted == 0 or not.
"""

from __future__ import annotations

import atexit
import contextlib
import hashlib
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import threading
import time
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
NON_ATTEMPT_KINDS = frozenset({"guard_ready", "child_launched", "child_exited", "sink_failure"})
CHILD_SINK_EXIT = 97

# `root`/`root_id`: the immutable root audit sink (fixed by the first install; the authority the session audits).
# `diag`: diagnostic copies [(path, file id)]; they only ever receive copies of what the root also receives.
_state: dict = {"installed": False, "attempts": [], "expect_depth": 0, "allowed": {}, "root": None, "root_id": None,
                "owns_root": False, "diag": [], "ready": False, "token": uuid.uuid4().hex, "parent_token": None, "sink_errors": 0,
                "tracked": [], "integrity": [], "loss_depth": 0}
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


def _marker_dir() -> str | None:
    return None if _state["root"] is None else _state["root"] + ".failed"


def failure_markers() -> list[str]:
    """Tokens of processes that reported losing their audit evidence by creating an EMPTY file (creating a file
    needs no bytes, so it survives a full disk, RLIMIT_FSIZE=0 and a replaced root)."""
    d = _marker_dir()
    if d is None:
        return []
    try:
        with _sink_io():
            return sorted(n.split(".")[0] for n in os.listdir(d))
    except OSError as exc:
        if _state["owns_root"]:
            raise SinkCorrupt(f"audit failure-marker directory {d} is unreadable: {exc}") from exc
        return []


def _report_failure(subject: str, reason: str) -> None:
    """Best-effort durable report that `subject` lost (or may have lost) audit evidence: a row, and an empty marker."""
    wrote = _write_all({"kind": "sink_failure", "proc": _state["token"], "subject": subject, "reason": reason,
                        "pid": os.getpid()})
    marked = False
    d = _marker_dir()
    if d is not None:
        with _sink_io():
            try:
                os.close(os.open(os.path.join(d, f"{subject}.{reason}"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600))
                marked = True
            except FileExistsError:
                marked = True
            except OSError:
                pass
    if not (wrote or marked):
        _state["sink_errors"] += 1


def _is_sink(target) -> bool:
    if not isinstance(target, (str, bytes, os.PathLike)):
        return False
    real = os.path.realpath(os.fsdecode(target))
    if _state["root"] is not None and real.startswith(os.path.realpath(_state["root"] + ".failed") + os.sep):
        return True  # the failure markers are evidence too
    return any(real == os.path.realpath(sk) for sk in [_state["root"], *[d[0] for d in _state["diag"]]] if sk)


def _record(kind: str, detail: str) -> None:
    entry = {"kind": kind, "detail": detail, "expected": _state["expect_depth"] > 0, "pid": os.getpid(),
             "proc": _state["token"]}
    _state["attempts"].append(entry)
    if _state["root"] is not None and not _write_all(entry):
        _state["sink_errors"] += 1
        if _state.get("child_fatal"):
            # The denial stands, but its evidence could not be collected: a child that cannot report must not go on.
            # Tell the parent through a channel that needs no bytes in the sink (an empty marker file) before leaving.
            _report_failure(_state["token"], "post_ready_sink_loss")
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
    if owns:
        with _sink_io():
            os.makedirs(real + ".failed", exist_ok=True)


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
        _report_failure(_state["token"], "install_failed")
        os.write(2, f"offline guard: {exc}; refusing to run\n".encode())
        os._exit(CHILD_SINK_EXIT)
    if _state["ready"]:
        return
    row = {"kind": "guard_ready", "proc": _state["token"], "parent_proc": _state["parent_token"],
           "pid": os.getpid(), "ppid": os.getppid()}
    if _state["root"] is None or not _write_all(row):
        _report_failure(_state["token"], "ready_failed")
        os.write(2, b"offline guard: no writable audit sink in the child; refusing to run\n")
        os._exit(CHILD_SINK_EXIT)
    _state["ready"] = True
    atexit.register(finalize_children, 0, 2.0)  # a child records the fate of the children it launched


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


def _reach(me: str, pairs: dict, attr: str) -> dict:
    """Descendants of `me` -> True only if EVERY path to them passed through a launch carrying `attr`."""
    edges: dict[str, list[tuple[str, bool]]] = {}
    for (parent, child), flags in pairs.items():
        edges.setdefault(parent, []).append((child, flags[attr]))
    reach = {me: False}
    changed = True
    while changed:
        changed = False
        for parent in list(reach):
            for child, flagged in edges.get(parent, []):
                via = reach[parent] or flagged
                if child not in reach or (reach[child] and not via):
                    reach[child], changed = via, True
    return reach


def audit(rows: list[dict], me: str, *, start: int = 0, owner: bool = False, markers=(), known_failures=()) -> dict:
    """What the session owned by process `me` must account for in `rows[start:]`.

    Lineage is rebuilt from the sink itself (launch ids, not pids): an edge `parent -> child` comes from each
    `child_launched` row (`expected` when the parent launched it inside `expect_denied`, `loss_expected` inside
    `expect_evidence_loss`) and each `guard_ready` row. A descendant is deliberate only if every path to it passed
    through such a launch; a writer's own flags are ignored. Returns
    `unexpected` (attempt rows by descendants), `uninitialized` (launch ids that never wrote `guard_ready`),
    `integrity` (tokens of descendants that LOST audit evidence after or before becoming ready, from their own
    row/marker or the parent's finalization of their exit; also launches whose exit was never recorded),
    `expected_losses` (the same, excused by `expect_evidence_loss`) and, for the owner of the root, `orphans`.
    `expect_denied` never excuses a lost-evidence finding: it excuses only denied probes whose evidence arrived."""
    pairs: dict[tuple, dict] = {}  # (parent, child) -> flags; the same edge is seen twice (launched + ready)
    ready, exited = set(), set()
    for r in rows:
        if r["kind"] == "child_launched":
            f = pairs.setdefault((r.get("proc"), r.get("launch")), {"expected": False, "loss": False})
            f["expected"] = f["expected"] or bool(r.get("expected"))
            f["loss"] = f["loss"] or bool(r.get("loss_expected"))
        elif r["kind"] == "guard_ready":
            ready.add(r.get("proc"))
            if r.get("parent_proc"):
                pairs.setdefault((r["parent_proc"], r.get("proc")), {"expected": False, "loss": False})
        elif r["kind"] == "child_exited":
            exited.add(r.get("launch"))
    reach, reach_loss = _reach(me, pairs, "expected"), _reach(me, pairs, "loss")
    new = rows[start:]
    # A row's own `expected` flag is the writer's claim and is ignored: only a launch made inside the owner's
    # `expect_denied` (an edge above) can mark a descendant's attempts as deliberate.
    attempts = [r for r in new if r["kind"] not in NON_ATTEMPT_KINDS and r.get("proc") != me]
    failed = {r.get("subject") or r.get("proc") for r in new if r["kind"] == "sink_failure"} | set(markers) | set(known_failures)
    failed |= {r["launch"] for r in new if r["kind"] == "child_launched" and r.get("proc") in reach and r.get("guarded")
               and r.get("launch") not in exited}      # a launch whose final state nobody recorded is indeterminate
    failed.discard(me)
    in_tree = [t for t in sorted(failed) if t in reach]
    outside = [t for t in sorted(failed) if t not in reach] if owner else []
    return {"unexpected": [r for r in attempts if r.get("proc") in reach and not reach[r["proc"]]],
            "orphans": [r for r in attempts if r.get("proc") not in reach] if owner else [],
            "uninitialized": sorted({str(r.get("launch")) for r in new if r["kind"] == "child_launched" and r.get("guarded")
                                     and r.get("proc") in reach and r.get("launch") not in ready}),
            "integrity": [t for t in in_tree if not reach_loss[t]] + outside,
            "expected_losses": [t for t in in_tree if reach_loss[t]]}


def finalize_children(since: int = 0, timeout: float = 5.0) -> list[dict]:
    """Establish the final state of the guarded children THIS process launched (`since` = index into the launch list),
    whether or not their launcher ever looked at the result: reap or wait (bounded; a child still running when the
    bound ends is killed and counts as lost evidence), record a `child_exited` row, and report as lost evidence any
    child that exited with the sink-failure code, died of SIGXFSZ (a write past RLIMIT_FSIZE), or had no outcome.
    Returns the failure descriptors. Idempotent per child."""
    failures = []
    deadline = time.monotonic() + timeout
    for entry in _state["tracked"][since:]:
        if entry["finalized"] or entry["owner_pid"] != os.getpid():
            continue  # a forked copy of this process must not judge (or wait for) its parent's children
        proc = entry["popen"]
        rc = proc.poll()
        if rc is None:
            try:
                rc = proc.wait(timeout=max(0.0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
                rc = None
        entry["finalized"] = True
        reason = ("outstanding" if rc is None else "exit_sink_failure" if rc == CHILD_SINK_EXIT
                  else "sigxfsz" if rc == -signal.SIGXFSZ else None)
        _write_all({"kind": "child_exited", "proc": _state["token"], "launch": entry["launch"], "returncode": rc,
                    "pid": os.getpid()})
        if reason:
            _report_failure(entry["launch"], reason)
            failures.append({"launch": entry["launch"], "reason": reason, "returncode": rc,
                             "loss_expected": entry["loss_expected"]})
            _state["integrity"].append(failures[-1])
    return failures


@contextlib.contextmanager
def expect_evidence_loss():
    """A test that deliberately makes a child lose its audit evidence says so HERE, for the launches inside the block.
    The loss is reported as `expected_losses` instead of `integrity`; denied probes need `expect_denied` separately,
    and `expect_denied` never covers a loss."""
    install()
    _state["loss_depth"] += 1
    try:
        yield
    finally:
        _state["loss_depth"] -= 1


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
            loss = _state["loss_depth"] > 0
            row = {"kind": "child_launched", "proc": _state["token"], "launch": launch, "child_pid": self.pid,
                   "guarded": python_child, "expected": _state["expect_depth"] > 0, "loss_expected": loss,
                   "pid": os.getpid()}
            _state["tracked"].append({"launch": launch, "popen": self, "finalized": False, "loss_expected": loss,
                                      "owner_pid": os.getpid()})
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
    first = len(_state["tracked"])
    with sink_to(log):
        proc = subprocess.run([sys.executable, "-c", _BOOT, str(Path(__file__).resolve().parent), str(script), *argv],
                              capture_output=True, text=True, env=env, cwd=cwd)
        lost = [f for f in finalize_children(first) if not f["loss_expected"]]
    if lost:
        raise NetworkDenied(f"offline guard: child launch(es) {[f['launch'] for f in lost]} lost their audit evidence")
    all_rows = sink_rows(log)
    missing = audit(all_rows, _state["token"])["uninitialized"]
    if missing:
        raise NetworkDenied(f"offline guard: child launch(es) {missing} never initialized the guard")
    return proc, [r for r in all_rows if r.get("kind") not in NON_ATTEMPT_KINDS]
