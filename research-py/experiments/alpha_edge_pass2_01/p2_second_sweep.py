"""Second adversarial sweep: re-derive every Pass-2 verdict from the persisted raw scenario numbers using independent code
(literal thresholds, brute-force grid adjacency, Pass-1 edge registry counts) and cross-check identity, denominators and
fail-open hazards. Writes results/second_sweep.json; exit code 1 on any finding.

Usage (from research-py/): python experiments/alpha_edge_pass2_01/p2_second_sweep.py"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import p2_pass1 as p1
import p2_protocol as pp
import p2_runner as pr
import census as ce  # noqa: E402
import search_space as ss  # noqa: E402
import simulate as sm  # noqa: E402

HERE = Path(__file__).resolve().parent
RES = HERE / "results"


def _strict(line: str):
    return json.loads(line, parse_constant=lambda c: (_ for _ in ()).throw(ValueError(f"non-finite {c}")))


def brute_neighbors(items: list[dict], keys=None) -> list[set]:
    """Independent adjacent-grid adjacency: j neighbours i iff same family, exactly one numeric param differs, and no
    value of that param (among items agreeing with i on every other param) lies strictly between the two."""
    out = [set() for _ in items]
    num = lambda v: isinstance(v, (int, float)) and not isinstance(v, bool)  # noqa: E731
    for i, a in enumerate(items):
        for j, b in enumerate(items):
            if i == j or a["family"] != b["family"] or set(a["params"]) != set(b["params"]):
                continue
            diff = [k for k in a["params"] if a["params"][k] != b["params"][k]]
            if len(diff) != 1 or not num(a["params"][diff[0]]):
                continue
            k = diff[0]
            same = [c["params"][k] for c in items if c["family"] == a["family"] and set(c["params"]) == set(a["params"])
                    and all(c["params"][o] == a["params"][o] for o in a["params"] if o != k)]
            lo, hi = sorted((a["params"][k], b["params"][k]))
            if not any(lo < v < hi for v in same):
                out[i].add(j)
    return out


def check_strategy(recs, edges, ledger, findings):
    mod_by_cfg_sym = {(e["config_id"], e["scope"]) for e in edges if e["kind"] == "STRATEGY_EDGE" and
                      e["edge_class"] == "DISCOVERED_MODERATE"}
    moderate_per_cfg: dict = {}
    for cid, _ in mod_by_cfg_sym:
        moderate_per_cfg[cid] = moderate_per_cfg.get(cid, 0) + 1
    cfgs = ss.build_configs()
    nb = brute_neighbors(cfgs)
    evaluable_per_ci: dict = {}
    for r in ledger:
        if r["d"] == "EVALUABLE":
            evaluable_per_ci[r["c"]] = evaluable_per_ci.get(r["c"], 0) + 1
    cfg_index = {c["config_id"]: i for i, c in enumerate(cfgs)}
    near = 0
    for r in recs:
        sc = r["scenarios"]
        fail = []
        if not sc["S2"]["trade_count"] >= 30:
            fail.append("S2")
        for g in ("S3", "S5"):
            if sc[g]["alpha_usd"] != sc[g]["net_pnl_usd"] - sc[g]["benchmark_net_pnl_usd"]:
                findings.append(f"{r['trial_id']}: {g} alpha != net - benchmark")
            if not (sc[g]["net_pnl_usd"] > 0 and sc[g]["alpha_usd"] > 0):
                fail.append(g)
        ya = {int(k): v for k, v in sc["S6"]["year_alpha_usd"].items()}
        if sorted(ya) != list(range(2016, 2024)):
            findings.append(f"{r['trial_id']}: S6 years not exactly 2016..2023")
        pos = [y for y in ya if ya[y] > 0]
        if len(pos) < 6:
            fail.append("S6")
        best = max(pos, key=lambda y: (ya[y], -y))
        if not sum(v for y, v in ya.items() if y != best) > 0:
            fail.append("S7")
        if not sc["S8"]["share"] <= 0.80:
            fail.append("S8")
        ci = cfg_index[r["config_id"]]
        n_mod = sum((cfgs[j]["config_id"], r["symbol"]) in mod_by_cfg_sym for j in nb[ci])
        s9 = sc["S9"]
        if s9["n_neighbors"] != len(nb[ci]) or (nb[ci] and s9["n_moderate_neighbors"] != n_mod):
            findings.append(f"{r['trial_id']}: S9 neighbour counts differ from brute force")
        if nb[ci] and not n_mod / len(nb[ci]) >= 0.25:
            fail.append("S9")
        if not (sc["S10"]["max_drawdown_usd"] <= 20_000.0 and sc["S10"]["worst_rolling_net_usd"] >= -6_000.0):
            fail.append("S10")
        near += abs(sc["S10"]["max_drawdown_usd"] - 20_000.0) < 1e-6 or abs(sc["S10"]["worst_rolling_net_usd"] + 6_000.0) < 1e-6
        if sc["S11"]["moderate_symbols"] != moderate_per_cfg[r["config_id"]] or sc["S11"]["evaluable_symbols"] != evaluable_per_ci[ci]:
            findings.append(f"{r['trial_id']}: S11 counts differ from the Pass-1 edge registry/ledger")
        if r["failed_gates"] != fail:
            findings.append(f"{r['trial_id']}: failed_gates {r['failed_gates']} != re-derived {fail}")
        if r["verdict"] != (pp.VERDICTS_STRATEGY[1] if fail else pp.VERDICTS_STRATEGY[0]):
            findings.append(f"{r['trial_id']}: verdict differs from re-derivation")
    return {"records": len(recs), "near_boundary_s10_records": near}


def check_conditional(recs, edges, fledger, findings):
    conds = ss.build_conditions(ss.build_configs())
    nb = brute_neighbors(conds)
    cidx = {c["condition_id"]: i for i, c in enumerate(conds)}
    led = {(r["condition_id"], r["horizon"]): r for r in fledger}
    by_f = {r["factor_id"]: r for r in fledger}
    strong = {(cidx[r["condition_id"]], r["horizon"]) for r in fledger if r["class"] == "DISCOVERED_STRONG"}
    for r in recs:
        sc = r["scenarios"]
        fail = []
        if sc["C1"]["status"] != "PASS" or sc["C1"]["q_value"] != by_f[r["factor_id"]]["q_value"] or \
                sc["C1"]["p_value"] != by_f[r["factor_id"]]["p_value"] or \
                sc["C1"]["direction_adjusted_effect"] != by_f[r["factor_id"]]["effect"]:
            findings.append(f"{r['factor_id']}: C1 evidence differs from the accepted factor ledger")
        py = sc["C2"]["per_year"]
        if sorted(int(k) for k in py) != list(range(2016, 2024)):
            findings.append(f"{r['factor_id']}: C2 years not exactly 2016..2023")
        if sum(1 for v in py.values() if v["effect"] is not None and v["effect"] > 0) < 6:
            fail.append("C2")
        c3 = sc["C3"]
        if not (c3["remaining_effect"] is not None and c3["remaining_effect"] > 0 and c3["remaining_events"] >= 30):
            fail.append("C3")
        if not (sc["C4"]["symbols_represented"] > 1 and sc["C4"]["top_symbol_event_share"] <= 0.50):
            fail.append("C4")
        slices = sc["C5"]["per_symbol"]
        if len(slices) != sc["C4"]["symbols_represented"]:
            findings.append(f"{r['factor_id']}: a leave-one-symbol-out slice was dropped")
        if not all(v["effect"] is not None and v["effect"] > 0 and v["remaining_events"] >= 30 for v in slices.values()):
            fail.append("C5")
        ci = cidx[r["condition_id"]]
        if nb[ci]:
            n_s = sum((j, r["horizon"]) in strong for j in nb[ci])
            if sc["C6"]["n_neighbors"] != len(nb[ci]) or sc["C6"]["n_strong_neighbors"] != n_s:
                findings.append(f"{r['factor_id']}: C6 neighbour counts differ from brute force")
            if not n_s / len(nb[ci]) >= 0.25:
                fail.append("C6")
        elif sc["C6"]["status"] != "NOT_APPLICABLE":
            findings.append(f"{r['factor_id']}: C6 without neighbours must be NOT_APPLICABLE")
        i = pp.HORIZONS.index(r["horizon"])
        adj = [h for h in (pp.HORIZONS[i - 1] if i else None, pp.HORIZONS[i + 1] if i + 1 < len(pp.HORIZONS) else None) if h]
        ev = [led[(r["condition_id"], h)] for h in adj if led[(r["condition_id"], h)]["status"] == "succeeded"]
        sup = [x for x in ev if x["effect"] is not None and x["effect"] > 0 and x["p_value"] is not None and x["p_value"] <= 0.10]
        want = "NOT_APPLICABLE" if not ev else "HORIZON_SUPPORTED" if sup else "HORIZON_ISOLATED"
        if r["horizon_class"] != want:
            findings.append(f"{r['factor_id']}: C8 class differs from re-derivation")
        if r["failed_gates"] != fail:
            findings.append(f"{r['factor_id']}: failed_gates {r['failed_gates']} != re-derived {fail}")
        if r["verdict"] != (pp.VERDICTS_CONDITIONAL[1] if fail else pp.VERDICTS_CONDITIONAL[0]):
            findings.append(f"{r['factor_id']}: verdict differs from re-derivation")
        if r["executable_pnl"] is not False or r["VALIDATION_STATUS"] != "NOT_VALIDATED" or r["PROMOTION_AUTHORITY"] != "NONE":
            findings.append(f"{r['factor_id']}: authority/executable label violated")
    return {"records": len(recs)}


def independent_stress_sample(recs, w, findings) -> int:
    """Re-run S3/S4/S5 for a fixed sample with literal cost numbers (not the protocol multipliers)."""
    U, _fence = p1.load_discovery_universe(w["uni"], w["bm"])
    n = 0
    for r in recs[::20]:
        sd, sig = U.sd[r["symbol"]], U.build(r["symbol"], r["family"], r["params"])
        s, win = sig.s, slice(sig.s + 1, None)
        for tag, comm, slip in (("S3", 20.0, 10), ("S4", 30.0, 15)):
            net = float(sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, s, commission_bps=comm, slippage_bps=slip).net[win].sum())
            bnet = float(sm.simulate(sd.hm, sd.lm, sd.cm, sm.benchmark_d(sd.n, s), s, commission_bps=comm, slippage_bps=slip).net[win].sum())
            if (net, bnet) != (r["scenarios"][tag]["net_pnl_usd"], r["scenarios"][tag]["benchmark_net_pnl_usd"]):
                findings.append(f"{r['trial_id']}: {tag} differs from an independent literal-cost re-simulation")
        d = sig.d.copy()
        shifted = d.copy()
        shifted[1:] = d[:-1]
        shifted[0] = False
        net5 = float(sm.simulate(sd.hm, sd.lm, sd.cm, shifted, s).net[win].sum())
        if net5 != r["scenarios"]["S5"]["net_pnl_usd"]:
            findings.append(f"{r['trial_id']}: S5 differs from an independent one-bar shift")
        n += 1
    return n


def main() -> int:
    findings: list[str] = []
    strat = [_strict(line) for line in open(RES / "strategy_robustness_ledger.jsonl", encoding="utf-8")]
    cond = [_strict(line) for line in open(RES / "conditional_robustness_ledger.jsonl", encoding="utf-8")]
    w = pr.load_world()
    manifest = pr.derive_manifest(w)
    if (len(strat), len(cond)) != (789, 135) or manifest != p1.load_json(pr.COHORT):
        findings.append("denominator or cohort differs from the frozen manifest")
    if [r["candidate_id"] for r in strat] != [c["candidate_id"] for c in manifest["strategy"]] or \
            [r["candidate_id"] for r in cond] != [c["candidate_id"] for c in manifest["conditional"]]:
        findings.append("ledger order/identity differs from the frozen cohort")
    mod_ids = {e["trial_id"] for e in w["edges"] if e["kind"] == "STRATEGY_EDGE" and e["edge_class"] == "DISCOVERED_MODERATE"}
    strong_ids = {e["factor_id"] for e in w["edges"] if e["kind"] == "CONDITIONAL_EDGE" and e["edge_class"] == "DISCOVERED_STRONG"}
    if {r["trial_id"] for r in strat} != mod_ids or {r["factor_id"] for r in cond} != strong_ids:
        findings.append("ledger population differs from the Pass-1 MODERATE/STRONG sets")
    s_info = check_strategy(strat, w["edges"], w["sledger"], findings)
    c_info = check_conditional(cond, w["edges"], w["fledger"], findings)
    sample = independent_stress_sample(strat, w, findings)
    code = {f.name: len(re.findall(r"TODO|FIXME|NotImplemented|xfail", f.read_text(encoding="utf-8")))
            for f in sorted(HERE.glob("p2_*.py")) if f.name != Path(__file__).name}
    if any(code.values()):
        findings.append(f"unresolved markers in code: {code}")
    out = {"schema_version": "alpha_edge_pass2_second_sweep_v1", "findings": findings, "strategy": s_info,
           "conditional": c_info, "independent_literal_cost_resimulations": sample, "code_marker_counts": code,
           "checks": ["verdict+failed_gates re-derived from raw numbers with literal thresholds",
                      "S9/C6 neighbour counts vs brute-force grid adjacency",
                      "S11 counts vs Pass-1 edge registry + search ledger",
                      "C1 p/q/effect vs accepted factor ledger", "C8 re-derivation", "C5 slices never dropped",
                      "cohort/identity/denominator vs frozen manifest and Pass-1 class sets",
                      "strict JSON (no NaN/Infinity)", "S3/S4/S5 independent re-simulation sample",
                      "TODO/FIXME/NotImplemented/xfail scan"],
           "result": "NO_FINDINGS" if not findings else "FINDINGS"}
    (RES / "second_sweep.json").write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({k: out[k] for k in ("result", "independent_literal_cost_resimulations")}), findings[:10])
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
