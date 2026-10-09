"""Stage authorization: a mutable `executable` flag is not sufficient authority for any consequential stage,
and the check lives in every stage function, not only in the CLI dispatcher. Offline and synthetic: the
HMAC key is a test literal, no authorization is ever written to disk, no provider/registry/run directory is
touched."""

from __future__ import annotations

import argparse
import contextlib
import copy
import json
import os
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import _netguard  # noqa: E402
import holdout_incident as hi  # noqa: E402
import stage_authorization as sa  # noqa: E402
import stage_auth_testkit as kit  # noqa: E402
from test_hermetic_provider_isolation import _load_runner, _opened_declaration  # noqa: E402

KEY = "k" * 40
NOW = datetime(2026, 10, 10, 12, 0, tzinfo=timezone.utc)
KISS = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
ACK = ["HOA-KISS-EXT032-01"]
EFFECTFUL = [s for s, c in sa.STAGE_CLASS.items() if c != sa.READ_ONLY]


CA_ACK = ["CA_DISCOVERY_PROCESS_DATE_TO_EXTRACTION_TIME"]


def auth(classes, **kw):
    kw.setdefault("acknowledged_incidents", ACK)
    kw.setdefault("acknowledged_data_boundaries", CA_ACK)
    return sa.mint(KISS, list(classes), operator="op", approval_ref="APPROVAL-1", key=KEY, now=NOW, **kw)


def check(a, cls, decl=KISS, key=KEY, now=NOW, entries=None):
    sa.verify(decl, cls, a, key=key, now=now, incident_entries=entries)


def test_a_valid_authorization_authorizes_exactly_its_class_for_exactly_this_declaration():
    a = auth([sa.PROVIDER_FETCH])
    check(a, sa.PROVIDER_FETCH)
    for other in (sa.REGISTRATION, sa.ATTEMPT, sa.JUDGE_FINALIZE, sa.DATA_MATERIALIZATION):
        with pytest.raises(sa.AuthorizationError, match="does not authorize"):
            check(a, other)


@pytest.mark.parametrize("label,mutate,match", [
    ("no authorization", lambda a: None, "no valid stage authorization"),
    ("tampered class list", lambda a: {**a, "authorized_classes": sorted([*a["authorized_classes"], sa.ATTEMPT])}, "signature"),
    ("tampered expiry", lambda a: {**a, "expires_utc": (NOW + timedelta(days=30)).isoformat()}, "signature"),
    ("no signature", lambda a: {k: v for k, v in a.items() if k != "signature"}, "signature"),
    ("foreign schema", lambda a: {**a, "schema": "other"}, "no valid stage authorization"),
])
def test_missing_or_altered_authorizations_are_refused(label, mutate, match):
    with pytest.raises(sa.AuthorizationError, match=match):
        check(mutate(auth([sa.PROVIDER_FETCH])), sa.PROVIDER_FETCH)


def test_wrong_or_absent_secret_is_refused():
    a = auth([sa.PROVIDER_FETCH])
    for key in (None, "", "short", "j" * 40):
        with pytest.raises(sa.AuthorizationError):
            check(a, sa.PROVIDER_FETCH, key=key)


def test_a_wrong_declaration_identity_is_refused_but_a_gate_reissue_is_not_an_identity_change():
    a = auth([sa.PROVIDER_FETCH])
    reissued = copy.deepcopy(KISS)
    reissued["execution_gate"] = {"status": "AUTHORIZED", "executable": True, "blocker": None}
    check(a, sa.PROVIDER_FETCH, decl=reissued)
    for patch in (lambda d: d["capital_sizing"].update(allocation_fraction_bps=1001),
                  lambda d: d["universe"].update(symbols=["SPY", "QQQ", "IWM", "XLE"]),
                  lambda d: d["data"].update(end_utc="2026-09-02T00:00:00Z"),
                  lambda d: d["economic_protocol"]["cost_model"].update(commission_bps_per_side=1.0),
                  lambda d: d.update(campaign_id="OTHER")):
        mutated = copy.deepcopy(KISS)
        patch(mutated)
        with pytest.raises(sa.AuthorizationError, match="different declaration identity"):
            check(a, sa.PROVIDER_FETCH, decl=mutated)


