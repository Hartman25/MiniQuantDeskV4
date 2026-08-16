from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

from mqk_research.data.bars_provenance import provenance_identity_fragment, require_registered_bars_provenance
from mqk_research.exp_distributed.hashing import short_hash
from mqk_research.exp_distributed.runner import default_db_path, default_root
from mqk_research.exp_distributed.storage import REGISTRY_SCHEMA_VERSION, ResearchResultStore
from mqk_research.ml.economic_walkforward import (
    PROTOCOL_ID as ECONOMIC_PROTOCOL_ID,
    EconomicWalkForwardSpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec, run_walkforward_eval
from mqk_research.ml.util_hash import file_record

# RESEARCH-ECONOMIC-WALKFORWARD-01
#
# Registered entry point for the economic protocol. Distinct trial identity
# from mqk_research.ml.registry_integration's classification-only
# walk_forward_eval_v2 candidates: this module's protocol_id is
# ECONOMIC_PROTOCOL_ID ("economic_walk_forward_v1"), and a successful
# attempt's result_id/result_summary come from the ECONOMIC evaluation, not
# AUC/logloss.
#
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01: `bars_provenance` is a
# REQUIRED argument (no default) on both build_economic_trial_identity and
# run_registered_economic_walkforward_eval -- the official registered path
# cannot forget or skip the durable bars provenance contract (see
# mqk_research.data.bars_provenance). This is the one place in the codebase
# where that contract is structurally, not just conventionally, enforced.
#
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02 (Defect 3): economic bars
# DATA IDENTITY is now carried ENTIRELY by `bars_provenance`'s
# canonical_semantic_bars_hash (via provenance_identity_fragment) -- the
# physical bars_path's own file-bytes sha256 is deliberately NOT included in
# `identity` (and therefore never participates in trial_id). Two bars CSVs
# that are byte-for-byte different only because of physical row order share
# the same canonical_semantic_bars_hash and MUST share the same trial_id;
# including the physical sha256 here would silently create a distinct
# "candidate" out of a pure formatting difference. The physical bars_path
# record remains fully preserved and auditable per ATTEMPT (not per trial)
# -- see economic_walkforward.run_economic_walkforward's own output
# artifact, `out["inputs"]["bars_csv"]` -- so no provenance is lost, it is
# just correctly scoped to evidence rather than candidate identity.

__all__ = [
    "ECONOMIC_PROTOCOL_ID",
    "build_economic_trial_identity",
    "run_registered_economic_walkforward_eval",
]


def _content_identity(path: Path) -> Dict[str, Any]:
    """File identity by content, not filesystem location."""
    record = file_record(path)
    return {"sha256": record["sha256"], "bytes": record["bytes"]}


def build_economic_trial_identity(
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    features_path: Path,
    targets_path: Path,
    schema_path: Path,
    bars_path: Path,
    label_col: str,
    end_ts_col: str,
    wf_spec: WalkForwardSpec,
    l2: float,
    lr: float,
    steps: int,
    standardize: bool,
    clip_z: float,
    economic_spec: EconomicWalkForwardSpec,
    bars_provenance: Dict[str, Any],
) -> Tuple[str, Dict[str, Any]]:
    """Canonical, result-independent trial identity for a registered economic
    walk-forward candidate. Includes everything that materially changes the
    candidate's economic meaning: classification data/spec/model AND economic
    protocol/bars-data-identity/signal-policy/cost-model/annualization AND
    (BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01) the bars provenance
    identity fragment (provider, price-adjustment convention, corporate-
    action policy/evidence, canonical semantic bars content, query window,
    symbol universe, universe mode). Deliberately excludes anything derived
    from evaluation output (AUC, logloss, returns, eval_ids, artifact paths,
    the manifest's own artifact_sha256/row_count physical-file facts) —
    changing a RESULT (or a byte-identical semantic reorder of the same
    bars) must never change a trial_id. (Defect 3) `bars_path`'s own
    physical file-bytes identity is likewise deliberately excluded from
    `identity` for the same reason -- see module docstring; the canonical
    semantic bars authority lives entirely in `bars_provenance`. `bars_path`
    itself is still required and existence-checked here (fail-closed on a
    missing file) even though its content never enters `identity`."""
    bars_path = Path(bars_path)
    if not bars_path.exists():
        raise FileNotFoundError(f"Missing required registry input: {bars_path}")
    normalized_economic_spec = economic_spec.normalized()
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "protocol_id": ECONOMIC_PROTOCOL_ID,
        "data_identity": {
            "features_csv": _content_identity(features_path),
            "targets_csv": _content_identity(targets_path),
            "feature_schema": _content_identity(schema_path),
            # (Defect 3) NOT economic_bars_csv physical sha256/bytes -- the
            # canonical semantic bars authority below is the sole economic
            # bars identity facet; see build_economic_trial_identity's and
            # this module's docstrings.
            "bars_provenance": provenance_identity_fragment(bars_provenance),
        },
        "evaluation_spec": {
            "label_col": label_col,
            "end_ts_col": end_ts_col,
            "train_years": wf_spec.train_years,
            "test_months": wf_spec.test_months,
            "step_months": wf_spec.step_months,
            "min_rows_per_fold": wf_spec.min_rows_per_fold,
            "purge_enabled": wf_spec.purge_enabled,
            "label_end_ts_col": wf_spec.label_end_ts_col,
            "embargo_seconds": wf_spec.embargo_seconds,
            "holdout_months": wf_spec.holdout_months,
        },
        "model_spec": {
            "l2": l2,
            "lr": lr,
            "steps": steps,
            "standardize": standardize,
            "clip_z": clip_z,
        },
        "economic_protocol": economic_protocol_identity(normalized_economic_spec),
    }
    trial_id = short_hash(identity, length=32)
    return trial_id, identity


