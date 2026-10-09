"""An ordinary spawned child inherits the offline guard. The audit hook lives in the parent; these tests prove
the child gets its own before it runs any code, whatever environment the caller hands it, and that a child cannot
be started in a way that skips it. Synthetic `.env.local` files and a loopback sink only: no provider request,
no real credential, no external connection."""

from __future__ import annotations

import contextlib
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
EXPERIMENTS = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(EXPERIMENTS))

import _netguard  # noqa: E402

@contextlib.contextmanager
def probing(log):
    """Deliberate probes: explicit expected-denial semantics plus a diagnostic copy. The root sink still receives
    every row; nothing here exempts a `sink_to` scope from the session audit."""
    with _netguard.expect_denied(), _netguard.sink_to(log):
        yield


CHILD = """
import os, socket, sys
os.environ.pop("MQK_HERMETIC_NO_PROVIDER", None)      # the child tries to drop the provider-boundary flag
out = {}
try:
    out["read"] = open(sys.argv[1]).read().strip()[:22]
except Exception as e:
    out["read"] = "REFUSED:" + type(e).__name__
try:
    socket.create_connection(("127.0.0.1", int(sys.argv[2])), timeout=2).close()
    out["connect"] = "SUCCEEDED"
except Exception as e:
    out["connect"] = "REFUSED:" + type(e).__name__
try:
    socket.getaddrinfo("data.alpaca.markets", 443)
    out["resolve"] = "SUCCEEDED"
except Exception as e:
    out["resolve"] = "REFUSED:" + type(e).__name__
print("RESULT " + repr(out))
"""


def plant_env_local(directory: Path) -> Path:
    staged = directory / "staged"
    staged.write_text("ALPACA_API_KEY_PAPER=fake\n", encoding="utf-8")
    target = directory / ".env.local"
    staged.rename(target)  # created without opening a ".env.local" path: the parent guard refuses even writes
    return target


class Sink:
    """A loopback listener standing in for 'the network'. Any accepted connection is a leak."""

    def __init__(self):
        self.srv = socket.socket()
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(4)
        self.srv.settimeout(0.3)
        self.port = self.srv.getsockname()[1]
        self.accepted = 0
        self._stop = False
        self._t = threading.Thread(target=self._run, daemon=True)
        self._t.start()

    def _run(self):
        while not self._stop:
            try:
                conn, _ = self.srv.accept()
                self.accepted += 1
                conn.close()
            except OSError:
                continue

    def close(self):
        self._stop = True
        self._t.join(timeout=2)
        self.srv.close()


@pytest.fixture
def world(tmp_path):
    sink = Sink()
    secret = plant_env_local(tmp_path)
    yield tmp_path, secret, sink
    sink.close()


def run_child(world, *, env, via, log: Path):
    tmp, secret, sink = world
    argv = [sys.executable, "-c", CHILD, str(secret), str(sink.port)]
    with probing(log):
        if via == "run":
            return subprocess.run(argv, capture_output=True, text=True, env=env, cwd=tmp)
        if via == "popen":
            p = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, cwd=tmp)
            out, err = p.communicate()
            return subprocess.CompletedProcess(argv, p.returncode, out, err)
        if via == "positional_env":
            p = subprocess.Popen(argv, -1, None, None, subprocess.PIPE, subprocess.PIPE, None, True, False, str(tmp), env,
                                 universal_newlines=True)
            out, err = p.communicate()
            return subprocess.CompletedProcess(argv, p.returncode, out, err)
    raise AssertionError(via)


def result_of(proc) -> dict:
    line = [l for l in proc.stdout.splitlines() if l.startswith("RESULT ")]
    assert line, proc.stdout + proc.stderr
    return eval(line[0][len("RESULT "):])  # a dict literal printed by the controlled child above


def log_rows(log: Path) -> list[dict]:
    """The attempts a child recorded (not the guard_ready / child_launched bookkeeping)."""
    return [r for r in _netguard.sink_rows(log) if r["kind"] not in _netguard.NON_ATTEMPT_KINDS]