def test_expired_stale_not_yet_valid_and_overlong_authorizations_are_refused():
    a = auth([sa.PROVIDER_FETCH], valid_for=timedelta(hours=1))
    check(a, sa.PROVIDER_FETCH, now=NOW + timedelta(minutes=59))
    for when in (NOW + timedelta(hours=1), NOW + timedelta(days=3), NOW - timedelta(seconds=1)):
        with pytest.raises(sa.AuthorizationError, match="expired|not yet valid"):
            check(a, sa.PROVIDER_FETCH, now=when)
    with pytest.raises(sa.AuthorizationError, match="at most 7 days"):
        auth([sa.PROVIDER_FETCH], valid_for=timedelta(days=8))
    forged = {**a, "expires_utc": (NOW + timedelta(days=30)).isoformat()}
    forged["signature"] = sa._signature(forged, KEY)  # even correctly signed, the window bound is enforced
    with pytest.raises(sa.AuthorizationError, match="invalid window"):
        check(forged, sa.PROVIDER_FETCH)


def test_an_authorization_without_an_operator_or_approval_reference_is_refused():
    for field in ("operator", "approval_ref"):
        a = auth([sa.PROVIDER_FETCH])
        a[field] = ""
        a["signature"] = sa._signature(a, KEY)
        with pytest.raises(sa.AuthorizationError, match="no operator approval"):
            check(a, sa.PROVIDER_FETCH)


def test_the_pending_holdout_incident_must_be_acknowledged_and_blocks_promotion_and_paper_outright(monkeypatch):
    with pytest.raises(sa.AuthorizationError, match="does not acknowledge"):
        check(auth([sa.PROVIDER_FETCH], acknowledged_incidents=[]), sa.PROVIDER_FETCH)
    for cls in (sa.PROMOTION, sa.PAPER):
        with pytest.raises(SystemExit, match="ACCESS_INCIDENT_PENDING_ADJUDICATION"):
            check(auth([cls]), cls)
    kit.use_incident_key(monkeypatch)
    check(auth([sa.PROMOTION], acknowledged_incidents=[]), sa.PROMOTION, entries=kit.adjudicated_entries())


def test_a_consumed_window_blocks_promotion_and_paper_and_still_needs_acknowledgement(monkeypatch):
    kit.use_incident_key(monkeypatch)
    consumed = kit.adjudicated_entries(hi.ADJUDICATED_CONSUMED)
    for cls in (sa.PROMOTION, sa.PAPER):
        with pytest.raises(SystemExit, match="ADJUDICATED_HOLDOUT_CONSUMED"):
            check(auth([cls]), cls, entries=consumed)
    with pytest.raises(sa.AuthorizationError, match="does not acknowledge"):
        check(auth([sa.PROVIDER_FETCH], acknowledged_incidents=[]), sa.PROVIDER_FETCH, entries=consumed)
    check(auth([sa.PROVIDER_FETCH]), sa.PROVIDER_FETCH, entries=consumed)  # development stages may proceed, acknowledged


def test_an_unauthenticated_preserved_adjudication_does_not_unblock_promotion(monkeypatch):
    signed = kit.adjudicated_entries()
    monkeypatch.delenv(hi.KEY_ENV, raising=False)  # no operator secret in this process
    with pytest.raises(SystemExit, match="ACCESS_INCIDENT_PENDING_ADJUDICATION"):
        check(auth([sa.PROMOTION]), sa.PROMOTION, entries=signed)


def test_the_fetch_authorization_must_acknowledge_the_declared_data_boundary():
    assert KISS["data"]["required_fetch_acknowledgements"] == CA_ACK
    with pytest.raises(sa.AuthorizationError, match="data boundaries"):
        check(auth([sa.PROVIDER_FETCH], acknowledged_data_boundaries=[]), sa.PROVIDER_FETCH)
    check(auth([sa.REGISTRATION], acknowledged_data_boundaries=[]), sa.REGISTRATION)  # only the fetch needs it


