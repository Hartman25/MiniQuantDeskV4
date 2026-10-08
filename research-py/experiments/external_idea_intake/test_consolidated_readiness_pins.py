"""Pins the consolidated readiness record and the M1.9 preflight runbook to the code they describe, so a
stale claim (engine inventory, migration head, universe bound, ledger gate, catalog hash) fails here."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import intake  # noqa: E402

ROOT = HERE.parents[2]
DOC = (ROOT / "docs/research/M1_CANDIDATE_SUPPLY_OOS_PAPER_READINESS_CONSOLIDATED_01.md").read_text(encoding="utf-8")
RUNBOOK = (ROOT / "docs/runbooks/m1_9_paper_deployment_preflight.md").read_text(encoding="utf-8")
CRATES = ROOT / "core-rs/crates"


def registered_engine_ids() -> list[str]:
    src = (CRATES / "mqk-strategy/src/engines/mod.rs").read_text(encoding="utf-8")
    body = re.search(r"pub const REGISTERED_STRATEGY_IDS: &\[&str\] = &\[(.*?)\];", src, re.S).group(1)
    ids = []
    for mod, const in re.findall(r"(\w+)::(NAME|SHORT_NAME)", body):
        text = (CRATES / f"mqk-strategy/src/engines/{mod}.rs").read_text(encoding="utf-8")
        ids.append(re.search(rf'const {const}: &str = "([^"]+)"', text).group(1))
    return ids


def test_inventory_names_every_registered_engine_identity():
    ids = registered_engine_ids()
    assert len(ids) == len(set(ids)) == 24
    missing = [i for i in ids if i not in DOC and i.replace("_short", "") not in DOC]
    assert not missing, missing


def test_universe_bound_equals_registry_count_as_the_doc_states():
    src = (CRATES / "mqk-portfolio/src/dynamic_selection.rs").read_text(encoding="utf-8")
    bound = int(re.search(r"pub const MAX_STRATEGY_UNIVERSE: usize = (\d+);", src).group(1))
    assert bound == len(registered_engine_ids()) == 24
    assert "`MAX_STRATEGY_UNIVERSE = 24`" in DOC


def test_migration_head_is_0091_as_the_runbook_states():
    manifest = json.loads((CRATES / "mqk-db/migrations/manifest.json").read_text(encoding="utf-8"))
    last = json.dumps(manifest["migrations"][-1])
    assert "0091_strategy_held_sizing_state" in last
    assert "0091_strategy_held_sizing_state" in RUNBOOK and "`91`" in RUNBOOK


def test_ledger_gate_in_code_matches_the_documented_ten_and_five():
    src = (CRATES / "mqk-integrity/src/soak_ledger.rs").read_text(encoding="utf-8")
    assert "required_sessions: 10," in src and "required_consecutive_clean: 5," in src
    assert "10 countable / 5 consecutive clean" in DOC


def test_catalog_hash_in_the_record_is_the_one_the_intake_pins():
    assert intake.EXPECTED_SHA256 in DOC


def test_record_states_the_blocked_catalog_and_authorizes_nothing():
    for phrase in ("CATALOG_INTAKE_BLOCKED_WORKBOOK_NOT_SUPPLIED", "`M1_BLOCKED`", "NOT_AUTHORIZED", "NOT_EXECUTABLE",
                   "`execution_gate.executable = false`", "not pushed"):
        assert phrase in DOC, phrase
    assert "EXT-" not in "".join(p.read_text(encoding="utf-8") for p in
                                 (ROOT / "research-py/experiments").glob("**/PREDECLARED_*.json"))


def test_runbook_authorizes_no_mutation():
    for phrase in ("does not authorize applying migrations", "Live stays disabled", "read-only"):
        assert phrase in RUNBOOK, phrase
    assert "mqk db migrate" in RUNBOOK and "never done by this runbook" in RUNBOOK