@pytest.mark.parametrize("via", ["run", "popen", "positional_env"])
@pytest.mark.parametrize("env_kind", ["minimal", "empty", "inherited", "flag_removed_and_path_cleared"])
def test_an_ordinary_child_cannot_read_a_fake_env_local_resolve_or_connect(world, tmp_path, via, env_kind):
    base = {
        "minimal": {"PATH": os.environ["PATH"]},
        "empty": {},
        "inherited": dict(os.environ),
        "flag_removed_and_path_cleared": {k: v for k, v in os.environ.items() if k not in ("MQK_HERMETIC_NO_PROVIDER", "PYTHONPATH")},
    }[env_kind]
    log = tmp_path / f"child_{via}_{env_kind}.log"
    proc = run_child(world, env=base, via=via, log=log)
    got = result_of(proc)
    assert got["read"].startswith("REFUSED:") and got["connect"].startswith("REFUSED:") and got["resolve"].startswith("REFUSED:"), got
    assert world[2].accepted == 0, "the loopback sink saw a connection: the guard leaked"
    kinds = {r["kind"] for r in log_rows(log)}
    assert {"secret_file_read", "network"} <= kinds, "the child's attempts must be recorded, not only refused"
    assert {r["pid"] for r in log_rows(log)}.isdisjoint({os.getpid()})


def test_multiprocessing_spawn_children_are_guarded_too(tmp_path):
    probe = tmp_path / "mp_probe.py"
    probe.write_text(
        "import multiprocessing as mp, socket\n"
        "def child(q):\n"
        "    try:\n        socket.getaddrinfo('example.invalid', 80); q.put('RESOLVED')\n"
        "    except Exception as e:\n        q.put('REFUSED:' + type(e).__name__)\n"
        "if __name__ == '__main__':\n"
        "    ctx = mp.get_context('spawn'); q = ctx.Queue(); p = ctx.Process(target=child, args=(q,)); p.start()\n"
        "    print('RESULT', q.get(timeout=30)); p.join()\n", encoding="utf-8")
    log = tmp_path / "mp.log"
    with probing(log):
        proc = subprocess.run([sys.executable, str(probe)], capture_output=True, text=True, cwd=tmp_path,
                              env={"PATH": os.environ["PATH"]})
    assert "RESULT REFUSED:NetworkDenied" in proc.stdout, proc.stdout + proc.stderr[-400:]
    assert any(r["kind"] == "network" for r in log_rows(log))


def test_a_grandchild_started_by_a_child_with_a_cleared_environment_is_guarded_too(world, tmp_path):
    tmp, secret, sink = world
    child = ("import subprocess, sys\n"
             "r = subprocess.run([sys.executable, '-c', sys.argv[3], sys.argv[1], sys.argv[2]], env={}, capture_output=True, text=True)\n"
             "print(r.stdout.strip() or r.stderr.strip()[-300:])\n")
    log = tmp_path / "grand.log"
    with probing(log):
        proc = subprocess.run([sys.executable, "-c", child, str(secret), str(sink.port), CHILD], capture_output=True, text=True,
                              env={"PATH": os.environ["PATH"]}, cwd=tmp)
    got = eval([l for l in proc.stdout.splitlines() if l.startswith("RESULT ")][0][len("RESULT "):])
    assert got["read"].startswith("REFUSED:") and got["connect"].startswith("REFUSED:"), got
    assert sink.accepted == 0
    assert len({r["pid"] for r in log_rows(log)}) == 1 and any(r["kind"] == "secret_file_read" for r in log_rows(log))


def test_a_shell_launched_python_child_is_refused_because_the_guard_cannot_instrument_the_shell(world, tmp_path):
    tmp, secret, sink = world
    cmd = f'"{sys.executable}" -c \'{CHILD}\' "{secret}" {sink.port}'
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="not a Python interpreter"):
            subprocess.run(cmd, shell=True, capture_output=True, text=True, cwd=tmp)
    assert [a["kind"] for a in seen] == ["unsupported_launcher"] and sink.accepted == 0


@pytest.mark.parametrize("flags", [["-I"], ["-S"], ["-E"], ["-Ic"], ["-sI"], ["-Wignore", "-S"], ["-W", "ignore", "-I"],
                                   ["-X", "dev", "-S"], ["-Xdev", "-E"], ["-B", "-s", "-E"], ["-bI"], ["-W", "ignore", "-Wall", "-I"]])