def test_read_only_stages_need_no_authorization_and_every_effectful_stage_has_a_distinct_class():
    for stage, cls in sa.STAGE_CLASS.items():
        if cls == sa.READ_ONLY:
            sa.require_stage(KISS, stage)
    assert {sa.STAGE_CLASS[s] for s in ("fetch", "register", "trials", "judge")} == {
        sa.PROVIDER_FETCH, sa.REGISTRATION, sa.ATTEMPT, sa.JUDGE_FINALIZE}
    assert set(sa.STAGE_CLASS) == {"check", "gate", "summary", "reuse_data", "fetch", "register", "trials", "backtest",
                                   "judge", "finalize", "review"}
    with pytest.raises(sa.AuthorizationError, match="not a known runner stage"):
        sa.require_stage(KISS, "promote")


def test_every_runner_stage_is_guarded_and_known_so_a_new_unguarded_stage_fails_here(tmp_path):
    rb = _load_runner(_opened_declaration(tmp_path))
    assert set(rb.STAGES) == set(sa.STAGE_CLASS), "a stage without an authorization class (or a class without a stage)"
    for name, fn in rb.STAGES.items():
        assert hasattr(fn, "__wrapped__") and fn.__wrapped__.__name__ == f"stage_{name}", f"{name} is not wrapped by @staged"


def test_frozen_historical_declarations_are_matched_by_content_and_keep_their_behaviour():
    for name, pinned in sa.HISTORICAL_DECLARATION_SHA256.items():
        decl = json.loads((HERE / name).read_text(encoding="utf-8"))
        assert sa._content_sha256(decl) == pinned, name
        for stage in sa.STAGE_CLASS:
            sa.require_stage(decl, stage)  # no authorization, no key: unchanged historical behaviour
    assert not sa.is_frozen_historical(KISS)
    mutated = json.loads((HERE / "PREDECLARED_BATCH_03.json").read_text(encoding="utf-8"))
    mutated["batch_id"] = "m1_batch_03_but_not_really"
    assert not sa.is_frozen_historical(mutated)
    with pytest.raises(sa.AuthorizationError):
        sa.require_stage(mutated, "fetch")


# ------------------------------------------------------------ direct calls into the real runner module

@pytest.fixture
def opened_runner(tmp_path, monkeypatch):
    """A runner loaded on a gate-OPEN mutant of the declaration with every side-effect seam booby-trapped."""
    rb = _load_runner(_opened_declaration(tmp_path))
    rb.require_executable_declaration(rb.DECL)  # the CLI-level gate is open: the flag alone is what we test
    monkeypatch.delenv(sa.KEY_ENV, raising=False)
    monkeypatch.delenv(sa.AUTH_FILE_ENV, raising=False)
    touched = []
    monkeypatch.setattr(rb, "_load_alpaca_env", lambda: touched.append("credentials"))
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: touched.append("subprocess"))
    from mqk_research.exp_distributed import storage
    monkeypatch.setattr(storage.ResearchResultStore, "__init__", lambda *a, **k: touched.append("registry"))
    rb._touched = touched
    return rb, tmp_path


@pytest.mark.parametrize("stage", EFFECTFUL)
def test_every_effectful_stage_called_directly_with_only_the_flag_open_is_refused_before_any_effect(stage, opened_runner):
    rb, tmp = opened_runner
    before = len(_netguard.attempts())
    with pytest.raises(sa.AuthorizationError, match="fail-closed"):
        getattr(rb, f"stage_{stage}")(argparse.Namespace(execute=True))
    assert rb._touched == [], f"{stage} touched {rb._touched} before authorization"
    assert not (tmp / "run").exists()
    assert len(_netguard.attempts()) == before


def test_effectful_helpers_cannot_be_called_outside_an_authorized_stage(opened_runner):
    rb, tmp = opened_runner
    with pytest.raises(sa.AuthorizationError, match="outside an authorized runner stage"):
        rb._run_cli("backtest", "native-fingerprint")
    with pytest.raises(sa.AuthorizationError, match="outside an authorized runner stage"):
        rb._save_index({})
    assert rb._touched == [] and not (tmp / "run").exists()


