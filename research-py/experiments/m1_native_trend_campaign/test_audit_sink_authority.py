"""The ROOT audit sink is the authority the session audits, and nothing a guarded process does can replace it,
divert rows from it, or make an unreadable sink look like zero attempts. Diagnostic scopes (`sink_to`) only
receive COPIES of what the root also receives. Synthetic `.env.local` files and loopback-free probes only: no
provider request, no real credential.

Threat model: this is a Python audit hook, not an OS sandbox. Ordinary and swallowed denied attempts, rebinding
calls and redirections are covered; arbitrary native code, raw syscalls or `ctypes` inside a hostile process are
outside it (see docs/research/M1_KISS_EXT032_REVIEW_CORRECTION_01.md)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import traceback
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
EXPERIMENTS = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(EXPERIMENTS))

import _netguard  # noqa: E402
import conftest as ct  # noqa: E402
from test_subprocess_guard_inheritance import plant_env_local  # noqa: E402

ROOT = Path(_netguard.root_sink())
SWALLOWED_READ = "try:\n open('.env' + '.local').read()\nexcept Exception as e:\n print('DENIED', type(e).__name__)\n"


def root_rows() -> list[dict]:
    return _netguard.sink_rows(ROOT)


def secret_rows_by_others(rows=None) -> list[dict]:
    return [r for r in (root_rows() if rows is None else rows)
            if r["kind"] == "secret_file_read" and r["proc"] != _netguard.process_token()]


def run_child(code: str, cwd: Path, **kw):
    return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=cwd, **kw)


# ------------------------------------------------------------------ the root cannot be rebound (owner process)

def test_the_root_is_fixed_and_a_second_install_naming_another_sink_raises_without_changing_it(tmp_path):
    before = (_netguard.root_sink(), dict(_netguard._state)["root_id"])
    other = tmp_path / "other.log"
    for target in (other, os.devnull, tmp_path):
        with pytest.raises(_netguard.SinkError):
            _netguard.install(sink=target)
    assert (_netguard.root_sink(), _netguard._state["root_id"]) == before and not other.exists()
    _netguard.install()                       # idempotent
    _netguard.install(sink=ROOT)              # naming the same file is a no-op
    link = tmp_path / "alias.log"
    link.symlink_to(ROOT)
    _netguard.install(sink=link)              # the same file through a symlink: same identity, no-op
    assert _netguard.root_sink() == before[0]
    plant_env_local(tmp_path)
    mark = len(root_rows())
    with _netguard.expect_denied():
        with pytest.raises(_netguard.NetworkDenied):
            open(tmp_path / ".env.local")
    rows = root_rows()[mark:]
    assert [r["kind"] for r in rows if r["proc"] == _netguard.process_token()] == ["secret_file_read"], rows
    assert not other.exists()


# --------------------------------------------- a guarded child cannot rebind, divert or hide its denials from the root

CHILD_REBIND = (
    "import os, sys, _netguard\n"
    "res = {}\n"
    "for name, target in (('other', sys.argv[1]), ('devnull', os.devnull)):\n"
    "    try:\n"
    "        _netguard.install(sink=target)\n"
    "        res[name] = 'ACCEPTED'\n"
    "    except Exception as e:\n"
    "        res[name] = type(e).__name__\n"
    "_netguard.install(); _netguard.install(sink=os.environ['MQK_NETGUARD_LOG'])\n"
    + SWALLOWED_READ + "print('RES', res)\n")


@pytest.mark.parametrize("via", ["install_after_ready", "sink_to_devnull_in_child", "sink_to_other_in_child", "env_redirect"])
def test_a_guarded_child_that_rebinds_or_diverts_still_leaves_its_denial_in_the_root(tmp_path, via):
    plant_env_local(tmp_path)
    other = tmp_path / "other.log"
    mark = len(root_rows())
    env = None
    if via == "install_after_ready":
        code, argv = CHILD_REBIND, []
        code = code.replace("sys.argv[1]", repr(str(other)))
    elif via == "sink_to_devnull_in_child":
        code = ("import os, _netguard\ntry:\n with _netguard.sink_to(os.devnull):\n  pass\n print('SINKTO accepted')\n"
                "except Exception as e:\n print('SINKTO', type(e).__name__)\n"
                "with _netguard.expect_denied() if False else __import__('contextlib').nullcontext():\n pass\n") + SWALLOWED_READ
    elif via == "sink_to_other_in_child":
        code = ("import _netguard\nwith _netguard.sink_to(%r):\n" % str(other)) + "".join(" " + l + "\n" for l in SWALLOWED_READ.splitlines())
    else:
        code, env = SWALLOWED_READ, {**os.environ, _netguard.LOG_ENV: os.devnull, _netguard.DIAG_ENV: os.devnull}
    with _netguard.expect_denied():
        proc = run_child(code, tmp_path, env=env)
    assert proc.returncode == 0 and "DENIED NetworkDenied" in proc.stdout, proc.stdout + proc.stderr[-400:]
    if via == "install_after_ready":
        assert "'other': 'SinkRebindError'" in proc.stdout and "'devnull': 'SinkRebindError'" in proc.stdout, proc.stdout
    if via == "sink_to_devnull_in_child":
        assert "SINKTO SinkInvalid" in proc.stdout
    got = secret_rows_by_others(root_rows()[mark:])
    assert len(got) == 1, f"the child's denied secret read is missing from the ROOT sink: {proc.stdout}"
    if via != "sink_to_other_in_child":
        assert not other.exists()
    else:
        assert [r["kind"] for r in _netguard.sink_rows(other) if r["kind"] == "secret_file_read"] == ["secret_file_read"]


# ------------------------------------------- scopes are copies; nested scopes and unwinding; ready/launch lineage

def test_nested_parent_scopes_copy_to_every_open_scope_and_the_root_and_unwind_cleanly(tmp_path):
    plant_env_local(tmp_path)
    b, c = tmp_path / "b.log", tmp_path / "c.log"
    mark = len(root_rows())
    with _netguard.expect_denied():
        with _netguard.sink_to(b):
            run_child(SWALLOWED_READ, tmp_path)                     # B only
            with _netguard.sink_to(c):
                run_child(SWALLOWED_READ, tmp_path)                 # B and C
            run_child(SWALLOWED_READ, tmp_path)                     # B again: C's scope has ended
        run_child(SWALLOWED_READ, tmp_path)                         # no scope
        with pytest.raises(RuntimeError):
            with _netguard.sink_to(b):
                raise RuntimeError("exceptional unwind")
        assert _netguard._state["diag"] == []
        run_child(SWALLOWED_READ, tmp_path)                         # after the failed scope: root only
    count = lambda rows: len(secret_rows_by_others(rows))           # noqa: E731
    assert count(root_rows()[mark:]) == 5
    assert count(_netguard.sink_rows(b)) == 3 and count(_netguard.sink_rows(c)) == 1
    root_after = root_rows()
    for copy in (b, c):                                             # a scope never holds a row the root lacks
        assert all(r in root_after for r in _netguard.sink_rows(copy))


def test_a_scope_cannot_be_a_device_directory_fifo_or_a_link_to_one(tmp_path):
    fifo = tmp_path / "fifo"
    os.mkfifo(fifo)
    (tmp_path / "dir").mkdir()
    link = tmp_path / "to_null.log"
    link.symlink_to(os.devnull)
    for bad in (os.devnull, tmp_path / "dir", fifo, link, "", tmp_path / "missing_dir" / "x.log"):
        with pytest.raises(_netguard.SinkInvalid):
            with _netguard.sink_to(bad):
                pytest.fail("a scope was opened on an invalid destination")
    assert _netguard._state["diag"] == []


def test_a_relative_scope_is_pinned_to_its_absolute_file_when_the_working_directory_changes(tmp_path, monkeypatch):
    plant_env_local(tmp_path)
    monkeypatch.chdir(tmp_path)
    with _netguard.expect_denied():
        with _netguard.sink_to("rel.log"):
            monkeypatch.chdir(tmp_path.parent)
            run_child(SWALLOWED_READ, tmp_path)
    assert len(secret_rows_by_others(_netguard.sink_rows(tmp_path / "rel.log"))) == 1


def test_a_sink_path_is_validated_against_the_file_identity_it_had_when_validated(tmp_path):
    log = tmp_path / "id.log"
    real, file_id = _netguard._validated(log, create=True)
    assert _netguard._append(real, file_id, {"kind": "x"})
    os.rename(log, tmp_path / "moved.log")
    log.write_text("", encoding="utf-8")                           # a different file now sits at the path
    assert not _netguard._append(real, file_id, {"kind": "x"}), "a row was written to a file that is not the validated sink"
    assert _netguard._append(real, _netguard._validated(log, create=False)[1], {"kind": "x"})


def test_ready_and_launch_lineage_is_recorded_in_the_root_and_in_each_open_scope_for_children_and_grandchildren(tmp_path):
    b = tmp_path / "lineage.log"
    grand = "import subprocess, sys\nsubprocess.run([sys.executable, '-c', 'pass'], env={})\n"
    with _netguard.sink_to(b):
        run_child(grand, tmp_path)
        run_child("pass", tmp_path)
    for rows in (_netguard.sink_rows(b), root_rows()):
        launched = {r["launch"] for r in rows if r["kind"] == "child_launched" and r["guarded"]}
        ready = {r["proc"]: r["parent_proc"] for r in rows if r["kind"] == "guard_ready"}
        assert len(launched & set(ready)) >= 3, (launched, ready)
        assert set(ready) >= launched
        # the grandchild's parent is the child, not this process: lineage is carried by tokens, not pids
        assert any(parent in ready for parent in ready.values())
        assert _netguard.audit(rows, _netguard.process_token())["uninitialized"] == []


# --------------------------------------- the session audit, with the actual conftest, in a real pytest subprocess

NESTED = '''
import json, os, subprocess, sys
from pathlib import Path
import _netguard

CASE = {case!r}
CODE = "try:\\n open('.env' + '.local').read()\\nexcept Exception as e:\\n print('DENIED', type(e).__name__)\\n"
if CASE == "launder":  # the child marks its own probe as deliberate
    CODE = "import _netguard\\nwith _netguard.expect_denied():\\n " + CODE.replace("\\n", "\\n ")


def run():
    r = subprocess.run([sys.executable, "-c", CODE], capture_output=True, text=True)
    assert r.returncode == 0 and "DENIED" in r.stdout, r.stdout + r.stderr


def test_unmarked_child_swallows_a_denied_secret_read():
    staged = Path("staged"); staged.write_text("ALPACA_API_KEY_PAPER=fake\\n"); staged.rename(".env.local")
    start = len(_netguard.sink_rows(_netguard.root_sink()))
    b, c = Path("b.log").resolve(), Path("c.log").resolve()
    if CASE == "single":
        with _netguard.sink_to(b):
            run()
    elif CASE == "nested":
        with _netguard.sink_to(b):
            with _netguard.sink_to(c):
                run()
    elif CASE == "launder":
        run()
    elif CASE == "unwind":
        try:
            with _netguard.sink_to(b):
                raise RuntimeError("unwind")
        except RuntimeError:
            pass
        assert _netguard._state["diag"] == []
        run()
    me = _netguard.process_token()
    mine = lambda rows: [r for r in rows if r["kind"] == "secret_file_read" and r["proc"] != me]
    out = {{"root": len(mine(_netguard.sink_rows(_netguard.root_sink())[start:])),
           "b": len(mine(_netguard.sink_rows(b))) if b.exists() else None,
           "c": len(mine(_netguard.sink_rows(c))) if c.exists() else None}}
    Path("results.json").write_text(json.dumps(out))
'''


@pytest.mark.parametrize("case,expect", [("single", {"b": 1, "c": None}), ("nested", {"b": 1, "c": 1}), ("unwind", {"b": 0, "c": None}),
                                         ("launder", {"b": None, "c": None})])
def test_a_child_swallowing_a_denial_under_a_scope_fails_the_unmarked_nested_session_and_reaches_the_root(tmp_path, case, expect):
    test = tmp_path / "test_nested.py"
    test.write_text(NESTED.format(case=case), encoding="utf-8")
    summary = tmp_path / "summary.json"
    env = {"PATH": os.environ["PATH"], "PYTHONPATH": str(EXPERIMENTS), "MQK_NETGUARD_SUMMARY": str(summary)}
    mark = len(root_rows())
    with _netguard.expect_denied():              # THIS test launches the leaky session on purpose
        proc = subprocess.run([sys.executable, "-m", "pytest", str(test), "-p", "conftest", "-p", "no:cacheprovider", "-q",
                               "--rootdir", str(tmp_path)], capture_output=True, text=True, env=env, cwd=tmp_path)
    out = proc.stdout + proc.stderr
    assert "1 passed, 1 error" in out and proc.returncode != 0, out[-1200:]       # the body passed; the AUDIT failed it
    assert "a spawned child attempted external network/secret access" in out
    got = json.loads((tmp_path / "results.json").read_text())
    assert got["root"] == 1 and got["b"] == expect["b"] and got["c"] == expect["c"], got
    summed = json.loads(summary.read_text())
    assert summed["unexpected_child_attempts"] == 1 and summed["uninitialized_children"] == 0, summed
    assert summed["sink_integrity_errors"] == 0 and summed["unexpected_attempts"] >= 1
    assert len(secret_rows_by_others(root_rows()[mark:])) == 1                   # and the outer root holds it too


# -------------------------------------------- an unreadable or malformed root is an error, never zero attempts

@pytest.mark.parametrize("content", [
    '{"kind": "network", "proc": "x"}\nthis is not json\n', '[1, 2]\n', '{"no_kind": 1}\n', '{"kind": "network"}',  # partial row
    '{"kind": 3}\n'])
def test_a_malformed_or_partial_sink_raises_instead_of_reporting_fewer_attempts(tmp_path, content):
    log = tmp_path / "x.log"
    log.write_text(content, encoding="utf-8")
    with pytest.raises(_netguard.SinkCorrupt):
        _netguard.sink_rows(log)
    with pytest.raises(_netguard.SinkCorrupt):
        _netguard.sink_rows(tmp_path / "absent.log")
    assert _netguard.sink_rows(_empty(tmp_path)) == []


def _empty(tmp_path):
    p = tmp_path / "empty.log"
    p.write_text("", encoding="utf-8")
    return p


def test_the_session_audit_and_summary_surface_a_corrupt_root(tmp_path, monkeypatch):
    log = tmp_path / "root.log"
    log.write_text('{"kind": "guard_ready", "proc": "a"}\nGARBAGE\n', encoding="utf-8")
    monkeypatch.setattr(ct, "_SESSION_CHILD_LOG", log)
    with pytest.raises(_netguard.SinkCorrupt):
        ct._rows()
    with pytest.raises(_netguard.SinkCorrupt):
        ct._audit()

    class Session:
        exitstatus = 0
    summary = tmp_path / "summary.json"
    monkeypatch.setenv(ct.SUMMARY_ENV, str(summary))
    session = Session()
    before = _netguard._state["sink_errors"]
    ct.pytest_sessionfinish(session, 0)
    got = json.loads(summary.read_text())
    assert got["sink_integrity_errors"] == 1 and got["unexpected_attempts"] >= 1 and session.exitstatus == 1
    assert _netguard._state["sink_errors"] == before


def test_an_orphan_attempt_in_the_owned_root_counts_but_foreign_lineage_in_an_inherited_root_does_not():
    rows = [{"kind": "guard_ready", "proc": "kid", "parent_proc": "me"},
            {"kind": "network", "proc": "kid", "expected": False},
            {"kind": "network", "proc": "stranger", "expected": False}]
    owner = _netguard.audit(rows, "me", owner=True)
    assert [r["proc"] for r in owner["unexpected"]] == ["kid"] and [r["proc"] for r in owner["orphans"]] == ["stranger"]
    assert _netguard.audit(rows, "me", owner=False)["orphans"] == []
    expected_edge = [{"kind": "child_launched", "proc": "me", "launch": "kid", "guarded": True, "expected": True},
                     {"kind": "guard_ready", "proc": "kid", "parent_proc": "me"},
                     {"kind": "network", "proc": "kid", "expected": False}]
    assert _netguard.audit(expected_edge, "me", owner=True)["unexpected"] == []
    mixed = expected_edge + [{"kind": "guard_ready", "proc": "kid", "parent_proc": "other"},
                             {"kind": "guard_ready", "proc": "other", "parent_proc": "me"}]
    assert [r["proc"] for r in _netguard.audit(mixed, "me", owner=True)["unexpected"]] == ["kid"]  # one unmarked path


# ------------------------------- no destination parameter exists that could bypass the root (direct-write seams)

def test_no_internal_writer_accepts_a_destination_other_than_the_validated_sinks():
    import inspect
    assert list(inspect.signature(_netguard._write_all).parameters) == ["row"]
    assert not hasattr(_netguard, "_write_sink") and not hasattr(_netguard, "current_sink")


# ---------------- wrapper and audit-hook backstop each refuse a forbidden launcher on their own (no incidental kills)

FORBIDDEN = [(["/bin/sh", "-c", "MARK"], "unsupported_launcher"), (["env", "python", "-c", "MARK"], "unsupported_launcher"),
             ([sys.executable, "-I", "-c", "MARK"], "guard_bypass_spawn")]


@pytest.mark.parametrize("argv,kind", FORBIDDEN)
def test_the_wrapper_refuses_before_cpython_launch_machinery_is_even_entered(tmp_path, monkeypatch, argv, kind):
    marker = tmp_path / "ran"
    argv = [a.replace("MARK", f"open({str(marker)!r}, 'w')" if "python" in argv[0] or argv[0] == sys.executable else f"touch {marker}")
            for a in argv]
    entered: list = []
    monkeypatch.setattr(subprocess.Popen, "_execute_child", lambda *a, **k: entered.append(a))
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied) as err:
            subprocess.Popen(argv, cwd=tmp_path)
    assert not entered and not marker.exists(), "the launch reached CPython's process machinery"
    assert [a["kind"] for a in seen] == [kind]
    assert any(f.name == "screen_spawn" for f in traceback.extract_tb(err.value.__traceback__)), "refused by something other than the wrapper's screen"


@pytest.mark.parametrize("argv,kind", FORBIDDEN)
def test_the_audit_hook_backstop_refuses_the_same_launchers_when_the_wrapper_is_bypassed(tmp_path, argv, kind):
    original = _netguard._state["original_popen_init"]
    marker = tmp_path / "ran"
    argv = [a.replace("MARK", f"open({str(marker)!r}, 'w')" if "python" in argv[0] or argv[0] == sys.executable else f"touch {marker}")
            for a in argv]
    bare = subprocess.Popen.__new__(subprocess.Popen)
    with _netguard.expect_denied() as seen:
        with pytest.raises(_netguard.NetworkDenied) as err:
            original(bare, argv, cwd=tmp_path, env=_netguard.guarded_env({"PATH": os.environ["PATH"]}))
    assert not marker.exists() and [a["kind"] for a in seen] == [kind]
    assert any(f.name == "_hook" for f in traceback.extract_tb(err.value.__traceback__)), "refused by something other than the audit hook"


def test_the_control_with_both_independent_defenses_disabled_the_forbidden_launcher_would_run(tmp_path, monkeypatch):
    """Proof the two tests above are sensitive: with the shared launch policy neutralised, the very same launch
    executes. (The shared policy function is what both defenses call; nothing else is changed.)"""
    marker = tmp_path / "ran"
    monkeypatch.setattr(_netguard, "launch_refusal", lambda *a, **k: None)
    subprocess.run(["/bin/sh", "-c", f"touch {marker}"], cwd=tmp_path)
    assert marker.exists(), "the control launch did not execute: the refusal tests above would prove nothing"


# ------------------------------------------------------------------ further seams found by the second sweep

def test_a_descendants_own_expected_flag_does_not_excuse_its_attempts():
    """Only a launch made inside the owner's `expect_denied` (an edge) can excuse a descendant; a writer's own
    `expected` claim on its rows is ignored."""
    rows = [{"kind": "child_launched", "proc": "me", "launch": "kid", "guarded": True, "expected": False},
            {"kind": "guard_ready", "proc": "kid", "parent_proc": "me"},
            {"kind": "secret_file_read", "proc": "kid", "expected": True}]
    assert [r["kind"] for r in _netguard.audit(rows, "me", owner=True)["unexpected"]] == ["secret_file_read"]
    rows[0]["expected"] = True
    assert _netguard.audit(rows, "me", owner=True)["unexpected"] == []


@pytest.mark.parametrize("name", [".env", ".env.local", ".env.production"])
def test_a_secret_named_file_cannot_be_an_audit_sink(tmp_path, name):
    with pytest.raises(_netguard.SinkInvalid):
        with _netguard.sink_to(tmp_path / name):
            pass
    assert not (tmp_path / name).exists()


@pytest.mark.parametrize("field", ["proc", "parent_proc", "launch"])
def test_lineage_fields_of_the_wrong_type_make_the_sink_corrupt(tmp_path, field):
    log = tmp_path / "t.log"
    log.write_text(json.dumps({"kind": "guard_ready", field: ["not", "a", "string"]}) + "\n", encoding="utf-8")
    with pytest.raises(_netguard.SinkCorrupt):
        _netguard.sink_rows(log)


def test_a_scope_naming_the_same_file_twice_or_the_root_writes_each_file_once(tmp_path):
    b = tmp_path / "dup.log"
    mark = len(root_rows())
    with _netguard.sink_to(b), _netguard.sink_to(b), _netguard.sink_to(ROOT):
        _netguard._write_all({"kind": "probe_row", "proc": _netguard.process_token()})
    assert [r["kind"] for r in _netguard.sink_rows(b)] == ["probe_row"]
    assert [r["kind"] for r in root_rows()[mark:]].count("probe_row") == 1


# ------------------------------------------------ fail-closed paths of the wrapper and of a child's own recording

def test_a_launch_that_cannot_be_recorded_is_refused_and_the_child_is_stopped(tmp_path, monkeypatch):
    marker = tmp_path / "survived"
    real = _netguard._write_all
    monkeypatch.setattr(_netguard, "_write_all", lambda row: False if row["kind"] == "child_launched" else real(row))
    code = f"import time, pathlib\ntime.sleep(1.5)\npathlib.Path({str(marker)!r}).write_text('x')\n"
    with pytest.raises(_netguard.NetworkDenied, match="could not be recorded"):
        subprocess.Popen([sys.executable, "-c", code], cwd=tmp_path)
    monkeypatch.undo()
    import time
    time.sleep(2.5)
    assert not marker.exists(), "an unrecorded child kept running"


def test_a_child_whose_own_evidence_can_no_longer_be_written_terminates_instead_of_continuing(tmp_path):
    plant_env_local(tmp_path)
    code = ("import _netguard\n_netguard._state['root_id'] = (0, 0)      # simulate the root having been replaced under it\n"
            + SWALLOWED_READ + "print('CONTINUED')\n")
    with _netguard.expect_denied(), _netguard.expect_evidence_loss():
        proc = run_child(code, tmp_path)
    assert proc.returncode == _netguard.CHILD_SINK_EXIT and "CONTINUED" not in proc.stdout, proc.stdout + proc.stderr
    assert "audit sink unwritable" in proc.stderr
