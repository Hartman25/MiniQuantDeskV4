"""Stage authorization: a mutable `executable` flag is not sufficient authority for any consequential stage,
and the check lives in every stage function, not only in the CLI dispatcher. Offline and synthetic: the
HMAC key is a test literal, no authorization is ever written to disk, no provider/registry/run directory is
touched."""

from __future__ import annotations

import argparse
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