def test_a_python_child_in_an_isolation_mode_that_would_skip_the_guard_is_refused_before_it_starts(tmp_path, flags):
    marker = tmp_path / "ran"
    code = f"open(r'{marker}', 'w').write('x')"
    argv = [sys.executable, *flags, "-c", code] if "c" not in flags[-1] else [sys.executable, *flags[:-1], "-Ic", code]
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="isolated mode"):
            subprocess.run(argv)
    assert not marker.exists() and [a["kind"] for a in seen] == ["guard_bypass_spawn"]


@pytest.mark.parametrize("argv,shell,kind", [
    (["curl", "https://example.invalid"], False, "network_tool_spawn"), (["wget", "-q", "x"], False, "network_tool_spawn"),
    ("curl https://example.invalid", True, "unsupported_launcher"), (["sh", "-c", "nc -z example.invalid 80"], False, "unsupported_launcher")])
def test_launching_a_network_client_is_refused(argv, shell, kind):
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied):
            subprocess.run(argv, shell=shell)
    assert [a["kind"] for a in seen] == [kind]


@pytest.mark.parametrize("argv,shell", [(["cat", "/tmp/x/.env.local"], False), (["sh", "-c", "cat ./.env.local"], False),
                                         ("grep KEY .env", True), (["head", ".env.production"], False)])
def test_a_non_python_child_is_never_handed_a_secret_file(argv, shell):
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="not a Python interpreter"):
            subprocess.run(argv, shell=shell)
    assert [a["kind"] for a in seen] == ["unsupported_launcher"]


def test_spawn_apis_the_guard_cannot_follow_into_a_child_are_refused(tmp_path):
    marker = tmp_path / "ran"
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied):
            os.system(f"touch {marker}")
        with pytest.raises(_netguard.NetworkDenied):
            os.posix_spawn("/bin/sh", ["sh", "-c", f"touch {marker}"], dict(os.environ))
    assert not marker.exists() and {a["detail"] for a in seen} == {"os.system", "os.posix_spawn"}


def test_the_audit_backstop_refuses_a_python_child_launched_around_the_wrapper(tmp_path):
    """If something calls the original Popen initializer directly (bypassing the guarded wrapper), the audit hook
    still refuses a Python child whose environment does not carry the guard."""
    original = _netguard._state["original_popen_init"]
    marker = tmp_path / "ran"
    bare = subprocess.Popen.__new__(subprocess.Popen)
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="without the guard"):
            original(bare, [sys.executable, "-c", f"open(r'{marker}', 'w')"], env={"PATH": os.environ["PATH"]})
    assert not marker.exists() and [a["kind"] for a in seen] == ["unguarded_python_child"]


def test_a_child_attempt_in_an_ordinary_test_fails_that_test_even_when_the_child_swallows_the_error(tmp_path):
    """End to end through the real conftest: a test that spawns a child which tries an external connection and
    ignores the failure still fails the session, and a clean child does not."""
    test = tmp_path / "test_child.py"
    test.write_text(
        "import subprocess, sys\n"
        "def test_leaky_child():\n"
        "    subprocess.run([sys.executable, '-c', 'import socket\\ntry:\\n socket.create_connection((\"127.0.0.1\", 9), timeout=1)\\nexcept Exception:\\n pass'])\n"
        "def test_clean_child():\n"
        "    subprocess.run([sys.executable, '-c', 'print(1)'])\n", encoding="utf-8")
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS)}
    with _netguard.expect_denied():
        proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                               "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "2 passed, 1 error" in out, out[-800:]  # the teardown check errors the leaky test
    assert "a spawned child attempted external network/secret access" in out


# ----------------------------------------------- intermediary launchers and nested interpreters are refused, not scanned

NESTED_READ = "print('LEAK', open('.env' + '.local').read().strip()[:22])"  # no secret word on any command line


def _refused(argv, **kw):
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied):
            subprocess.run(argv, capture_output=True, text=True, **kw)
    return [a["kind"] for a in seen]


