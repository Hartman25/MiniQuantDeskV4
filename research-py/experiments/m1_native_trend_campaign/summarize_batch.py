"""Read-only 15-row evidence table for batch 01 (no stage re-runs, no thresholds invented).

Reads: trials_index.json, judge/judge.json, per-trial economic aggregate, Rust backtest
metrics/robustness/stress artifacts and the scanner review CSVs. Writes batch_results.json
and prints a markdown table.
"""

from __future__ import annotations

import csv
import glob
import json
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
RUN = HERE / "runs" / "run_batch_01"
DECL = json.loads((HERE / "PREDECLARED_BATCH_01.json").read_text(encoding="utf-8"))
TRIALS = [(t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]]
IDX = json.loads((RUN / "trials_index.json").read_text(encoding="utf-8"))
JUDGE = json.loads((RUN / "judge" / "judge.json").read_text(encoding="utf-8"))
DSR = {r["trial_id"]: r for r in JUDGE["dsr_results"]}
EXCLUDED = {r["trial_id"]: r["reason"] for r in JUDGE["excluded_trial_ids"]}


def one(pattern: str) -> Path:
    hits = glob.glob(str(pattern))
    assert len(hits) == 1, (pattern, hits)
    return Path(hits[0])


def profitable_months(daily_csv: Path) -> float:
    df = pd.read_csv(daily_csv)
    df["m"] = pd.to_datetime(df["date"]).dt.to_period("M")
    monthly = df.groupby("m")["net_daily_return"].apply(lambda s: (1.0 + s).prod() - 1.0)
    return float((monthly > 0).mean())


def review_rows() -> dict:
    out = {}
    for strategy in {s for s, _ in TRIALS}:
        path = one(RUN / "scan" / strategy / "reviews" / "*" / "review_decisions.csv")
        with open(path, newline="", encoding="utf-8") as fh:
            for row in csv.DictReader(fh):
                out[(strategy, row["symbol"])] = row
    return out


def main() -> None:
    reviews = review_rows()
    rows = []
    for strategy, sym in TRIALS:
        rec = IDX[f"{strategy}/{sym}"]
        econ = json.loads(Path(rec["economic_path"]).read_text(encoding="utf-8"))
        agg = econ["aggregate"]
        daily = Path(econ["outputs"]["economic_daily_returns_csv"]["path"])
        bt = RUN / "backtest" / strategy / sym
        metrics = json.loads(one(bt / "*" / "metrics.json").read_text(encoding="utf-8"))
        gaunt = json.loads(one(bt / "*" / "robustness_gauntlet.json").read_text(encoding="utf-8"))
        stress = json.loads(one(bt / "*" / "stress_suite.json").read_text(encoding="utf-8"))
        bench = metrics.get("benchmark") or {}
        d = DSR.get(rec["trial_id"], {})
        rev = reviews[(strategy, sym)]
        failed_scen = [s["name"] for s in gaunt["scenarios"] if s.get("applicable", True) and not s.get("passed", False)]
        na_scen = [s["name"] for s in gaunt["scenarios"] if not s.get("applicable", True)]
        rows.append({
            "strategy": strategy, "symbol": sym, "trial_id": rec["trial_id"], "attempt_index": rec["attempt_index"],
            "semantic_fingerprint": rec["semantic_fingerprint"],
            "net_return": agg["net_total_return"], "gross_return": agg["gross_total_return"],
            "cost_drag": agg["cost_drag"], "sharpe": agg["net_sharpe"], "cagr": agg["annualized_net_return"],
            "max_drawdown": agg["max_drawdown"], "profitable_months": profitable_months(daily),
            "active_days": agg["active_days"], "turnover": agg["total_turnover"],
            "profitable_folds": f'{agg["profitable_fold_count"]}/{agg["folds_used"]}',
            "position_agreement": rec["execution_fidelity"],
            "dsr": d.get("deflated_sharpe_ratio"), "judge_status": "excluded:" + EXCLUDED[rec["trial_id"]] if rec["trial_id"] in EXCLUDED else "included",
            "rust_total_return_pct": metrics["total_return_pct"], "rust_trade_count": metrics["trade_count"],
            "rust_profit_factor": metrics["profit_factor"], "rust_exposure_time_pct": metrics.get("exposure_time_pct"),
            "rust_max_drawdown_pct": metrics["max_drawdown_pct"],
            "benchmark": bench,
            "robustness_failed": failed_scen, "robustness_not_applicable": na_scen,
            "stress_failed": [s["name"] for s in stress["scenarios"] if not s.get("passed", False)],
            "review_state": rev["review_state"], "reason_codes": rev["reason_codes"],
        })
    (RUN / "batch_results.json").write_text(json.dumps(rows, indent=1, sort_keys=True), encoding="utf-8")
    print("| # | strategy | sym | net | gross | Sharpe | DSR | CAGR | maxDD | PF(rust) | prof.mo | agree | trades | cost drag | judge | review | reasons |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    def f(v, spec=".3f"):
        return "n/a" if v is None else format(v, spec)

    for i, r in enumerate(rows, 1):
        print(f'| {i} | {r["strategy"]} | {r["symbol"]} | {f(r["net_return"])} | {f(r["gross_return"])} | {f(r["sharpe"], ".2f")} | '
              f'{f(r["dsr"])} | {f(r["cagr"])} | {f(r["max_drawdown"])} | {f(r["rust_profit_factor"], ".2f")} | '
              f'{f(r["profitable_months"], ".2f")} | {f(r["position_agreement"])} | {r["rust_trade_count"]} | {f(r["cost_drag"])} | '
              f'{r["judge_status"]} | {r["review_state"]} | {r["reason_codes"]} |')

if __name__ == "__main__":
    main()
