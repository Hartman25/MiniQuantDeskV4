"""An ordinary spawned child inherits the offline guard. The audit hook lives in the parent; these tests prove
the child gets its own before it runs any code, whatever environment the caller hands it, and that a child cannot
be started in a way that skips it. Synthetic `.env.local` files and a loopback sink only: no provider request,
no real credential, no external connection."""

from __future__ import annotations

import json
import os
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
    env = dict(env)
    env[_netguard.LOG_ENV] = str(log)
    argv = [sys.executable, "-c", CHILD, str(secret), str(sink.port)]
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
    return [json.loads(l) for l in log.read_text(encoding="utf-8").splitlines()] if log.exists() else []


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
    proc = subprocess.run([sys.executable, str(probe)], capture_output=True, text=True, cwd=tmp_path,
                          env={"PATH": os.environ["PATH"], _netguard.LOG_ENV: str(log)})
    assert "RESULT REFUSED:NetworkDenied" in proc.stdout, proc.stdout + proc.stderr[-400:]
    assert any(r["kind"] == "network" for r in log_rows(log))


def test_a_grandchild_started_by_a_child_with_a_cleared_environment_is_guarded_too(world, tmp_path):
    tmp, secret, sink = world
    child = ("import subprocess, sys\n"
             "r = subprocess.run([sys.executable, '-c', sys.argv[3], sys.argv[1], sys.argv[2]], env={}, capture_output=True, text=True)\n"
             "print(r.stdout.strip() or r.stderr.strip()[-300:])\n")
    log = tmp_path / "grand.log"
    proc = subprocess.run([sys.executable, "-c", child, str(secret), str(sink.port), CHILD], capture_output=True, text=True,
                          env={"PATH": os.environ["PATH"], _netguard.LOG_ENV: str(log)}, cwd=tmp)
    got = eval([l for l in proc.stdout.splitlines() if l.startswith("RESULT ")][0][len("RESULT "):])
    assert got["read"].startswith("REFUSED:") and got["connect"].startswith("REFUSED:"), got
    assert sink.accepted == 0
    assert len({r["pid"] for r in log_rows(log)}) == 1 and any(r["kind"] == "secret_file_read" for r in log_rows(log))


def test_a_shell_launched_python_child_is_guarded(world, tmp_path):
    tmp, secret, sink = world
    log = tmp_path / "shell.log"
    cmd = f'"{sys.executable}" -c \'{CHILD}\' "{secret}" {sink.port}'
    proc = subprocess.run(cmd, shell=True, capture_output=True, text=True, env={"PATH": os.environ["PATH"], _netguard.LOG_ENV: str(log)}, cwd=tmp)
    got = result_of(proc)
    assert got["read"].startswith("REFUSED:") and got["connect"].startswith("REFUSED:") and sink.accepted == 0


@pytest.mark.parametrize("flags", [["-I"], ["-S"], ["-E"], ["-Ic"], ["-sI"], ["-Wignore", "-S"]])
def test_a_python_child_in_an_isolation_mode_that_would_skip_the_guard_is_refused_before_it_starts(tmp_path, flags):
    marker = tmp_path / "ran"
    code = f"open(r'{marker}', 'w').write('x')"
    argv = [sys.executable, *flags, "-c", code] if "c" not in flags[-1] else [sys.executable, *flags[:-1], "-Ic", code]
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="isolated mode"):
            subprocess.run(argv)
    assert not marker.exists() and [a["kind"] for a in seen] == ["guard_bypass_spawn"]


@pytest.mark.parametrize("argv,shell", [(["curl", "https://example.invalid"], False), (["wget", "-q", "x"], False),
                                         ("curl https://example.invalid", True), (["sh", "-c", "nc -z example.invalid 80"], False)])
def test_launching_a_network_client_is_refused(argv, shell):
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="network client"):
            subprocess.run(argv, shell=shell)
    assert [a["kind"] for a in seen] == ["network_tool_spawn"]


@pytest.mark.parametrize("argv,shell", [(["cat", "/tmp/x/.env.local"], False), (["sh", "-c", "cat ./.env.local"], False),
                                         ("grep KEY .env", True), (["head", ".env.production"], False)])
def test_a_non_python_child_is_never_handed_a_secret_file(argv, shell):
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied, match="secret file"):
            subprocess.run(argv, shell=shell)
    assert [a["kind"] for a in seen] == ["secret_file_spawn"]


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
    proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                           "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "2 passed, 1 error" in out, out[-800:]  # the teardown check errors the leaky test
    assert "a spawned child attempted external network/secret access" in out