@pytest.mark.parametrize("build", [
    lambda py, code: ["/bin/sh", "-c", f'{py} -I -c "{code}"'],                       # the independent reproduction
    lambda py, code: ["/bin/sh", "-c", f"{py} -S -c '{code}'"],
    lambda py, code: ["sh", "-c", f'exec {py} -E -c "{code}"'],
    lambda py, code: ["bash", "-c", f'{py} -c "{code}"'],                               # even a plain nested Python
    lambda py, code: ["/bin/dash", "-c", f"{py} -I -c \\\"{code}\\\""],              # shell quoting variants
    lambda py, code: ["zsh", "-c", f"'{py}' -I -c '{code}'"],
    lambda py, code: ["env", py, "-I", "-c", code],                                      # intermediary env
    lambda py, code: ["/usr/bin/env", "-i", py, "-c", code],
    lambda py, code: ["env", "-S", f"{py} -I -c '{code}'"],
    lambda py, code: ["nice", py, "-c", code],
    lambda py, code: ["timeout", "10", py, "-I", "-c", code],
    lambda py, code: ["xargs", "-0", py, "-c", code],
    lambda py, code: ["busybox", "sh", "-c", f"{py} -I -c '{code}'"],
    lambda py, code: ["perl", "-e", f"exec('{py}', '-I', '-c', '{code}')"],
])
def test_an_intermediary_launcher_or_nested_interpreter_is_refused_and_never_reads_the_secret(tmp_path, build):
    plant_env_local(tmp_path)
    marker = tmp_path / "ran"
    code = NESTED_READ + f"; open(r'{marker}', 'w')"
    assert _refused(build(sys.executable, code), cwd=tmp_path) == ["unsupported_launcher"]
    assert not marker.exists(), "the nested child ran"


def test_shell_true_is_refused_whatever_the_command(tmp_path):
    plant_env_local(tmp_path)
    marker = tmp_path / "ran"
    for cmd in (f"touch {marker}", f'{sys.executable} -I -c "open(\'{marker}\', \'w\')"', "true"):
        assert _refused(cmd, shell=True, cwd=tmp_path) == ["unsupported_launcher"]
    assert _refused(["true"], shell=True, executable="/bin/bash", cwd=tmp_path) == ["unsupported_launcher"]
    assert not marker.exists()


def test_the_executable_argument_cannot_disguise_a_shell_as_python(tmp_path):
    marker = tmp_path / "ran"
    kinds = _refused([sys.executable, "-c", f"open(r'{marker}', 'w')"], executable="/bin/sh", cwd=tmp_path)
    assert kinds == ["unsupported_launcher"] and not marker.exists()


def test_alternate_interpreter_paths_are_judged_by_what_they_are_not_what_they_are_called(tmp_path):
    marker = tmp_path / "ran"
    link = tmp_path / "py"
    link.symlink_to(sys.executable)                                 # a real interpreter under another name: guarded
    assert _refused([str(link), "-I", "-c", "pass"], cwd=tmp_path) == ["guard_bypass_spawn"]
    fake = tmp_path / "python3"                                       # a script NAMED python is not an interpreter
    fake.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
    fake.chmod(0o755)
    assert _refused([str(fake)], cwd=tmp_path) == ["unsupported_launcher"] and not marker.exists()
    disguised = tmp_path / "disguised" / "python"                     # a symlink NAMED python that is really a shell
    disguised.parent.mkdir()
    disguised.symlink_to(shutil.which("sh"))
    assert _refused([str(disguised), "-c", f"touch {marker}"], cwd=tmp_path) == ["unsupported_launcher"]
    assert _refused(["python", "-c", f"touch {marker}"], env={"PATH": str(disguised.parent)}, cwd=tmp_path) == ["unsupported_launcher"]
    assert not marker.exists()
    other = tmp_path / "bin"
    other.mkdir()
    (other / "python3").symlink_to(fake)                              # via PATH lookup too
    assert _refused(["python3"], env={"PATH": str(other)}, cwd=tmp_path) == ["unsupported_launcher"]
    assert not marker.exists()


