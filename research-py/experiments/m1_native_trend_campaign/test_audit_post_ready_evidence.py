"""A guarded child that was ready, then lost the ability to write its audit evidence, must never be reported as a
clean audit - even when its caller ignores the return code. Evidence comes from three independent channels: the
child's own empty failure marker (creating a file needs no bytes), the parent's finalization of the child's exit
(exit code 97 / death by SIGXFSZ / still running), and a recorded final state for every guarded launch.
Synthetic `.env.local` files only; no provider, no network, no real credential."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
EXPERIMENTS = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(EXPERIMENTS))

import _netguard  # noqa: E402
from test_subprocess_guard_inheritance import plant_env_local  # noqa: E402

ROOT = Path(_netguard.root_sink())
ME = _netguard.process_token

LOSE_THEN_PROBE = ("import resource, signal\n{ignore}"
                   "resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))\n"
                   "open('.env' + '.local').read()\n")
IGNORE_XFSZ = "signal.signal(signal.SIGXFSZ, signal.SIG_IGN)\n"
DEFAULT_XFSZ = "signal.signal(signal.SIGXFSZ, signal.SIG_DFL)\n"   # CPython ignores SIGXFSZ unless told otherwise


def lossy(ignore=True) -> list[str]:
    return [sys.executable, "-c", LOSE_THEN_PROBE.format(ignore=IGNORE_XFSZ if ignore else DEFAULT_XFSZ)]


def audit_since(mark: int, markers_before, integrity: int = 0) -> dict:
    return _netguard.audit(_netguard.sink_rows(ROOT), ME(), start=mark, owner=True,
                           markers=set(_netguard.failure_markers()) - set(markers_before),
                           known_failures={f["launch"] for f in _netguard._state["integrity"][integrity:]})


@pytest.fixture
def window():
    """(row index, marker baseline, tracked index, integrity index) at the start of the test."""
    state = (len(_netguard.sink_rows(ROOT)), frozenset(_netguard.failure_markers()), len(_netguard._state["tracked"]),
             len(_netguard._state["integrity"]))
    return state


# ---- every way a caller can (not) look at the child: the verdict never depends on the caller

def _run(argv, cwd):
    subprocess.run(argv, capture_output=True, cwd=cwd)                      # return code deliberately not checked


def _wait(argv, cwd):
    subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE).wait()


def _communicate(argv, cwd):
    subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE).communicate()


def _unobserved(argv, cwd):
    subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)   # never waited, never polled
    time.sleep(1.0)


CALLERS = {"run": _run, "wait": _wait, "communicate": _communicate, "unobserved": _unobserved}


@pytest.mark.parametrize("caller", sorted(CALLERS))
@pytest.mark.parametrize("ignore_sigxfsz", [True, False], ids=["exit97", "sigxfsz"])
def test_a_child_that_lost_its_sink_after_ready_is_never_a_clean_audit(tmp_path, window, caller, ignore_sigxfsz):
    mark, markers_before, tracked, integrity = window
    plant_env_local(tmp_path)
    with _netguard.expect_denied(), _netguard.expect_evidence_loss():
        CALLERS[caller](lossy(ignore_sigxfsz), tmp_path)
        failures = _netguard.finalize_children(tracked, 10.0)
    assert [f["reason"] for f in failures] == ["exit_sink_failure" if ignore_sigxfsz else "sigxfsz"], failures
    assert failures[0]["returncode"] == (_netguard.CHILD_SINK_EXIT if ignore_sigxfsz else -25)
    found = audit_since(mark, markers_before, integrity)
    # excused only by the explicit loss scope, and visible as such (non-vacuous); the probe row itself never arrived
    assert found["integrity"] == [] and found["expected_losses"] == [failures[0]["launch"]], found
    kinds = [r["kind"] for r in _netguard.sink_rows(ROOT)[mark:]]
    assert "guard_ready" in kinds and "secret_file_read" not in kinds, kinds        # ready, then evidence lost
    assert found["uninitialized"] == []                                          # so ready-tracking alone proved nothing


@pytest.mark.parametrize("caller", sorted(CALLERS))
def test_without_the_loss_scope_the_same_child_fails_the_audit_even_inside_expect_denied(tmp_path, window, caller):
    """`expect_denied` excuses a delivered denied probe; it is NOT authority to lose the evidence."""
    mark, markers_before, tracked, integrity = window
    plant_env_local(tmp_path)
    with _netguard.expect_denied():
        CALLERS[caller](lossy(), tmp_path)
        _netguard.finalize_children(tracked, 10.0)
    found = audit_since(mark, markers_before, integrity)
    assert len(found["integrity"]) == 1 and found["expected_losses"] == [], found
    # settle the books for the session: the failure above was the point of this test
    launch = found["integrity"][0]
    _netguard._state["integrity"][:] = [f for f in _netguard._state["integrity"] if f["launch"] != launch]
    _netguard._write_all({"kind": "child_launched", "proc": ME(), "launch": launch, "guarded": False, "loss_expected": True,
                          "child_pid": 0, "expected": False, "pid": os.getpid()})


def test_a_denied_probe_whose_evidence_arrived_is_deliberate_and_clean(tmp_path, window):
    mark, markers_before, tracked, integrity = window
    plant_env_local(tmp_path)
    with _netguard.expect_denied():
        subprocess.run([sys.executable, "-c", "try:\n open('.env' + '.local').read()\nexcept Exception: pass\n"], cwd=tmp_path)
        assert _netguard.finalize_children(tracked, 10.0) == []
    found = audit_since(mark, markers_before, integrity)
    assert found["integrity"] == [] and found["expected_losses"] == [] and found["unexpected"] == []
    assert [r["kind"] for r in _netguard.sink_rows(ROOT)[mark:] if r["kind"] == "secret_file_read"] == ["secret_file_read"]
    assert _netguard.finalize_children(tracked, 1.0) == []           # idempotent: nothing is counted twice


# ---- other ways the evidence can go missing after ready

def _child_code(prelude: str) -> list[str]:
    return [sys.executable, "-c", f"import _netguard, os\n{prelude}\nopen('.env' + '.local').read()\n"]


@pytest.mark.parametrize("name,prelude", [
    ("required_copy_removed", "with _netguard._sink_io():\n os.remove(os.environ['MQK_NETGUARD_DIAG'])"),
    ("root_replaced_under_it", "_netguard._state['root_id'] = (0, 0)"),
])
def test_losing_only_a_required_copy_or_the_root_after_ready_is_a_lost_evidence_failure(tmp_path, window, name, prelude):
    mark, markers_before, tracked, integrity = window
    plant_env_local(tmp_path)
    copy = tmp_path / "copy.log"
    with _netguard.expect_denied(), _netguard.expect_evidence_loss(), _netguard.sink_to(copy):
        subprocess.run(_child_code(prelude), capture_output=True, cwd=tmp_path)       # return code ignored
        failures = _netguard.finalize_children(tracked, 10.0)
    assert [f["returncode"] for f in failures] == [_netguard.CHILD_SINK_EXIT]
    found = audit_since(mark, markers_before, integrity)
    assert found["integrity"] == [] and len(found["expected_losses"]) == 1
    if name == "required_copy_removed":       # the root still took the attempt row first; the copy could not
        assert [r["kind"] for r in _netguard.sink_rows(ROOT)[mark:] if r["kind"] == "secret_file_read"] == ["secret_file_read"]
    markers = set(_netguard.failure_markers()) - set(markers_before)
    assert failures[0]["launch"] in markers, "the child left no marker: only the parent's finalization knew"


def test_parent_finalization_must_report_lost_required_copy_even_after_clean_child_exit(tmp_path, window):
    """child_exited in the root is not sufficient when that same write missed a required diagnostic sink."""
    mark, markers_before, tracked, integrity = window
    copy = tmp_path / "required_copy.log"
    with _netguard.expect_evidence_loss(), _netguard.sink_to(copy):
        child = subprocess.run([sys.executable, "-c", "pass"], capture_output=True, cwd=tmp_path)
        assert child.returncode == 0
        with _netguard._sink_io():  # simulate an external filesystem failure between launch and finalization
            copy.unlink()
        failures = _netguard.finalize_children(tracked, 5.0)
    assert [(f["reason"], f["returncode"]) for f in failures] == [("final_state_sink_loss", 0)]
    found = audit_since(mark, markers_before, integrity)
    assert found["integrity"] == [] and found["expected_losses"] == [failures[0]["launch"]], found
    kinds = [r["kind"] for r in _netguard.sink_rows(ROOT)[mark:]]
    assert kinds.count("child_exited") == 1 and "sink_failure" in kinds, kinds
    assert failures[0]["launch"] in _netguard.failure_markers()


def test_a_grandchild_that_lost_its_sink_is_found_although_the_child_exits_cleanly_and_ignores_it(tmp_path, window):
    mark, markers_before, tracked, integrity = window
    plant_env_local(tmp_path)
    child = ("import subprocess, sys\n"
             f"subprocess.run({lossy()!r}, capture_output=True)\n")             # the child ignores the grandchild's code
    with _netguard.expect_denied(), _netguard.expect_evidence_loss():
        r = subprocess.run([sys.executable, "-c", child], capture_output=True, cwd=tmp_path)
        assert r.returncode == 0
        failures = _netguard.finalize_children(tracked, 10.0)
    assert failures == []                                          # the direct child was fine; the loss is one level down
    found = audit_since(mark, markers_before, integrity)
    assert found["integrity"] == [] and len(found["expected_losses"]) == 1, found
    rows = _netguard.sink_rows(ROOT)[mark:]
    assert any(r["kind"] == "sink_failure" or r["kind"] == "child_exited" and r.get("returncode") == 97 for r in rows)


def test_a_child_still_running_when_the_bound_ends_is_stopped_and_counted_not_waited_for(tmp_path, window):
    mark, markers_before, tracked, integrity = window
    started = time.monotonic()
    with _netguard.expect_evidence_loss():
        p = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], cwd=tmp_path)
        failures = _netguard.finalize_children(tracked, 1.0)
    assert time.monotonic() - started < 15 and [f["reason"] for f in failures] == ["outstanding"]
    assert p.poll() is not None, "the outstanding child was left running"
    found = audit_since(mark, markers_before, integrity)
    assert found["integrity"] == [] and len(found["expected_losses"]) == 1


def test_a_launch_with_no_recorded_final_state_is_lost_evidence(window):
    rows = [{"kind": "child_launched", "proc": "me", "launch": "L1", "guarded": True},
            {"kind": "guard_ready", "proc": "L1", "parent_proc": "me"},
            {"kind": "child_launched", "proc": "me", "launch": "L2", "guarded": True},
            {"kind": "guard_ready", "proc": "L2", "parent_proc": "me"},
            {"kind": "child_exited", "proc": "me", "launch": "L2", "returncode": 0}]
    assert _netguard.audit(rows, "me")["integrity"] == ["L1"]
    rows[0]["loss_expected"] = True
    found = _netguard.audit(rows, "me")
    assert found["integrity"] == [] and found["expected_losses"] == ["L1"]
    # a failure reported by the child's own row/marker is counted once however many channels repeat it
    twice = rows[2:] + [{"kind": "sink_failure", "proc": "L2", "subject": "L2"}, {"kind": "sink_failure", "proc": "me", "subject": "L2"}]
    assert _netguard.audit(twice + [{"kind": "guard_ready", "proc": "L2", "parent_proc": "me"}], "me", markers=["L2"],
                           known_failures=["L2"])["integrity"] == ["L2"]
    # a reported failure of a process outside this lineage is the owner's problem, not an inherited session's
    stranger = [{"kind": "sink_failure", "proc": "X", "subject": "X"}]
    assert _netguard.audit(stranger, "me", owner=True)["integrity"] == ["X"]
    assert _netguard.audit(stranger, "me", owner=False)["integrity"] == []


# ---- the session verdict, with the actual conftest, in a real pytest subprocess

NESTED = '''
import json, os, signal, subprocess, sys, time
from pathlib import Path
import _netguard
import conftest
conftest.READY_GRACE_SECONDS = 1.0
CASE = {case!r}
LOSE = ("import resource, signal\\nsignal.signal(signal.SIGXFSZ, signal.SIG_IGN)\\n"
        "resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))\\nopen('.env' + '.local').read()\\n")
PROBE = "try:\\n open('.env' + '.local').read()\\nexcept Exception: pass\\n"


if CASE == "module_level_benign":
    subprocess.Popen([sys.executable, "-c", "pass"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
if CASE == "module_level_child":
    subprocess.Popen([sys.executable, "-c", LOSE], cwd=os.getcwd(),
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def test_body():
    staged = Path("staged"); staged.write_text("ALPACA_API_KEY_PAPER=fake\\n"); staged.rename(".env.local")
    start = len(_netguard.sink_rows(_netguard.root_sink()))
    if CASE == "lost_run":
        subprocess.run([sys.executable, "-c", LOSE], capture_output=True)       # return code deliberately ignored
    elif CASE == "lost_inside_expect_denied":
        with _netguard.expect_denied():
            subprocess.run([sys.executable, "-c", LOSE], capture_output=True)
    elif CASE == "delivered_probe_in_expect_denied":
        with _netguard.expect_denied():
            subprocess.run([sys.executable, "-c", PROBE], capture_output=True)
    elif CASE == "abandoned_child":
        subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    rows = _netguard.sink_rows(_netguard.root_sink())[start:]
    Path("results.json").write_text(json.dumps([r["kind"] for r in rows]))
'''


def run_nested(tmp_path, case, *, loss_scope=True):
    test = tmp_path / "test_nested.py"
    test.write_text(NESTED.format(case=case), encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    mark = len(_netguard.sink_rows(ROOT))
    scopes = [_netguard.expect_denied()] + ([_netguard.expect_evidence_loss()] if loss_scope else [])
    for scope in scopes:
        scope.__enter__()
    try:
        proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                               "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    finally:
        for scope in reversed(scopes):
            scope.__exit__(None, None, None)
    return proc, json.loads(summary.read_text()), mark


@pytest.mark.parametrize("case", ["lost_run", "lost_inside_expect_denied"])
def test_the_session_verdict_fails_when_a_ready_child_lost_its_sink_and_the_caller_ignored_it(tmp_path, case):
    plant_env_local(tmp_path)
    proc, summary, mark = run_nested(tmp_path, case)
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "1 passed, 1 error" in out and "lost audit evidence" in out, out[-1500:]   # body passed; AUDIT failed
    assert summary["sink_integrity_errors"] == 1 and summary["unexpected_attempts"] >= 1, summary
    assert summary["uninitialized_children"] == 0 and summary["unexpected_child_attempts"] == 0, summary
    kinds = json.loads((tmp_path / "results.json").read_text())
    assert "guard_ready" in kinds and "secret_file_read" not in kinds       # ready, then silent: only the new channels saw it
    assert set(_netguard.failure_markers()) , "no failure marker was left in the shared marker directory"


def test_an_abandoned_running_child_fails_the_nested_session_and_is_not_left_behind(tmp_path):
    proc, summary, mark = run_nested(tmp_path, "abandoned_child")
    out = proc.stdout + proc.stderr
    assert proc.returncode != 0 and "lost audit evidence or had no recorded outcome" in out, out[-1500:]
    assert summary["sink_integrity_errors"] == 1, summary
    launched = [r for r in _netguard.sink_rows(ROOT)[mark:] if r["kind"] == "child_launched" and r["guarded"]]
    for r in launched:
        with pytest.raises(ProcessLookupError):
            os.kill(r["child_pid"], 0)          # nothing from the nested session still runs


def test_a_delivered_probe_under_expect_denied_leaves_the_nested_session_clean(tmp_path):
    plant_env_local(tmp_path)
    proc, summary, mark = run_nested(tmp_path, "delivered_probe_in_expect_denied", loss_scope=False)
    out = proc.stdout + proc.stderr
    # the NESTED session did not mark the launch deliberate itself? it did (expect_denied inside): so it is clean
    assert proc.returncode == 0 and "1 passed" in out, out[-1500:]
    assert summary["sink_integrity_errors"] == 0 and summary["unexpected_child_attempts"] == 0, summary
    assert summary["expected_evidence_losses"] == 0


# ---- adjacent seams found by the second sweep

def test_a_forked_copy_of_the_process_never_finalizes_its_parents_children(monkeypatch):
    entry = {"launch": "x" * 32, "popen": object(), "finalized": False, "loss_expected": False, "owner_pid": os.getpid() + 1}
    monkeypatch.setitem(_netguard._state, "tracked", [*_netguard._state["tracked"], entry])
    assert _netguard.finalize_children(len(_netguard._state["tracked"]) - 1, 0.1) == [] and entry["finalized"] is False


def test_a_child_cannot_delete_or_replace_the_failure_markers(tmp_path):
    marker = Path(_netguard._marker_dir()) / "zz-test-marker.x"
    with _netguard._sink_io():
        marker.touch()
    try:
        code = (f"import os, _netguard\nfor f in (lambda: os.remove({str(marker)!r}), lambda: os.rename({str(marker)!r}, {str(marker)!r} + '.m'),\n"
                f"          lambda: open({str(marker)!r}, 'w')):\n try:\n  f()\n  print('ALLOWED')\n except Exception as e:\n  print('REFUSED', type(e).__name__)\n")
        with _netguard.expect_denied():
            proc = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=tmp_path)
        assert proc.stdout.split() == ["REFUSED", "NetworkDenied"] * 3, proc.stdout + proc.stderr
        assert marker.exists()
    finally:
        with _netguard._sink_io():
            marker.unlink(missing_ok=True)
    # the child's three refused tamper attempts are denied attempts that arrived; nothing was lost
    assert _netguard.finalize_children(len(_netguard._state["tracked"]) - 1, 5.0) == []


def test_the_marker_directory_itself_cannot_be_renamed_by_audited_code(tmp_path):
    """Protect the failure-marker DIRECTORY as well as the files inside; check without touching the directory."""
    marker_dir = _netguard._marker_dir()
    assert marker_dir is not None
    with _netguard.expect_denied(), pytest.raises(_netguard.NetworkDenied, match="audit sink"):
        _netguard._hook("os.rename", (marker_dir, str(tmp_path / "relocated-markers")))
    assert Path(marker_dir).is_dir()


def test_a_child_launched_outside_any_test_is_judged_at_session_finish(tmp_path):
    """No per-test fixture owns a child started at collection time; the session finalization must."""
    plant_env_local(tmp_path)
    test = tmp_path / "test_nested.py"
    test.write_text(NESTED.format(case="module_level_child"), encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    with _netguard.expect_denied(), _netguard.expect_evidence_loss():
        proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                               "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    got = json.loads(summary.read_text())
    assert "1 passed" in proc.stdout and proc.returncode != 0, proc.stdout + proc.stderr    # tests passed; the session did not
    assert got["sink_integrity_errors"] == 1 and got["unexpected_attempts"] >= 1, got


def test_run_guarded_raises_when_its_child_lost_its_evidence_and_not_when_the_loss_is_scoped(tmp_path):
    plant_env_local(tmp_path)
    script = tmp_path / "lose.py"
    script.write_text(LOSE_THEN_PROBE.format(ignore=IGNORE_XFSZ), encoding="utf-8")
    mark = len(_netguard.sink_rows(ROOT))
    with _netguard.expect_denied(), _netguard.expect_evidence_loss():
        _netguard.run_guarded(script, [], env={"PATH": os.environ["PATH"]}, cwd=tmp_path, log=tmp_path / "scoped.log")
    with _netguard.expect_denied():
        before = len(_netguard._state["integrity"])
        with pytest.raises(_netguard.NetworkDenied, match="lost their audit evidence"):
            _netguard.run_guarded(script, [], env={"PATH": os.environ["PATH"]}, cwd=tmp_path, log=tmp_path / "unscoped.log")
    # settle the books: the unscoped failure above was the assertion; excuse that launch for the session audit
    for f in _netguard._state["integrity"][before:]:
        _netguard._write_all({"kind": "child_launched", "proc": ME(), "launch": f["launch"], "guarded": False,
                              "loss_expected": True, "child_pid": 0, "expected": False, "pid": os.getpid()})
    del _netguard._state["integrity"][before:]


def test_a_failure_marker_alone_is_enough_to_fail_a_launch_whose_rows_were_lost():
    rows = [{"kind": "child_launched", "proc": "me", "launch": "L", "guarded": True},
            {"kind": "guard_ready", "proc": "L", "parent_proc": "me"},
            {"kind": "child_exited", "proc": "me", "launch": "L", "returncode": 0}]
    assert _netguard.audit(rows, "me")["integrity"] == []
    assert _netguard.audit(rows, "me", markers=["L"])["integrity"] == ["L"]


def test_a_benign_child_launched_outside_any_test_is_finalized_cleanly_at_session_finish(tmp_path):
    """Without the session-finish finalization the same child would have no recorded outcome."""
    test = tmp_path / "test_nested.py"
    test.write_text(NESTED.format(case="module_level_benign"), encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                           "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    got = json.loads(summary.read_text())
    assert proc.returncode == 0 and "1 passed" in proc.stdout, proc.stdout + proc.stderr
    assert got["sink_integrity_errors"] == 0 and got["unexpected_attempts"] == 0, got


def test_the_childs_own_marker_exists_even_when_the_parent_never_tracked_it(tmp_path):
    """A child launched around the wrapper (so no parent finalization exists) still reports its own post-ready loss."""
    original = _netguard._state["original_popen_init"]
    plant_env_local(tmp_path)
    import uuid
    launch = uuid.uuid4().hex
    env = _netguard.guarded_env({"PATH": os.environ["PATH"]}, launch)
    _netguard._write_all({"kind": "child_launched", "proc": ME(), "launch": launch, "child_pid": 0, "guarded": True,
                          "expected": False, "loss_expected": True, "pid": os.getpid()})
    proc = subprocess.Popen.__new__(subprocess.Popen)
    _netguard._in_guarded_popen.depth = 1
    try:
        original(proc, lossy(), env=env, cwd=tmp_path, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    finally:
        _netguard._in_guarded_popen.depth = 0
    proc.communicate()
    _netguard._write_all({"kind": "child_exited", "proc": ME(), "launch": launch, "returncode": proc.returncode, "pid": os.getpid()})
    assert proc.returncode == _netguard.CHILD_SINK_EXIT
    assert os.path.exists(os.path.join(_netguard._marker_dir(), f"{launch}.post_ready_sink_loss")), "the child left no marker of its own"


def test_a_popen_that_never_started_a_process_is_not_judged_and_never_crashes_finalization():
    ghost = subprocess.Popen.__new__(subprocess.Popen)          # constructed, never executed: pid is None
    entry = {"launch": "g" * 32, "popen": ghost, "finalized": False, "loss_expected": False, "owner_pid": os.getpid()}
    _netguard._state["tracked"].append(entry)
    assert _netguard.finalize_children(len(_netguard._state["tracked"]) - 1, 0.5) == [] and entry["finalized"] is True


def test_session_summary_counts_sink_errors_discovered_during_child_finalization(monkeypatch, tmp_path):
    """Session closeout must not snapshot sink_errors before finalizing the last children."""
    from types import SimpleNamespace
    import conftest as guard_conftest
    before = _netguard._state["sink_errors"]

    def finalization_loses_evidence(since, timeout):
        _netguard._state["sink_errors"] += 1
        return []

    empty = {"unexpected": [], "orphans": [], "uninitialized": [], "integrity": [], "expected_losses": []}
    monkeypatch.setattr(_netguard, "finalize_children", finalization_loses_evidence)
    monkeypatch.setattr(guard_conftest, "_audit", lambda **kw: empty)
    summary_path = tmp_path / "summary.json"
    monkeypatch.setenv(guard_conftest.SUMMARY_ENV, str(summary_path))
    session = SimpleNamespace(exitstatus=0)
    try:
        guard_conftest.pytest_sessionfinish(session, 0)
        assert session.exitstatus != 0
        assert json.loads(summary_path.read_text())["sink_integrity_errors"] == 1
    finally:
        _netguard._state["sink_errors"] = before


def test_losing_both_the_row_and_the_marker_is_counted_by_the_reporting_process(monkeypatch):
    before = _netguard._state["sink_errors"]
    monkeypatch.setattr(_netguard, "_write_all", lambda row: False)
    monkeypatch.setattr(_netguard, "_marker_dir", lambda: None)
    _netguard._report_failure("t" * 32, "post_ready_sink_loss")
    assert _netguard._state["sink_errors"] == before + 1
    _netguard._state["sink_errors"] = before                      # this test caused and verified the loss


def test_a_parent_whose_own_evidence_write_fails_counts_a_sink_error(tmp_path, monkeypatch):
    before = _netguard._state["sink_errors"]
    monkeypatch.setattr(_netguard, "_write_all", lambda row: False)
    with _netguard.expect_denied():
        with pytest.raises(_netguard.NetworkDenied):
            open(tmp_path / ".env.local")
    assert _netguard._state["sink_errors"] == before + 1
    _netguard._state["sink_errors"] = before
