"""Batch 03 scan identity row for XBI: a universe-membership row only, never provider/data authority.

The scanner consumes registry rows solely through (symbol, enabled, equity); bar provenance comes from
the bars provenance manifest. The supplement therefore must stay minimal, mirror the production row
schema, cover only symbols absent from the production registry, and never silently admit any other.
"""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]

os.environ["MQK_M1_BATCH_DECLARATION"] = "PREDECLARED_BATCH_03.json"
try:
    SPEC = importlib.util.spec_from_file_location("run_batch03_supplement_under_test", HERE / "run_batch.py")
    rb = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(rb)
finally:
    os.environ.pop("MQK_M1_BATCH_DECLARATION", None)

PRODUCTION = json.loads((REPO / "config" / "instruments" / "equities.json").read_text(encoding="utf-8"))
PRODUCTION_SYMBOLS = {r["symbol"] for r in PRODUCTION}


def test_supplement_covers_exactly_the_symbols_absent_from_the_production_registry() -> None:
    batch_symbols = {s for _, s in rb.TRIALS}
    assert set(rb.SCAN_REGISTRY_SUPPLEMENT) == batch_symbols - PRODUCTION_SYMBOLS == {"XBI"}
    assert not set(rb.SCAN_REGISTRY_SUPPLEMENT) & PRODUCTION_SYMBOLS


@pytest.mark.parametrize("symbol", sorted(rb.SCAN_REGISTRY_SUPPLEMENT))
def test_supplement_row_mirrors_the_production_identity_schema(symbol: str) -> None:
    row = rb.SCAN_REGISTRY_SUPPLEMENT[symbol]
    template = next(r for r in PRODUCTION if r["symbol"] == "SPY")
    assert {"instrument_id", "symbol", "asset_class", "provider", "provider_symbol", "venue", "currency",
            "enabled", "timeframes", "notes"} <= set(row)
    assert set(row) <= set(template)
    assert row["instrument_id"] == f"equity:US:{symbol}"
    assert (row["symbol"], row["provider_symbol"], row["asset_class"], row["enabled"]) == (
        symbol, symbol, "equity", True)
    assert "1D" in row["timeframes"]
    assert "not in config/instruments/equities.json" in row["notes"]


def test_a_batch_symbol_in_neither_registry_is_refused_not_admitted(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(rb, "TRIALS", [*rb.TRIALS, (rb.TRIALS[0][0], "NOTAREALSYM")])
    with pytest.raises(AssertionError):
        rb.stage_review(None)