def test_the_closed_declaration_refuses_a_direct_fetch_before_credentials_http_directories_or_registry(tmp_path, monkeypatch):
    rb = _load_runner(HERE / "PREDECLARED_KISS_EXT032_ETF_01.json")
    touched = []
    monkeypatch.setattr(rb, "_load_alpaca_env", lambda: touched.append("credentials"))
    from mqk_research.exp_distributed import storage
    monkeypatch.setattr(storage.ResearchResultStore, "__init__", lambda *a, **k: touched.append("registry"))
    monkeypatch.setattr(Path, "mkdir", lambda *a, **k: touched.append("mkdir"))
    with pytest.raises(sa.AuthorizationError):
        rb.stage_fetch(argparse.Namespace(execute=True))
    assert touched == [] and not (HERE / rb.DECL["run_dir"]).exists()


def test_a_valid_authorization_for_the_wrong_class_does_not_open_registration_or_attempts(opened_runner, tmp_path, monkeypatch):
    rb, tmp = opened_runner
    fetch_only = sa.mint(rb.DECL, [sa.PROVIDER_FETCH], operator="op", approval_ref="A", key=KEY,
                         now=datetime.now(timezone.utc), acknowledged_incidents=ACK, acknowledged_data_boundaries=CA_ACK)
    path = tmp / "auth.json"
    path.write_text(json.dumps(fetch_only), encoding="utf-8")
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    monkeypatch.setenv(sa.AUTH_FILE_ENV, str(path))
    for stage in ("register", "trials", "backtest", "judge", "finalize", "review"):
        with pytest.raises(sa.AuthorizationError, match="does not authorize"):
            getattr(rb, f"stage_{stage}")(argparse.Namespace(execute=True))
    assert rb._touched == []


def test_a_valid_fetch_authorization_still_stops_at_the_provider_boundary_in_a_hermetic_process(opened_runner, monkeypatch):
    """Positive path: authorization passes, then the (independent) hermetic provider boundary refuses."""
    rb, tmp = opened_runner
    monkeypatch.undo()  # drop the booby traps; keep only the real boundary + audit guard below
    a = sa.mint(rb.DECL, [sa.PROVIDER_FETCH], operator="op", approval_ref="A", key=KEY, now=datetime.now(timezone.utc),
                acknowledged_incidents=ACK, acknowledged_data_boundaries=CA_ACK)
    path = tmp / "auth_ok.json"
    path.write_text(json.dumps(a), encoding="utf-8")
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    monkeypatch.setenv(sa.AUTH_FILE_ENV, str(path))
    monkeypatch.setenv("MQK_HERMETIC_NO_PROVIDER", "1")
    before = len(_netguard.attempts())
    from mqk_research.data.alpaca_historical import ProviderAccessDenied
    with pytest.raises(ProviderAccessDenied):
        rb.stage_fetch(argparse.Namespace(execute=True))
    assert len(_netguard.attempts()) == before
    assert not (tmp / "run" / "data").exists()


def test_the_cli_dispatcher_with_the_flag_open_and_credentials_present_is_refused_with_zero_attempts(tmp_path):
    decl = _opened_declaration(tmp_path)
    env = {k: v for k, v in os.environ.items() if k not in ("MQK_HERMETIC_NO_PROVIDER", sa.KEY_ENV, sa.AUTH_FILE_ENV)}
    env.update(ALPACA_API_KEY_PAPER="fake-key-0000", ALPACA_API_SECRET_PAPER="fake-secret-0000",
               MQK_M1_BATCH_DECLARATION=str(decl), MQK_M1_CLI=str(tmp_path / "no-such-cli"))
    for stage in ("fetch", "register", "trials"):
        proc, attempts = _netguard.run_guarded(HERE / "run_batch.py", [stage, "--execute"], env=env, cwd=tmp_path,
                                               log=tmp_path / f"{stage}.log")
        assert proc.returncode != 0 and "fail-closed" in proc.stderr + proc.stdout, stage
        assert attempts == [], f"{stage}: the authorization must refuse before credentials or HTTP"
        assert not (tmp_path / "run").exists()


# ------------------------------------------------------------------ F3: a "read-only" stage cannot run an unauthorized binary

