from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Tuple

from mqk_research.exp_distributed.hashing import short_hash
from mqk_research.ml.util_hash import file_record

# RESEARCH-UNIVERSE-SNAPSHOT-IDENTITY-01
#
# A narrow, reusable Research universe SNAPSHOT/IDENTITY seam -- distinct
# from, and never wired into, the production Paper/Live instrument registry
# (core-rs/crates/mqk-md/src/instrument_registry.rs,
# config/instruments/equities.json). This module only READS that config
# file as one possible SOURCE for a snapshot; it never writes to it, and a
# Research universe snapshot never authorizes Paper/Live trading of any
# symbol (see docs/research/BROAD_RESEARCH_UNIVERSE_CURRENT_TRUTH_AUDIT.md,
# which found no existing Research-side universe-source module before this
# patch).
#
# Also distinct from mqk_research.universe.build
# (`build_universe_swing_v1`), which is a DOWNSTREAM rank/filter stage that
# consumes an already-populated features.symbol column -- this module is
# the upstream symbol-SOURCE/identity seam a caller like that would sit on
# top of, not a competing universe builder.
#
# THREE UNIVERSE CONCEPTS (never collapse -- see the audit doc above):
#   CURRENT_DISCOVERY_UNIVERSE            -- not implemented anywhere yet.
#   CURRENT_REGISTRY_RESEARCH_SEED        -- what this module snapshots.
#   HISTORICAL_POINT_IN_TIME_RESEARCH_UNIVERSE -- not implemented; every
#       snapshot this module can currently produce MUST durably declare
#       point_in_time_membership=False (see SURVIVORSHIP_* below).
#   ACTIVE_PAPER_UNIVERSE                 -- unrelated; a large Research
#       snapshot never authorizes Paper/Live concurrency.

SCHEMA_VERSION = "research-universe-snapshot-v1"

UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY = "current_enabled_equity_registry_snapshot_v1"
KNOWN_UNIVERSE_SOURCE_KINDS = frozenset({UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY})

# mission "SURVIVORSHIP TRUTH": a current registry snapshot applied
# backward through history is NOT point-in-time membership -- this must be
# said, not implied, in every snapshot this source kind produces.
SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME = "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"

ASSET_CLASS_EQUITY = "equity"

_SELECTION_RULE_ENABLED_EQUITY_V1 = "enabled_true_and_asset_class_equity_v1"


@dataclass(frozen=True)
class UniverseSnapshot:
    """research-universe-snapshot-v1 (mission "UNIVERSE SNAPSHOT V1"). Every
    field the mission requires is preserved durably; `universe_id` is a
    content/semantic identity (see _universe_identity_fragment) -- never
    defined by a filesystem path alone, and never by any result/P&L value
    (this dataclass carries no such field at all)."""

    schema_version: str
    universe_source_kind: str
    source_content_identity: Dict[str, Any]
    selection_rule_id: str
    symbols: Tuple[str, ...]
    symbol_count: int
    asset_class: str
    membership_asof_basis: str
    point_in_time_membership: bool
    survivorship_classification: str
    universe_id: str

    def to_json_dict(self) -> Dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "universe_source_kind": self.universe_source_kind,
            "source_content_identity": self.source_content_identity,
            "selection_rule_id": self.selection_rule_id,
            "symbols": list(self.symbols),
            "symbol_count": self.symbol_count,
            "asset_class": self.asset_class,
            "membership_asof_basis": self.membership_asof_basis,
            "point_in_time_membership": self.point_in_time_membership,
            "survivorship_classification": self.survivorship_classification,
            "universe_id": self.universe_id,
        }


def _canonicalize_symbols(raw_symbols: List[Any]) -> Tuple[str, ...]:
    """Uppercase, strip, then fail closed on a blank or duplicate entry
    (UNIVERSE ID TESTS 3/4) -- never silently drop or dedupe a malformed
    source row. Sorted deterministically so the resulting tuple (and any
    identity derived from it) never depends on source row order (UNIVERSE
    ID TEST 1/2)."""
    seen: Dict[str, bool] = {}
    canonical: List[str] = []
    for raw in raw_symbols:
        symbol = str(raw).strip().upper()
        if not symbol:
            raise RuntimeError("Fail-closed: blank symbol encountered while building a universe snapshot")
        if symbol in seen:
            raise RuntimeError(
                f"Fail-closed: duplicate symbol {symbol!r} encountered while building a universe snapshot"
            )
        seen[symbol] = True
        canonical.append(symbol)
    return tuple(sorted(canonical))


