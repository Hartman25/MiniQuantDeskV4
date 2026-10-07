"""Pre-freeze proof for Census-02, run from the exact behavior commit. Result-free: it derives counts / identities from the
production authority code and runs the named load-bearing tests. It is evidence, not a bound behavior source."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXP2 = HERE.parent
sys.path.insert(0, str(EXP2))
sys.path.insert(0, str(EXP2.parents[1] / "tests"))
import c2_environment as env  # noqa: E402
import c2_factors as fx  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_policy as pol  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_strategy as st  # noqa: E402

REPO = pr.REPO
T = "research-py/tests/test_alpha_edge_census_02"
LOAD_BEARING = {
    "freeze_first_missing_or_invalid_never_reaches_loader": f"{T}_campaign.py::test_a_missing_or_invalid_freeze_never_reaches_the_loader",
    "freeze_first_uncommitted_never_reaches_loader": f"{T}_campaign.py::test_uncommitted_freeze_never_reaches_the_loader",
    "gate_executes_before_loader_then_synthetic_run": f"{T}_campaign.py::test_freeze_gate_executes_before_the_loader_and_the_run_proceeds_on_synthetic_bars",
    "default_loader_is_the_guarded_entrance": f"{T}_campaign.py::test_default_loader_is_the_guarded_entrance_and_receives_the_frozen_protocol_id",
    "direct_data_entrance_refused_without_gate": f"{T}_campaign.py::test_direct_data_entrance_without_the_freeze_gate_is_refused_before_any_io",
    "import_time_no_io": f"{T}_campaign.py::test_importing_the_runner_and_cli_performs_no_io_and_creates_no_run_directory",
    "single_guarded_entrance_ast": f"{T}_protocol.py::test_only_the_single_guarded_entrance_can_reach_data_acquisition",
    "no_dynamic_imports": f"{T}_protocol.py::test_no_dynamic_import_in_any_bound_source_beyond_the_reviewed_stdlib_one",
    "manifest_covers_reachable_sources": f"{T}_protocol.py::test_manifest_covers_every_reachable_behavior_source_or_documents_why_not",
    "complete_registration_precedes_evaluation": f"{T}_campaign.py::test_complete_population_registration_precedes_any_evaluation",
    "partial_or_lazy_registration_refused": f"{T}_campaign.py::test_partial_lazy_or_winner_only_factor_registration_cannot_be_evaluated",
    "reserve_fences": f"{T}_protocol.py::test_confirmation_contaminated_and_holdout_rows_are_fenced",
    "census02_bar_loader_fence": f"{T}.py::test_symbol_data_builder_is_fenced_before_any_array_is_built",
    "result_independent_identity": f"{T}.py::test_trial_identity_is_result_independent_and_side_bound",
    "no_hard_30_trade_veto": f"{T}_campaign.py::test_no_hard_30_trade_veto_only_the_typed_minimum_of_5",
    "environment_mismatch_refused": f"{T}_protocol.py::test_numerical_environment_mismatch_is_refused_before_anything_else",
    "operator_policy_exact": f"{T}_campaign.py::test_operator_policy_is_encoded_exactly",
}


def git_head() -> str:
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()


def main() -> int:
    dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=REPO, capture_output=True, text=True).stdout
    if dirty.strip():
        raise SystemExit("tracked tree must be clean: the proof is bound to an exact committed behavior state")
    d = pol.approved_decisions()
    head = git_head()
    pid = pr.frozen_protocol_id(pr.build_structural_protocol(), d, pr.behavior_source_manifest(), env.environment_identity())
    cells = pop.strategy_cells(d, pid)
    spop, fpop = pop.strategy_population(d, pid), pop.factor_population(d)
    configs = gr.build_configs(d["grammar_tiers"], False)
    ctx = {"universe_identity": {"u": "proof-only"}, "data_provenance_identity": {"p": "proof-only"}}
    specs = list(fx.iter_factor_specs(pop.conditions(d), ctx))
    rec = lambda net, rt, a=None: {"d": "EVALUABLE", "executable_pnl": True, "m": {  # noqa: E731
        "cash_zero": {"net_pnl_usd": net, "round_trips": rt}, **({"passive_short_hold": {"net_alpha_usd": a}} if a is not None else {})}}
    facts = {
        "1_strategy_configs": (len(configs), 470),
        "2_class_c_scope_symbols": (len(pop.class_c_scope(d)), 20),
        "3_strategy_trial_coordinates": (len(cells), 9400),
        "4_short_conditions": (len(pop.conditions(d)), 215),
        "5_horizons": (len(gr.HORIZONS), 5),
        "6_factor_semantic_coordinates": (len(pop.factor_coordinates(d)), 1075),
        "7_complement_tags_preserved": ((spop["complement_tagged_configs"], spop["complement_tagged_trials"]), (56, 1120)),
        "8_all_short_factors_lower_is_better": ({s.direction for _c, _h, s in specs}, {"lower_is_better"}),
        "9_benchmark_rule": (d["benchmark_rule"], "SIDE_AWARE_SHORT_NET_AND_PASSIVE_SHORT_ALPHA_LONGSHORT_NET_VS_CASH"),
        "10_annual_borrow_fee_bps": (d["etf_borrow_assumption"]["annual_borrow_fee_bps"], 100.0),
        "11_conditional_scope_all_seed_symbols": ((d["conditional_scope"], fpop["scope_symbol_count"]), ("ALL_SEED_SYMBOLS", 88)),
        "12_ssr_flag_only": (d["ssr_handling"], "FLAG_ONLY"),
        "13_trade_band_no_hard_veto": ((st.strategy_outcome("short", rec(1.0, 5, 1.0), d["benchmark_rule"])[0],
                                         st.strategy_outcome("short", rec(1.0, 4, 1.0), d["benchmark_rule"])[0]),
                                        ("QUALIFIED", "INSUFFICIENT_CLOSED_ROUND_TRIPS")),
        "14_environment_identity_complete": (sorted(env.environment_identity()), ["numpy", "pandas", "python"]),
    }
    checks = [{"id": k, "observed": sorted(v[0]) if isinstance(v[0], set) else v[0], "expected": sorted(v[1]) if isinstance(v[1], set) else v[1],
               "ok": v[0] == v[1]} for k, v in facts.items()]
    run = subprocess.run([sys.executable, "-B", "-m", "pytest", "-q", "-p", "no:cacheprovider", "--tb=line", *LOAD_BEARING.values()],
                         cwd=REPO, capture_output=True, text=True)
    summary = [ln for ln in run.stdout.splitlines() if ln.strip()][-1]
    proof = {"schema_version": "census02_freeze_proof_v1", "behavior_head": head, "protocol_id_for_this_behavior_state": pid,
             "strategy_population_root": spop["population_root"], "factor_coordinate_root": fpop["coordinate_root"],
             "environment_identity": env.environment_identity(), "derived_facts": checks,
             "load_bearing_tests": {"node_ids": LOAD_BEARING, "pytest_returncode": run.returncode, "summary": summary},
             "real_census02_attempts": 0, "discovery_bars_read": 0, "rows_2024_read": 0, "confirmation_rows_consumed": 0,
             "final_holdout_rows_consumed": 0}
    ok = all(c["ok"] for c in checks) and run.returncode == 0
    proof["all_ok"] = ok
    (HERE / "CENSUS02_FREEZE_PROOF.json").write_text(json.dumps(proof, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    print({"all_ok": ok, "summary": summary, "behavior_head": head})
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