STUB_BODY = """#!/bin/sh
echo executed >> "{marker}"
echo "semantic_fingerprint={fp}"
echo "required_history_bars=2"
echo "timeframe_secs=86400"
"""


_VOUCHED: list = []


@pytest.fixture(autouse=True)
def _vouched_stubs():
    """The offline guard refuses every non-Python child; each stub this test writes is vouched for explicitly
    (path and bytes), for this test only."""
    with contextlib.ExitStack() as stack:
        _VOUCHED.append(stack)
        yield
        _VOUCHED.pop()


def make_stub(path: Path, marker: Path, fp: str = "ab" * 32) -> Path:
    path.write_text(STUB_BODY.format(marker=marker, fp=fp), encoding="utf-8")
    path.chmod(0o755)
    _VOUCHED[-1].enter_context(_netguard.allow_executable(path))
    return path


@pytest.fixture
def stub_world(tmp_path, monkeypatch):
    """A gate-open graded runner whose MQK_M1_CLI is a harmless stub that records every execution."""
    rb = _load_runner(_opened_declaration(tmp_path))
    marker = tmp_path / "marker"
    stub = make_stub(tmp_path / "cli_a.sh", marker)
    monkeypatch.setattr(rb, "CLI", stub)
    monkeypatch.delenv(sa.KEY_ENV, raising=False)
    monkeypatch.delenv(sa.AUTH_FILE_ENV, raising=False)
    return rb, stub, marker, tmp_path


def grant(monkeypatch, rb, classes, **kw):
    a = sa.mint(rb.DECL, classes, operator="op", approval_ref="A", key=KEY, now=datetime.now(timezone.utc),
                acknowledged_incidents=ACK, **kw)
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    monkeypatch.setenv(sa.AUTH_FILE_ENV, "synthetic")
    monkeypatch.setattr(sa, "load_auth_file", lambda _p: a)
    return a


def pin(path: Path) -> str:
    import hashlib
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_check_without_authorization_validates_offline_and_never_executes_the_binary(stub_world, capsys):
    rb, stub, marker, _ = stub_world
    rb.stage_check(argparse.Namespace())
    out = capsys.readouterr().out
    assert "cli_present=True" in out and "cli_identity_check=SKIPPED_NOT_AUTHORIZED" in out
    assert not marker.exists(), "the stub binary was executed by a stage that needs no authorization"


def test_check_with_a_pinned_authorization_runs_the_identity_cross_check(stub_world, monkeypatch, capsys):
    rb, stub, marker, _ = stub_world
    grant(monkeypatch, rb, [sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub))
    rb.stage_check(argparse.Namespace())
    assert "cli_identity_check=PERFORMED" in capsys.readouterr().out
    assert len(marker.read_text().split()) == len(rb.TRIALS), "one native-fingerprint call per declared trial"


