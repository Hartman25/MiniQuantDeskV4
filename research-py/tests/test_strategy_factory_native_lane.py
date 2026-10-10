"""The strategy-factory CI lane fails clearly when the native path is not really proven."""

from __future__ import annotations

import importlib.util
import xml.etree.ElementTree as ET

import pytest

from mqk_research.strategy_factory.service import detect_grammar
from support import factory_e2e as E

GUARD = E.REPO / "scripts" / "guards" / "check_factory_native_lane.py"


def _guard():
    spec = importlib.util.spec_from_file_location("check_factory_native_lane", GUARD)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _junit(passed=(), skipped=(), failed=()):
    root = ET.Element("testsuite")
    for n in passed:
        ET.SubElement(root, "testcase", name=n)
    for n, msg in skipped:
        ET.SubElement(ET.SubElement(root, "testcase", name=n), "skipped", message=msg)
    for n in failed:
        ET.SubElement(ET.SubElement(root, "testcase", name=n), "failure", message="boom")
    return root


@pytest.mark.skipif(not E.REQUIRE_NATIVE, reason="only the strict native lane (MQK_FACTORY_REQUIRE_NATIVE=1) requires the binary")
def test_required_native_binary_resolves_the_grammar_engine():
    assert E.cli_available(), f"MQK_FACTORY_REQUIRE_NATIVE=1 but no native binary at {E.DEFAULT_CLI}; set MQK_FACTORY_CLI"
    assert detect_grammar(E.DEFAULT_CLI), "the native binary does not resolve a grammar_v1 strategy"


def test_guard_passes_only_when_every_required_test_executed_and_passed():
    g = _guard()
    assert g.verdict(_junit(passed=g.REQUIRED)) == []
    optional = next(iter(g.OPTIONAL_SKIP))
    assert g.verdict(_junit(passed=g.REQUIRED, skipped=[(optional, "no verified local historical bars on this machine")])) == []


@pytest.mark.parametrize("damage", ["missing", "skipped", "failed", "optional_skip_wrong_reason", "other_test_skipped"])
def test_guard_rejects_a_missing_skipped_or_failed_required_test(damage):
    g = _guard()
    victim, rest = g.REQUIRED[0], g.REQUIRED[1:]
    if damage == "missing":
        root = _junit(passed=rest)
    elif damage == "skipped":
        root = _junit(passed=rest, skipped=[(victim, "native mqk-cli binary not built on this machine")])
    elif damage == "failed":
        root = _junit(passed=rest, failed=[victim])
    elif damage == "other_test_skipped":
        root = _junit(passed=g.REQUIRED, skipped=[("test_some_unrelated_factory_test", "anything")])
    else:
        root = _junit(passed=g.REQUIRED, skipped=[(next(iter(g.OPTIONAL_SKIP)), "native mqk-cli binary not built on this machine")])
    assert g.verdict(root)


def test_strict_native_mode_removes_the_skip_mark_and_non_strict_mode_keeps_it(monkeypatch):
    monkeypatch.setattr(E, "cli_available", lambda: False)
    monkeypatch.setattr(E, "REQUIRE_NATIVE", True)
    assert E.native_marks().args[0] is False                      # absent binary + strict: the tests run and fail, they do not skip
    monkeypatch.setattr(E, "REQUIRE_NATIVE", False)
    assert E.native_marks().args[0] is True


def test_the_ci_lane_builds_the_binary_requires_it_and_runs_the_guard():
    text = (E.REPO / ".github" / "workflows" / "strategy-factory.yml").read_text(encoding="utf-8")
    for needle in ('MQK_FACTORY_REQUIRE_NATIVE: "1"', "cargo build -p mqk-cli", "MQK_FACTORY_CLI:", "-k \"strategy_factory\"",
                   "--junitxml", 'python scripts/guards/check_factory_native_lane.py "$RUNNER_TEMP/factory_native.xml"', "grammar_rule_v1", '"strategy-factory/**"'):
        assert needle in text, needle
    assert "--workspace" not in text and "--all-targets" not in text          # bounded: no workspace-wide Rust sweep
