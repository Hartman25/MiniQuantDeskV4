"""IR-B02-01: the immutable Batch 02 predeclaration carries six copied Batch 01 descriptions.

The erratum is the audit authority; these tests prove (1) the original bytes never move, (2) the erratum
names exactly the stale paths and nothing executable, (3) correcting ONLY those paths changes no trial id,
sizing argument, benchmark policy, economic spec, attempt inventory or selection/holdout input, with
negative controls showing the harness would notice if it did, and (4) a future declaration cannot pass by
copying earlier-campaign text.
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import batch02_erratum as ea  # noqa: E402
from test_batch02_registration_gate import fingerprints, manifest, register, rb  # noqa: E402,F401
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

RAW = (HERE / ea.ORIGINAL_NAME).read_bytes()
ORIG = json.loads(RAW)
ERR = ea.load_json(ea.ERRATUM_NAME)
OLD = ea.load_json("PREDECLARED_BATCH_01_CORRECTED.json")
STALE_PATHS = {
    "/partition/warmup_note",
    "/economic_protocol/quantity_semantics/executable_targets",
    "/economic_protocol/quantity_semantics/rule",
    "/native_backtest/initial_cash_rule",
    "/native_backtest/evidence_root_layout",
    "/universe/survivorship_note",
}
# Every other descriptive leaf Batch 02 shares byte-for-byte with Batch 01, each read and found still true.
REVIEWED_STILL_TRUE = {
    "/activity_report/report_per_symbol/2", "/activity_report/report_per_symbol/4",
    "/activity_report/report_per_symbol/5", "/activity_report/report_per_symbol/8", "/activity_report/rule",
    "/data/fallback_if_corporate_action_gate_refuses/note", "/data/reuse_verified_data_from/rule",
    "/execution_fidelity/enforced_by", "/execution_fidelity/metric", "/experiment/judge_scope", "/holdout/rule",
    "/holdout/status", "/native_backtest/bars", "/partition/holdout_rule", "/promotion_policy/note",
    "/rejection_gates/0", "/rejection_gates/1", "/rejection_gates/2", "/rejection_gates/3", "/rejection_gates/4",
    "/rejection_gates/5", "/scanner_review/data", "/selection_authority/eligibility",
    "/selection_authority/population", "/universe/symbol_roles/EFA", "/universe/trial_order_rule",
}
STALE_MARKERS = ("fixed-one-share", "warm-ups end during 2016-2017", "+1 desired / +N held",
                 "Benchmark V2 share ONE", "run_batch_01", "deflates over all five")


def test_original_predeclaration_bytes_are_unchanged():
    assert ea.original_bytes_intact(RAW), "PREDECLARATION_BYTES_CHANGED"
    assert ERR["original_predeclaration_sha256"] == ea.ORIGINAL_SHA256
    assert ERR["original_predeclaration_sha256_lf_blob"] == ea.ORIGINAL_SHA256_LF_BLOB
    assert not ea.original_bytes_intact(RAW + b" ") and not ea.original_bytes_intact(RAW.replace(b"1000", b"1001", 1))


def test_erratum_declares_a_non_economic_post_run_correction():
    assert ERR["status"] == "POST_RUN_NON_ECONOMIC_METADATA_ERRATUM"
    assert ERR["original_predeclaration_path"].endswith("/" + ea.ORIGINAL_NAME)
    assert ERR["discovered_by"] == "independent review" and ERR["discovered_after_results"] is True
    for flag in ("original_bytes_modified", "economic_attempts_rerun", "economic_results_modified",
                 "strategy_parameters_modified", "thresholds_modified", "holdout_consumed"):
        assert ERR[flag] is False, flag
    truth = ERR["operative_batch02_truth"]
    assert truth["required_history_bars"] == {"H1": 1, "H2": 1, "H3": 60}
    assert truth["baseline_allocation_fraction_bps"] == ORIG["capital_sizing"]["allocation_fraction_bps"] == 1000
    assert truth["half_exposure_stress_allocation_fraction_bps"] == \
        ORIG["robustness"]["p7a_p7b_stress"]["stress_sizing"]["allocation_fraction_bps"] == 500
    assert truth["benchmark_policy_id"] == ORIG["benchmark"]["policy_id"]
    assert truth["run_root"] == ORIG["run_dir"]


def test_the_erratum_names_exactly_the_stale_paths_and_each_is_descriptive_only():
    fields = ERR["stale_fields"]
    assert {f["json_path"] for f in fields} == STALE_PATHS and len(fields) == len(STALE_PATHS)
    for f in fields:
        assert ea.pointer_get(ORIG, f["json_path"]) == f["original_value"], f["id"]
        assert f["corrected_value"] != f["original_value"], f["id"]
        assert f["classification"] == "NON_EXECUTION_AUTHORITATIVE_DESCRIPTION" and f["executable_authority"], f["id"]
        assert isinstance(f["corrected_value"], str) and isinstance(f["original_value"], str)
        assert any(m in f["original_value"] for m in STALE_MARKERS), f["id"]
        assert not any(m in f["corrected_value"] for m in STALE_MARKERS), f["id"]


def test_the_corrected_copy_differs_from_the_original_only_at_the_listed_paths():
    corrected = ea.corrected_declaration(ORIG, ERR)
    a, b = ea.leaves(ORIG), ea.leaves(corrected)
    assert a.keys() == b.keys()
    assert {p for p in a if a[p] != b[p]} == STALE_PATHS
    assert ea.original_bytes_intact((HERE / ea.ORIGINAL_NAME).read_bytes()), "the in-memory correction must not touch the file"


def test_no_other_batch01_description_was_copied_unreviewed():
    assert ea.unreviewed_copied_descriptions(ORIG, OLD, REVIEWED_STILL_TRUE | STALE_PATHS) == []
    # Negative control: without the erratum the six are exactly what the detector reports.
    assert set(ea.unreviewed_copied_descriptions(ORIG, OLD, REVIEWED_STILL_TRUE)) == STALE_PATHS
    # A future declaration that blindly clones Batch 01 text fails the same detector.
    clone = copy.deepcopy(ORIG)
    ea.pointer_set(clone, "/native_backtest/evidence_root_layout", "runs/run_batch_03/backtest/<strategy_id>/<symbol>")
    assert set(ea.unreviewed_copied_descriptions(clone, OLD, REVIEWED_STILL_TRUE)) ==         STALE_PATHS - {"/native_backtest/evidence_root_layout"}


# ---------------------------------------------------------------------------------------------------------
# Invariance: correcting only the descriptive fields cannot reach any identity or executable input.
# ---------------------------------------------------------------------------------------------------------

def test_no_declaration_reader_names_a_stale_field():
    sources = "".join((HERE / n).read_text(encoding="utf-8") for n in
                      ("run_batch.py", "summarize_batch.py", "select_batch.py", "holdout_guard.py"))
    for key in ("warmup_note", "quantity_semantics", "initial_cash_rule", "evidence_root_layout", "survivorship_note"):
        assert f'"{key}"' not in sources, key


@pytest.fixture
def world(tmp_path):
    return ResearchResultStore(tmp_path / "research.sqlite3"), manifest(tmp_path)


def _snap(decl, man):
    return ea.snapshot_under(rb, decl, fingerprints(), man)


@pytest.mark.parametrize("scope", [None] + [f"E{i}" for i in range(1, 7)])
def test_correcting_only_the_descriptive_fields_moves_no_identity_or_executable_input(world, scope):
    _, man = world
    corrected = ea.corrected_declaration(ORIG, ERR, None if scope is None else {scope})
    assert corrected != ORIG
    before, after = _snap(ORIG, man), _snap(corrected, man)
    assert ea.snapshot_diff(before, after) == []
    assert before == after
    assert len(before["trial_ids"]) == 15 and len({r[2] for r in before["trial_ids"]}) == 15
    assert before["sizing_args"] == ["--sizing-policy", "fixed_initial_capital_fraction_v1", "--allocation-fraction-bps",
                                     "1000", "--max-position-notional-usd", "50000"]
    assert before["stress_plan"]["allocation_fraction_bps"] == 500
    assert before["benchmark"]["scanner_policy"] == "capital_fraction_matched_passive_buy_hold_v1"


def test_registration_and_attempt_inventory_are_unchanged_under_the_corrected_declaration(world):
    store, man = world
    corrected = ea.corrected_declaration(ORIG, ERR)
    with ea.using_declaration(rb, ORIG):
        expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    with ea.using_declaration(rb, corrected):
        again = rb.expected_trial_ids(fingerprints(), man)
        assert again == expected
        assert rb.registration_gate(store, rb.EXPERIMENT, again, require_zero_attempts=True) == {
            "registered": 15, "attempts": 0}
    for _ in range(2):
        store.begin_attempt(trial_id=expected[0][2], origin="test", metadata={})
    with ea.using_declaration(rb, ORIG):
        before = rb.registration_gate(store, rb.EXPERIMENT, expected, require_zero_attempts=False)
    with ea.using_declaration(rb, corrected):
        after = rb.registration_gate(store, rb.EXPERIMENT, expected, require_zero_attempts=False)
    assert before == after == {"registered": 15, "attempts": 2}


def test_harness_state_is_restored_after_each_swap(world):
    _, man = world
    keep = copy.deepcopy(rb.DECL)
    _snap(ea.corrected_declaration(ORIG, ERR), man)
    assert rb.DECL == keep and rb.TRIALS == [(t["strategy_id"], t["symbol"]) for t in keep["universe"]["trials"]]


def _set(path, value):
    return lambda d: ea.pointer_set(d, path, value)


def _stress_bps_400(d):
    s = d["robustness"]["p7a_p7b_stress"]["stress_sizing"]
    s["allocation_fraction_bps"], s["nominal_entry_budget_micros"] = 400, 4_000_000_000


def _bps_1001(d):
    d["capital_sizing"].update(allocation_fraction_bps=1001, nominal_entry_budget_micros=10_010_000_000)


def _swap_trials(d):
    t = d["universe"]["trials"]
    t[0], t[1] = t[1], t[0]
    t[0]["order"], t[1]["order"] = 1, 2


EXECUTABLE_CONTROLS = {
    "baseline bps": _bps_1001,
    "stress bps": _stress_bps_400,
    "commission": _set("/economic_protocol/cost_model/commission_bps_per_side", 5.0),
    "execution slippage": _set("/economic_protocol/execution_pricing/slippage_bps", 1),
    "signal entry threshold": _set("/economic_protocol/signal_policy/entry_threshold", 0.4),
    "stress execution slippage": _set("/robustness/p7a_p7b_stress/stress_execution_slippage_bps", 16),
    "robustness block counts": _set("/robustness/block_counts/0", 7),
    "partition test months": _set("/partition/test_months", 6),
    "H3 required history": _set("/hypotheses/2/required_history_bars", 59),
    "hypothesis rationale (registry text)": _set("/hypotheses/0/economic_rationale", "changed"),
    "trial order": _swap_trials,
    "promotion threshold": _set("/promotion_policy/MQK_PROMOTION_MIN_SHARPE", 0.4),
    "run dir": _set("/run_dir", "runs/run_batch_03"),
    "data pin": _set("/data/reuse_verified_data_from/expected_row_count", 13399),
    "max position cap": lambda d: d["capital_sizing"].update(max_position_notional_usd=60000),
}


@pytest.mark.parametrize("name", sorted(EXECUTABLE_CONTROLS))
def test_negative_control_an_executable_edit_is_noticed_by_the_same_harness(world, name):
    _, man = world
    mutated = copy.deepcopy(ORIG)
    EXECUTABLE_CONTROLS[name](mutated)
    try:
        after = _snap(mutated, man)
    except (SystemExit, ValueError):
        return  # a fail-closed refusal is also "noticed"
    assert ea.snapshot_diff(_snap(ORIG, man), after), name


@pytest.mark.parametrize("name", ["scanner benchmark policy", "scanner benchmark policy removed", "initial cash"])
def test_negative_control_benchmark_and_capital_edits_are_refused_or_move_the_snapshot(world, name):
    _, man = world
    mutated = copy.deepcopy(ORIG)
    if name == "scanner benchmark policy":
        mutated["scanner_review"]["benchmark_policy"] = "capital_matched_exact_target_buy_hold_v1"
    elif name == "scanner benchmark policy removed":
        mutated["scanner_review"].pop("benchmark_policy")
    else:
        mutated["native_backtest"]["initial_cash_micros"] = 90_000_000_000
    with pytest.raises(SystemExit):
        _snap(mutated, man)


def test_a_falsely_descriptive_executable_field_would_fail_the_invariance_proof(world):
    """Mutation proof for the erratum itself: list an executable path as 'descriptive' and the harness reports it."""
    _, man = world
    fake = copy.deepcopy(ERR)
    fake["stale_fields"].append({"id": "EX", "json_path": "/economic_protocol/cost_model/commission_bps_per_side",
                                 "original_value": 10.0, "corrected_value": 9.0})
    corrected = ea.corrected_declaration(ORIG, fake)
    diff = ea.snapshot_diff(_snap(ORIG, man), _snap(corrected, man))
    assert {"trial_ids", "economic_spec", "economic_inputs"} <= set(diff)


def test_correction_refuses_when_the_original_value_has_moved():
    fake = copy.deepcopy(ERR)
    fake["stale_fields"][0]["original_value"] = "something else"
    with pytest.raises(ValueError, match="no longer carries"):
        ea.corrected_declaration(ORIG, fake)
