"""Census-02 command line. `run` is the ONLY real-data entrance and its first load-bearing operation is
c2_protocol.require_freeze. `write-policy` / `freeze` read no data. Importing this module performs no I/O."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_policy as pol  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_runner as rn  # noqa: E402

POLICY_FILE = HERE / "CENSUS02_OPERATOR_POLICY.json"


def _dump(path: Path, obj: dict) -> None:
    path.write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def cmd_write_policy(_a) -> None:
    _dump(POLICY_FILE, pol.policy_document())
    print(f"wrote {POLICY_FILE.name}")


def cmd_freeze(_a) -> None:
    """Write the FROZEN_BY_OPERATOR predeclaration for the current clean, committed behavior state. Read-only on data."""
    if pr.PREDECLARATION_FILE.exists():
        raise SystemExit(f"{pr.PREDECLARATION_FILE.name} already exists; a freeze is immutable")
    if json.loads(POLICY_FILE.read_text(encoding="utf-8")) != pol.policy_document():
        raise SystemExit("CENSUS02_OPERATOR_POLICY.json differs from the approved policy in code")
    doc = pr.build_predeclaration()
    _dump(pr.PREDECLARATION_FILE, doc)
    print({"protocol_id": doc["protocol_id"], "behavior_head": doc["behavior_head"],
           "strategy_trials": doc["strategy_population"]["trial_count"], "factors": doc["factor_population"]["factor_count"]})


def cmd_run(a) -> None:
    print(rn.run_campaign(max_strategy_chunks=a.max_strategy_chunks, max_factors=a.max_factors))


def main(argv=None) -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("write-policy").set_defaults(fn=cmd_write_policy)
    sub.add_parser("freeze").set_defaults(fn=cmd_freeze)
    r = sub.add_parser("run")
    r.add_argument("--max-strategy-chunks", type=int, default=None)
    r.add_argument("--max-factors", type=int, default=None)
    r.set_defaults(fn=cmd_run)
    a = ap.parse_args(argv)
    a.fn(a)


if __name__ == "__main__":
    main()
