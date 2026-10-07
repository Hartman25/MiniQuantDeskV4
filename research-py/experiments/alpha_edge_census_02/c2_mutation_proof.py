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
             ("    if len(cm) < 3 or not (np.all(lm > 0) and np.all(cm > 0) and np.all(hm >= cm) and np.all(cm >= lm)):", "    if False:")],
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
            [("or (np.any(d < 0) and fee is None):", "or (False and fee is None):")],
            "test_short_without_explicit_borrow_fee"),
    "M15": ("short mirror emits LONG positions", "c2_signals.py",
            [("    return SigS(-sig.d.astype(np.int8), sig.cond, sig.s)", "    return SigS(sig.d.astype(np.int8), sig.cond, sig.s)")],
            "test_short_families_never_go_long or test_mirror_semantics or test_s13_upside"),
    "M16": ("freeze guard accepts a PROPOSED status", "c2_protocol.py",
            [("    if doc.get(\"status\") != STATUS_FROZEN:", "    if False:")], "test_freeze_guard_positive_control"),
    "M17": ("freeze guard does not require a committed file", "c2_protocol.py",
            [("    head = require_committed([predeclaration], repo)", "    head = 'x'")], "test_freeze_guard_positive_control"),
    "M18": ("freeze guard accepts a protocol id that does not match the decisions", "c2_protocol.py",
            [('    if doc.get("protocol_id") != frozen_protocol_id(structural, doc.get("decisions"), manifest):', "    if False:")],
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
    "M27": ("D1: protocol cost truth drifts from the simulator slippage constant", "c2_protocol.py",
            [('"fill_slippage_bps_per_side": _c1sim.SLIPPAGE_BPS', '"fill_slippage_bps_per_side": 0')],
            "test_protocol_cost_truth_equals_the_accepted_constants"),
    "M28": ("D1: Census-01 simulator slippage constant changes under an unchanged protocol literal", "../alpha_edge_census_01/simulate.py",
            [("SLIPPAGE_BPS = 5\n", "SLIPPAGE_BPS = 6\n")],
            "test_protocol_cost_truth_equals_the_accepted_constants or test_executed_fill_economics"),
    "M29": ("D2: a long/short cell is forced to need a passive hold", "c2_simulate.py",
            [('    if rule == "NET_POSITIVE_ONLY_ALL_SIDES" or side == "long_short":', '    if rule == "NET_POSITIVE_ONLY_ALL_SIDES":')],
            "test_long_short_qualifies_on_net_vs_cash"),
    "M30": ("D2: short-only qualification ignores the passive-short-hold alpha", "c2_simulate.py",
            [('    return bool(net_positive and m["passive_short_hold"]["net_alpha_usd"] > 0)', "    return bool(net_positive)")],
            "test_short_only_needs_net_positive_and_alpha"),
    "M31": ("D2: the passive long hold becomes a qualification benchmark for long/short", "c2_protocol.py",
            [('"passive_long_hold": "DIAGNOSTIC_ONLY"', '"passive_long_hold": "QUALIFICATION_ALPHA"')],
            "test_evaluated_cells_record_side_aware_benchmarks"),
    "M32": ("D3: complement exclusion leaves every short condition registered", "c2_grammar.py",
            [("    return configs, build_conditions(configs, complements_excluded)", "    return configs, build_conditions(build_configs(tiers), False)")],
            "test_grammar_counts_and_arithmetic or test_complement_exclusion"),
    "M33": ("D4: short factors are registered higher_is_better", "c2_factors.py",
            [("        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=direction,",
              '        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction="higher_is_better",')],
            "test_short_factors_are_lower_is_better or test_direction_is_identity_bound"),
    "M34": ("D4: direction no longer reaches the factor identity (a flip is invisible)", "c2_factors.py",
            [("        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=direction,",
              "        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=SHORT_FACTOR_DIRECTION,")],
            "test_direction_is_identity_bound"),
    "M35": ("D4: direction-adjusted effect is +raw", "c2_factors.py",
            [("    return -float(raw_effect)", "    return float(raw_effect)")],
            "test_direction_adjusted_effect_is_minus_the_raw_effect"),
    "M36": ("D5: freeze guard no longer compares the behavior-source manifest", "c2_protocol.py",
            [('    if doc.get("behavior_source_manifest") != manifest:', "    if False:")],
            "test_freeze_guard_reds_on_one_byte_of_drift or test_freeze_guard_positive_control"),
    "M37": ("D5: a Census-02 behavior module is omitted from the manifest", "c2_protocol.py",
            [('_C2 + "c2_signals.py",\n    _C2 + "c2_simulate.py",', '_C2 + "c2_simulate.py",')],
            "test_manifest_covers_every_reachable_behavior_source"),
    "M38": ("D5: the Census-01 partition fence is omitted from the manifest", "c2_protocol.py",
            [('_C1 + "simulate.py", _C1 + "partitions.py", _C1 + "calendar_authority.py",', '_C1 + "simulate.py", _C1 + "calendar_authority.py",')],
            "test_manifest_covers_every_reachable_behavior_source"),
    "M39": ("D5: the RSI/z-score indicator source is omitted from the manifest", "c2_protocol.py",
            [('    _SRC + "indicators/core.py", _SRC + "factors/contracts.py",', '    _SRC + "factors/contracts.py",')],
            "test_manifest_covers_every_reachable_behavior_source"),
    "M40": ("D5: the manifest hashes only a source prefix (late drift is invisible)", "c2_protocol.py",
            [("hashlib.sha256(raw.replace(", "hashlib.sha256(raw[:64].replace(")],
            "test_freeze_guard_reds_on_one_byte_of_drift"),
    "M41": ("D5: the manifest is not bound into the protocol id", "c2_protocol.py",
            [('"decisions": decisions, "behavior_source_manifest": source_manifest})', '"decisions": decisions})')],
            "test_manifest_hash_is_line_ending_normalised_and_binds_the_protocol_id"),
    "M42": ("D6: EQUITY_SYMBOLS_ONLY is accepted as a conditional scope", "c2_protocol.py",
            [('(("ALL_SEED_SYMBOLS",), _opt("ALL_SEED_SYMBOLS"))', '(("ALL_SEED_SYMBOLS",), _opt("ALL_SEED_SYMBOLS", "EQUITY_SYMBOLS_ONLY"))')],
            "test_operator_decisions_must_be_complete_and_consistent"),
    "M43": ("D7: the ETF scope is no longer checked against the frozen seed universe", "c2_borrow.py",
            [("    if outside:\n", "    if False:\n")],
            "test_etf_scope_outside_the_frozen_seed_universe"),
    "M44": ("D8: a zero low price is accepted", "c2_simulate.py",
            [("np.all(lm > 0) and np.all(cm > 0)", "np.all(cm > 0)")],
            "test_zero_or_negative_low_is_refused"),
    "M45": ("D8: a negative commission override is accepted", "c2_simulate.py",
            [("or commission_bps < 0):", "):")], "test_malformed_cost_overrides_are_refused"),
    "M46": ("D8: a non-integral slippage override is accepted", "c2_simulate.py",
            [("or not isinstance(slippage_bps, (int, np.integer))", "")], "test_malformed_cost_overrides_are_refused"),
    "M47": ("D8: a malformed borrow fee is tolerated when no short is held", "c2_simulate.py",
            [("    if (fee is not None and (isinstance(fee, bool) or not np.isfinite(fee) or fee < 0)) or", "    if (False) or")],
            "test_valid_cost_overrides_are_accepted_and_malformed_borrow_fee"),
}


