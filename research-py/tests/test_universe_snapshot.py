"""
RESEARCH-UNIVERSE-SNAPSHOT-IDENTITY-01 (DIRECT-RANK-AND-BROAD-UNIVERSE-
RESEARCH-01 Patch D) -- a narrow, reusable Research universe
snapshot/identity seam (mqk_research.universe.snapshot), distinct from the
production Paper/Live instrument registry and from the existing downstream
rank/filter module mqk_research.universe.build.

Covers the mission's Patch D "UNIVERSE ID TESTS" (12 items, referenced by
number in each test's docstring).
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

import pytest

from mqk_research.universe.snapshot import (
    SCHEMA_VERSION,
    SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME,
    UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY,
    build_current_enabled_equity_registry_snapshot,
)


def _entry(symbol: str, *, enabled: bool = True, asset_class: str = "equity", **extra: Any) -> Dict[str, Any]:
    row = {
        "instrument_id": f"equity:US:{symbol}", "symbol": symbol, "asset_class": asset_class,
        "provider": "test", "provider_symbol": symbol, "venue": "NASDAQ", "currency": "USD",
        "enabled": enabled, "timeframes": ["1D"], "notes": "test fixture",
    }
    row.update(extra)
    return row


def _write_registry(path: Path, entries: List[Dict[str, Any]]) -> Path:
    path.write_text(json.dumps(entries), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# REQUIRED TESTS 1/2: row/symbol order invariance
# ---------------------------------------------------------------------------


def test_registry_row_order_does_not_change_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 1."""
    entries = [_entry("AAA"), _entry("BBB"), _entry("CCC")]
    forward = _write_registry(tmp_path / "forward.json", entries)
    backward = _write_registry(tmp_path / "backward.json", list(reversed(entries)))
    snap_forward = build_current_enabled_equity_registry_snapshot(forward)
    snap_backward = build_current_enabled_equity_registry_snapshot(backward)
    assert snap_forward.universe_id == snap_backward.universe_id
    assert snap_forward.symbols == snap_backward.symbols


def test_symbol_order_does_not_change_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 2: a differently-shuffled (not just reversed) row
    order over the same entry set produces the same universe_id and the
    same canonical (sorted) symbols tuple."""
    entries = [_entry(s) for s in ("ZZZ", "AAA", "MMM", "BBB")]
    shuffled = [entries[2], entries[0], entries[3], entries[1]]
    a = build_current_enabled_equity_registry_snapshot(_write_registry(tmp_path / "a.json", entries))
    b = build_current_enabled_equity_registry_snapshot(_write_registry(tmp_path / "b.json", shuffled))
    assert a.universe_id == b.universe_id
    assert a.symbols == tuple(sorted(("ZZZ", "AAA", "MMM", "BBB")))
    assert b.symbols == a.symbols


# ---------------------------------------------------------------------------
# REQUIRED TESTS 3-6: fail-closed / filter correctness
# ---------------------------------------------------------------------------


def test_duplicate_symbol_fails_closed(tmp_path: Path) -> None:
    """REQUIRED TEST 3."""
    entries = [_entry("AAA"), _entry("AAA")]
    path = _write_registry(tmp_path / "reg.json", entries)
    with pytest.raises(RuntimeError, match="duplicate"):
        build_current_enabled_equity_registry_snapshot(path)


def test_blank_symbol_fails_closed(tmp_path: Path) -> None:
    """REQUIRED TEST 4."""
    entries = [_entry("AAA"), _entry("   ")]
    path = _write_registry(tmp_path / "reg.json", entries)
    with pytest.raises(RuntimeError, match="blank"):
        build_current_enabled_equity_registry_snapshot(path)


def test_disabled_symbol_excluded(tmp_path: Path) -> None:
    """REQUIRED TEST 5."""
    entries = [_entry("AAA"), _entry("BBB", enabled=False)]
    path = _write_registry(tmp_path / "reg.json", entries)
    snap = build_current_enabled_equity_registry_snapshot(path)
    assert snap.symbols == ("AAA",)


def test_non_equity_excluded(tmp_path: Path) -> None:
    """REQUIRED TEST 6."""
    entries = [_entry("AAA"), _entry("BBB", asset_class="crypto")]
    path = _write_registry(tmp_path / "reg.json", entries)
    snap = build_current_enabled_equity_registry_snapshot(path)
    assert snap.symbols == ("AAA",)


# ---------------------------------------------------------------------------
# REQUIRED TESTS 7-9: membership changes ARE identity-bearing
# ---------------------------------------------------------------------------


def test_adding_enabled_equity_changes_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 7."""
    before = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "before.json", [_entry("AAA")])
    )
    after = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "after.json", [_entry("AAA"), _entry("BBB")])
    )
    assert before.universe_id != after.universe_id


