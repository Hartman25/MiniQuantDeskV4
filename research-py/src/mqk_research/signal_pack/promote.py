from __future__ import annotations

import json
import shutil
from datetime import datetime, timezone
from pathlib import Path

from mqk_research.contracts import (
    PROMOTED_ARTIFACT_CONTRACT_VERSION,
    SIGNAL_PACK_REQUIRED_FILES,
    PromotedArtifactLineage,
    PromotedArtifactManifest,
    derive_artifact_id,
)
from mqk_research.ml.util_hash import sha256_json
from mqk_research.registry.index import append_index
from mqk_research.signal_pack.gates import check_eval_gates


def promote_signal_pack(
    run_dir: Path,
    *,
    min_rows: int = 1,
    require_eval: bool = False,
    min_folds_used: int = 2,
    min_auc_mean: float = 0.52,
) -> Path:
    run_dir = Path(run_dir)
    sp_dir = run_dir / "signal_pack"
    sp_json = sp_dir / "signal_pack.json"
    signals_csv = sp_dir / "signals.csv"

    if not sp_json.exists() or not signals_csv.exists():
        raise FileNotFoundError("signal_pack missing; run mqk-signal-pack-export first")

    sp = json.loads(sp_json.read_text(encoding="utf-8"))
    sp_id = sha256_json(sp)

    if require_eval:
        ok, msg = check_eval_gates(
            run_dir, min_folds_used=min_folds_used, min_auc_mean=min_auc_mean
        )
        if not ok:
            raise RuntimeError(msg)

    # simple gate: require at least N signal rows
    import pandas as pd

    df = pd.read_csv(signals_csv)
    if len(df) < min_rows:
        raise RuntimeError(f"Fail-closed: too few signal rows ({len(df)} < {min_rows})")

    # promoted location: research-py/promoted/signal_packs/<id>/
    project_root = run_dir.parents[1] if len(run_dir.parents) >= 2 else run_dir
    dest = Path(project_root) / "promoted" / "signal_packs" / sp_id
    dest.mkdir(parents=True, exist_ok=True)

    # copy artifact files deterministically
    for name in ["signals.csv", "signal_pack.json"]:
        shutil.copy2(sp_dir / name, dest / name)

    # TV-01: derive canonical artifact_id and write promoted_manifest.json.
    # artifact_id is content-addressed and deterministic — same signal_pack.json
    # content always produces the same artifact_id.
    artifact_id = derive_artifact_id("signal_pack", sp_id)

    # source_dir: relative path from project_root to run_dir (posix, portable).
    try:
        source_dir_str = run_dir.relative_to(project_root).as_posix()
    except ValueError:
        source_dir_str = run_dir.as_posix()

    # data_root: posix-relative path from project_root to the promoted artifact dir.
    # Layout rule: promoted/signal_packs/<artifact_id>
    # Given artifact_id, a consumer can reconstruct this as:
    #   <project_root>/promoted/signal_packs/<artifact_id>/
    data_root_str = dest.relative_to(project_root).as_posix()

    lineage = PromotedArtifactLineage(
        signal_pack_id=sp_id,
        signal_pack_schema_version=sp.get("schema_version", "signal_pack_v1"),
        dataset_id=(sp.get("ids") or {}).get("dataset_id"),
        model_id=(sp.get("ids") or {}).get("model_id"),
        source_dir=source_dir_str,
    )

    manifest = PromotedArtifactManifest(
        schema_version=PROMOTED_ARTIFACT_CONTRACT_VERSION,
        artifact_id=artifact_id,
        artifact_type="signal_pack",
        stage="promoted",
        produced_by="research-py",
        data_root=data_root_str,
        required_files=SIGNAL_PACK_REQUIRED_FILES,
        optional_files=[],
        lineage=lineage,
        produced_at_utc=datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    )

    (dest / "promoted_manifest.json").write_text(manifest.to_json(), encoding="utf-8")

    # append index record — artifact_id is now included alongside signal_pack_id
    append_index(
        project_root,
        {
            "type": "signal_pack_promoted",
            "signal_pack_id": sp_id,
            "artifact_id": artifact_id,
            "path": str(dest),
        },
    )

    return dest


def main_promote(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(
        prog="mqk-signal-pack-promote", description="Promote a signal pack (scaffold)"
    )
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--min-rows", type=int, default=1)
    ap.add_argument("--require-eval", action="store_true")
    ap.add_argument("--min-folds-used", type=int, default=2)
    ap.add_argument("--min-auc-mean", type=float, default=0.52)
    args = ap.parse_args(argv)

    out = promote_signal_pack(
        Path(args.run_dir),
        min_rows=args.min_rows,
        require_eval=args.require_eval,
        min_folds_used=args.min_folds_used,
        min_auc_mean=args.min_auc_mean,
    )
    print(f"OK promoted_dir={out}")
    return 0