@pytest.mark.parametrize("argv,expected", [
    (["py", "-I"], True), (["py", "-S"], True), (["py", "-E"], True), (["py", "-sI"], True), (["py", "-BE"], True),
    (["py", "-W", "ignore", "-I"], True), (["py", "-Wignore", "-S"], True), (["py", "-X", "dev", "-E"], True),
    (["py", "-Xdev", "-I"], True), (["py", "--check-hash-based-pycs", "always", "-I"], True), (["py", "-O", "-S", "x.py"], True),
    (["py", "-c", "-I"], False), (["py", "-m", "mod", "-I"], False), (["py", "script.py", "-I"], False), (["py", "-"], False),
    (["py", "-W", "-I"], False),     # `-I` here is the argument of -W, not a flag
    (["py", "-B", "-c", "x"], False), (["py", "-W", "ignore", "-m", "json.tool"], False), (["py"], False)])
def test_python_isolation_flag_parsing_table(argv, expected):
    assert _netguard._python_skips_guard(argv) is expected


def test_only_a_vouched_executable_with_unchanged_bytes_may_run(tmp_path):
    marker = tmp_path / "ran"
    stub = tmp_path / "stub.sh"
    stub.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
    stub.chmod(0o755)
    assert _refused([str(stub)]) == ["unsupported_launcher"] and not marker.exists()      # not vouched
    with _netguard.allow_executable(stub):
        assert subprocess.run([str(stub)]).returncode == 0 and marker.exists()           # vouched: runs
        marker.unlink()
        stub.write_text(stub.read_text() + "# substituted\n", encoding="utf-8")        # same path, new bytes
        assert _refused([str(stub)]) == ["unsupported_launcher"] and not marker.exists()
        assert _refused(["/bin/sh", str(stub)]) == ["unsupported_launcher"]               # vouching is not transitive
    stub.write_text(f"#!/bin/sh\ntouch {marker}\n", encoding="utf-8")
    assert _refused([str(stub)]) == ["unsupported_launcher"]                              # the vouching ended


def test_the_audit_backstop_refuses_a_shell_launched_around_the_wrapper(tmp_path):
    original = _netguard._state["original_popen_init"]
    marker = tmp_path / "ran"
    bare = subprocess.Popen.__new__(subprocess.Popen)
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="not a Python interpreter"):
            original(bare, ["/bin/sh", "-c", f"touch {marker}"], env={"PATH": os.environ["PATH"]})
    assert not marker.exists() and [a["kind"] for a in seen] == ["unsupported_launcher"]


def test_a_python_child_with_the_guard_directory_not_first_on_the_path_is_refused_by_the_backstop(tmp_path):
    original = _netguard._state["original_popen_init"]
    decoy = tmp_path / "decoy"
    decoy.mkdir()
    (decoy / "sitecustomize.py").write_text("pass\n", encoding="utf-8")
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": os.pathsep.join([str(decoy), str(_netguard.GUARD_SITE)])}
    bare = subprocess.Popen.__new__(subprocess.Popen)
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="without the guard"):
            original(bare, [sys.executable, "-c", "pass"], env=env)
    assert [a["kind"] for a in seen] == ["unguarded_python_child"]
    # and the wrapper itself normalises such an environment so the guard's sitecustomize always wins
    assert _netguard.guarded_env(env)["PYTHONPATH"].split(os.pathsep)[0] == str(_netguard.GUARD_SITE)


# ------------------------------------------------------- the audit sink belongs to the parent, and children must report

REDIRECTS = [("devnull", lambda tmp: "/dev/null"), ("other_file", lambda tmp: str(tmp / "elsewhere.log")),
             ("empty", lambda tmp: ""), ("relative", lambda tmp: "elsewhere.log")]


@pytest.mark.parametrize("name,target", REDIRECTS, ids=[r[0] for r in REDIRECTS])
@pytest.mark.parametrize("via", ["run", "popen"])
def test_a_child_cannot_redirect_the_audit_sink_by_its_environment(world, tmp_path, name, target, via):
    tmp, secret, sink = world
    log = tmp_path / "parent_controlled.log"
    env = {**os.environ, _netguard.LOG_ENV: target(tmp_path)}
    run_child(world, env=env, via=via, log=log)
    assert {"secret_file_read", "network"} <= {r["kind"] for r in log_rows(log)}, "the denied attempts were not collected"
    assert not (tmp_path / "elsewhere.log").exists() and not (tmp / "elsewhere.log").exists()


