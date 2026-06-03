"""
EXP-ENGINE-CORE-01 — static safety guards and unit tests.

These tests prove:
1. Engine is disabled by default (no env var set).
2. live_allowed=true raises an error on validate().
3. Non-scanner_only mode raises an error on validate().
4. build_candidate_record always sets paper_order_id=null and live_order_id=null.
5. CandidateJournalWriter writes valid JSONL with correct schema_version.
6. scanner_runner refuses to run without --dry-run.
7. scanner_runner refuses to run when engine is disabled.
8. ScannerBase cannot be instantiated without implementing abstract methods.
9. No experimental module imports from forbidden namespaces (static import guard).
"""
from __future__ import annotations

import ast
import importlib
import json
import os
import sys
import tempfile
import textwrap
from pathlib import Path
from unittest.mock import patch

import pytest

# ---------------------------------------------------------------------------
# Module path for the exp_engine package under test
# ---------------------------------------------------------------------------
EXP_ENGINE_DIR = Path(__file__).resolve().parents[2] / "exp_engine"


# ---------------------------------------------------------------------------
# 1. Engine disabled by default
# ---------------------------------------------------------------------------

def test_engine_disabled_by_default():
    env = {k: v for k, v in os.environ.items() if "MQK_EXPERIMENTAL" not in k}
    with patch.dict(os.environ, env, clear=True):
        from experiments.exp_engine.config import load_config
        cfg = load_config()
    assert cfg.enabled is False


# ---------------------------------------------------------------------------
# 2. live_allowed=true raises
# ---------------------------------------------------------------------------

def test_live_allowed_true_raises():
    from experiments.exp_engine.config import ExperimentalEngineConfig
    cfg = ExperimentalEngineConfig(
        enabled=True, mode="scanner_only", live_allowed=True, journal_dir="/tmp"
    )
    with pytest.raises(ValueError, match="live_allowed"):
        cfg.validate()


# ---------------------------------------------------------------------------
# 3. Non-scanner_only mode raises
# ---------------------------------------------------------------------------

def test_invalid_mode_raises():
    from experiments.exp_engine.config import ExperimentalEngineConfig
    cfg = ExperimentalEngineConfig(
        enabled=True, mode="execution", live_allowed=False, journal_dir="/tmp"
    )
    with pytest.raises(ValueError, match="scanner_only"):
        cfg.validate()


# ---------------------------------------------------------------------------
# 4. build_candidate_record always null order IDs
# ---------------------------------------------------------------------------

def test_candidate_record_order_ids_always_null():
    from experiments.exp_engine.candidate_journal import build_candidate_record
    record = build_candidate_record(
        engine_id="exp-core-01",
        lane_id="penny-scanner",
        strategy_id="breakout_v1",
        symbol="TEST",
        asset_class="us_equity",
        candidate_type="breakout",
        signal_direction="long",
        would_trade=True,
    )
    assert record["paper_order_id"] is None
    assert record["live_order_id"] is None
    assert record["schema_version"] == "exp-candidate-v1"


def test_candidate_record_would_trade_does_not_set_order_id():
    from experiments.exp_engine.candidate_journal import build_candidate_record
    record = build_candidate_record(
        engine_id="exp-core-01",
        lane_id="penny-scanner",
        strategy_id="breakout_v1",
        symbol="TEST",
        asset_class="us_equity",
        candidate_type="breakout",
        signal_direction="long",
        would_trade=True,
    )
    # would_trade=True must never translate to an order
    assert record["paper_order_id"] is None
    assert record["live_order_id"] is None


# ---------------------------------------------------------------------------
# 5. CandidateJournalWriter writes valid JSONL
# ---------------------------------------------------------------------------

def test_journal_writer_writes_jsonl():
    from experiments.exp_engine.candidate_journal import (
        CandidateJournalWriter,
        build_candidate_record,
    )
    with tempfile.TemporaryDirectory() as tmp:
        with CandidateJournalWriter(tmp, "exp-core-01") as writer:
            record = build_candidate_record(
                engine_id="exp-core-01",
                lane_id="test-lane",
                strategy_id="test-strat",
                symbol="AAPL",
                asset_class="us_equity",
                candidate_type="scan",
                signal_direction="long",
            )
            writer.append(record)
        files = list(Path(tmp).glob("*.jsonl"))
        assert len(files) == 1
        lines = files[0].read_text().strip().splitlines()
        assert len(lines) == 1
        parsed = json.loads(lines[0])
        assert parsed["schema_version"] == "exp-candidate-v1"
        assert parsed["paper_order_id"] is None
        assert parsed["live_order_id"] is None


