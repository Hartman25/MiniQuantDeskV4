"""RESEARCH-MULTIPLE-TESTING-JUDGE-01 / P9 BKT-ROBUSTNESS-GAUNTLET-01 --
DSR/PBO sensitivity CLI tests.

Direct-fixture, in-process tests (mirrors test_multiple_testing_judge.py's
own "no subprocess" convention for unit-level coverage) plus one genuine
subprocess-level test proving the actual `python -m
mqk_research.ml.dsr_pbo_sensitivity_cli` invocation Rust will use actually
works end-to-end.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.dsr_pbo_sensitivity_cli import _run_sensitivity, main

from tests.test_multiple_testing_judge import _dates, _register_trial_with_result

# ---------------------------------------------------------------------------
# In-process, direct-fixture tests
# ---------------------------------------------------------------------------


def _two_trial_registry(tmp_path: Path, *, exp: str = "exp.sens", n: int = 40) -> Path:
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    dates = _dates(n)
    _register_trial_with_result(
        store, experiment_id=exp, hypothesis_id="hyp", strategy_id="a",
        run_dir=tmp_path / "a", dates=dates,
        net_returns=[0.01] * (n // 2) + [-0.005] * (n - n // 2),
    )
    _register_trial_with_result(
        store, experiment_id=exp, hypothesis_id="hyp", strategy_id="b",
        run_dir=tmp_path / "b", dates=dates,
        net_returns=[-0.004] * (n // 2) + [0.012] * (n - n // 2),
    )
    return db


def _trial_id_for_strategy(db: Path, experiment_id: str, strategy_id: str) -> str:
    store = ResearchResultStore(db)
    trials = store.list_trials(experiment_id=experiment_id, strategy_id=strategy_id)
    assert len(trials) == 1, f"expected exactly 1 trial for {strategy_id!r}, got {len(trials)}"
    return trials[0]["trial_id"]


def test_evaluated_for_real_two_trial_comparison_scope(tmp_path):
    db = _two_trial_registry(tmp_path)
    trial_id = _trial_id_for_strategy(db, "exp.sens", "a")

    result = _run_sensitivity(registry_db=db, trial_id=trial_id, block_counts=[8, 10, 12])

    assert result["status"] == "evaluated"
    assert result["trial_id"] == trial_id
    assert result["experiment_id"] == "exp.sens"
    assert len(result["per_block"]) == 3
    assert result["dsr_min"] <= result["dsr_max"]
    assert result["pbo_min"] <= result["pbo_max"]
    assert result["dsr_range"] == result["dsr_max"] - result["dsr_min"]
    assert result["pbo_range"] == result["pbo_max"] - result["pbo_min"]
    assert result["strategy_id"] == "a", (
        "strategy_id must identify the ACTUAL registered strategy for this trial -- "
        "the Rust caller cross-checks this against the backtest candidate's own "
        "strategy_name to catch a Research-trial mismatch before merging"
    )


def test_deterministic_across_repeated_calls(tmp_path):
    db = _two_trial_registry(tmp_path)
    trial_id = _trial_id_for_strategy(db, "exp.sens", "a")

    first = _run_sensitivity(registry_db=db, trial_id=trial_id, block_counts=[8, 10])
    second = _run_sensitivity(registry_db=db, trial_id=trial_id, block_counts=[8, 10])
    assert first == second


def test_single_trial_population_is_not_evaluable(tmp_path):
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    _register_trial_with_result(
        store, experiment_id="exp.single", hypothesis_id="hyp", strategy_id="only",
        run_dir=tmp_path / "only", dates=_dates(20), net_returns=[0.01] * 20,
    )
    trial_id = _trial_id_for_strategy(db, "exp.single", "only")

    result = _run_sensitivity(registry_db=db, trial_id=trial_id, block_counts=[8])
    assert result["status"] == "not_evaluable"
    # A single-trial population has no comparison scope at all (mirrors the
    # frozen judge's own `_resolve_comparison_scope` behavior for M < 2) --
    # this is the exact reason text `dsr_pbo_sensitivity_scenario`'s Rust
    # caller checks to classify a scenario as genuinely inapplicable rather
    # than a failure.
    assert "not part of the judged comparison scope" in result["reason"]
    assert result["strategy_id"] == "only", "strategy_id must be present even on a not_evaluable outcome"


def test_unknown_trial_id_raises_key_error(tmp_path):
    db = _two_trial_registry(tmp_path)
    try:
        _run_sensitivity(registry_db=db, trial_id="does-not-exist", block_counts=[8])
        assert False, "expected KeyError for unknown trial_id"
    except KeyError:
        pass


def test_main_reports_error_status_and_exit_1_for_unknown_trial(tmp_path, capsys):
    db = _two_trial_registry(tmp_path)
    exit_code = main([
        "--registry-db", str(db),
        "--trial-id", "does-not-exist",
        "--block-counts", "8,10",
    ])
    assert exit_code == 1
    out = json.loads(capsys.readouterr().out)
    assert out["status"] == "error"


def test_main_reports_evaluated_status_and_exit_0(tmp_path, capsys):
    db = _two_trial_registry(tmp_path)
    trial_id = _trial_id_for_strategy(db, "exp.sens", "a")
    exit_code = main([
        "--registry-db", str(db),
        "--trial-id", trial_id,
        "--block-counts", "8,10,12",
    ])
    assert exit_code == 0
    out = json.loads(capsys.readouterr().out)
    assert out["status"] == "evaluated"


# ---------------------------------------------------------------------------
# Real subprocess test -- proves the exact invocation Rust uses actually works
# ---------------------------------------------------------------------------


def test_real_subprocess_invocation_matches_in_process_call(tmp_path):
    db = _two_trial_registry(tmp_path)
    trial_id = _trial_id_for_strategy(db, "exp.sens", "a")

    src_dir = Path(__file__).resolve().parents[1] / "src"
    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "mqk_research.ml.dsr_pbo_sensitivity_cli",
            "--registry-db",
            str(db),
            "--trial-id",
            trial_id,
            "--block-counts",
            "8,10,12",
        ],
        env={**__import__("os").environ, "PYTHONPATH": str(src_dir)},
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert proc.returncode == 0, f"stderr={proc.stderr}"
    out = json.loads(proc.stdout)
    assert out["status"] == "evaluated"

    direct = _run_sensitivity(registry_db=db, trial_id=trial_id, block_counts=[8, 10, 12])
    assert out["dsr_min"] == direct["dsr_min"]
    assert out["dsr_max"] == direct["dsr_max"]
    assert out["pbo_min"] == direct["pbo_min"]
    assert out["pbo_max"] == direct["pbo_max"]
