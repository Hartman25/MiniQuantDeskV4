from __future__ import annotations

from typing import Any, Dict, Tuple

from .hashing import short_hash
from .models import JobSpec

# Identifies the semantics of "what a distributed-sweep candidate trial is" —
# a param/universe/dataset/timeframe combination, EXCLUDING the evaluation
# window. A window is an evaluation slice of a candidate, not a new candidate
# (RESEARCH-EXPERIMENT-REGISTRY-01 mission: window overcounting is the primary
# statistical defect this patch must not introduce).
PROTOCOL_ID = "exp-distributed-candidate-v1"


def build_candidate_trial_identity(
    job: JobSpec,
    *,
    hypothesis_id: str,
    experiment_id: str,
) -> Tuple[str, Dict[str, Any]]:
    """Derive the (trial_id, identity) pair for the candidate a JobSpec belongs
    to. Deliberately excludes job.window and job.job_index/job_id — those
    identify an evaluation slice/execution, not the statistical candidate."""
    normalized_universe = sorted(dict.fromkeys(symbol.strip().upper() for symbol in job.symbols if symbol.strip()))
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": job.strategy_id,
        "protocol_id": PROTOCOL_ID,
        "params": job.params,
        "universe": normalized_universe,
        "timeframe": job.timeframe,
        "data_identity": {
            "dataset_sha256": job.dataset_fingerprint.dataset_sha256,
        },
    }
    trial_id = short_hash(identity, length=32)
    return trial_id, identity