def test_removing_enabled_equity_changes_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 8."""
    before = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "before.json", [_entry("AAA"), _entry("BBB")])
    )
    after = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "after.json", [_entry("AAA")])
    )
    assert before.universe_id != after.universe_id


def test_toggling_enabled_changes_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 9."""
    on = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "on.json", [_entry("AAA"), _entry("BBB", enabled=True)])
    )
    off = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "off.json", [_entry("AAA"), _entry("BBB", enabled=False)])
    )
    assert on.universe_id != off.universe_id


# ---------------------------------------------------------------------------
# REQUIRED TEST 10: no result/P&L field can enter universe_id
# ---------------------------------------------------------------------------


def test_no_result_field_can_enter_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 10: the dataclass/identity fragment structurally have
    no return/Sharpe/result-shaped field at all."""
    snap = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "reg.json", [_entry("AAA")])
    )
    blob = json.dumps(snap.to_json_dict()).lower()
    for forbidden in ("sharpe", "return", "pnl", "p&l", "profit"):
        assert forbidden not in blob


# ---------------------------------------------------------------------------
# REQUIRED TEST 11: path relocation alone does not manufacture a new
# universe if semantic source content/rule is identical
# ---------------------------------------------------------------------------


def test_path_relocation_alone_does_not_change_universe_id(tmp_path: Path) -> None:
    """REQUIRED TEST 11."""
    entries = [_entry("AAA"), _entry("BBB")]
    dir_a = tmp_path / "location_a"
    dir_b = tmp_path / "location_b" / "nested"
    dir_a.mkdir()
    dir_b.mkdir(parents=True)
    snap_a = build_current_enabled_equity_registry_snapshot(_write_registry(dir_a / "equities.json", entries))
    snap_b = build_current_enabled_equity_registry_snapshot(_write_registry(dir_b / "renamed.json", entries))
    assert snap_a.universe_id == snap_b.universe_id
    # The physical path IS still recorded (for audit), but is not identity.
    assert snap_a.source_content_identity["path"] != snap_b.source_content_identity["path"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 12: point_in_time_membership=False durably present
# ---------------------------------------------------------------------------


def test_point_in_time_membership_false_durably_present(tmp_path: Path) -> None:
    """REQUIRED TEST 12."""
    snap = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "reg.json", [_entry("AAA")])
    )
    assert snap.point_in_time_membership is False
    assert snap.survivorship_classification == SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME
    d = snap.to_json_dict()
    assert d["point_in_time_membership"] is False
    assert d["survivorship_classification"] == SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME


# ---------------------------------------------------------------------------
# Schema/source-kind sanity
# ---------------------------------------------------------------------------


def test_schema_and_source_kind_fields(tmp_path: Path) -> None:
    snap = build_current_enabled_equity_registry_snapshot(
        _write_registry(tmp_path / "reg.json", [_entry("AAA")])
    )
    assert snap.schema_version == SCHEMA_VERSION == "research-universe-snapshot-v1"
    assert snap.universe_source_kind == UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY
    assert snap.symbol_count == len(snap.symbols) == 1


def test_missing_registry_file_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        build_current_enabled_equity_registry_snapshot(tmp_path / "does_not_exist.json")