# Manifest-detection mutations: one semantic edit per behavior class on the REAL source. The recomputed behavior-source
# manifest must change in exactly that file (so require_freeze refuses before any real-data access) and be identical
# again after the byte-exact restore.
MANIFEST_MUTATIONS = {
    "F01": ("grammar", "c2_grammar.py", "for e in (90, 80, 70) for x in (50, 30)", "for e in (95, 80, 70) for x in (50, 30)"),
    "F02": ("signal", "c2_signals.py", "return _short(s1._state_sig(sd, sd.c < m, np.isfinite(m)))",
            "return _short(s1._state_sig(sd, sd.c <= m, np.isfinite(m)))"),
    "F03": ("simulator", "c2_simulate.py", "commission = s1.COMMISSION_BPS if", "commission = 0.0 if"),
    "F04": ("borrow", "c2_borrow.py", '"recall": "NONE_ASSUMED"', '"recall": "NONE_ASSUMED_X"'),
    "F05": ("factor direction", "c2_protocol.py", '"direction": "lower_is_better"', '"direction": "higher_is_better"'),
    "F06": ("partition fence", "../alpha_edge_census_01/partitions.py", 'DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2024-01-01", tz="UTC")',
            'DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2025-01-01", tz="UTC")'),
    "F07": ("price/cost arithmetic", "../alpha_edge_census_01/simulate.py", "SLIPPAGE_BPS = 5\n", "SLIPPAGE_BPS = 6\n"),
    "F08": ("Census-01 signal helper", "../alpha_edge_census_01/signals.py", "LAST_DISCOVERY_DATE = dt.date(2023, 12, 31)",
            "LAST_DISCOVERY_DATE = dt.date(2024, 12, 31)"),
    "F09": ("Census-01 grammar", "../alpha_edge_census_01/search_space.py", 'CADENCES = ("daily", "month_end")', 'CADENCES = ("daily",)'),
    "F10": ("calendar", "../alpha_edge_census_01/calendar_authority.py", 'CONTRACT_ID = "us_equity_regular_sessions_v1"',
            'CONTRACT_ID = "us_equity_regular_sessions_v2"'),
    "F11": ("indicator (RSI)", "../../src/mqk_research/indicators/core.py", "return 100.0 - (100.0 / (1.0 + rs))",
            "return 100.0 - (100.0 / (2.0 + rs))"),
    "F12": ("factor identity contract", "../../src/mqk_research/factors/contracts.py", 'DIRECTION_LOWER_IS_BETTER = "lower_is_better"',
            'DIRECTION_LOWER_IS_BETTER = "lower_is_best"'),
    "F13": ("identity hashing", "../../src/mqk_research/exp_distributed/hashing.py", "return stable_hash(obj)[:length]",
            "return stable_hash(obj)[:length - 1]"),
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(kexpr: str) -> tuple[int, str]:
    # -B: a same-length mutation must never be masked by a stale .pyc (mtime/size cache) after restore
    p = subprocess.run([sys.executable, "-B", "-m", "pytest", *TESTFILES, "-q", "--tb=line", "-p", "no:cacheprovider", "-x",
                        "-k", kexpr], cwd=RP, capture_output=True, text=True)
    lines = [ln for ln in p.stdout.splitlines() if ln.strip()]
    return p.returncode, "\n".join(lines[-3:])


def _manifest_json() -> str:
    code = ("import sys, json; sys.path.insert(0, %r); import c2_protocol as p; print(json.dumps(p.behavior_source_manifest(), sort_keys=True))"
            % str(EXP))
    return subprocess.run([sys.executable, "-B", "-c", code], cwd=RP, capture_output=True, text=True, check=True).stdout


def run_manifest_mutation(mid: str) -> dict:
    klass, fname, old, new = MANIFEST_MUTATIONS[mid]
    path = (EXP / fname).resolve()
    original = path.read_bytes()
    digest = sha(original)
    before = json.loads(_manifest_json())
    text = original.decode("utf-8")
    if text.count(old) != 1:
        raise SystemExit(f"{mid}: expected exactly one occurrence of {old[:60]!r}, found {text.count(old)}")
    try:
        path.write_bytes(text.replace(old, new).encode("utf-8"))
        during = json.loads(_manifest_json())
    finally:
        path.write_bytes(original)
    after = json.loads(_manifest_json())
    changed = sorted(k for k, v in during["sources"].items() if before["sources"][k] != v)
    rel = str(path.relative_to(RP.parent)).replace("\\", "/")
    return {"description": f"{klass}: one semantic edit on the real source must move the freeze manifest", "file": fname,
            "manifest_changed_exactly_this_file": changed == [rel], "red_under_mutation": changed == [rel],
            "green_after_restore": after == before, "restored_byte_for_byte": sha(path.read_bytes()) == digest}


def check_anchors(ids: list[str]) -> None:
    """Refuse to start unless every mutation anchor matches exactly once (a stale anchor must not abort a long run)."""
    problems = []
    for mid in ids:
        if mid in MANIFEST_MUTATIONS:
            _k, fname, old, _new = MANIFEST_MUTATIONS[mid]
            edits, files = [(old, _new)], [fname]
        else:
            _d, fname, edits, _k = MUTATIONS[mid]
            files = [fname]
        text = (EXP / files[0]).resolve().read_text(encoding="utf-8")
        problems += [f"{mid}: {old[:50]!r} x{text.count(old)}" for old, _n in edits if text.count(old) != 1]
    if problems:
        raise SystemExit("stale mutation anchors:\n" + "\n".join(problems))


def main(ids: list[str]) -> int:
    check_anchors(ids)
    results, bad = {}, 0
    for mid in [i for i in ids if i in MANIFEST_MUTATIONS]:
        r = run_manifest_mutation(mid)
        results[mid] = r
        print(f"{mid} {'RED' if r['red_under_mutation'] else 'STILL GREEN (FAIL)'} green_after_restore={r['green_after_restore']} "
              f"restored={r['restored_byte_for_byte']} :: {r['description']}", flush=True)
        bad += (not r["red_under_mutation"]) + (not r["restored_byte_for_byte"]) + (not r["green_after_restore"])
    ids = [i for i in ids if i in MUTATIONS]
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
    sys.exit(main(sys.argv[1:] or sorted(MUTATIONS) + sorted(MANIFEST_MUTATIONS)))