def test_an_authorization_without_a_binary_pin_or_with_the_wrong_class_runs_nothing(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    grant(monkeypatch, rb, [sa.NATIVE_IDENTITY_RESOLUTION])  # no cli_sha256
    with pytest.raises(sa.AuthorizationError, match="pins a native binary"):
        rb.stage_check(argparse.Namespace())
    grant(monkeypatch, rb, [sa.REGISTRATION], cli_sha256=pin(stub))  # pinned, but the wrong class for the identity check
    rb.stage_check(argparse.Namespace())  # falls back to declaration-only validation
    assert not marker.exists()


def test_an_altered_cli_path_or_a_swapped_binary_is_refused(stub_world, monkeypatch):
    rb, stub, marker, tmp = stub_world
    grant(monkeypatch, rb, [sa.NATIVE_IDENTITY_RESOLUTION, sa.REGISTRATION], cli_sha256=pin(stub))
    other_marker = tmp / "other_marker"
    other = make_stub(tmp / "cli_b.sh", other_marker, fp="cd" * 32)  # different bytes at a different path
    monkeypatch.setattr(rb, "CLI", other)
    with pytest.raises(sa.AuthorizationError, match="differs from the one the authorization pins"):
        rb.stage_check(argparse.Namespace())
    assert not other_marker.exists() and not marker.exists()
    monkeypatch.setattr(rb, "CLI", stub)
    stub.write_text(stub.read_text() + "\n# swapped after authorization\n", encoding="utf-8")  # same path, new bytes
    with pytest.raises(sa.AuthorizationError, match="differs from the one the authorization pins"):
        rb.stage_check(argparse.Namespace())
    assert not marker.exists()


@pytest.mark.parametrize("kind", ["directory", "missing"])
def test_a_non_regular_cli_is_refused_even_under_a_valid_pin(stub_world, monkeypatch, kind):
    rb, stub, marker, tmp = stub_world
    grant(monkeypatch, rb, [sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub))
    target = tmp / "not_a_file"
    if kind == "directory":
        target.mkdir()
    monkeypatch.setattr(rb, "CLI", target)
    with pytest.raises(sa.AuthorizationError, match="regular file"):
        rb._ACTIVE["stage"], rb._ACTIVE["auth"] = "check", sa.optional_authorization(rb.DECL, sa.NATIVE_IDENTITY_RESOLUTION)
        try:
            rb._run_cli("backtest", "native-fingerprint")
        finally:
            rb._ACTIVE["stage"], rb._ACTIVE["auth"] = None, None
    assert not marker.exists()


def test_gate_is_no_longer_a_free_read_only_stage_and_runs_no_binary_without_authorization(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    assert sa.STAGE_CLASS["gate"] == sa.NATIVE_IDENTITY_RESOLUTION
    with pytest.raises(sa.AuthorizationError, match="fail-closed"):
        rb.stage_gate(argparse.Namespace())
    grant(monkeypatch, rb, [sa.REGISTRATION], cli_sha256=pin(stub))  # wrong class
    with pytest.raises(sa.AuthorizationError, match="does not authorize"):
        rb.stage_gate(argparse.Namespace())
    assert not marker.exists()


def test_the_helper_cannot_execute_the_binary_directly_or_from_a_read_only_stage(stub_world):
    rb, stub, marker, _ = stub_world
    with pytest.raises(sa.AuthorizationError, match="outside an authorized runner stage"):
        rb._run_cli("backtest", "native-fingerprint")
    with pytest.raises(sa.AuthorizationError, match="read-only stage 'summary' may not execute"):
        rb.staged("summary")(lambda _a: rb._run_cli("backtest", "native-fingerprint"))(None)
    with pytest.raises(sa.AuthorizationError, match="no operator authorization secret"):
        rb.staged("check")(lambda _a: rb._run_cli("backtest", "native-fingerprint"))(None)
    assert not marker.exists()


def test_every_binary_executing_stage_runs_only_the_pinned_binary(stub_world, monkeypatch):
    rb, stub, marker, tmp = stub_world
    grant(monkeypatch, rb, [c for c in sa.AUTHORIZABLE if c not in sa.INCIDENT_BLOCKED], cli_sha256=pin(stub))
    for name in ("register", "trials", "backtest", "finalize", "review", "gate"):
        probe = rb.staged(name)(lambda _a: rb._run_cli("backtest", "native-fingerprint"))
        probe(None)  # authorized and pinned: the stub runs
    assert len(marker.read_text().split()) == 6
    monkeypatch.setattr(rb, "CLI", make_stub(tmp / "cli_c.sh", tmp / "m3", fp="ef" * 32))
    for name in ("register", "trials", "backtest", "finalize", "review", "gate"):
        with pytest.raises(sa.AuthorizationError, match="differs"):
            rb.staged(name)(lambda _a: rb._run_cli("backtest", "native-fingerprint"))(None)
    assert not (tmp / "m3").exists()


def test_frozen_historical_declarations_still_run_their_binary_as_before(tmp_path, monkeypatch):
    rb = _load_runner(HERE / "PREDECLARED_BATCH_03.json")
    marker = tmp_path / "marker"
    monkeypatch.setattr(rb, "CLI", make_stub(tmp_path / "cli.sh", marker))
    assert sa.is_frozen_historical(rb.DECL)
    rb.staged("check")(lambda _a: rb._run_cli("backtest", "native-fingerprint"))(None)
    assert marker.exists()


# ------------------------------------------- the executable helper re-verifies; mutable runner state confers nothing

def _inject(rb, stage, auth):
    rb._ACTIVE["stage"], rb._ACTIVE["auth"] = stage, auth


def _probe(rb):
    try:
        rb._run_cli("backtest", "native-fingerprint")
    finally:
        _inject(rb, None, None)


def _mint_for(rb, classes, *, now=None, valid_for=timedelta(hours=1), **kw):
    return sa.mint(rb.DECL, classes, operator="op", approval_ref="A", key=KEY, now=now or datetime.now(timezone.utc),
                   valid_for=valid_for, acknowledged_incidents=ACK, **kw)


@pytest.mark.parametrize("stage", ["check", "gate", "register", "trials", "backtest", "finalize", "review"])
def test_a_forged_unsigned_active_state_executes_nothing_even_with_the_right_pin(stub_world, monkeypatch, stage):
    rb, stub, marker, _ = stub_world
    for key_present in (False, True):  # with or without the operator secret in the environment
        if key_present:
            monkeypatch.setenv(sa.KEY_ENV, KEY)
        _inject(rb, stage, {"cli_sha256": pin(stub)})
        with pytest.raises(sa.AuthorizationError):
            _probe(rb)
        _inject(rb, stage, {"schema": sa.SCHEMA, "cli_sha256": pin(stub), "signature": "0" * 64,
                            "authorized_classes": list(sa.AUTHORIZABLE)})
        with pytest.raises(sa.AuthorizationError):
            _probe(rb)
    assert not marker.exists(), "a caller-populated _ACTIVE ran the binary"


@pytest.mark.parametrize("label,mutate,match", [
    ("pin replaced after signing", lambda a: {**a, "cli_sha256": "cd" * 32}, "signature"),
    ("class list widened after signing", lambda a: {**a, "authorized_classes": sorted(sa.AUTHORIZABLE)}, "signature"),
    ("expiry extended after signing", lambda a: {**a, "expires_utc": "2099-01-01T00:00:00+00:00"}, "signature"),
])
def test_a_tampered_signed_authorization_executes_nothing(stub_world, monkeypatch, label, mutate, match):
    rb, stub, marker, _ = stub_world
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    genuine = _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION, sa.REGISTRATION], cli_sha256=pin(stub))
    _inject(rb, "check", mutate(genuine))
    with pytest.raises(sa.AuthorizationError, match=match):
        _probe(rb)
    assert not marker.exists()


