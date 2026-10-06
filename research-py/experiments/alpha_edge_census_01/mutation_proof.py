"""Mutation proof M01-M18: each mutation must turn its load-bearing test RED; the source is then restored byte-for-byte.

Usage (from research-py/): python experiments/alpha_edge_census_01/mutation_proof.py [M01 M07 ...]"""

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

# id -> (description, file, [(old, new)], pytest -k expression)
MUTATIONS = {
    "M01": ("allow a post-2024-12-31 row", "partitions.py",
            [("if latest >= DISCOVERY_END_EXCLUSIVE:", "if latest >= DISCOVERY_END_EXCLUSIVE + pd.Timedelta(days=60):")],
            "test_partition_fence_refuses_reserve_and_holdout"),
    "M02": ("allow a 2026-03-01+ row", "partitions.py",
            [("latest = ts.max()", "latest = ts[ts < FINAL_HOLDOUT_START].max()")],
            "test_partition_fence_refuses_reserve_and_holdout"),
    "M03": ("remove a registered cell before attempt #1 (gate accepts missing)", "census.py",
            [("    if got != expected:", "    if not (got <= expected):")],
            "test_gate_refuses_missing_extra_duplicate_and_unfrozen"),
    "M04": ("add an undeclared cell (gate accepts extra)", "census.py",
            [("    if got != expected:", "    if not (got >= expected):")],
            "test_gate_refuses_missing_extra_duplicate_and_unfrozen"),
    "M05": ("derive edge id from result alpha", "edge_registry.py",
            [("horizon: int | None = None) -> str:", "horizon: int | None = None, net_alpha: float = 0.0) -> str:"),
             ('body = {"kind": kind, "trial_id": trial_id}', 'body = {"kind": kind, "trial_id": trial_id, "a": net_alpha}')],
            "test_edge_id_is_result_independent"),
    "M06": ("canonicalization depends on param key order", "search_space.py",
            [("json.dumps(obj, sort_keys=True,", "json.dumps(obj, sort_keys=False,")],
            "test_identity_is_param_order_invariant_and_param_sensitive"),
    "M07": ("identity ignores actual params", "search_space.py",
            [('"params": config["params"],', '"params": {},')],
            "test_identity_is_param_order_invariant_and_param_sensitive or test_search_space_shape_and_unique_identity"),
    "M08": ("same-bar execution", "simulate.py",
            [("    h[1:] = d[:-1]\n", "    h = d.copy()\n")],
            "test_fills_are_strictly_after_signal_bar_and_never_same_bar or test_vector_simulator_matches_scalar_reference"),
    "M09": ("forward return used as executable P&L (lookahead on next bar's move)", "simulate.py",
            [("    dc[1:] = cm[1:] - cm[:-1]\n", "    dc[:-1] = cm[1:] - cm[:-1]\n")],
            "test_vector_simulator_matches_scalar_reference or test_simulator_net_prefix_is_independent_of_future_bars"),
    "M10": ("winner-only search ledger", "edge_registry.py",
            [("            emit(lf, lh, ledger)\n",
              "            if m is not None and m['net_alpha_usd'] > 0:\n                emit(lf, lh, ledger)\n")],
            "test_search_ledger_holds_the_full_denominator_not_only_winners"),
    "M11": ("drop a failed symbol from the 88 in the bars manifest", "census.py",
            [('            rows[sym] = {"disposition": (status or {}).get("disposition", "DATA_UNAVAILABLE_NOT_ACQUIRED")}\n            continue\n',
              "            continue\n")],
            "test_bars_manifest_binds_hashes_keeps_unavailable_symbols_and_load_refuses_drift"),
    "M12": ("claim point_in_time_membership=true is accepted", "search_space.py",
            [('snap["point_in_time_membership"] is not False', "False")],
            "test_universe_builder_fails_closed_when_snapshot_claims_point_in_time"),
    "M13": ("single-feature model accepts two features", "signals.py",
            [('if not isinstance(p.get("feature"), str) or set(p) != {"feature", "label_horizon", "p_entry"}:', "if False:")],
            "test_s19_refuses_a_model_given_two_features"),
    "M14": ("S18 quantile uses full-period rows", "signals.py",
            [("                tr = x[:start]\n", "                tr = x\n")],
            "test_s18_thresholds_and_s19_fits_use_only_rows_before_the_fold"),
    "M15": ("retry of a poor result registered as a new trial", "census.py",
            [("store.begin_attempts_bulk([c[3] for c in part], origin=ORIGIN,", "store.begin_attempts_bulk(" + RETRY_FORK + ", origin=ORIGIN,")],
            "test_interrupted_chunk_is_retried_as_new_attempt_with_identical_economics"),
    "M16": ("silent SIP->IEX fallback", "data.py",
            [('asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="sip")', 'asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="iex")')],
            "test_acquisition_never_falls_back_to_iex_and_keeps_symbol"),
    "M17": ("parameter-neighborhood analysis deletes an island edge", "edge_registry.py",
            [("                nbh, island = neighborhood(pos_s, ci, j)\n",
              "                nbh, island = neighborhood(pos_s, ci, j)\n                if island:\n                    continue\n")],
            "test_neighborhood_flags_never_delete_island_edges or test_registry_records_every_positive_observation_and_nothing_else"),
    "M18": ("conditional forward return labelled executable P&L", "edge_registry.py",
            [('"executable_pnl": False}', '"executable_pnl": True}')],
            "test_conditional_edges_are_non_executable_labels_never_pnl or test_every_registry_record_is_not_validated_with_no_promotion_authority_and_labels"),
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_tests(kexpr: str) -> tuple[int, str]:
    p = subprocess.run([sys.executable, "-m", "pytest", TESTFILE, "-q", "--tb=line", "-p", "no:cacheprovider", "-k", kexpr],
                       cwd=RP, capture_output=True, text=True)
    lines = [ln for ln in p.stdout.splitlines() if ln.strip()]
    return p.returncode, "\n".join(lines[-4:])


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
        red = rc != 0
        results[mid] = {"description": desc, "file": fname, "tests": kexpr, "red_under_mutation": red,
                        "restored_byte_for_byte": restored, "pytest_tail": tail}
        print(f"{mid} {'RED' if red else 'STILL GREEN (FAIL)'} restored={restored} :: {desc}", flush=True)
        bad += (not red) + (not restored)
    out = RP / "runs" / "alpha_edge_census_01" / "mutation_proof_log.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    prior = json.loads(out.read_text(encoding="utf-8")) if out.exists() else {}
    prior.update(results)
    out.write_text(json.dumps(prior, indent=1, sort_keys=True), encoding="utf-8")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or sorted(MUTATIONS)))
