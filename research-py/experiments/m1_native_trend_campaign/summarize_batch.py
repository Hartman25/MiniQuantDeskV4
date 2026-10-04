"""Read-only 15-row evidence table for a batch (no stage re-runs, no thresholds invented).
The declaration is named by MQK_M1_BATCH_DECLARATION (default PREDECLARED_BATCH_01.json).

Reads: trials_index.json, judge/judge.json, per-trial economic aggregate, Rust backtest
metrics/robustness/stress artifacts and the scanner review CSVs. Writes batch_results.json
and prints a markdown table.
"""

from __future__ import annotations

import glob
import json
import os
import sys
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from select_batch import REQUIRED_ROBUSTNESS_SCENARIOS  # noqa: E402
DECL = json.loads((HERE / os.environ.get("MQK_M1_BATCH_DECLARATION", "PREDECLARED_BATCH_01.json")).read_text(encoding="utf-8"))
RUN = HERE / DECL["run_dir"]
TRIALS = [(t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]]
IDX = json.loads((RUN / "trials_index.json").read_text(encoding="utf-8"))
JUDGE = json.loads((RUN / "judge" / "judge.json").read_text(encoding="utf-8"))
DSR = {r["trial_id"]: r for r in JUDGE["dsr_results"]}
EXCLUDED = {r["trial_id"]: r["reason"] for r in JUDGE["excluded_trial_ids"]}
CAPITAL_FRACTION = DECL["scanner_review"].get("benchmark_policy") == "capital_fraction_matched_passive_buy_hold_v1"


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
    """Review rows from review_decisions.json (carries the Benchmark V2 evidence when present)."""
    out = {}
    for strategy in {s for s, _ in TRIALS}:
        path = one(RUN / "scan" / strategy / "reviews" / "*" / "review_decisions.json")
        for row in json.loads(path.read_text(encoding="utf-8")):
            out[(strategy, row["symbol"])] = {
                "review_state": row["review_state"], "reason_codes": ";".join(row["reason_codes"]),
                "blockers": ";".join(row["blockers"]), "benchmark_v2": row.get("benchmark_v2"),
                "benchmark_capital_fraction": row.get("benchmark_capital_fraction")}
    return out


def main() -> None:
    reviews = review_rows()
    rows = []
    for strategy, sym in TRIALS:
        rec = IDX[f"{strategy}/{sym}"]
        if "failed" in rec:  # an economic failure stays in the population, finalized failed, never retried
            rows.append({
                "strategy": strategy, "symbol": sym, "trial_id": rec["trial_id"], "economic_failed": rec["failed"],
                "semantic_fingerprint": rec["semantic_fingerprint"], "attempt_index": None,
                "judge_status": "excluded:" + EXCLUDED[rec["trial_id"]] if rec["trial_id"] in EXCLUDED else "not_in_judge",
                "dsr": DSR.get(rec["trial_id"], {}).get("deflated_sharpe_ratio"),
                "robustness_failed": ["economic_failed"], "robustness_not_applicable": [], "stress_failed": [],
                "review_state": reviews[(strategy, sym)]["review_state"],
                "reason_codes": reviews[(strategy, sym)]["reason_codes"],
                "review_blockers": reviews[(strategy, sym)]["blockers"], "benchmark_evidence": None})
            continue
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
        # The review alpha is judged against exactly ONE benchmark class per declaration; the other
        # class's evidence can never stand in for it.
        if CAPITAL_FRACTION and rev["benchmark_v2"] is not None:
            raise SystemExit(f"fail-closed: {strategy}/{sym} carries Benchmark V2 evidence in a capital-fraction batch")
        if not CAPITAL_FRACTION and rev["benchmark_capital_fraction"] is not None:
            raise SystemExit(f"fail-closed: {strategy}/{sym} carries capital-fraction evidence in a fixed-quantity batch")
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
            "robustness_missing": sorted(set(REQUIRED_ROBUSTNESS_SCENARIOS) - {x["name"] for x in gaunt["scenarios"]})
            + (["deferred_scenarios_present"] if gaunt.get("deferred") else []),
            "stress_failed": [s["name"] for s in stress["scenarios"] if not s.get("passed", False)],
            "review_state": rev["review_state"], "reason_codes": rev["reason_codes"], "review_blockers": rev["blockers"],
            "benchmark_v2": rev["benchmark_v2"], "benchmark_capital_fraction": rev["benchmark_capital_fraction"],
            "benchmark_evidence": rev["benchmark_capital_fraction"] if CAPITAL_FRACTION else rev["benchmark_v2"],
        })
    (RUN / "batch_results.json").write_text(json.dumps(rows, indent=1, sort_keys=True), encoding="utf-8")
    print("| # | strategy | sym | net | gross | Sharpe | DSR | CAGR | maxDD | PF(rust) | prof.mo | agree | trades | cost drag | qty | V2 ret% | alpha V2% | legacy bench% (info) | judge | review | reasons |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    def f(v, spec=".3f"):
        return "n/a" if v is None else format(v, spec)

    def v2(r, field):
        b = r.get("benchmark_evidence")
        return None if not b else b.get(field)

    def qty_micros(r):
        b = r.get("benchmark_evidence")
        return "n/a" if not b else b["candidate_target_qty_micros"] / 1e6

    for i, r in enumerate(rows, 1):
        if r.get("economic_failed"):
            print(f'| {i} | {r["strategy"]} | {r["symbol"]} | FAILED ATTEMPT: {r["economic_failed"][:90]} | | | | | | | | | | | | | | | {r["judge_status"]} | {r["review_state"]} | {r["reason_codes"]} |')
            continue
        print(f'| {i} | {r["strategy"]} | {r["symbol"]} | {f(r["net_return"])} | {f(r["gross_return"])} | {f(r["sharpe"], ".2f")} | '
              f'{f(r["dsr"])} | {f(r["cagr"])} | {f(r["max_drawdown"])} | {f(r["rust_profit_factor"], ".2f")} | '
              f'{f(r["profitable_months"], ".2f")} | {f(r["position_agreement"])} | {r["rust_trade_count"]} | {f(r["cost_drag"])} | '
              f'{qty_micros(r)} | {f(v2(r, "benchmark_account_return_pct"))} | {f(v2(r, "alpha_pct"))} | '
              f'{f(v2(r, "legacy_buy_and_hold_return_pct"))} | '
              f'{r["judge_status"]} | {r["review_state"]} | {r["reason_codes"]} |')

if __name__ == "__main__":
    main()
