"""No test may reach a real provider. Three independent layers are exercised: the provider boundary
(hermetic flag), the credential-file boundary (.env.local), and the process-wide audit-hook network
guard, including with credentials present, the flag removed, the gate opened and a stage called directly.
All credentials below are fake literals; nothing is printed."""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import os
import socket
import sys
import urllib.request
from pathlib import Path

import pandas as pd
import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import _netguard  # noqa: E402
from mqk_research.data import alpaca_historical as ah  # noqa: E402
import stage_auth_testkit  # noqa: E402

DECL_NAME = "PREDECLARED_KISS_EXT032_ETF_01.json"
FAKE_CREDS = {"ALPACA_API_KEY_PAPER": "fake-key-0000", "ALPACA_API_SECRET_PAPER": "fake-secret-0000"}


def _load_runner(decl_path: Path):
    os.environ["MQK_M1_BATCH_DECLARATION"] = str(decl_path)
    try:
        spec = importlib.util.spec_from_file_location(f"rb_hermetic_{abs(hash(str(decl_path)))}", HERE / "run_batch.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    finally:
        del os.environ["MQK_M1_BATCH_DECLARATION"]


def _opened_declaration(tmp: Path) -> Path:
    """A mutated copy with the gate flipped open and a private run dir: the incident's trigger, offline."""
    decl = json.loads((HERE / DECL_NAME).read_text(encoding="utf-8"))
    decl["execution_gate"] = {"status": "MUTATED_OPEN", "executable": True, "blocker": None}
    decl["run_dir"] = str(tmp / "run")
    decl["experiment"]["registry_db_relative_path"] = str(tmp / "run" / "registry" / "research.sqlite3")
    path = tmp / "opened.json"
    path.write_text(json.dumps(decl), encoding="utf-8")
    return path


def _plant_env_local(directory: Path) -> Path:
    """Create a `.env.local` without opening it by that name (the guard refuses even writes)."""
    staged = directory / "staged_env"
    staged.write_text("ALPACA_API_KEY_PAPER=fake\nALPACA_API_SECRET_PAPER=fake\n", encoding="utf-8")
    target = directory / ".env.local"
    staged.rename(target)
    assert target.exists()
    return target


def test_the_session_is_hermetic_by_construction():
    assert os.environ.get("MQK_HERMETIC_NO_PROVIDER") == "1"
    assert not [k for k in os.environ if k.upper().startswith(("ALPACA", "APCA", "TIINGO", "TWELVEDATA"))]


@pytest.mark.parametrize("probe", [
    lambda: socket.create_connection(("203.0.113.1", 443), timeout=1),
    lambda: socket.getaddrinfo("data.alpaca.markets", 443),
    lambda: urllib.request.urlopen("https://data.alpaca.markets/v2/stocks/bars", timeout=1),
])
def test_the_network_guard_refuses_and_records_every_external_attempt(probe):
    with _netguard.expect_denied() as seen:
        with pytest.raises(Exception):
            probe()
    assert seen and all(a["kind"] == "network" for a in seen)


def test_a_secret_file_cannot_be_read_even_when_it_exists(tmp_path):
    secret = _plant_env_local(tmp_path)
    with _netguard.expect_denied() as seen:
        with pytest.raises(OSError):
            secret.read_text(encoding="utf-8")
    assert [a["kind"] for a in seen] == ["secret_file_read"]


def test_an_unexpected_attempt_is_counted_even_if_the_caller_swallows_it():
    before = len(_netguard.unexpected_attempts())
    try:
        socket.getaddrinfo("example.invalid", 80)
    except Exception:
        pass
    assert len(_netguard.unexpected_attempts()) == before + 1
    _netguard._state["attempts"][-1]["expected"] = True  # this probe is the test's own; keep the session at zero
    assert len(_netguard.unexpected_attempts()) == before


def test_the_provider_boundary_refuses_real_environment_credentials_and_the_default_transport(monkeypatch):
    for k, v in FAKE_CREDS.items():
        monkeypatch.setenv(k, v)
    with pytest.raises(ah.ProviderAccessDenied):
        ah.load_alpaca_credentials()
    assert ah.load_alpaca_credentials(env=dict(FAKE_CREDS)).api_key  # an explicit synthetic env stays usable offline
    with pytest.raises(ah.ProviderAccessDenied):
        ah._default_http_get("https://data.alpaca.markets/x", {}, {})


def test_opened_gate_plus_credentials_plus_env_local_plus_direct_stage_call_attempts_nothing(tmp_path, monkeypatch):
    rb = _load_runner(_opened_declaration(tmp_path))
    _plant_env_local(tmp_path)
    monkeypatch.setattr(rb, "REPO", tmp_path)
    for k, v in FAKE_CREDS.items():
        monkeypatch.setenv(k, v)
    rb.require_executable_declaration(rb.DECL)  # the CLI-level gate IS open in this mutation
    stage_auth_testkit.grant_runner_stages(monkeypatch, rb)  # even fully authorized, the provider boundary stands alone
    before = len(_netguard.attempts())
    with pytest.raises(ah.ProviderAccessDenied):
        rb.stage_fetch(argparse.Namespace(execute=True))  # imported module, direct call
    assert len(_netguard.attempts()) == before, "zero attempted external requests, not merely zero downloads"
    assert not (tmp_path / "run").exists()


def test_with_the_flag_removed_the_audit_guard_alone_stops_the_request_and_the_env_file_read(tmp_path, monkeypatch):
    """Layers are independent: even if the provider-boundary flag is stripped, no byte leaves the process."""
    monkeypatch.delenv("MQK_HERMETIC_NO_PROVIDER")
    for k, v in FAKE_CREDS.items():
        monkeypatch.setenv(k, v)
    creds = ah.load_alpaca_credentials()
    with _netguard.expect_denied() as seen:
        with pytest.raises(Exception):
            ah.fetch_historical_bars(symbols=["SPY"], start_utc=pd.Timestamp("2016-01-01", tz="UTC"),
                                     end_utc=pd.Timestamp("2016-02-01", tz="UTC"), asof="2026-10-09",
                                     timeframe="1Day", credentials=creds)
    assert seen and all(a["kind"] == "network" for a in seen)
    rb = _load_runner(_opened_declaration(tmp_path))
    _plant_env_local(tmp_path)
    monkeypatch.setattr(rb, "REPO", tmp_path)
    for k in FAKE_CREDS:
        monkeypatch.delenv(k)
    with _netguard.expect_denied() as seen:
        with pytest.raises(OSError):
            rb._load_alpaca_env()
    assert [a["kind"] for a in seen] == ["secret_file_read"]


def test_a_spawned_child_with_credentials_and_the_flag_removed_cannot_reach_the_network(tmp_path):
    """The audit guard is installed in the child too: with the provider-boundary flag stripped and credentials
    present, the default transport is refused at connect and nothing is written."""
    probe = tmp_path / "probe.py"
    probe.write_text(
        "import sys\nsys.path.insert(0, %r)\nimport pandas as pd\nfrom mqk_research.data import alpaca_historical as ah\n"
        "try:\n    ah.fetch_historical_bars(symbols=['SPY'], start_utc=pd.Timestamp('2016-01-01', tz='UTC'),\n"
        "        end_utc=pd.Timestamp('2016-02-01', tz='UTC'), asof='2026-10-09', timeframe='1Day',\n"
        "        credentials=ah.AlpacaCredentials('fake-key-0000', 'fake-secret-0000'))\nexcept Exception as exc:\n"
        "    print('refused', type(exc).__name__)\n    sys.exit(3)\nsys.exit(0)\n" % str(HERE.parents[1] / "src"),
        encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "MQK_HERMETIC_NO_PROVIDER"}
    proc, attempts = _netguard.run_guarded(probe, [], env=env, cwd=tmp_path, log=tmp_path / "guard.log")
    assert proc.returncode == 3, proc.stdout + proc.stderr
    assert attempts and all(a["kind"] == "network" for a in attempts)


def test_a_spawned_runner_stage_in_a_hermetic_child_attempts_nothing(tmp_path):
    decl = _opened_declaration(tmp_path)
    env = dict(os.environ)
    env.update(FAKE_CREDS)
    env.update(MQK_M1_BATCH_DECLARATION=str(decl), MQK_M1_CLI=str(tmp_path / "no-such-cli"), MQK_HERMETIC_NO_PROVIDER="1")
    proc, attempts = _netguard.run_guarded(HERE / "run_batch.py", ["fetch", "--execute"], env=env, cwd=tmp_path,
                                           log=tmp_path / "guard.log")
    assert proc.returncode != 0 and attempts == []
    assert not (tmp_path / "run").exists()
