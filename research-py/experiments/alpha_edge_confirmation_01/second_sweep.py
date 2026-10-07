"""Second adversarial sweep: re-derives the Confirmation results from the committed result files, the durable attempt store and
the acquired bars WITHOUT using c1_eval's estimator, decision rule or BH helper. Read-only; writes the sweep report."""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c1_cohort as cc  # noqa: E402
import c1_eval as ce  # noqa: E402
import c1_protocol as cp  # noqa: E402
import c1_runner as cr  # noqa: E402

RES = HERE / "results"
FENCE = pd.Timestamp("2026-03-01", tz="UTC")
START = pd.Timestamp("2025-01-01", tz="UTC")


def jl(name):
    return [json.loads(x) for x in (RES / name).read_text(encoding="utf-8").splitlines() if x.strip()]


def bh(p: dict, alpha: float) -> dict:
    ids = sorted(p, key=lambda k: (p[k], k))
    m = len(ids)
    q, run = {}, 1.0
    for rank in range(m, 0, -1):
        run = min(run, p[ids[rank - 1]] * m / rank)
        q[ids[rank - 1]] = run
    return q


def decide(events, eff, p, q):
    if events < 30:
        return "NOT_EVALUABLE_IN_CONFIRMATION"
    if eff <= 0:
        return "NOT_CONFIRMED"
    return "CONFIRMED_STRONG" if (p <= 0.10 and q <= 0.10) else "CONFIRMED_DIRECTIONAL_ONLY"


