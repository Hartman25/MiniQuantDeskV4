"""Mutation proof C01-C18 plus retained M-series: each mutation must turn its load-bearing test RED; the source is then
restored byte-for-byte.

Usage (from research-py/): python experiments/alpha_edge_census_01/mutation_proof.py [C01 C07 ...]"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

EXP = Path(__file__).resolve().parent
RP = EXP.parents[1]
TESTFILE = "tests/test_alpha_edge_census_01.py"

RETRY_FORK = (
    '[(c[3] if not digest[c[3]]["attempts"] else [store.register_trials_bulk([{"trial_id": c[3] + "-retry", '
    '"experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": ss.hypothesis_id(c[1]["family"]), "strategy_id": c[3], '
    '"protocol_id": "x", "identity": {"retry_of": c[3]}}]), c[3] + "-retry"][1]) for c in part]')

PART = ("test_ir1_partition_fence_refuses_2024_2025_and_holdout_rows or test_ir23_partition_labels or test_partition_label_literals "
        "or test_classify_timestamp")
GATE = "test_gate_refuses_missing_extra_duplicate_unfrozen_and_attempted or test_ir24"
ECON = ("test_fill_vector_matches_rust_mirror_scalar_function or test_vector_simulator_matches_scalar_reference "
        "or test_protocol_declares_corrected_economics or test_metrics_values_against_hand_computation "
        "or test_capital_sizing or test_matched_benchmark")

# id -> (description, file, [(old, new)], pytest -k expression)
MUTATIONS = {
    "C01": ("move the discovery fence 2024 -> 2025", "partitions.py",
            [('DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2024-01-01", tz="UTC")',
              'DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2025-01-01", tz="UTC")')],
            PART + " or test_request_contract_ends_exactly_at_the_discovery_fence"),
    "C02": ("register an S15 family", "search_space.py",
            [('    "S14": ("CALENDAR", "symbol", _grid_S14),\n}',
              '    "S14": ("CALENDAR", "symbol", _grid_S14),\n    "S15": ("EXTRA", "symbol", _grid_S14),\n}')],
            "test_ir2_families or test_ir3_exact or test_ir5_s15 or test_ir5_grammar_authority"),
    "C03": ("add a 2-session horizon", "search_space.py",
            [("CONDITIONAL_HORIZONS = (1, 3, 5, 10, 20)", "CONDITIONAL_HORIZONS = (1, 2, 3, 5, 10, 20)")],
            "test_ir6 or test_ir7"),
    "C04": ("bypass FactorSpec registration", "conditional.py",
            [("    return [register_factor(Path(registry_db), spec) for _c, _h, spec in iter_factor_specs(conditions, ctx)]",
              "    return [spec.compute_factor_id() for _c, _h, spec in iter_factor_specs(conditions, ctx)]")],
            "test_ir7 or test_d1_no_semantic_coordinate"),
    "C05": ("a factor result enters factor_id", "conditional.py",
            [("    spec = factor_spec(condition, horizon, ctx)\n    frame, aux = build_frame(U, condition, horizon)\n",
              "    frame, aux = build_frame(U, condition, horizon)\n"
              "    spec = factor_spec(condition, horizon, {**ctx, \"data_provenance_identity\": "
              "{**ctx[\"data_provenance_identity\"], \"r\": float(frame[\"label_fwd_ret\"].sum())}})\n")],
            "test_ir8_ir9"),
    "C06": ("record a negative-net, positive-alpha StrategyEdge", "edge_registry.py",
            [('    if not m["net_pnl_usd"] > 0.0:\n        return None\n', "")],
            "test_ir10 or test_ir13 or test_registry_records_exactly"),
    "C07": ("record a 4-round-trip StrategyEdge", "edge_registry.py",
            [('m["round_trips"] < ss.MIN_CLOSED_ROUND_TRIPS', 'm["round_trips"] < ss.MIN_CLOSED_ROUND_TRIPS - 1')],
            "test_ir11"),
    "C08": ("record an n=29 ConditionalEdge", "edge_registry.py",
            [('ev.get("event_count", 0) < ss.MIN_CONDITIONAL_EVENTS', 'ev.get("event_count", 0) < ss.MIN_CONDITIONAL_EVENTS - 1')],
            "test_ir16"),
    "C09": ("FDR population may differ from the registered factor population (winners only)", "edge_registry.py",
            [('    if sorted(fdr["declared_factor_ids"]) != sorted(expected) or fdr["family"] != cd.FACTOR_FAMILY:\n        raise',
              '    if False:\n        raise')],
            "test_registry_refuses_fdr_built_from_winners"),
    "C10": ("relabel the 2024 contaminated window as an unread reserve", "partitions.py",
            [('CONTAMINATED = "CONTAMINATED_BY_REJECTED_RUN"', 'CONTAMINATED = "RESERVED_UNREAD"')],
            PART + " or test_frozen_authority_manifests"),
    "C11": ("rejected-run population/attempts satisfy the corrected gate", "census.py",
            [("    digest = store.trial_attempt_digest(ss.EXPERIMENT_ID)\n    expected = {c[3] for c in cells}",
              "    digest = store.trial_attempt_digest(ss.REJECTED_EXPERIMENT_ID)\n    expected = {c[3] for c in cells}")],
            GATE),
    "C12": ("delete losing StrategyEdge cells from the denominator", "edge_registry.py",
            [('            emit(lf, "l", ledger)\n',
              '            if m is not None and m["net_pnl_usd"] > 0:\n                emit(lf, "l", ledger)\n')],
            "test_ir27"),
    "C13": ("delete a negative ConditionalEdge factor from the denominator", "edge_registry.py",
            [('            below = conditional_positive_below_floor(r, cls)\n            summ["factors_total"] += 1\n',
              '            below = conditional_positive_below_floor(r, cls)\n'
              '            if r["status"] == "succeeded" and not (r["events"]["direction_adjusted_effect"] or 0) > 0:\n'
              '                continue\n            summ["factors_total"] += 1\n')],
            "test_ir28 or test_ir20"),
    "C14": ("conditional label frame routed into StrategyEdge economics", "census.py",
            [("    so = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, sig.s)\n    met = sm.metrics(so, bench(sd, sig.s)",
              "    import conditional as _cd\n    _label_frame = _cd.build_frame(U, {**config, 'condition_id': 'x'}, 1)\n"
              "    so = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, sig.s)\n    met = sm.metrics(so, bench(sd, sig.s)")],
            "test_ir25"),
    "C15": ("remove the cost model", "simulate.py",
            [("COMMISSION_BPS = 10.0", "COMMISSION_BPS = 0.0"), ("SLIPPAGE_BPS = 5", "SLIPPAGE_BPS = 0")],
            ECON),
    "C16": ("remove the matched benchmark", "simulate.py",
            [("    d = np.zeros(n, bool)\n    d[s:] = True\n    return d", "    d = np.zeros(n, bool)\n    return d")],
            ECON + " or test_strategy_edge_needs_cost_benchmark"),
    "C17": ("a result alters the edge id", "edge_registry.py",
            [('edge_id("STRATEGY_EDGE", rec["t"])', 'edge_id("STRATEGY_EDGE", rec["t"] + str(m["net_alpha_usd"]))')],
            "test_ids_do_not_depend_on_results_or_layout or test_registry_is_deterministic"),
    "C18": ("candidate id depends on list position/shard layout", "search_space.py",
            [("    for c in configs:\n        for s in sorted(symbols):\n            yield c, s, trial_id(trial_identity(c, s, ids))",
              "    for ci, c in enumerate(configs):\n        for s in sorted(symbols):\n"
              "            yield c, s, trial_id({**trial_identity(c, s, ids), \"pos\": ci})")],
            "test_c18"),
    "M03": ("gate accepts a missing cell", "census.py",
            [("    if got != expected:", "    if not (got <= expected):")], GATE),
    "M04": ("gate accepts an undeclared extra cell", "census.py",
            [("    if got != expected:", "    if not (got >= expected):")], GATE),
    "M06": ("canonicalization depends on param key order", "search_space.py",
            [("json.dumps(obj, sort_keys=True,", "json.dumps(obj, sort_keys=False,")],
            "test_identity_is_param_order_invariant_and_param_sensitive"),
    "M07": ("trial identity ignores actual params", "search_space.py",
            [('"params": config["params"], "scope": symbol,', '"params": {}, "scope": symbol,')],
            "test_identity_is_param_order_invariant_and_param_sensitive or test_c18"),
    "M08": ("same-bar execution", "simulate.py",
            [("    h[1:] = d[:-1]\n", "    h = d.copy()\n")],
            "test_fills_are_strictly_after_signal_bar_and_never_same_bar or test_vector_simulator_matches_scalar_reference"),
    "M09": ("lookahead on the next bar's move", "simulate.py",
            [("    dc[1:] = cm[1:] - cm[:-1]\n", "    dc[:-1] = cm[1:] - cm[:-1]\n")],
            "test_vector_simulator_matches_scalar_reference or test_simulator_net_prefix_is_independent_of_future_bars"),
    "M12": ("point_in_time_membership=true accepted", "search_space.py",
            [('snap["point_in_time_membership"] is not False', "False")],
            "test_seed_builder_fails_closed_when_snapshot_claims_point_in_time"),
    "M15": ("retry of a poor result registered as a new trial", "census.py",
            [("store.begin_attempts_bulk([c[3] for c in part], origin=ORIGIN,",
              "store.begin_attempts_bulk(" + RETRY_FORK + ", origin=ORIGIN,")],
            "test_interrupted_chunk_is_retried_as_new_attempt_with_identical_economics"),
    "M20": ("factor-run resume leaves orphan started attempts unfinalized", "run_census.py",
            [('if att["status"] == "started":', 'if False:')],
            "test_factor_run_resume_finalizes_orphan_and_records_only_terminal_configs"),
    "M21": ("factor-run treats a failed-status record as terminal", "run_census.py",
            [('and all(r["status"] in terminal for r in rows)', "")],
            "test_factor_run_resume_finalizes_orphan_and_records_only_terminal_configs"),
    "D01": ("S04 exit enters the condition identity", "search_space.py",
            [('"S03": ("fast", "slow"), "S04": ("entry",),', '"S03": ("fast", "slow"), "S04": ("entry", "exit"),')],
            "test_d1_semantic or test_d1_projection or test_d1_execution_only_param_changes"),
    "D02": ("S07 hold enters the condition identity", "search_space.py",
            [('"S07": ("decline_sessions", "atr_window", "mult", "trend"),',
              '"S07": ("decline_sessions", "atr_window", "mult", "trend", "hold"),')],
            "test_d1_semantic or test_d1_projection or test_d1_execution_only_param_changes"),
    "D03": ("two V3 factor ids manufactured for one condition+horizon", "conditional.py",
            [("            yield c, h, factor_spec(c, h, ctx)\n\n\ndef register_all_factors",
              "            for sid in c[\"source_config_ids\"]:\n                yield c, h, __import__(\"dataclasses\").replace(\n"
              "                    factor_spec(c, h, ctx), name=f\"{c['family']}:{sid}:h{h}\")\n\n\ndef register_all_factors")],
            "test_d1_semantic_population or test_d1_no_semantic_coordinate or test_ir7"),
    "D04": ("the 2,170-factor V2 population is used as the V3 FDR population", "conditional.py",
            [("build_fdr_population_report(Path(registry_db), family=FACTOR_FAMILY, p_value_evidence=items, alpha=alpha)",
              "build_fdr_population_report(Path(registry_db), family=FACTOR_FAMILY_V2, p_value_evidence=items, alpha=alpha)")],
            "test_d1_v2_factor_ids"),
    "D07": ("drop negative-effect V3 factors from the FDR denominator", "conditional.py",
            [('for e in evidence if e.get("pvalue")]',
              'for e in evidence if e.get("pvalue") and (e["events"]["direction_adjusted_effect"] or 0) > 0]')],
            "test_ir20 or test_registry_refuses_fdr_built"),
    "D08": ("an execution-only parameter may enter conditional adjacency", "edge_registry.py",
            [('    for c in conditions:\n        if set(c["params"]) - set(ss.CONDITION_PARAM_KEYS[c["family"]]):\n'
              '            raise RegistryRefusal(f"{c[\'family\']}: execution-only parameter in conditional adjacency")\n'
              '    return neighbor_map(conditions)', '    return neighbor_map(conditions)')],
            "test_d1_conditional_adjacency"),
    "D09": ("a result value alters the V3 factor identity", "conditional.py",
            [("    spec = factor_spec(condition, horizon, ctx)\n    frame, aux = build_frame(U, condition, horizon)\n",
              "    frame, aux = build_frame(U, condition, horizon)\n"
              "    spec = factor_spec(condition, horizon, {**ctx, \"data_provenance_identity\": "
              "{**ctx[\"data_provenance_identity\"], \"r\": float(frame[\"label_fwd_ret\"].sum())}})\n")],
            "test_ir8_ir9"),
    "D10": ("a V2 factor attempt can satisfy the V3 population", "conditional.py",
            [("    reg = [f[\"factor_id\"] for f in list_factors(Path(registry_db), family=FACTOR_FAMILY)]",
              "    reg = [f[\"factor_id\"] for f in list_factors(Path(registry_db))]")],
            "test_d1_v2_factor_ids"),
    "M16": ("silent SIP->IEX fallback", "data.py",
            [('asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="sip")',
              'asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="iex")')],
            "test_acquisition_never_falls_back_to_iex_and_keeps_symbol"),
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(kexpr: str) -> tuple[int, str]:
    p = subprocess.run([sys.executable, "-m", "pytest", TESTFILE, "-q", "--tb=line", "-p", "no:cacheprovider", "-x",
                        "-k", kexpr], cwd=RP, capture_output=True, text=True)
    lines = [ln for ln in p.stdout.splitlines() if ln.strip()]
    return p.returncode, "\n".join(lines[-4:])


def main(ids: list[str]) -> int:
    out = RP / "runs" / "alpha_edge_census_01_corrected" / "mutation_proof_log.json"
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
        red = rc != 0
        results[mid] = {"description": desc, "file": fname, "tests": kexpr, "red_under_mutation": red,
                        "restored_byte_for_byte": restored, "pytest_tail": tail}
        print(f"{mid} {'RED' if red else 'STILL GREEN (FAIL)'} restored={restored} :: {desc}", flush=True)
        bad += (not red) + (not restored)
    out.parent.mkdir(parents=True, exist_ok=True)
    prior = json.loads(out.read_text(encoding="utf-8")) if out.exists() else {}
    prior.update(results)
    out.write_text(json.dumps(prior, indent=1, sort_keys=True), encoding="utf-8")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or sorted(MUTATIONS)))
