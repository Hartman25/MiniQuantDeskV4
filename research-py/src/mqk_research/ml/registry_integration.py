from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

from mqk_research.exp_distributed.hashing import short_hash
from mqk_research.exp_distributed.runner import default_db_path, default_root
from mqk_research.exp_distributed.storage import REGISTRY_SCHEMA_VERSION, ResearchResultStore
from mqk_research.ml.eval_walkforward import WalkForwardSpec, run_walkforward_eval
from mqk_research.ml.holdout_ledger import compute_holdout_id
from mqk_research.ml.util_hash import file_record

# Matches the walk-forward artifact's own "schema_version" field
# (mqk_research.ml.eval_walkforward.run_walkforward_eval). Kept as a distinct
# constant (not imported from there) because it identifies EVALUATION
# SEMANTICS for trial identity purposes, not artifact shape — the two are
# allowed to diverge in the future without this module silently following.
PROTOCOL_ID = "walk_forward_eval_v2"

__all__ = [
    "PROTOCOL_ID",
    "build_trial_identity",
    "run_registered_walkforward_eval",
]


def _content_identity(path: Path) -> Dict[str, Any]:
    """File identity by content, not filesystem location — two run_dirs with
    byte-identical features.csv are the same candidate input regardless of
    where they happen to live on disk."""
    record = file_record(path)
    return {"sha256": record["sha256"], "bytes": record["bytes"]}


def build_trial_identity(
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    features_path: Path,
    targets_path: Path,
    schema_path: Path,
    label_col: str,
    end_ts_col: str,
    spec: WalkForwardSpec,
    l2: float,
    lr: float,
    steps: int,
    standardize: bool,
    clip_z: float,
) -> Tuple[str, Dict[str, Any]]:
    """Canonical, result-independent trial identity for a registered
    walk-forward candidate. Deliberately excludes anything derived from
    evaluation output (AUC, logloss, eval_id, artifact paths)."""
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "protocol_id": PROTOCOL_ID,
        "data_identity": {
            "features_csv": _content_identity(features_path),
            "targets_csv": _content_identity(targets_path),
            "feature_schema": _content_identity(schema_path),
        },
        "evaluation_spec": {
            "label_col": label_col,
            "end_ts_col": end_ts_col,
            "train_years": spec.train_years,
            "test_months": spec.test_months,
            "step_months": spec.step_months,
            "min_rows_per_fold": spec.min_rows_per_fold,
            "purge_enabled": spec.purge_enabled,
            "label_end_ts_col": spec.label_end_ts_col,
            "embargo_seconds": spec.embargo_seconds,
            "holdout_months": spec.holdout_months,
        },
        "model_spec": {
            "l2": l2,
            "lr": lr,
            "steps": steps,
            "standardize": standardize,
            "clip_z": clip_z,
        },
    }
    trial_id = short_hash(identity, length=32)
    return trial_id, identity


