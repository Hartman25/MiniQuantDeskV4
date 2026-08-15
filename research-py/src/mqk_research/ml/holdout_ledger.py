from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, Optional

from mqk_research.exp_distributed.hashing import short_hash
from mqk_research.exp_distributed.storage import ResearchResultStore

# RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01
#
# Durable, atomic record of when a dataset's reserved final holdout region is
# CONSUMED (scored) for the first time. This module is purely additive: it
# does not evaluate, read, or score any holdout-region data itself, and does
# not change the meaning of "reserved_not_evaluated" anywhere else in the
# codebase (see RESEARCH-PURGED-HOLDOUT-01 / eval_walkforward.py,
# RESEARCH-ECONOMIC-WALKFORWARD-01 / economic_walkforward.py -- neither is
# touched by this module). It exists so that a FUTURE patch that gains the
# ability to score a holdout has a durable fact to check first: "has this
# exact holdout already been consumed?" No caller in this repository invokes
# consume_holdout() as of this patch.
#
# holdout_id is a pure content hash of immutable, performance-independent
# facts (dataset identity, holdout boundary, protocol/schema version) --
# never of any result/metric value. Two callers computing the same
# holdout_id from the same facts always get the same id, regardless of
# insertion order or of what a later consumption records for
# consumer_identity/result_identity.

HOLDOUT_LEDGER_SCHEMA_VERSION = "research_holdout_ledger_v1"

__all__ = [
    "HOLDOUT_LEDGER_SCHEMA_VERSION",
    "compute_holdout_id",
    "reserve_holdout",
    "consume_holdout",
    "get_holdout",
]


def compute_holdout_id(
    *,
    dataset_identity: Dict[str, Any],
    holdout_start_utc: str,
    holdout_end_utc: str,
    protocol_version: str,
) -> str:
    """Deterministic holdout identity. `dataset_identity` must be a dict of
    immutable content-hash facts (e.g. {"features_csv": {"sha256": ...,
    "bytes": ...}, ...}, matching the convention used by
    mqk_research.ml.registry_integration.build_trial_identity) -- never a
    result/metric value. A different protocol_version (e.g. a future
    evaluator with different holdout semantics) intentionally produces a
    different holdout_id even for byte-identical data and boundaries -- the
    ledger must never conflate holdout regions defined under different
    rules."""
    basis = {
        "schema_version": HOLDOUT_LEDGER_SCHEMA_VERSION,
        "protocol_version": protocol_version,
        "dataset_identity": dataset_identity,
        "holdout_start_utc": holdout_start_utc,
        "holdout_end_utc": holdout_end_utc,
    }
    return short_hash(basis, length=32)


def reserve_holdout(
    registry_db: Path,
    *,
    dataset_identity: Dict[str, Any],
    holdout_start_utc: str,
    holdout_end_utc: str,
    protocol_version: str,
) -> str:
    """Compute the deterministic holdout_id and idempotently record it in the
    RESERVED state. Returns the holdout_id."""
    holdout_id = compute_holdout_id(
        dataset_identity=dataset_identity,
        holdout_start_utc=holdout_start_utc,
        holdout_end_utc=holdout_end_utc,
        protocol_version=protocol_version,
    )
    store = ResearchResultStore(Path(registry_db))
    store.reserve_holdout(
        holdout_id=holdout_id,
        dataset_identity=dataset_identity,
        holdout_start_utc=holdout_start_utc,
        holdout_end_utc=holdout_end_utc,
        protocol_version=protocol_version,
    )
    return holdout_id


def consume_holdout(
    registry_db: Path,
    *,
    holdout_id: str,
    consumer_identity: Dict[str, Any],
    consumed_at: str,
    result_identity: Optional[str] = None,
) -> None:
    """Atomically transition `holdout_id` from RESERVED to CONSUMED. See
    ResearchResultStore.consume_holdout for the fail-closed / one-time
    consumption contract this delegates to."""
    store = ResearchResultStore(Path(registry_db))
    store.consume_holdout(
        holdout_id=holdout_id,
        consumer_identity=consumer_identity,
        consumed_at=consumed_at,
        result_identity=result_identity,
    )


def get_holdout(registry_db: Path, holdout_id: str) -> Dict[str, Any]:
    store = ResearchResultStore(Path(registry_db))
    return store.get_holdout(holdout_id)
