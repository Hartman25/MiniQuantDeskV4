from __future__ import annotations

from pathlib import Path

from mqk_research.ml.oos_replay_bundle import build_replay_bundle


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

    manifest_path = build_replay_bundle(
        Path(args.registry_db),
        trial_id=args.trial_id,
        economic_eval_id=args.economic_eval_id,
        out_dir=Path(args.out_dir),
        excluded_symbols=excluded_symbols,
    )
    print(f"OK bundle={manifest_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