def test_a_genuine_authorization_of_the_wrong_class_executes_nothing_in_the_helper(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    wrong = _mint_for(rb, [sa.REGISTRATION, sa.DATA_MATERIALIZATION], cli_sha256=pin(stub))
    for stage in ("check", "gate", "trials", "judge"):  # none of these classes is the one the stage needs
        _inject(rb, stage, wrong)
        with pytest.raises(sa.AuthorizationError, match="does not authorize"):
            _probe(rb)
    native_only = _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub))  # right for check, wrong for these
    for stage in ("register", "trials", "backtest", "finalize", "review"):
        _inject(rb, stage, native_only)
        with pytest.raises(sa.AuthorizationError, match="does not authorize"):
            _probe(rb)
    _inject(rb, "summary", _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub)))
    with pytest.raises(sa.AuthorizationError, match="may not execute"):
        _probe(rb)
    assert not marker.exists()


def test_an_expired_or_not_yet_valid_authorization_executes_nothing_even_when_injected(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    now = datetime.now(timezone.utc)
    for issued in (now - timedelta(days=2), now + timedelta(days=1)):
        _inject(rb, "check", _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION], now=issued, cli_sha256=pin(stub)))
        with pytest.raises(sa.AuthorizationError, match="expired, not yet valid"):
            _probe(rb)
    assert not marker.exists()


def test_authorization_that_expires_between_stage_entry_and_the_call_is_rechecked_at_the_call(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    a = _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub), valid_for=timedelta(minutes=5))
    now = datetime.now(timezone.utc)
    assert sa.authorize_native_execution(rb.DECL, "check", a, stub, key=KEY, now=now) == stub.resolve()
    with pytest.raises(sa.AuthorizationError, match="expired"):
        sa.authorize_native_execution(rb.DECL, "check", a, stub, key=KEY, now=now + timedelta(minutes=6))