def test_a_child_that_rewrites_its_own_sink_variable_after_start_still_reports_to_the_parent(world, tmp_path):
    tmp, secret, sink = world
    log = tmp_path / "late.log"
    code = ("import os, socket\nos.environ['MQK_NETGUARD_LOG'] = '/dev/null'\n"
            "try:\n socket.create_connection(('127.0.0.1', 9), timeout=1)\nexcept Exception:\n pass\n")
    with probing(log):
        subprocess.run([sys.executable, "-c", code], capture_output=True, cwd=tmp)
    assert [r["kind"] for r in log_rows(log)] == ["network"]


def test_every_guarded_child_announces_itself_before_running_and_grandchildren_too(world, tmp_path):
    tmp, secret, sink = world
    log = tmp_path / "ready.log"
    grand = ("import subprocess, sys\n"
             "subprocess.run([sys.executable, '-c', 'pass'], env={})\n")
    with probing(log):
        subprocess.run([sys.executable, "-c", grand], cwd=tmp)
        subprocess.run([sys.executable, "-c", "pass"], cwd=tmp)
    rows = _netguard.sink_rows(log)
    launched = {r["launch"] for r in rows if r["kind"] == "child_launched" and r["guarded"]}
    ready = {r["proc"] for r in rows if r["kind"] == "guard_ready"}
    assert len(launched) == 3 and launched <= ready, (launched, ready)
    assert _netguard.audit(rows, _netguard.process_token())["uninitialized"] == []
    root = _netguard.sink_rows(_netguard.root_sink())
    assert all(r in root for r in rows), "a diagnostic copy holds a row the root sink does not"


def test_a_launched_child_that_never_announced_the_guard_is_detected():
    rows = [{"kind": "child_launched", "proc": "me", "launch": "L1", "guarded": True},
            {"kind": "child_launched", "proc": "me", "launch": "L2", "guarded": True},
            {"kind": "child_launched", "proc": "me", "launch": "L3", "guarded": False},   # a vouched non-Python stub
            {"kind": "child_launched", "proc": "stranger", "launch": "L4", "guarded": True},  # another session's launch
            {"kind": "guard_ready", "proc": "L1", "parent_proc": "me"}]
    assert _netguard.audit(rows, "me")["uninitialized"] == ["L2"]


def test_run_guarded_fails_when_a_child_it_launched_never_initialized_the_guard(tmp_path, monkeypatch):
    script = tmp_path / "s.py"
    script.write_text("pass\n", encoding="utf-8")
    log = tmp_path / "g.log"
    real_rows = _netguard.sink_rows
    monkeypatch.setattr(_netguard, "sink_rows", lambda p: [r for r in real_rows(p) if r["kind"] != "guard_ready"])  # the parent never sees it
    with pytest.raises(_netguard.NetworkDenied, match="never initialized the guard"):
        _netguard.run_guarded(script, [], env={"PATH": os.environ["PATH"]}, cwd=tmp_path, log=log)


def test_a_scope_naming_an_unusable_destination_is_refused_before_anything_launches(tmp_path):
    directory = tmp_path / "a_directory_is_not_a_log"
    directory.mkdir()
    marker = tmp_path / "ran"
    for bad in (directory, os.devnull, tmp_path / "nope" / "x.log"):
        with pytest.raises(_netguard.SinkInvalid):
            with _netguard.sink_to(bad):
                subprocess.run([sys.executable, "-c", f"open(r'{marker}', 'w')"])
    assert not marker.exists() and _netguard._state["diag"] == []


def test_a_launch_whose_required_destination_stopped_being_writable_is_refused(tmp_path):
    copy = tmp_path / "copy.log"
    marker = tmp_path / "ran"
    errors_before = _netguard._state["sink_errors"]
    with _netguard.expect_denied() as seen:
        with _netguard.sink_to(copy):
            with _netguard._sink_io():
                os.remove(copy)          # an outside actor (not this process's audited code) removes the file
            with pytest.raises(_netguard.NetworkDenied, match="audit sink is not writable"):
                subprocess.run([sys.executable, "-c", f"open(r'{marker}', 'w')"])
    assert not marker.exists() and [a["kind"] for a in seen] == ["audit_sink_unavailable"]
    assert _netguard._state["sink_errors"] == errors_before + 1, "the lost copy of the refusal row was not counted"
    _netguard._state["sink_errors"] = errors_before  # this test caused and verified the loss; the session must not inherit it


