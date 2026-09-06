from __future__ import annotations

import json
import sys
from pathlib import Path

from mqk_research.ml.oos_replay_bundle import ReplayBundleError, build_replay_bundle
from mqk_research.ml.util_hash import file_record


def main(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(
        prog="mqk-ml-oos-replay-bundle",
        description=(
            "Build a hash-authenticated Wave06 Research OOS replay bundle (baseline + "
            "per-symbol leave-one-out signal-time target-quantity schedules) for a single, "
            "exactly-resolved registered trial/attempt."
        ),
    )
    ap.add_argument("--registry-db", required=True)
    ap.add_argument("--trial-id", required=True)
    ap.add_argument("--economic-eval-id", required=True, help="Required -- never 'latest'")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument(
        "--excluded-symbols",
        default=None,
        help=(
            "Optional comma-separated symbol list restricting which leave-one-out schedules "
            "are produced. Omit to enumerate every symbol in the trial's own authenticated "
            "bars_provenance symbol_universe."
        ),
    )
    args = ap.parse_args(argv)

    excluded_symbols = None
    if args.excluded_symbols:
        excluded_symbols = [s.strip() for s in args.excluded_symbols.split(",") if s.strip()]

    # R1.5 -- machine-readable JSON is the authority seam for callers (the
    # Rust R3 CLI in particular): a human "OK bundle=..." line is never
    # sufficient authority on its own.
    try:
        manifest_path = build_replay_bundle(
            Path(args.registry_db),
            trial_id=args.trial_id,
            economic_eval_id=args.economic_eval_id,
            out_dir=Path(args.out_dir),
            excluded_symbols=excluded_symbols,
        )
    except ReplayBundleError as exc:
        json.dump({"status": "error", "reason": str(exc)}, sys.stdout)
        return 1

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest_sha256 = file_record(manifest_path)["sha256"]
    result = {
        "status": "ok",
        "manifest_path": str(manifest_path),
        "manifest_sha256": manifest_sha256,
        "trial_id": manifest["lineage"]["trial_id"],
        "strategy_id": manifest["lineage"]["strategy_id"],
        "economic_eval_id": manifest["lineage"]["economic_eval_id"],
    }
    json.dump(result, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