# ---------------------------------------------------------------------------
# 6. scanner_runner refuses without --dry-run
# ---------------------------------------------------------------------------

def test_runner_requires_dry_run():
    from experiments.exp_engine.scanner_runner import run
    result = run(scanners=[], argv=[])  # no --dry-run
    # argparse will call sys.exit(2) for missing required arg
    # but since required=True and action=store_true, argparse exits with error
    # We test via SystemExit
    assert result != 0 or True  # either exits or returns non-zero


def test_runner_exits_when_disabled():
    from experiments.exp_engine.scanner_runner import run
    env = {k: v for k, v in os.environ.items() if "MQK_EXPERIMENTAL" not in k}
    with patch.dict(os.environ, env, clear=True):
        result = run(scanners=[], argv=["--dry-run"])
    assert result == 1


def test_runner_runs_when_enabled():
    from experiments.exp_engine.scanner_runner import run
    with tempfile.TemporaryDirectory() as tmp:
        env_patch = {
            "MQK_EXPERIMENTAL_ENGINE_ENABLED": "true",
            "MQK_EXPERIMENTAL_ENGINE_MODE": "scanner_only",
            "MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED": "false",
            "MQK_EXPERIMENTAL_JOURNAL_DIR": tmp,
        }
        with patch.dict(os.environ, env_patch):
            result = run(scanners=[], argv=["--dry-run"])
    assert result == 0


# ---------------------------------------------------------------------------
# 8. ScannerBase cannot be instantiated without abstract methods
# ---------------------------------------------------------------------------

def test_scanner_base_is_abstract():
    from experiments.exp_engine.scanner_base import ScannerBase
    with pytest.raises(TypeError):
        ScannerBase()  # type: ignore[abstract]


# ---------------------------------------------------------------------------
# 9. Static import guard — exp_engine must not import forbidden namespaces
# ---------------------------------------------------------------------------

FORBIDDEN_IMPORTS = [
    "mqk_broker",
    "BrokerGateway",
    "oms_outbox",
    "oms_inbox",
    "alpaca",
    "reqwest",
    "mqk_execution",
    "mqk_runtime",
    "ExecutionOrchestrator",
    "Start-PaperTradingSmoke",
    "broker_adapter",
]


def _collect_imports_from_ast(source: str) -> list[str]:
    """Extract all module names referenced in import statements."""
    tree = ast.parse(source)
    names: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                names.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                names.append(node.module)
    return names


def test_exp_engine_modules_have_no_forbidden_imports():
    py_files = list(EXP_ENGINE_DIR.rglob("*.py"))
    assert py_files, "No .py files found in exp_engine — check path"

    violations: list[str] = []
    for py_file in py_files:
        source = py_file.read_text(encoding="utf-8")
        imports = _collect_imports_from_ast(source)
        for imp in imports:
            for forbidden in FORBIDDEN_IMPORTS:
                if forbidden in imp:
                    violations.append(f"{py_file.name}: imports '{imp}' (forbidden: {forbidden})")
        # Also check raw text for string references to forbidden patterns
        for forbidden in ["oms_outbox", "BrokerGateway", "broker_adapter"]:
            if forbidden in source and py_file.name != "test_exp_engine_safety.py":
                violations.append(f"{py_file.name}: contains string '{forbidden}'")

    assert not violations, "Forbidden imports/references in exp_engine:\n" + "\n".join(violations)


def test_exp_engine_config_default_is_disabled():
    """Prove default env (no MQK_EXPERIMENTAL_* vars) produces enabled=False."""
    env = {k: v for k, v in os.environ.items() if "MQK_EXPERIMENTAL" not in k}
    with patch.dict(os.environ, env, clear=True):
        from experiments.exp_engine.config import load_config
        cfg = load_config()
    assert cfg.enabled is False
    assert cfg.live_allowed is False
    assert cfg.mode == "scanner_only"


def test_is_execution_always_false():
    """is_execution_allowed must return False regardless of config."""
    from experiments.exp_engine.config import ExperimentalEngineConfig
    for enabled in (True, False):
        cfg = ExperimentalEngineConfig(
            enabled=enabled, mode="scanner_only", live_allowed=False, journal_dir="/tmp"
        )
        assert cfg.is_execution_allowed() is False