def run_registered_economic_walkforward_eval(
    run_dir: Path,
    *,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    bars_csv: Path,
    economic_spec: EconomicWalkForwardSpec,
    bars_provenance: Dict[str, Any],
    hypothesis_text: Optional[str] = None,
    registry_db: Optional[Path] = None,
    end_ts_col: str = "end_ts",
    label_col: str = "target",
    l2: float = 1e-3,
    lr: float = 0.05,
    steps: int = 500,
    standardize: bool = True,
    clip_z: float = 8.0,
    wf_spec: WalkForwardSpec | None = None,
) -> Path:
    """Official, registered entry point for the economic_walk_forward_v1
    protocol.

    Dependency direction (mirrors RESEARCH-EXPERIMENT-REGISTRY-01):
        candidate identity -> trial_id -> attempt_id -> classification eval
        -> economic eval -> economic_eval_id -> attempt finalized with
        economic_eval_id / net aggregate summary.
    AUC/logloss remain available in the classification artifact but are
    secondary evidence — the ECONOMIC result is what gets registered as this
    attempt's primary result.

    `bars_provenance` (BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01) is a
    REQUIRED durable provenance manifest (see
    mqk_research.data.bars_provenance.build_bars_provenance_manifest) —
    verified fail-closed here (require_registered_bars_provenance) BEFORE
    any classification/economic evaluation runs, folded into the trial
    identity, and re-verified against the actually-loaded bars content by
    run_economic_walkforward's corporate-action preflight.
    """
    if not experiment_id.strip() or not hypothesis_id.strip() or not strategy_id.strip():
        raise ValueError(
            "run_registered_economic_walkforward_eval requires non-empty experiment_id, "
            "hypothesis_id, and strategy_id"
        )
    require_registered_bars_provenance(bars_provenance)

    run_dir = Path(run_dir)
    bars_csv = Path(bars_csv)
    normalized_wf_spec = (wf_spec or WalkForwardSpec()).normalized()
    normalized_economic_spec = economic_spec.normalized()

    features_path = run_dir / "features.csv"
    targets_path = run_dir / "targets.csv"
    schema_path = run_dir / "feature_schema.json"
    for required_path in (features_path, targets_path, schema_path, bars_csv):
        if not required_path.exists():
            raise FileNotFoundError(f"Missing required registry input: {required_path}")

    store = ResearchResultStore(registry_db or default_db_path(default_root()))
    store.register_hypothesis(
        hypothesis_id=hypothesis_id, experiment_id=experiment_id, hypothesis_text=hypothesis_text
    )

    trial_id, identity = build_economic_trial_identity(
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        features_path=features_path,
        targets_path=targets_path,
        schema_path=schema_path,
        bars_path=bars_csv,
        label_col=label_col,
        end_ts_col=end_ts_col,
        wf_spec=normalized_wf_spec,
        l2=l2,
        lr=lr,
        steps=steps,
        standardize=standardize,
        clip_z=clip_z,
        economic_spec=normalized_economic_spec,
        bars_provenance=bars_provenance,
    )
    store.register_trial(
        trial_id=trial_id,
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        protocol_id=ECONOMIC_PROTOCOL_ID,
        identity=identity,
    )
    attempt_id, attempt_index = store.begin_attempt(trial_id=trial_id, origin="mqk-ml-eval-economic-wf")

    try:
        wf_out_path = run_walkforward_eval(
            run_dir,
            end_ts_col=end_ts_col,
            label_col=label_col,
            l2=l2,
            lr=lr,
            steps=steps,
            standardize=standardize,
            clip_z=clip_z,
            spec=normalized_wf_spec,
        )
        economic_out_path = run_economic_walkforward(
            run_dir,
            bars_csv=bars_csv,
            spec=normalized_economic_spec,
            walk_forward_eval_path=wf_out_path,
            provenance_manifest=bars_provenance,
        )
    except Exception as exc:
        store.finalize_attempt(
            attempt_id, status="failed", failure_reason=f"{type(exc).__name__}: {exc}"
        )
        raise

    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))
    economic_eval_id = economic_out["ids"]["economic_eval_id"]
    economic_out["registry"] = {
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "strategy_id": strategy_id,
        "trial_id": trial_id,
        "attempt_id": attempt_id,
        "attempt_index": attempt_index,
        "status": "succeeded",
    }
    economic_out_path.write_text(
        json.dumps(economic_out, sort_keys=True, separators=(",", ":")), encoding="utf-8"
    )

    store.finalize_attempt(
        attempt_id,
        status="succeeded",
        result_id=economic_eval_id,
        artifact_paths={
            "walk_forward_eval": str(wf_out_path),
            "economic_walk_forward": str(economic_out_path),
        },
        result_summary=economic_out.get("aggregate"),
    )
    return economic_out_path
