"""The canonical daily timeframe identity is an explicit declaration opt-in.

Every closed (historical) declaration stays on the raw-label identity so its recorded trial ids
cannot change; a future declaration opts in with `data.timeframe_identity = canonical_semantic_v1`.
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import run_batch as rb  # noqa: E402

HISTORICAL = sorted(HERE.glob("PREDECLARED_BATCH_0*.json"))


@pytest.mark.parametrize("path", HISTORICAL, ids=lambda p: p.name)
def test_every_historical_declaration_keeps_the_raw_label_identity(path):
    decl = json.loads(path.read_text(encoding="utf-8"))
    if "data" in decl:
        assert rb.canonical_timeframe_identity(decl) is False


def test_opt_in_is_exact_and_unknown_values_are_refused():
    decl = copy.deepcopy(json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8")))
    decl["data"]["timeframe_identity"] = "canonical_semantic_v1"
    assert rb.canonical_timeframe_identity(decl) is True
    for bad in ("raw_label_v1", "canonical_semantic_v2", True, ""):
        decl["data"]["timeframe_identity"] = bad
        with pytest.raises(SystemExit, match="timeframe_identity"):
            rb.canonical_timeframe_identity(decl)
