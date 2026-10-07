"""Mutation proof for the Census-02 infrastructure: each mutation must turn its load-bearing test RED; the source is
restored byte-for-byte. Usage (from research-py/): python experiments/alpha_edge_census_02/c2_mutation_proof.py [M01 ...]"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

EXP = Path(__file__).resolve().parent
RP = EXP.parents[1]
TESTFILES = ["tests/test_alpha_edge_census_02_protocol.py", "tests/test_alpha_edge_census_02.py"]
OUT = EXP / "results" / "c2_mutation_proof_log.json"

# id -> (description, file, [(old, new)], pytest -k expression)
MUTATIONS = {
    "M01": ("same-bar fill: position follows the decision on the SAME bar", "c2_simulate.py",
            [("    h[1:] = d[:-1]                                   # position after the fill on bar b is the decision after bar b-1",
              "    h[:] = d")], "test_same_bar_short_fill_rejected or test_vector_simulator_matches_scalar"),
    "M02": ("P&L marks against the NEXT bar's close (future data in the cover/mark)", "c2_simulate.py",
            [("    dc[1:] = cm[1:] - cm[:-1]\n", "    dc[:-1] = cm[1:] - cm[:-1]\n")],
            "test_cover_chronology or test_vector_simulator_matches_scalar or test_same_bar_short_fill"),
    "M03": ("price-bar guard removed: a fwd_ret label becomes executable P&L", "c2_simulate.py",
            [("        if not isinstance(a, np.ndarray) or a.dtype != np.int64:", "        if False:"),
             ("    if len(cm) < 3 or not (np.all(cm > 0) and np.all(hm >= cm) and np.all(cm >= lm)):", "    if False:")],
            "test_fwd_ret_labels_are_never_executable_pnl"),
    "M04": ("a result value enters the trial identity", "c2_grammar.py",
            [('"partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"]}',
              '"partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"], "r": config.get("result")}')],
            "test_trial_identity_is_result_independent"),
    "M05": ("an attempt id enters the trial identity (a retry mints a trial)", "c2_grammar.py",
            [('"partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"]}',
              '"partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"], "a": config.get("attempt_id")}')],
            "test_trial_identity_is_result_independent or test_retry_is_a_new_attempt"),
    "M06": ("ledger tolerates a duplicated trial id", "c2_protocol.py",
            [("    if len(set(led)) != len(led):", "    if False:")],
            "test_retry_is_a_new_attempt or test_denominator_cannot_shrink"),
    "M07": ("discovery fence removed from the bar loader", "c2_protocol.py",
            [("    pt.require_discovery_only(bars[\"end_ts\"], what=what)\n", "")],
            "test_confirmation_contaminated_and_holdout_rows_are_fenced or test_fence_accepts"),
    "M08": ("any symbol is treated as a frozen ETF (borrow-unavailable equity becomes executable)", "c2_borrow.py",
            [('    return EVIDENCE_C if symbol in assumption["etf_short_scope"] else EVIDENCE_A', "    return EVIDENCE_C")],
            "test_borrow_classification_and_individual_equity_never_executable"),
    "M09": ("evidence-class gate removed from the cell evaluator", "c2_simulate.py",
            [("    if evidence_class not in bw.EXECUTABLE_CLASSES:", "    if False:")],
            "test_borrow_classification_and_individual_equity_never_executable"),
    "M10": ("class B reachable without point-in-time borrow data", "c2_borrow.py",
            [("    if evidence_class == EVIDENCE_B and not POINT_IN_TIME_BORROW_SUPPORTED:", "    if False:")],
            "test_borrow_classification_and_individual_equity_never_executable"),
    "M11": ("denominator may shrink: missing trials tolerated", "c2_protocol.py",
            [("sorted(set(led) - set(pop))\n    if missing or extra:", "sorted(set(led) - set(pop))\n    if extra:")], "test_denominator_cannot_shrink"),
    "M12": ("winner-only registration allowed", "c2_protocol.py",
            [("    assert_complete_ledger(population_ids, ledger_ids)\n    pop = set(population_ids)", "    pop = set(population_ids)")],
            "test_denominator_cannot_shrink"),
    "M13": ("benchmark of a short strategy is a LONG hold", "c2_simulate.py",
            [('    if side == "short":\n        return -1', '    if side == "short":\n        return 1')],
            "test_benchmark_is_same_direction_hold or test_borrow_classification"),
    "M14": ("short borrow fee defaults to zero when omitted", "c2_simulate.py",
            [("    if np.any(d < 0) and (borrow_fee_bps_annual is None or", "    if False and (borrow_fee_bps_annual is None or")],
            "test_short_without_explicit_borrow_fee"),
    "M15": ("short mirror emits LONG positions", "c2_signals.py",
            [("    return SigS(-sig.d.astype(np.int8), sig.cond, sig.s)", "    return SigS(sig.d.astype(np.int8), sig.cond, sig.s)")],
            "test_short_families_never_go_long or test_mirror_semantics or test_s13_upside"),
    "M16": ("freeze guard accepts a PROPOSED status", "c2_protocol.py",
            [("    if doc.get(\"status\") != STATUS_FROZEN:", "    if False:")], "test_freeze_guard_positive_control"),
    "M17": ("freeze guard does not require a committed file", "c2_protocol.py",
            [("    head = require_committed([predeclaration], repo)", "    head = 'x'")], "test_freeze_guard_positive_control"),
    "M18": ("freeze guard accepts a protocol id that does not match the decisions", "c2_protocol.py",
            [('    if doc.get("protocol_id") != frozen_protocol_id(structural, doc.get("decisions")):', "    if False:")],
            "test_freeze_guard_positive_control"),
    "M19": ("operator decision completeness check removed", "c2_protocol.py",
            [("    if missing or extra:\n        raise FreezeRefusal(f\"operator decisions incomplete", "    if False:\n        raise FreezeRefusal(f\"operator decisions incomplete")],
            "test_operator_decisions_must_be_complete"),
    "M20": ("grammar drift: a parameter value added to SH05", "c2_grammar.py",
            [("for e in (90, 80, 70) for x in (50, 30)", "for e in (95, 90, 80, 70) for x in (50, 30)")],
            "test_grammar_counts_and_arithmetic or test_grammar_drift"),
    "M21": ("stop-and-reverse leg costs dropped on a flip", "c2_simulate.py",
            [("    cost_entry_m = entry_sh * adv_ps + entry_sh * px * (commission / 10_000.0)",
              "    cost_entry_m = np.where(sb != 0, 0, entry_sh * adv_ps + entry_sh * px * (commission / 10_000.0))")],
            "test_vector_simulator_matches_scalar_reference"),
    "M22": ("short borrow accrual removed", "c2_simulate.py",
            [("    if borrow_fee_bps_annual:", "    if False:")],
            "test_short_pnl_sign_and_borrow_fee_arithmetic or test_vector_simulator_matches_scalar"),
    "M23": ("ETF assumption accepts a missing fee", "c2_borrow.py",
            [("    if isinstance(fee, bool) or not isinstance(fee, (int, float)) or not math.isfinite(fee) or fee < 0:", "    if False:")],
            "test_etf_assumption_is_fail_closed"),
    "M24": ("Census-01 config ids may collide with Census-02 ids", "c2_grammar.py",
            [("            if cid in seen or cid in c1_ids:", "            if cid in seen:")],
            "test_forced_census01_id_collision_is_refused"),
    "M25": ("SH08 gap reads the NEXT bar's open (lookahead in a signal builder)", "c2_signals.py",
            [("    gap = sd.o - s1._shift(sd.c, 1)\n    ok, tv = trend_below(sd, p[\"trend\"])\n    return _short(s1._event_sig(sd, (gap >= p",
              "    gap = np.roll(sd.o, -1) - sd.c\n    ok, tv = trend_below(sd, p[\"trend\"])\n    return _short(s1._event_sig(sd, (gap >= p")],
            "test_every_config_is_causal_prefix_invariant"),
    "M26": ("SH13 reversal fades the DOWN day instead of the up day", "c2_signals.py",
            [('sign = r1 < 0 if p["mode"] == "breakdown" else r1 > 0', 'sign = r1 > 0 if p["mode"] == "breakdown" else r1 < 0')],
            "test_s13_upside_expansion_fade_is_the_mirror"),
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(kexpr: str) -> tuple[int, str]:
    # -B: a same-length mutation must never be masked by a stale .pyc (mtime/size cache) after restore
    p = subprocess.run([sys.executable, "-B", "-m", "pytest", *TESTFILES, "-q", "--tb=line", "-p", "no:cacheprovider", "-x",
                        "-k", kexpr], cwd=RP, capture_output=True, text=True)
    lines = [ln for ln in p.stdout.splitlines() if ln.strip()]
    return p.returncode, "\n".join(lines[-3:])


def main(ids: list[str]) -> int:
    results, bad = {}, 0
    for mid in ids:
        desc, fname, edits, kexpr = MUTATIONS[mid]
        path = EXP / fname
        original = path.read_bytes()
        digest = sha(original)
        text = original.decode("utf-8")
        try:
            for old, new in edits:
                if text.count(old) != 1:
                    raise SystemExit(f"{mid}: expected exactly one occurrence of {old[:60]!r}, found {text.count(old)}")
                text = text.replace(old, new)
            path.write_bytes(text.encode("utf-8"))
            rc, tail = run_tests(kexpr)
        finally:
            path.write_bytes(original)
        restored = sha(path.read_bytes()) == digest
        green_after, _ = run_tests(kexpr)
        red = rc != 0
        results[mid] = {"description": desc, "file": fname, "tests": kexpr, "red_under_mutation": red,
                        "green_after_restore": green_after == 0, "restored_byte_for_byte": restored}
        print(f"{mid} {'RED' if red else 'STILL GREEN (FAIL)'} green_after_restore={green_after == 0} restored={restored} :: {desc}",
              flush=True)
        bad += (not red) + (not restored) + (green_after != 0)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    prior = json.loads(OUT.read_text(encoding="utf-8")) if OUT.exists() else {}
    prior.update(results)
    OUT.write_text(json.dumps(prior, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or sorted(MUTATIONS)))
