"""WAVE03-RUN-CONTROLLER-CHECKS-01 -- proves the offline/check-mode harness
actually works when invoked exactly as the future RUN mission will invoke
it: as a real `python run_wave.py <stage>` subprocess, in an environment
deliberately stripped of Paper/Live credentials, never touching the
network. Complements test_predeclaration.py's REQUIRED TEST 19/20 (which
prove the same invariants IN-PROCESS, via run_wave.main()) with genuine
process-boundary proof -- this is what actually catches a broken
`if __name__ == "__main__"` entry point, a cwd-dependent path assumption,
or a hidden reliance on an inherited environment variable that an
in-process pytest call would never surface.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

EXPERIMENT_ROOT = Path(__file__).resolve().parent
RUN_WAVE_PY = EXPERIMENT_ROOT / "run_wave.py"

# Deliberately minimal: no ALPACA_*, no MQK_*, nothing from a real
# .env.local this dev box might have loaded into its own shell -- only what
# the interpreter/OS itself needs to start up.
_MINIMAL_ENV_KEYS = ("PATH", "SYSTEMROOT", "SYSTEMDRIVE", "COMSPEC", "TEMP", "TMP", "PATHEXT")


def _minimal_credential_free_env() -> dict:
    env = {k: os.environ[k] for k in _MINIMAL_ENV_KEYS if k in os.environ}
    assert not any(k.startswith("ALPACA_") for k in env)
    return env


def _run(args: list[str], *, timeout: float = 60.0) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(RUN_WAVE_PY), *args],
        cwd=str(EXPERIMENT_ROOT),
        env=_minimal_credential_free_env(),
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def test_check_stage_succeeds_without_credentials_via_real_subprocess() -> None:
    result = _run(["check"])
    assert result.returncode == 0, result.stderr
    assert "PREDECLARATION_AGREEMENT=PASS" in result.stdout
    assert "SEED_UNIVERSE_COUNT=88" in result.stdout


def test_check_stage_never_reads_env_local_or_needs_alpaca_credentials() -> None:
    """The subprocess environment has no .env.local-derived credential and
    no inherited ALPACA_* variable at all -- check succeeding here is
    structural proof it never reached ensure_bars()/_load_paper_credentials_
    into_env(), which are the only functions in this file that ever read
    .env.local or contact Alpaca."""
    env = _minimal_credential_free_env()
    result = subprocess.run(
        [sys.executable, str(RUN_WAVE_PY), "check"],
        cwd=str(EXPERIMENT_ROOT), env=env, capture_output=True, text=True, timeout=60.0,
    )
    assert result.returncode == 0, result.stderr
    assert "ALPACA_API_KEY_PAPER" not in result.stdout
    assert "ALPACA_API_SECRET_PAPER" not in result.stdout


def test_execute_required_stages_refused_without_execute_via_real_subprocess() -> None:
    for stage in ("rank01", "rank02", "rank03", "judge"):
        result = _run([stage])
        assert result.returncode == 3, f"stage={stage!r} stdout={result.stdout!r} stderr={result.stderr!r}"
        assert "REFUSED" in result.stderr
        assert "--execute" in result.stderr


def test_unknown_stage_rejected_with_usage_exit_code() -> None:
    result = _run(["not-a-real-stage"])
    assert result.returncode == 2
    assert "unknown stage" in result.stderr


def test_no_argv_rejected_with_usage_exit_code() -> None:
    result = _run([])
    assert result.returncode == 2
    assert "usage:" in result.stderr
