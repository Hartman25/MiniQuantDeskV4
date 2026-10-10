"""Holdout guard: the final holdout stays RESERVED / UNCONSUMED and no holdout-period row entered
any evaluation artifact of the batch.

Run `pre` before the first attempt and `post` after the last stage. It reads artifacts only (never the
provider), derives the holdout start the same way the bridge does, and fails closed on any ledger row that
is not `reserved`, any artifact whose latest timestamp reaches the holdout start, and (post) any category
with no artifact to check -- a guard that checked nothing proves nothing.
"""

from __future__ import annotations

import glob
import json
import os
import sqlite3
import sys
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "src"))
sys.path.insert(0, str(HERE))
import holdout_incident  # noqa: E402

# (category, glob under the run dir, timestamp column, kind)
CATEGORIES = (
    ("backtest_and_scanner_input_bars", "trials/*/*/bt_bars.csv", "end_ts", "epoch"),
    ("scanner_and_benchmark_bars", "scan/*/bars/1D/*_1D.csv", "end_ts", "epoch"),
    ("native_signal_generation", "trials/*/*/emit/native_signals.csv", "decision_ts", "epoch"),
    ("stress_signal_generation", "stress/*/*/emit/native_signals.csv", "decision_ts", "epoch"),
    ("research_folds_economic", "trials/*/*/run/eval/economic_returns.csv", "timestamp", "iso"),
    ("judge_inputs_daily", "trials/*/*/run/eval/economic_daily_returns.csv", "date", "iso"),
    ("backtest_equity", "backtest/*/*/*/equity_curve.csv", "ts_utc", "epoch"),
    ("backtest_fills", "backtest/*/*/*/fills.csv", "ts_utc", "epoch"),
    ("backtest_orders", "backtest/*/*/*/orders.csv", "ts_utc", "epoch"),
    ("robustness_stress", "stress/*/*/eval/economic_returns.csv", "timestamp", "iso"),
    ("placebo", "placebo/*/*/eval/economic_returns.csv", "timestamp", "iso"),
)


FIXED_PARTITION_CATEGORY = ("research_bars_fetch", "data/research_bars.csv", "end_ts", "iso")


class HoldoutBreach(Exception):
    pass


def _latest(path: Path, column: str, kind: str) -> pd.Timestamp:
    values = pd.read_csv(path, usecols=[column])[column]
    if kind == "epoch":
        ts = pd.to_datetime(values.astype("int64"), unit="s", utc=True)
    else:
        ts = pd.to_datetime(values, utc=True)
    return ts.max()


def holdout_start(decl: dict, run: Path) -> pd.Timestamp:
    from mqk_research.ml.native_signal_registry_integration import NativeSignalError, native_holdout_start
    bars = run / "data" / "research_bars.csv"
    boundary = decl["partition"].get("holdout_boundary")  # declared fixed partition; refuses bars that reach it
    try:
        starts = {native_holdout_start(bars, sym, decl["partition"]["holdout_months"], boundary)
                  for sym in decl["universe"]["symbols"]}
    except NativeSignalError as exc:
        raise HoldoutBreach(str(exc)) from exc
    if len(starts) != 1:
        raise HoldoutBreach(f"symbols derive different holdout starts: {sorted(map(str, starts))}")
    return starts.pop()


def ledger_state(registry: Path) -> list[tuple]:
    if not registry.exists():
        return []
    con = sqlite3.connect(f"file:{registry}?mode=ro", uri=True)
    try:
        return con.execute("select holdout_id, status, consumed_at, consumer_identity_json "
                           "from research_holdout_ledger order by holdout_id").fetchall()
    finally:
        con.close()


def check(decl: dict, run: Path, registry: Path, phase: str) -> dict:
    start = holdout_start(decl, run)
    ledger = ledger_state(registry)
    for holdout_id, status, consumed_at, consumer in ledger:
        if status != "reserved" or consumed_at is not None or consumer is not None:
            raise HoldoutBreach(f"holdout {holdout_id} is not RESERVED/UNCONSUMED: {status} {consumed_at}")
    # A clean per-run ledger and clean artifacts show only what THIS run recorded; they never certify the
    # window against access by other processes, so the incident ledger's truth rides in every report.
    report = {"phase": phase, "holdout_start_utc": start.isoformat(), "ledger_rows": len(ledger),
              "ledger_all_reserved": True, "categories": {},
              "access_incident": holdout_incident.truth_summary(decl)}
    # Under a fixed partition the fetch itself is bounded, so the research bars are a checked category too.
    categories = CATEGORIES + ((FIXED_PARTITION_CATEGORY,) if decl["partition"].get("holdout_boundary") else ())
    for name, pattern, column, kind in categories:
        files = sorted(glob.glob(str(run / pattern)))
        latest = None
        for f in files:
            end = _latest(Path(f), column, kind)
            if end >= start:
                raise HoldoutBreach(f"{name}: {f} reaches {end.isoformat()} >= holdout start {start.isoformat()}")
            latest = end if latest is None or end > latest else latest
        if phase == "post" and not files:
            raise HoldoutBreach(f"{name}: no artifact found to check")
        report["categories"][name] = {"files": len(files), "latest_utc": latest.isoformat() if latest is not None else None}
    if phase == "post" and not ledger:
        raise HoldoutBreach("no holdout ledger row: the reservation was never recorded")
    return report


def main() -> None:
    phase = sys.argv[1] if len(sys.argv) > 1 else "post"
    if phase not in ("pre", "post"):
        raise SystemExit("usage: holdout_guard.py pre|post")
    decl_name = os.environ["MQK_M1_BATCH_DECLARATION"]
    decl = json.loads((HERE / decl_name).read_text(encoding="utf-8"))
    run = HERE / decl["run_dir"]
    registry = HERE / decl["experiment"]["registry_db_relative_path"]
    report = check(decl, run, registry, phase)
    (run / f"holdout_guard_{phase}.json").write_text(json.dumps(report, indent=1, sort_keys=True), encoding="utf-8")
    print(json.dumps(report, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