@pytest.mark.parametrize("break_it", ["root_is_a_directory", "root_is_devnull", "no_root", "diagnostic_is_a_directory"])
def test_a_child_that_cannot_write_its_ready_record_refuses_to_run(tmp_path, break_it):
    """Launched around the wrapper (so the parent-side pre-check is not what stops it): the child's own sitecustomize
    cannot report to the sink, so it exits before running any of its code."""
    original = _netguard._state["original_popen_init"]
    marker = tmp_path / "ran"
    directory = tmp_path / "dir_sink"
    directory.mkdir()
    good = Path(_netguard.root_sink())
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(_netguard.GUARD_SITE), _netguard.LOG_ENV: str(good)}
    if break_it == "root_is_a_directory":
        env[_netguard.LOG_ENV] = str(directory)
    elif break_it == "root_is_devnull":
        env[_netguard.LOG_ENV] = os.devnull
    elif break_it == "no_root":
        env.pop(_netguard.LOG_ENV)
    else:
        env[_netguard.DIAG_ENV] = str(directory)
    proc = subprocess.Popen.__new__(subprocess.Popen)
    _netguard._in_guarded_popen.depth = 1   # the test stands in for the wrapper: only the child's own check is under test
    try:
        original(proc, [sys.executable, "-c", f"open(r'{marker}', 'w')"], env=env, stderr=subprocess.PIPE)
    finally:
        _netguard._in_guarded_popen.depth = 0
    _, err = proc.communicate()
    assert proc.returncode == _netguard.CHILD_SINK_EXIT and b"refusing to run" in err and not marker.exists(), err
    assert _netguard.sink_rows(good) == [r for r in _netguard.sink_rows(good)]  # the root sink is still readable


@pytest.mark.parametrize("where", ["child_pythonpath_first", "cwd_with_dash_c", "script_directory", "pythonpath_relative_dot"])
def test_a_conflicting_sitecustomize_cannot_displace_the_guard(world, tmp_path, where):
    tmp, secret, sink = world
    decoy = tmp_path / "decoy"
    decoy.mkdir()
    flag = tmp_path / "decoy_ran"
    (decoy / "sitecustomize.py").write_text(f"open(r'{flag}', 'w').close()\n", encoding="utf-8")
    (decoy / "usercustomize.py").write_text(f"open(r'{flag}', 'w').close()\n", encoding="utf-8")
    log = tmp_path / f"{where}.log"
    env = {"PATH": os.environ["PATH"]}
    cwd, argv = tmp, [sys.executable, "-c", CHILD, str(secret), str(sink.port)]
    if where == "child_pythonpath_first":
        env["PYTHONPATH"] = os.pathsep.join([str(decoy), str(_netguard.GUARD_SITE)])
    elif where == "cwd_with_dash_c":
        cwd = decoy
    elif where == "script_directory":
        (decoy / "child.py").write_text(CHILD, encoding="utf-8")
        argv, cwd = [sys.executable, str(decoy / "child.py"), str(secret), str(sink.port)], tmp
    elif where == "pythonpath_relative_dot":
        env["PYTHONPATH"] = "."
        cwd = decoy
    with probing(log):
        proc = subprocess.run(argv, capture_output=True, text=True, env=env, cwd=cwd)
    assert result_of(proc)["read"].startswith("REFUSED:") and sink.accepted == 0
    assert not flag.exists(), "a decoy sitecustomize/usercustomize ran in the child"
    assert _netguard.audit(_netguard.sink_rows(log), _netguard.process_token())["uninitialized"] == []