def test_an_authorization_for_another_declaration_executes_nothing(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    foreign = sa.mint(KISS, [sa.NATIVE_IDENTITY_RESOLUTION], operator="op", approval_ref="A", key=KEY,
                      now=datetime.now(timezone.utc), acknowledged_incidents=ACK, cli_sha256=pin(stub))
    assert sa.declaration_identity(KISS) != sa.declaration_identity(rb.DECL)
    _inject(rb, "check", foreign)
    with pytest.raises(sa.AuthorizationError, match="different declaration"):
        _probe(rb)
    assert not marker.exists()


def test_an_unknown_stage_name_or_no_stage_executes_nothing(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    good = _mint_for(rb, list(sa.AUTHORIZABLE), cli_sha256=pin(stub), acknowledged_data_boundaries=CA_ACK)
    for stage in ("not-a-stage", "paper", "promotion", ""):
        _inject(rb, stage, good)
        with pytest.raises(sa.AuthorizationError):
            _probe(rb)
    with pytest.raises(sa.AuthorizationError):
        sa.authorize_native_execution(rb.DECL, None, good, stub, key=KEY)
    assert not marker.exists()


def test_a_nested_stage_neither_inherits_nor_ends_the_callers_authority(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    grant(monkeypatch, rb, [sa.REGISTRATION, sa.NATIVE_IDENTITY_RESOLUTION], cli_sha256=pin(stub))

    def outer(_a):
        rb._run_cli("backtest", "native-fingerprint")            # authorized by the outer stage
        with pytest.raises(sa.AuthorizationError, match="may not execute"):
            rb.staged("summary")(lambda _b: rb._run_cli("backtest", "native-fingerprint"))(None)  # inner: read-only
        assert rb._ACTIVE["stage"] == "register"                  # the inner stage restored its caller
        rb._run_cli("backtest", "native-fingerprint")            # the outer authority survived the inner stage
    rb.staged("register")(outer)(None)
    assert len(marker.read_text().split()) == 2
    assert rb._ACTIVE == {"stage": None, "auth": None}


def test_a_failing_or_refused_stage_leaves_no_residual_authority(stub_world, monkeypatch):
    rb, stub, marker, _ = stub_world
    grant(monkeypatch, rb, [sa.REGISTRATION], cli_sha256=pin(stub))

    def boom(_a):
        raise RuntimeError("stage failed")
    with pytest.raises(RuntimeError):
        rb.staged("register")(boom)(None)
    assert rb._ACTIVE == {"stage": None, "auth": None}
    with pytest.raises(sa.AuthorizationError, match="outside an authorized runner stage"):
        rb._run_cli("backtest", "native-fingerprint")
    with pytest.raises(sa.AuthorizationError):                       # an unauthorized stage must not touch state either
        rb.staged("trials")(lambda _a: rb._run_cli("backtest", "native-fingerprint"))(None)
    assert rb._ACTIVE == {"stage": None, "auth": None} and not marker.exists()


def test_a_forged_active_state_cannot_write_the_trial_index_either(stub_world, monkeypatch):
    rb, stub, marker, tmp = stub_world
    monkeypatch.setattr(rb, "INDEX", tmp / "idx" / "trials_index.json")
    monkeypatch.setenv(sa.KEY_ENV, KEY)
    for stage, forged in (("register", None), ("register", {"authorized_classes": ["registry_registration"]}),
                          ("check", _mint_for(rb, [sa.NATIVE_IDENTITY_RESOLUTION])), ("summary", None)):
        _inject(rb, stage, forged)
        try:
            with pytest.raises(sa.AuthorizationError):
                rb._save_index({"x": 1})
        finally:
            _inject(rb, None, None)
    assert not (tmp / "idx").exists()
    _inject(rb, "register", _mint_for(rb, [sa.REGISTRATION]))      # a genuine, matching authorization still writes
    try:
        rb._save_index({"x": 1})
    finally:
        _inject(rb, None, None)
    assert (tmp / "idx" / "trials_index.json").exists()
