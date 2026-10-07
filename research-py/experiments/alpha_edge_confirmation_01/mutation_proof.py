"""Mutation proof C01-C25 for the Confirmation controls. Every mutation is applied to an ISOLATED copy of the committed tree
(the committed freeze binds c1_*.py, so mutating the live tree would trip the guard itself). Per mutation: RED (the
Confirmation test file fails, in an expected test), source restored byte-for-byte (sha256 checked), then GREEN again."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
TEST_REL = "research-py/tests/test_alpha_edge_confirmation_01.py"
EXP_REL = "research-py/experiments/alpha_edge_confirmation_01"
OUT = HERE / "results" / "confirmation_mutation_log.json"

GUARDS_OFF_COHORT = [
    ("c1_cohort.py", "    if len(surv) != cp.EXPECTED_COHORT:", "    if False:"),
    ("c1_cohort.py", '    if ranked != {r["factor_id"] for r in surv}:', "    if False:"),
    ("c1_cohort.py", "    if dict(sorted(fams.items())) != cp.EXPECTED_FAMILY_COUNTS:", "    if False:"),
]
_PASS_CHECK = ('        if c1["status"] != "PASS" or c1["fdr_status"] != "complete" or c1["factor_id"] != r["factor_id"]:',
               "        if False:")
_PMAP = ('    pmap = {i: (evidence[i]["p_value"] if evidence[i]["evaluable"] else 1.0) for i in ids}')
_BH_OFF = [("c1_eval.py", '    if fdr["hypothesis_count"] != len(ids):', "    if False:"),
           ("c1_eval.py", 'ev, q = evidence[e["factor_id"]], fdr["q_values"][e["factor_id"]]',
            'ev, q = evidence[e["factor_id"]], fdr["q_values"].get(e["factor_id"], 1.0)')]

M = [
    ("C01", "Confirmation access before the committed freeze (guard skips the committed check)",
     [("c1_cohort.py", "    for p in paths:\n        rel = str(Path(p).resolve()", "    for p in []:\n        rel = str(Path(p).resolve()")],
     ["C01_guard_refuses_freeze_files_present_but_uncommitted"]),
    ("C01b", "acquire_symbol no longer runs the freeze guard before the provider",
     [("c1_data.py", "    cc.require_freeze(**_freeze_kwargs(freeze))\n    from mqk_research.data.alpaca_historical import (\n"
                     "        AlpacaHistoricalExtractionError,",
       "    from mqk_research.data.alpaca_historical import (\n        AlpacaHistoricalExtractionError,")],
     ["acquire_and_load_refuse_before_any_provider_call_pre_freeze"]),
    ("C02", "cohort != 18 accepted (count, ranking and family guards off; a survivor dropped)", GUARDS_OFF_COHORT,
     ["cohort_not_18_is_refused"]),
    ("C03", "a rejected factor added to the cohort",
     GUARDS_OFF_COHORT + [("c1_cohort.py",) + _PASS_CHECK,
                          ("c1_cohort.py", '    surv = [r for r in rows if r["verdict"] == cp.PASS2_SURVIVOR_VERDICT]',
                           '    surv = [r for r in rows if r["verdict"] == cp.PASS2_SURVIVOR_VERDICT or r["factor_id"] == next('
                           'x["factor_id"] for x in rows if x["verdict"] != cp.PASS2_SURVIVOR_VERDICT)]')],
     ["cohort_is_exactly_the_18"]),
    ("C04", "the weakest p-value dropped from the BH denominator",
     _BH_OFF + [("c1_eval.py", _PMAP, _PMAP + "\n    pmap.pop(max(pmap, key=pmap.get))")],
     ["C04_C16_bh_denominator"]),
    ("C05", "direction flipped inside the estimator",
     [("c1_eval.py", "ev = cn.event_diagnostics(frame, aux, direction)", 'ev = cn.event_diagnostics(frame, aux, "lower_is_better")')],
     ["effect_and_events_match_an_independent_recomputation"]),
    ("C06", "factor parameters changed",
     [("c1_eval.py", "sig = cn.condition_sig(U, sym, condition)",
       'sig = cn.condition_sig(U, sym, {**condition, "params": {k: (v + 1 if isinstance(v, (int, float)) and not isinstance(v, bool) '
       'else v) for k, v in condition["params"].items()}})')],
     ["effect_and_events_match_an_independent_recomputation"]),
    ("C07", "horizon changed",
     [("c1_eval.py", "ret = sd.c[idx + horizon] / sd.c[idx] - 1.0", "ret = sd.c[idx + horizon - 1] / sd.c[idx] - 1.0")],
     ["effect_and_events_match_an_independent_recomputation"]),
    ("C08", "a 2024 warm-up observation scored",
     [("c1_eval.py", 'first_scored = int(np.searchsorted(sd.dates, SCORE_START_DATE, side="left"))', "first_scored = 0"),
      ("c1_eval.py", "    if per.min() < SCORE_START_TS:", "    if False:")],
     ["scored_frame_contains_no_warmup_rows", "C08_C11_assert_scored"]),
    ("C09", "2024 used in the unconditional baseline",
     [("c1_eval.py", '"label_fwd_ret": ret - ret.mean(),',
       '"label_fwd_ret": ret - (sd.c[horizon:] / sd.c[:-horizon] - 1.0).mean(),')],
     ["effect_and_events_match_an_independent_recomputation", "scored_frame_contains_no_warmup_rows"]),
    ("C10a", "provider row at/after the fence accepted by the transport",
     [("c1_data.py", "                    if t >= WINDOW_END_EXCLUSIVE:", "                    if False:")],
     ["C10_transport"]),
    ("C10b", "bar at/after the fence accepted by the symbol grid",
     [("c1_eval.py", "        if days and days[-1] >= dt.date.fromisoformat(cp.SCORE_END_EXCLUSIVE):", "        if False:")],
     ["C10_symbol_data"]),
    ("C10c", "window checker accepts a row at/after the fence",
     [("c1_data.py", "    if hi >= WINDOW_END_EXCLUSIVE:", "    if False:")],
     ["window_checker_refuses_fence"]),
    ("C11", "label endpoint at/after the fence not refused by assert_scored",
     [("c1_eval.py", "    if per.max() >= FENCE_TS or end.max() >= FENCE_TS:", "    if per.max() >= FENCE_TS:")],
     ["C08_C11_assert_scored"]),
    ("C12a", "forward label declared executable P&L",
     [("c1_protocol.py", '"PROMOTION_AUTHORITY": "NONE", "EXECUTABLE_PNL": False,', '"PROMOTION_AUTHORITY": "NONE", "EXECUTABLE_PNL": True,')],
     ["protocol_id_is_pinned", "C23_C24_label_contract", "every_threshold"]),
    ("C12b", "record-level no-P&L label check removed",
     [("c1_runner.py", "        if rec.get(key) != want:", "        if False:")],
     ["C12_record_without_the_no_pnl_label"]),
    ("C13", "event floor lowered below 30",
     [("c1_protocol.py", "MIN_EVENTS = 30", "MIN_EVENTS = 29")],
     ["protocol_thresholds", "protocol_id_is_pinned", "C13_below_the_event_floor"]),
    ("C14a", "permutation count changed", [("c1_protocol.py", "N_PERMUTATIONS = 200", "N_PERMUTATIONS = 199")],
     ["protocol_thresholds", "protocol_id_is_pinned"]),
    ("C14b", "null base seed changed", [("c1_protocol.py", "BASE_SEED = 0", "BASE_SEED = 1")],
     ["protocol_thresholds", "protocol_id_is_pinned"]),
    ("C15", "BH alpha changed", [("c1_protocol.py", "FDR_ALPHA = 0.10", "FDR_ALPHA = 0.20")],
     ["protocol_thresholds", "protocol_id_is_pinned"]),
    ("C16", "BH computed on evaluable winners only",
     _BH_OFF + [("c1_eval.py", _PMAP, '    pmap = {i: evidence[i]["p_value"] for i in ids if evidence[i]["evaluable"]}')],
     ["C04_C16_bh_denominator"]),
    ("C17", "p > 0.10 can be CONFIRMED_STRONG",
     [("c1_eval.py", "    if float(p) <= cp.P_STRONG_MAX and float(q) <= cp.FDR_ALPHA:", "    if float(q) <= cp.FDR_ALPHA:")],
     ["decision_rule_table", "C17_C18_C19"]),
    ("C18", "q > 0.10 can be CONFIRMED_STRONG",
     [("c1_eval.py", "    if float(p) <= cp.P_STRONG_MAX and float(q) <= cp.FDR_ALPHA:", "    if float(p) <= cp.P_STRONG_MAX:")],
     ["decision_rule_table", "C17_C18_C19"]),
    ("C19a", "zero effect called confirmed", [("c1_eval.py", "    if float(effect) <= 0.0:", "    if float(effect) < 0.0:")],
     ["decision_rule_table"]),
    ("C19b", "negative effect called confirmed", [("c1_eval.py", "    if float(effect) <= 0.0:", "    if False:")],
     ["decision_rule_table", "C17_C18_C19"]),
    ("C20", "a statistical failure is retried",
     [("c1_runner.py", '    if any(a["status"] == "failed" and a["failure_reason"] != cp.INTERRUPTED_REASON for a in atts):', "    if False:")],
     ["C20_statistical_failure"]),
    ("C21a", "persisted record under a foreign evaluation_id accepted",
     [("c1_runner.py", '        if rec.get("evaluation_id") != cand["evaluation_id"] or rec.get("factor_id") != cand["factor_id"]:', "        if False:")],
     ["C21_persisted_record"]),
    ("C21b", "resume under an unregistered evaluation_id proceeds",
     [("c1_runner.py", "    if foreign:\n", "    if False:\n")],
     ["C21_foreign_evaluation_id"]),
    ("C22", "a terminal succeeded attempt is re-executed",
     [("c1_runner.py", "    if succ:\n        if len(succ) != 1", "    if False:\n        if len(succ) != 1")],
     ["C22_terminal_succeeded"]),
    ("C23", "Promotion claim accepted",
     [("c1_eval.py", '    if str(doc.get("promotion", "NOT_CLAIMED")) != "NOT_CLAIMED" or str(doc.get("final_holdout", "RESERVED_UNCONSUMED")) != "RESERVED_UNCONSUMED":',
       '    if str(doc.get("final_holdout", "RESERVED_UNCONSUMED")) != "RESERVED_UNCONSUMED":')],
     ["C23_C24_label_contract"]),
    ("C24a", "Final Holdout consumption claim accepted",
     [("c1_eval.py", '    if str(doc.get("promotion", "NOT_CLAIMED")) != "NOT_CLAIMED" or str(doc.get("final_holdout", "RESERVED_UNCONSUMED")) != "RESERVED_UNCONSUMED":',
       '    if str(doc.get("promotion", "NOT_CLAIMED")) != "NOT_CLAIMED":')],
     ["C23_C24_label_contract"]),
    ("C24b", "consumption proof accepts a provider row at/after the fence",
     [("c1_runner.py", "    if raw_max is not None and ce.pd.Timestamp(raw_max) >= fence:", "    if False:")],
     ["C24_consumption_proof"]),
    ("C25", "ranking mutates the rows it presents (alters status/order source)",
     [("c1_eval.py", "    directional = sorted(", '    rows[:] = sorted(rows, key=lambda r: r["factor_id"], reverse=True)\n    directional = sorted(')],
     ["C25_ranking_is_read_only"]),
]


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(root: Path) -> tuple[int, list[str], str]:
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(root / "research-py" / "src")}
    p = subprocess.run([sys.executable, "-m", "pytest", str(root / TEST_REL), "-q", "--tb=line", "-p", "no:cacheprovider",
                        "--no-header", "-rfE"], cwd=root / EXP_REL.rsplit("/", 1)[0], env=env, capture_output=True, text=True,
                       timeout=900)
    failed = sorted(set(re.findall(r"^(?:FAILED|ERROR) \S*::(\S+)", p.stdout, flags=re.M)))
    return p.returncode, failed, p.stdout.strip().splitlines()[-1] if p.stdout.strip() else ""


def main() -> None:
    work = Path(tempfile.mkdtemp(prefix="mqk_conf_mut_"))
    subprocess.run(f'git -C "{REPO}" archive HEAD research-py core-rs/crates/mqk-integrity/src/sessions.rs | tar -x -C "{work}"', shell=True, check=True)
    (work / TEST_REL).write_bytes((REPO / TEST_REL).read_bytes())
    exp = work / EXP_REL
    pristine = {p.name: p.read_bytes() for p in exp.glob("c1_*.py")}
    rc, failed, tail = run_tests(work)
    if rc != 0:
        raise SystemExit(f"baseline in the isolated copy is not GREEN: {failed} {tail}")
    log = [{"id": "BASELINE", "description": "isolated copy of committed HEAD", "result": "GREEN", "summary": tail}]
    bad = 0
    for mid, desc, edits, expect in M:
        touched = sorted({e[0] for e in edits})
        for name, old, new in ((e[0], e[1], e[2]) for e in edits):
            text = pristine[name].decode("utf-8").replace("\r\n", "\n")
            if text.count(old) != 1:
                raise SystemExit(f"{mid}: anchor not unique/absent in {name}: {old[:70]!r} x{text.count(old)}")
        for name in touched:
            crlf = b"\r\n" in pristine[name]
            text = pristine[name].decode("utf-8").replace("\r\n", "\n")
            for n, old, new in ((e[0], e[1], e[2]) for e in edits):
                if n == name:
                    text = text.replace(old, new, 1)
            (exp / name).write_bytes((text.replace("\n", "\r\n") if crlf else text).encode("utf-8"))
        mut_sha = {n: sha((exp / n).read_bytes()) for n in touched}
        rc_red, failed, tail_red = run_tests(work)
        for name in touched:
            (exp / name).write_bytes(pristine[name])
        restored = all(sha((exp / n).read_bytes()) == sha(pristine[n]) for n in pristine)
        rc_green, _f, tail_green = run_tests(work)
        hit = [f for f in failed if any(x in f for x in expect)]
        ok = rc_red != 0 and bool(hit) and restored and rc_green == 0
        bad += 0 if ok else 1
        log.append({"id": mid, "description": desc, "files": touched, "mutated_sha256": mut_sha,
                    "red": {"exit": rc_red, "summary": tail_red, "failed_tests": failed, "expected_test_hit": hit},
                    "restored_byte_identical": restored, "green_after_restore": {"exit": rc_green, "summary": tail_green},
                    "verdict": "KILLED_RED_THEN_GREEN" if ok else "NOT_PROVEN"})
        print(mid, "OK" if ok else "FAIL", "|", hit[:2] or failed[:3], flush=True)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"schema_version": "alpha_edge_confirmation_mutation_log_v1", "isolated_copy": "git archive HEAD",
                               "mutations": len(M), "not_proven": bad, "log": log}, indent=1, sort_keys=True) + "\n",
                   encoding="utf-8", newline="\n")
    print(json.dumps({"mutations": len(M), "not_proven": bad}))
    if bad:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