def test_a_leaky_child_with_a_redirected_sink_still_fails_its_test_and_the_session_summary(tmp_path):
    """End to end through the real conftest: the leaky child supplies MQK_NETGUARD_LOG=/dev/null and swallows the
    error; the owning test still errors and the session summary counts the attempt."""
    test = tmp_path / "test_child.py"
    test.write_text(
        "import os, subprocess, sys\n"
        "def test_leaky_child():\n"
        "    env = dict(os.environ, MQK_NETGUARD_LOG='/dev/null')\n"
        "    subprocess.run([sys.executable, '-c', 'import socket\\ntry:\\n socket.create_connection((\"127.0.0.1\", 9), timeout=1)\\nexcept Exception:\\n pass'], env=env)\n"
        "def test_clean_child():\n"
        "    subprocess.run([sys.executable, '-c', 'print(1)'])\n", encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    with _netguard.expect_denied():
        proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                               "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "2 passed, 1 error" in out, out[-800:]
    assert "a spawned child attempted external network/secret access" in out
    got = json.loads(summary.read_text(encoding="utf-8"))
    assert got["unexpected_child_attempts"] == 1 and got["uninitialized_children"] == 0


def test_a_child_that_never_initialized_the_guard_fails_its_test_and_the_session_summary(tmp_path):
    test = tmp_path / "test_ghost.py"
    test.write_text(
        "import _netguard, os\n"
        "def test_ghost_child():\n"
        "    _netguard._write_all({'kind': 'child_launched', 'proc': _netguard.process_token(), 'launch': 'ghost-launch-0001',\n"
        "                          'child_pid': 2**22 + 1, 'guarded': True, 'expected': False, 'pid': os.getpid()})\n",
        encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                           "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    # the synthetic launch is also visible to THIS session (it is the nested session's parent): reconcile it
    _netguard._write_all({"kind": "guard_ready", "proc": "ghost-launch-0001", "parent_proc": None, "pid": 0, "ppid": 0})
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "never initialized the guard" in out, out[-800:]
    got = json.loads(summary.read_text(encoding="utf-8"))
    assert got["uninitialized_children"] == 1 and got["unexpected_attempts"] >= 1


# ------------------------------------------------ the audit sink cannot be rewritten by the code it is auditing

TAMPER = {
    "truncate": "open(os.environ['MQK_NETGUARD_LOG'], 'w').close()",
    "append_forged_ready": "open(os.environ['MQK_NETGUARD_LOG'], 'a').write('{\"kind\": \"guard_ready\", \"pid\": 1}\\n')",
    "os_open_trunc": "os.close(os.open(os.environ['MQK_NETGUARD_LOG'], os.O_WRONLY | os.O_TRUNC))",
    "remove": "os.remove(os.environ['MQK_NETGUARD_LOG'])",
    "rename": "os.rename(os.environ['MQK_NETGUARD_LOG'], os.environ['MQK_NETGUARD_LOG'] + '.moved')",
    "replace_over": "open(os.environ['MQK_NETGUARD_LOG'] + '.x', 'w').write(''); os.replace(os.environ['MQK_NETGUARD_LOG'] + '.x', os.environ['MQK_NETGUARD_LOG'])",
}


@pytest.mark.parametrize("name", sorted(TAMPER))
def test_a_child_cannot_erase_or_forge_the_audit_sink(world, tmp_path, name):
    tmp, secret, sink = world
    log = tmp_path / "tamper.log"
    code = ("import os, socket\ntry:\n socket.create_connection(('127.0.0.1', 9), timeout=1)\nexcept Exception:\n pass\n"
            f"try:\n {TAMPER[name]}\nexcept Exception as e:\n print('REFUSED', type(e).__name__)\n")
    with probing(log):
        proc = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=tmp)
    kinds = [r["kind"] for r in log_rows(log)]
    assert "REFUSED NetworkDenied" in proc.stdout, proc.stdout + proc.stderr[-300:]
    assert kinds[0] == "network" and "sink_tamper" in kinds, kinds          # the earlier evidence survived
    assert log.exists() and not Path(str(log) + ".moved").exists()


def test_the_parent_can_still_read_the_sink_and_the_guard_still_writes_it(tmp_path):
    log = tmp_path / "ok.log"
    with probing(log):
        subprocess.run([sys.executable, "-c", "pass"])
    assert open(log).read() and _netguard.sink_rows(log)