def run_registered_walkforward_eval(
    run_dir: Path,
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    hypothesis_text: Optional[str] = None,
    registry_db: Optional[Path] = None,
    end_ts_col: str = "end_ts",
    label_col: str = "target",
    l2: float = 1e-3,
    lr: float = 0.05,
    steps: int = 500,
    standardize: bool = True,
    clip_z: float = 8.0,
    spec: WalkForwardSpec | None = None,
) -> Path:
    """Official, registered entry point for purged walk-forward evaluation.

    Establishes trial identity and opens a durable 'started' attempt BEFORE
    calling the unchanged, statistically pure `run_walkforward_eval`. On
    exception the attempt is finalized failed (with reason) and the original
    exception is re-raised untouched. On success the attempt is finalized
    succeeded and a `registry` linkage section is appended to the artifact.

    Dependency direction (see RESEARCH-EXPERIMENT-REGISTRY-01):
        candidate identity -> trial_id -> attempt_id -> evaluation -> eval_id
        -> attempt finalized with eval_id.
    eval_id/results never feed back into trial_id.
    """
    if not experiment_id.strip() or not hypothesis_id.strip() or not strategy_id.strip():
        raise ValueError(
            "run_registered_walkforward_eval requires non-empty experiment_id, "
            "hypothesis_id, and strategy_id"
        )

    run_dir = Path(run_dir)
    normalized_spec = (spec or WalkForwardSpec()).normalized()

    features_path = run_dir / "features.csv"
    targets_path = run_dir / "targets.csv"
    schema_path = run_dir / "feature_schema.json"
    for required_path in (features_path, targets_path, schema_path):
        if not required_path.exists():
            raise FileNotFoundError(f"Missing required registry input: {required_path}")

    store = ResearchResultStore(registry_db or default_db_path(default_root()))
    store.register_hypothesis(
        hypothesis_id=hypothesis_id, experiment_id=experiment_id, hypothesis_text=hypothesis_text
    )

    trial_id, identity = build_trial_identity(
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        features_path=features_path,
        targets_path=targets_path,
        schema_path=schema_path,
        label_col=label_col,
        end_ts_col=end_ts_col,
        spec=normalized_spec,
        l2=l2,
        lr=lr,
        steps=steps,
        standardize=standardize,
        clip_z=clip_z,
    )
    store.register_trial(
        trial_id=trial_id,
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        protocol_id=PROTOCOL_ID,
        identity=identity,
    )
    attempt_id, attempt_index = store.begin_attempt(trial_id=trial_id, origin="mqk-ml-eval-wf")

    try:
        out_path = run_walkforward_eval(
            run_dir,
            end_ts_col=end_ts_col,
            label_col=label_col,
            l2=l2,
            lr=lr,
            steps=steps,
            standardize=standardize,
            clip_z=clip_z,
            spec=normalized_spec,
        )
        out = json.loads(out_path.read_text(encoding="utf-8"))

        # RESEARCH-HOLDOUT-RESERVATION-WIRING-01: durably reserve the exact
        # holdout region this real evaluation run computed and excluded from
        # discovery, BEFORE the attempt is trusted as a successful result.
        # `dataset_identity` reuses this trial's own `identity["data_identity"]`
        # (immutable content-hash facts already established above, never a
        # result/metric value). `protocol_version` is `PROTOCOL_ID`, the
        # canonical EVALUATION SEMANTICS identity this module already uses
        # for trial identity (see the module-level constant's doc comment) --
        # deliberately NOT `out["schema_version"]`, which identifies artifact
        # layout, not evaluation semantics. The two happen to share the same
        # string today, but must not be conflated: a future artifact-schema
        # bump alone must not manufacture a new holdout region, and a future
        # semantic protocol change must invalidate one even if the artifact
        # shape is untouched. A ledger failure (e.g. a hash collision against
        # conflicting facts) must deny this evaluation fail-closed, so it
        # stays inside this same try block and is finalized failed like any
        # other evaluation error.
        holdout_id = compute_holdout_id(
            dataset_identity=identity["data_identity"],
            holdout_start_utc=out["holdout"]["start_utc"],
            holdout_end_utc=out["holdout"]["end_utc"],
            protocol_version=PROTOCOL_ID,
        )
        store.reserve_holdout(
            holdout_id=holdout_id,
            dataset_identity=identity["data_identity"],
            holdout_start_utc=out["holdout"]["start_utc"],
            holdout_end_utc=out["holdout"]["end_utc"],
            protocol_version=PROTOCOL_ID,
        )
    except Exception as exc:
        store.finalize_attempt(
            attempt_id, status="failed", failure_reason=f"{type(exc).__name__}: {exc}"
        )
        raise

    eval_id = out["ids"]["eval_id"]
    out["registry"] = {
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "trial_id": trial_id,
        "attempt_id": attempt_id,
        "attempt_index": attempt_index,
        "status": "succeeded",
        "holdout_id": holdout_id,
    }
    out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    store.finalize_attempt(
        attempt_id,
        status="succeeded",
        result_id=eval_id,
        artifact_paths={"walk_forward_eval": str(out_path)},
        result_summary=out.get("summary"),
    )
    return out_path