def main() -> None:
    findings = []

    def check(fid, what, ok, detail, how="ALREADY CORRECT + PROVEN"):
        findings.append({"id": fid, "check": what, "result": how if ok else "DEFECT", "detail": detail})
        if not ok:
            raise SystemExit(f"second sweep DEFECT {fid}: {what}: {detail}")

    freeze = cc.require_freeze()
    manifest = freeze["manifest"]
    rows = jl("confirmation_factor_ledger.jsonl")
    ids = sorted(c["factor_id"] for c in manifest["candidates"])
    check("S01", "denominator is exactly the committed 18 and the ledger has one row per factor",
          sorted(r["factor_id"] for r in rows) == ids and len(ids) == 18 == len(set(ids)), f"{len(rows)} rows")
    fdr = json.loads((RES / "confirmation_fdr_report.json").read_text(encoding="utf-8"))
    check("S02", "FDR family is the complete 18 incl. non-evaluables, alpha 0.10",
          fdr["hypothesis_count"] == 18 and set(fdr["p_values"]) == set(ids) and fdr["alpha"] == 0.10, "18/18")
    p = {r["factor_id"]: (r["p_value"] if r["evaluable"] else 1.0) for r in rows}
    q = bh(p, 0.10)
    check("S03", "independent BH q-values equal the reported q-values (|diff| < 1e-12)",
          all(abs(q[i] - fdr["q_values"][i]) < 1e-12 and abs(q[i] - next(r for r in rows if r["factor_id"] == i)["q_value"]) < 1e-12
              for i in ids), "recomputed")
    bad = [r["factor_id"] for r in rows if r["evaluable"] and abs(r["p_value"] - (r["null"]["exceed_count"] + 1) / (r["null"]["n_permutations"] + 1)) > 1e-12]
    check("S04", "p = (exceed+1)/(n+1) with n=200, seed 0", not bad and all(r["null"]["n_permutations"] == 200 and r["null"]["base_seed"] == 0
                                                                            for r in rows if r["evaluable"]), "all evaluable rows")
    mism = [r["factor_id"] for r in rows if r["status"] != decide(r["events"], r["effect"], r["p_value"], q[r["factor_id"]])]
    check("S05", "independent decision table reproduces every disposition", not mism, f"mismatches={mism}")
    check("S06", "no STRONG row lacks events>=30, effect>0, p<=0.10 and q<=0.10; STRONG count recomputed",
          all((r["status"] == "CONFIRMED_STRONG") == (r["events"] >= 30 and r["effect"] > 0 and r["p_value"] <= 0.10 and q[r["factor_id"]] <= 0.10)
              for r in rows), f"strong={sum(r['status'] == 'CONFIRMED_STRONG' for r in rows)}")
    check("S07", "no 2024 scored row and no label endpoint at/after the fence in any factor",
          all(pd.Timestamp(r["scored_bounds"]["min_period"]) >= START and pd.Timestamp(r["scored_bounds"]["max_label_end"]) < FENCE
              for r in rows if r["scored_bounds"]["rows"]), "bounds per factor")
    cons = json.loads((RES / "confirmation_consumption_proof.json").read_text(encoding="utf-8"))
    check("S08", "no final-holdout row read/scored; provider max < fence; nothing at/after the fence loaded",
          cons["final_holdout_rows_read"] == 0 and cons["final_holdout_rows_scored"] == 0 and cons["rows_at_or_after_fence_loaded"] == 0
          and pd.Timestamp(cons["provider_raw_max_t"]) < FENCE and cons["CONFIRMATION_RESERVE_CONSUMED"] is True, cons["provider_raw_max_t"])
    check("S09", "every row carries NOT_VALIDATED / NONE / EXECUTABLE_PNL=false",
          all(r["VALIDATION_STATUS"] == "NOT_VALIDATED" and r["PROMOTION_AUTHORITY"] == "NONE" and r["EXECUTABLE_PNL"] is False for r in rows), "18 rows")

    store = cr.ResearchResultStore(cr.STORE_DB)
    att = {c["factor_id"]: store.list_attempts(cc.trial_id_of(c["evaluation_id"])) for c in manifest["candidates"]}
    check("S10", "exactly one succeeded attempt per factor, zero failed/started/extra (second run created nothing)",
          all(len(a) == 1 and a[0]["status"] == "succeeded" for a in att.values()), f"attempts={sum(len(a) for a in att.values())}")
    check("S11", "ledger evaluation_ids equal the stored succeeded records and the manifest (no foreign ids)",
          all(json.loads(att[r["factor_id"]][0]["result_summary_json"])["evaluation_id"] == r["evaluation_id"]
              == next(c["evaluation_id"] for c in manifest["candidates"] if c["factor_id"] == r["factor_id"]) for r in rows), "18/18")
    ledger = jl("confirmation_attempt_ledger.jsonl")
    check("S12", "attempt ledger file lists exactly the 18 attempts", len(ledger) == 18, f"{len(ledger)} rows")

    U, info = cr.load_universe()
    check("S13", "universe: 87 eligible, the single exclusion is typed, SPY present, nothing at/after the fence",
          info["eligible"] == 87 and info["universe_count"] == 88 and info["rows_at_or_after_fence"] == 0 and "SPY" in U.sd,
          json.dumps(info["excluded_by_disposition"]))
    worst_eff = worst_ev = 0.0
    n_rows = 0
    for c in manifest["candidates"]:
        r = next(x for x in rows if x["factor_id"] == c["factor_id"])
        h = c["horizon"]
        ev_sum, ev_n = 0.0, 0
        for sym in U.symbols:
            sd = U.sd[sym]
            sig = ce.cn.condition_sig(U, sym, c)
            if sig is None:
                continue
            first = int(np.searchsorted(sd.dates, np.datetime64("2025-01-01", "D"), side="left"))
            idx = np.arange(max(sig.s, first), sd.n - h)
            if idx.size:
                idx = idx[sd.dates[idx + h] < np.datetime64("2026-03-01", "D")]
            if not idx.size:
                continue
            ret = sd.c[idx + h] / sd.c[idx] - 1.0
            lab = ret - ret.mean()
            hit = sig.cond[idx] == 1.0
            ev_sum += float(lab[hit].sum())
            ev_n += int(hit.sum())
            n_rows += int(idx.size)
        eff = ev_sum / ev_n if ev_n else float("nan")
        worst_ev = max(worst_ev, abs(ev_n - r["events"]))
        if math.isfinite(eff):
            worst_eff = max(worst_eff, abs(eff - r["effect"]))
        check(f"S14:{c['factor_id'][:10]}", "independent event count/effect sign from raw bars",
              ev_n == r["events"] and (eff > 0) == (r["effect"] > 0) and abs(eff - r["effect"]) < 1e-9,
              f"events {ev_n}=={r['events']} effect {eff:.6g} vs {r['effect']:.6g}")
    out = {"schema_version": "alpha_edge_confirmation_second_sweep_v1", "defects": 0, "checks": len(findings),
           "max_abs_event_count_diff": worst_ev, "max_abs_effect_diff": worst_eff, "findings": findings}
    (RES / "confirmation_second_sweep.json").write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({k: v for k, v in out.items() if k != "findings"}))


if __name__ == "__main__":
    main()