def _universe_identity_fragment(
    *,
    universe_source_kind: str,
    selection_rule_id: str,
    symbols: Tuple[str, ...],
    asset_class: str,
    point_in_time_membership: bool,
) -> Dict[str, Any]:
    """Canonical, RESULT-INDEPENDENT identity fragment consumed by
    `universe_id`. Deliberately excludes `source_content_identity` (the
    source file's own physical path/sha256/byte-count) for the exact same
    reason economic_registry_integration.build_economic_trial_identity
    deliberately excludes a bars file's physical sha256 from trial identity
    (see that module's docstring, "Defect 3") -- physical file location/
    row-order/byte-layout is audit evidence, not semantic identity.
    `symbols` is already canonicalized+sorted by the caller before this is
    built, so relocating the same semantic source content to a different
    path (UNIVERSE ID TEST 11), or reordering its rows (TESTS 1/2), can
    never change `universe_id`. No P&L/result field is or could be present
    here -- this fragment is built purely from source SELECTION inputs."""
    return {
        "schema_version": SCHEMA_VERSION,
        "universe_source_kind": universe_source_kind,
        "selection_rule_id": selection_rule_id,
        "symbols": list(symbols),
        "asset_class": asset_class,
        "point_in_time_membership": point_in_time_membership,
    }


def build_current_enabled_equity_registry_snapshot(registry_path: Path) -> UniverseSnapshot:
    """`current_enabled_equity_registry_snapshot_v1` (mission "CURRENT
    REGISTRY SEED MODE"): reads a config/instruments/equities.json-shaped
    JSON array from `registry_path` and selects exactly the entries with
    `enabled == true AND asset_class == "equity"` -- mirroring the
    production Rust registry's own filter
    (instrument_registry.rs::enabled_equities) as an INDEPENDENT read-only
    Research observation. This function never mutates `registry_path` and
    is never consumed by the production registry loader.

    UNIVERSE membership and Research BAR PROVIDER are deliberately
    different concerns (mission) -- this snapshot says nothing about which
    market-data provider/timeframe a later Research run uses; a per-entry
    `provider`/`timeframes` field in the source file is not required to
    match any particular downstream Research data source.

    SURVIVORSHIP TRUTH: always returns `point_in_time_membership=False`
    and `survivorship_classification=
    SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME` for this
    source kind -- a current registry snapshot applied backward through
    history is NEVER promotion-grade historical universe evidence."""
    registry_path = Path(registry_path)
    if not registry_path.exists():
        raise FileNotFoundError(f"Fail-closed: missing universe source registry file: {registry_path}")
    raw = json.loads(registry_path.read_text(encoding="utf-8"))
    if not isinstance(raw, list):
        raise RuntimeError("Fail-closed: universe source registry file must be a JSON array of entries")

    raw_symbols = [
        entry.get("symbol")
        for entry in raw
        if bool(entry.get("enabled")) and entry.get("asset_class") == ASSET_CLASS_EQUITY
    ]
    symbols = _canonicalize_symbols(raw_symbols)

    identity_fragment = _universe_identity_fragment(
        universe_source_kind=UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY,
        selection_rule_id=_SELECTION_RULE_ENABLED_EQUITY_V1,
        symbols=symbols,
        asset_class=ASSET_CLASS_EQUITY,
        point_in_time_membership=False,
    )
    universe_id = short_hash(identity_fragment, length=32)

    return UniverseSnapshot(
        schema_version=SCHEMA_VERSION,
        universe_source_kind=UNIVERSE_SOURCE_KIND_CURRENT_ENABLED_EQUITY_REGISTRY,
        # Audit/debugging evidence only -- deliberately NOT part of
        # universe_id (see _universe_identity_fragment docstring).
        source_content_identity=file_record(registry_path),
        selection_rule_id=_SELECTION_RULE_ENABLED_EQUITY_V1,
        symbols=symbols,
        symbol_count=len(symbols),
        asset_class=ASSET_CLASS_EQUITY,
        membership_asof_basis="registry_file_current_state_no_declared_asof",
        point_in_time_membership=False,
        survivorship_classification=SURVIVORSHIP_CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME,
        universe_id=universe_id,
    )
