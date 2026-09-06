"""W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02 (Patch R1) -- shared,
canonical implementation of the economic-artifact authentication chain
already accepted in `p7a_p7b_economic_replay_stress_cli.py`. Extracted
byte-for-byte (no behavior change) so `oos_replay_bundle.py`,
`p7a_p7b_economic_replay_stress_cli.py`, and `genuine_shuffled_placebo_cli.py`
all reuse ONE interpretation of "authenticate a recorded artifact / recorded
input" rather than each re-deriving the formula.

No mutable artifact may authenticate itself: every check here recomputes
identity from CURRENT on-disk content or re-verifies recorded path/bytes/
sha256 against what is on disk today -- never trusts a file's own
self-declared id field.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.util_hash import sha256_file, sha256_json

# Keys `run_economic_walkforward` writes into `economic_walk_forward.json`
# AFTER computing `economic_eval_id = sha256_json(out)` (see
# economic_walkforward.py, `out["ids"] = ...`) or that a later caller
# (`economic_registry_integration.run_registered_economic_walkforward_eval`)
# appends to the file post-hoc (`out["registry"] = {...}`) -- neither was
# part of the original hash basis, so both must be excluded when
# recomputing that hash from the file as it exists on disk today.
ECONOMIC_EVAL_ID_EXCLUDED_KEYS = frozenset({"ids", "registry"})


class ReplayAuthorityError(Exception):
    """A recorded input's path/bytes/sha256 could not be re-verified, an
    artifact's content hash disagrees with its durable registry authority, or
    a reconstructed spec's protocol identity does not round-trip -- fail
    closed, never refetch-and-assume-identical or trust a self-declared id."""


def recompute_economic_eval_id(econ: Dict[str, Any]) -> str:
    """Recompute `economic_eval_id` exactly as `run_economic_walkforward`
    originally did, from the artifact's CURRENT on-disk content -- never
    trusting the file's own self-declared `ids.economic_eval_id` field,
    which could be forged/mutated independently of the surrounding content
    it is supposed to attest to."""
    basis = {k: v for k, v in econ.items() if k not in ECONOMIC_EVAL_ID_EXCLUDED_KEYS}
    return sha256_json(basis)


def verify_recorded_input(name: str, record: Dict[str, Any]) -> Path:
    """Re-verify one recorded `{path, bytes, sha256}` input still exists,
    unmutated, on disk. Fails closed (`ReplayAuthorityError`) on any missing
    path, byte-count mismatch, or hash mismatch -- never refetches or
    substitutes a different file."""
    path_str = record.get("path")
    if not path_str:
        raise ReplayAuthorityError(f"{name}: no path recorded")
    path = Path(path_str)
    if not path.exists():
        raise ReplayAuthorityError(
            f"{name}: recorded input no longer exists at {path} -- refusing to refetch or "
            "substitute; the exact original file is required"
        )
    actual_bytes = path.stat().st_size
    if record.get("bytes") != actual_bytes:
        raise ReplayAuthorityError(
            f"{name}: byte count changed since the original run (recorded {record.get('bytes')}, "
            f"actual {actual_bytes}) -- refusing to replay against a mutated input"
        )
    actual_sha256 = sha256_file(path)
    if record.get("sha256") != actual_sha256:
        raise ReplayAuthorityError(
            f"{name}: sha256 changed since the original run (recorded {record.get('sha256')!r}, "
            f"actual {actual_sha256!r}) -- refusing to replay against a mutated input"
        )
    return path


def resolve_trial_economic_artifact(
    store: ResearchResultStore, trial_id: str, economic_eval_id: str
) -> Path:
    """Resolve the EXACT succeeded attempt where `trial_id == T` and the
    durable registry's own `result_id == economic_eval_id` -- never "the
    latest successful attempt". `result_id` is written once, atomically, by
    `ResearchResultStore.finalize_attempt` at the time this attempt
    originally succeeded, and a terminal attempt can never be reopened or
    refinalized -- so `result_id` is durable, external authority, independent
    of whatever the `economic_walk_forward.json` file on disk says about
    itself today. A trial with zero or more-than-one succeeded attempt
    matching `economic_eval_id` fails closed (ambiguity is never resolved by
    guessing)."""
    trial = store.get_trial(trial_id)  # raises KeyError if unknown
    matching = [
        a
        for a in store.list_attempts(trial_id)
        if a["status"] == "succeeded" and a["result_id"] == economic_eval_id
    ]
    if not matching:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r} (strategy_id={trial['strategy_id']!r}) has no succeeded "
            f"attempt whose registered result_id equals economic_eval_id {economic_eval_id!r} "
            "-- refusing to guess which attempt to replay"
        )
    if len(matching) > 1:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r} has {len(matching)} succeeded attempts registered under "
            f"the SAME economic_eval_id {economic_eval_id!r} -- ambiguous, refusing to guess "
            "which one to replay"
        )
    attempt = matching[0]
    artifact_paths = json.loads(attempt["artifact_paths_json"] or "{}")
    economic_path_str = artifact_paths.get("economic_walk_forward")
    if not economic_path_str:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r}'s matching succeeded attempt has no recorded "
            "'economic_walk_forward' artifact path"
        )
    economic_path = Path(economic_path_str)
    if not economic_path.exists():
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r}'s recorded economic_walk_forward.json no longer exists "
            f"at {economic_path}"
        )
    return economic_path
