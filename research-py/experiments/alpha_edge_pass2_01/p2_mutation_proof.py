"""Pass-2 mutation proof (P01-P22 + Q-series): each mutation must turn its load-bearing test RED; the source is then
restored byte-for-byte and the same tests are re-run GREEN.

Usage (from research-py/): python experiments/alpha_edge_pass2_01/p2_mutation_proof.py [P01 P07 ...]"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

EXP2 = Path(__file__).resolve().parent
EXP1 = EXP2.parent / "alpha_edge_census_01"
RP = EXP2.parents[1]
TESTFILE = "tests/test_alpha_edge_pass2_01.py"
THRESH = "test_protocol_thresholds or test_protocol_document"
FENCE_OLD = '        pt.require_discovery_only(ts, what=f"pass2 bars {sym}")\n'

# id -> (description, directory, file, [(old, new)], pytest -k expression)
MUTATIONS = {
    "P01": ("read a 2024 row (fence only refuses >= 2025)", EXP2, "p2_pass1.py",
            [(FENCE_OLD, '        if (ts >= pt.CONTAMINATED_END_EXCLUSIVE).any():\n            raise pt.PartitionBreach("x")\n')],
            "test_p01_p03_fence_refuses and 2024-01-02"),
    "P02": ("read 2025+ confirmation data (fence only refuses the final holdout)", EXP2, "p2_pass1.py",
            [(FENCE_OLD, '        if (ts >= pt.RESERVE_END_EXCLUSIVE).any():\n            raise pt.PartitionBreach("FINAL_HOLDOUT")\n')],
            "test_p01_p03_fence_refuses and 2025-06-02"),
    "P03": ("read final-holdout data (no fence)", EXP2, "p2_pass1.py", [(FENCE_OLD, "        pass\n")], "test_p01_p03"),
    "P04": ("change the 789 Strategy cohort (WEAK also advances)", EXP2, "p2_cohort.py",
            [('mods = [e for e in strat if e["edge_class"] == pp.PASS1_STRATEGY_CLASS]',
              'mods = [e for e in strat if e["edge_class"] in (pp.PASS1_STRATEGY_CLASS, "DISCOVERED_WEAK")]')],
            "test_strategy_cohort_is_exactly"),
    "P04b": ("the Strategy cohort count refusal is removed", EXP2, "p2_cohort.py",
             [("    if len(mods) != pp.EXPECTED_STRATEGY_COHORT:", "    if False:")], "test_strategy_cohort_is_exactly"),
    "P05": ("change the 135 Conditional cohort (MODERATE also advances)", EXP2, "p2_cohort.py",
            [('e["kind"] == "CONDITIONAL_EDGE" and e["edge_class"] == pp.PASS1_CONDITIONAL_CLASS]',
              'e["kind"] == "CONDITIONAL_EDGE" and e["edge_class"] in (pp.PASS1_CONDITIONAL_CLASS, "DISCOVERED_MODERATE")]')],
            "test_conditional_cohort_is_exactly"),
    "P05b": ("the Conditional cohort count refusal is removed", EXP2, "p2_cohort.py",
             [("    if len(strong) != pp.EXPECTED_CONDITIONAL_COHORT:", "    if False:")], "test_conditional_cohort_is_exactly"),
    "P06": ("lower the 2X cost stress to 1X", EXP2, "p2_protocol.py",
            [("STRESS_2X, STRESS_3X, DELAY_SESSIONS = 2, 3, 1", "STRESS_2X, STRESS_3X, DELAY_SESSIONS = 1, 3, 1")],
            THRESH + " or test_stress_at_multiplier or test_pass2_protocol_id_is_pinned"),
    "P07": ("net-positive but benchmark-negative 2X stress passes", EXP2, "p2_strategy.py",
            [('"status": PASS if (_finite(net, alpha) and net > 0.0 and alpha > 0.0) else FAIL, **extra,',
              '"status": PASS if (_finite(net, alpha) and net > 0.0) else FAIL, **extra,')], "test_p07"),
    "P08": ("remove the one-bar decision delay", EXP2, "p2_strategy.py",
            [("    out[sessions:] = d[:-sessions]\n", "    out[:] = d\n")], "test_p08_p18"),
    "P09": ("drop the worst year instead of the best (leave-best-year-out)", EXP2, "p2_strategy.py",
            [("best = max(positive, key=lambda y: (per[y], -y))", "best = min(per, key=lambda y: (per[y], y))")],
            "test_p09 or test_s7"),
    "P10": ("require 5 positive years instead of 6", EXP2, "p2_protocol.py",
            [("MIN_POSITIVE_YEARS = 6 ", "MIN_POSITIVE_YEARS = 5 ")], "test_p10 or " + THRESH),
    "P11": ("allow regime concentration up to 0.95", EXP2, "p2_protocol.py",
            [("MAX_REGIME_CONCENTRATION = 0.80", "MAX_REGIME_CONCENTRATION = 0.95")], "test_p11 or " + THRESH),
    "P12": ("WEAK neighbours support S9", EXP2, "p2_strategy.py",
            [('        if r["class"] == pp.PASS1_STRATEGY_CLASS:',
              '        if r["class"] in (pp.PASS1_STRATEGY_CLASS, "DISCOVERED_WEAK"):')], "test_p12"),
    "P13": ("invent off-grid S9 neighbours", EXP2, "p2_strategy.py",
            [("er.neighbor_map(configs), moderate, evaluable, mcount)",
              "[[(i + 1) % len(configs)] for i in range(len(configs))], moderate, evaluable, mcount)")], "test_p13"),
    "P14": ("execution-only params allowed in the conditional neighbourhood", EXP2, "p2_conditional.py",
            [("er.conditional_neighbor_map(conditions), strong", "er.neighbor_map(conditions), strong")], "test_p14"),
    "P15": ("allow top-symbol share up to 0.95 through C4", EXP2, "p2_protocol.py",
            [("C_MAX_TOP_SYMBOL_SHARE = 0.50", "C_MAX_TOP_SYMBOL_SHARE = 0.95")], "test_p15 or " + THRESH),
    "P16": ("drop a failing leave-one-symbol-out slice", EXP2, "p2_conditional.py",
            [('        loo[s] = {"effect": e, "remaining_events": n,\n'
              '                  "status": PASS if (_pos(e) and n >= pp.C_MIN_EVENTS) else FAIL}\n',
              '        if _pos(e) and n >= pp.C_MIN_EVENTS:\n            loo[s] = {"effect": e, "remaining_events": n, "status": PASS}\n')],
            "test_p16 or test_c4_c5"),
    "P17": ("forward-return labels become Strategy P&L", EXP2, "p2_strategy.py",
            [("    net, bench = float(so.net[w].sum()), float(bo.net[w].sum())\n",
              "    net, bench = float(np.sum(sd.c[1:] / sd.c[:-1] - 1.0)) * 1e4, float(bo.net[w].sum())\n")],
            "test_p17 or test_stress_at_multiplier"),
    "P18": ("a same-bar fill is created", EXP1, "simulate.py",
            [("    h[1:] = d[:-1]\n", "    h[:] = d\n")], "test_p08_p18"),
    "P19": ("a robustness candidate is registered as a Pass-1 Strategy trial", EXP2, "p2_runner.py",
            [('"experiment_id": pp.EXPERIMENT_ID, "hypothesis_id": HYP[c["kind"]],',
              '"experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": HYP[c["kind"]],')], "test_p19 or test_freeze_gate"),
    "P20": ("rejected candidates are deleted from the Pass-2 denominator", EXP2, "p2_runner.py",
            [("        out.append(rec)\n    return out\n",
              '        if rec["verdict"].endswith("SURVIVOR"):\n            out.append(rec)\n    return out\n')],
            "test_p20"),
    "P21": ("a completed candidate is re-run on resume", EXP2, "p2_runner.py",
            [('    succ = [a for a in atts if a["status"] == "succeeded"]\n', "    succ = []\n")],
            "test_p21 or test_partial_completion"),
    "P22": ("Pass-2 output claims VALIDATED / promotion authority", EXP2, "p2_protocol.py",
            [('LABELS = {"VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE",',
              'LABELS = {"VALIDATION_STATUS": "VALIDATED", "PROMOTION_AUTHORITY": "PAPER_CANDIDATE",')],
            "test_p22 or test_strategy_record or " + THRESH),
    "Q01": ("S1 baseline replay tolerates any metric difference", EXP2, "p2_strategy.py",
            [('if got["d"] != "EVALUABLE" or json.dumps(_jsonable(got["m"]), sort_keys=True) != json.dumps(edge_metrics, sort_keys=True):',
              'if False:')], "test_s1_replay"),
    "Q02": ("MAIN drawdown bar loosened to 25% of capital", EXP2, "p2_protocol.py",
            [("MAX_DRAWDOWN_FRAC = 0.20", "MAX_DRAWDOWN_FRAC = 0.25")], "test_s10 or " + THRESH),
    "Q03": ("trade floor lowered to 20", EXP2, "p2_protocol.py",
            [("MIN_TRADES = 30 ", "MIN_TRADES = 20 ")], "test_s2 or " + THRESH),
    "Q04": ("a modified (uncommitted) freeze file is accepted", EXP2, "p2_cohort.py",
            [('        if subprocess.run(["git", "diff", "--quiet", "HEAD", "--", rel], cwd=repo).returncode != 0:',
              "        if False:")], "test_the_freeze_must_be_committed"),
    "Q05": ("a defect-failed candidate is retried by outcome", EXP2, "p2_runner.py",
            [('    if any(a["status"] == "failed" and a["failure_reason"] != pp.INTERRUPTED_REASON for a in atts):',
              "    if False:")], "test_a_defect_is_durably_failed"),
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(kexpr: str) -> tuple[int, str]:
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    p = subprocess.run([sys.executable, "-B", "-m", "pytest", TESTFILE, "-q", "--tb=line", "-p", "no:cacheprovider", "-x",
                        "-k", kexpr], cwd=RP, capture_output=True, text=True, env=env)
    lines = [ln for ln in p.stdout.splitlines() if ln.strip()]
    return p.returncode, "\n".join(lines[-4:])


def _purge_pyc(directory: Path, fname: str) -> None:
    for f in (directory / "__pycache__").glob(f"{Path(fname).stem}.*.pyc"):
        f.unlink()


def main(ids: list[str]) -> int:
    out = RP / "runs" / "alpha_edge_pass2_01" / "mutation_proof_log.json"
    results, bad = {}, 0
    for mid in ids:
        desc, directory, fname, edits, kexpr = MUTATIONS[mid]
        path = directory / fname
        original = path.read_bytes()
        digest = sha(original)
        text = original.decode("utf-8")
        try:
            for old, new in edits:
                if text.count(old) != 1:
                    raise SystemExit(f"{mid}: expected exactly one occurrence of {old[:70]!r}, found {text.count(old)}")
                text = text.replace(old, new)
            path.write_bytes(text.encode("utf-8"))
            _purge_pyc(directory, fname)
            rc, tail = run_tests(kexpr)
        finally:
            path.write_bytes(original)
            _purge_pyc(directory, fname)
        restored = sha(path.read_bytes()) == digest
        rc2, tail2 = run_tests(kexpr)
        red, green = rc != 0, rc2 == 0
        results[mid] = {"description": desc, "file": fname, "tests": kexpr, "red_under_mutation": red,
                        "restored_byte_for_byte": restored, "green_after_restore": green,
                        "pytest_tail_red": tail, "pytest_tail_green": tail2}
        print(f"{mid} {'RED' if red else 'STILL GREEN (FAIL)'} restored={restored} green_after={green} :: {desc}", flush=True)
        bad += (not red) + (not restored) + (not green)
    out.parent.mkdir(parents=True, exist_ok=True)
    prior = json.loads(out.read_text(encoding="utf-8")) if out.exists() else {}
    prior.update(results)
    out.write_text(json.dumps(prior, indent=1, sort_keys=True), encoding="utf-8")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or sorted(MUTATIONS)))
