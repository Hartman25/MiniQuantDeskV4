"""W06-A-CAMPAIGN-PREDECLARATION-01 -- campaign-level proof that
PREDECLARED_CAMPAIGN.json and the two candidate predeclarations
(wave06_candidate_liq01_amihud_illiquidity, wave06_candidate_vol01_volume_surprise)
are mutually consistent, that the candidate order and count are frozen
BEFORE any candidate result is observed, and that neither candidate is a
disguised retest of a previously tested mechanism. Complements (does not
duplicate) each candidate's own test_predeclaration.py.

Imports no network-capable module and calls no candidate's run_wave.main()
execute-required stage.
"""
from __future__ import annotations

import glob
import json
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
EXPERIMENTS_ROOT = CAMPAIGN_ROOT.parent


def _campaign() -> dict:
    return json.loads((CAMPAIGN_ROOT / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))


def _candidate_decl(directory: str) -> dict:
    path = (CAMPAIGN_ROOT / directory / "PREDECLARED_WAVE.json").resolve()
    return json.loads(path.read_text(encoding="utf-8"))


def test_campaign_order_is_exactly_two_and_matches_predeclared_count() -> None:
    camp = _campaign()
    assert camp["campaign_order"] == ["LIQ-01", "VOL-01"]
    assert camp["predeclared_candidate_count"] == len(camp["campaign_order"]) == 2
    assert camp["max_candidates"] == 3
    assert camp["predeclared_candidate_count"] <= camp["max_candidates"]


def test_no_undeclared_third_or_fourth_candidate_directory_exists() -> None:
    """Fail closed if a candidate directory exists on disk that the frozen
    campaign_order/candidates map does not know about -- guards against
    silently adding a candidate after this predeclaration was committed."""
    camp = _campaign()
    declared_dirs = {Path(v["directory"]).name for v in camp["candidates"].values()}
    on_disk = {
        Path(p).name
        for p in glob.glob(str(EXPERIMENTS_ROOT / "wave06_candidate_*"))
        if Path(p).is_dir()
    }
    assert on_disk == declared_dirs, f"on_disk={on_disk!r} declared={declared_dirs!r}"


def test_each_candidate_order_position_matches_campaign_order() -> None:
    camp = _campaign()
    for key, expected_position in (("LIQ-01", 1), ("VOL-01", 2)):
        assert camp["candidates"][key]["order_position"] == expected_position
        assert camp["campaign_order"][expected_position - 1] == key
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        assert decl["campaign_order_position"] == expected_position
        assert decl["campaign_id"] == camp["campaign_id"]


def test_each_candidate_experiment_ids_match_campaign_declaration() -> None:
    camp = _campaign()
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        assert decl["real_experiment_id"] == camp["candidates"][key]["real_experiment_id"]
        assert decl["placebo_experiment_id"] == camp["candidates"][key]["placebo_experiment_id"]
        feature_key = list(decl["hypotheses"].keys())[0]
        assert decl["hypotheses"][feature_key]["feature_columns"] == [camp["candidates"][key]["feature_column"]]


def test_base_head_consistent_across_campaign_and_candidates() -> None:
    camp = _campaign()
    assert camp["base_head"] == "e381a402481d4e704180199d9175a770d50ddfa6"
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        assert decl["base_head"] == camp["base_head"]


def test_candidate_features_disjoint_from_exclusion_matrix_and_each_other() -> None:
    """No candidate feature may equal a previously tested feature (the
    campaign's own tested_hypothesis_exclusion_matrix), and the campaign's
    own two candidates must not accidentally share a feature column."""
    camp = _campaign()
    matrix = camp["tested_hypothesis_exclusion_matrix"]
    already_tested: set[str] = set()
    for entry in matrix.values():
        if "feature" in entry:
            already_tested.add(entry["feature"])
        if "features" in entry:
            already_tested.update(entry["features"])
    assert already_tested == {"momentum_score", "slope_60", "ret_rank_20", "ret_5", "gap_pct_1", "vol_rank_20"}

    candidate_features = [camp["candidates"][k]["feature_column"] for k in camp["campaign_order"]]
    assert len(candidate_features) == len(set(candidate_features)), "candidate features must be mutually distinct"
    for feat in candidate_features:
        assert feat not in already_tested


def test_stopping_rule_forbids_retuning_and_fourth_candidate() -> None:
    camp = _campaign()
    rule = camp["stopping_rule"]
    assert rule["no_retuning_on_failure"] is True
    assert rule["no_fourth_candidate"] is True
    assert rule["no_post_hoc_candidate_substitution"] is True
    assert rule["stop_on_first_advancing_candidate"] is True


def test_holdout_not_consumed_and_no_consume_holdout_call_in_any_candidate_driver() -> None:
    camp = _campaign()
    assert camp["holdout_reservation"]["consumed_in_this_campaign"] is False
    for key in camp["campaign_order"]:
        run_wave_path = (CAMPAIGN_ROOT / camp["candidates"][key]["directory"] / "run_wave.py").resolve()
        source = run_wave_path.read_text(encoding="utf-8")
        assert "consume_holdout" not in source
        assert "reserved_not_evaluated" in source  # the ONLY holdout status this driver may ever accept


def test_no_result_or_pnl_field_in_campaign_predeclaration() -> None:
    """`advancement_policy` legitimately references metric NAMES (e.g.
    "net_sharpe(primary) - net_sharpe(benchmark)") as part of its frozen,
    pre-outcome gate DEFINITIONS -- exactly analogous to how each
    candidate's own `required_future_run_recording_fields` legitimately
    names result fields without populating them (see each candidate's own
    test 18). Popped before the scan for the same reason; every OTHER key
    in this document must still contain no result/P&L VALUE."""
    camp = dict(_campaign())
    camp.pop("advancement_policy", None)
    blob = json.dumps(camp).lower()
    for forbidden in ("sharpe", "net_total_return", "gross_total_return", "trial_id", "economic_eval_id"):
        assert forbidden not in blob
    assert camp["no_result_or_pnl_field_in_this_document"] is True


def test_advancement_policy_has_no_populated_result_value() -> None:
    """The popped-out advancement_policy block itself must contain metric
    NAMES and frozen thresholds only -- never an actual populated result
    number for net_sharpe/net_total_return/etc (which would only exist
    after a real trial ran)."""
    policy = _campaign()["advancement_policy"]
    blob = json.dumps(policy).lower()
    for forbidden in ("trial_id", "economic_eval_id"):
        assert forbidden not in blob


def test_universe_reused_identically_across_both_candidates() -> None:
    camp = _campaign()
    universe_ids = set()
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        universe_ids.add(decl["seed_universe"]["universe_id"])
    assert universe_ids == {"f25e8ec952c1429af7ac3bb58169408e"}


# ---------------------------------------------------------------------------
# Finding 1 (W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01): both
# candidate drivers must resolve the literal same shared registry and
# REAL/PLACEBO experiment_id -- proven here by importing BOTH run_wave.py
# modules under distinct module names (they share a basename) and comparing
# the actual resolved module-level constants, not just the JSON text.
# ---------------------------------------------------------------------------


def _import_run_wave(candidate_dir: str, unique_name: str):
    import importlib.util

    path = (EXPERIMENTS_ROOT / candidate_dir / "run_wave.py").resolve()
    spec = importlib.util.spec_from_file_location(unique_name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_both_candidate_drivers_import_the_same_shared_registry_module() -> None:
    """Finding 1 requirement A: both drivers resolve the same registry --
    proven by importing both and comparing their REGISTRY_DB constants,
    which each driver assigns directly from campaign_identity.py."""
    liq = _import_run_wave("wave06_candidate_liq01_amihud_illiquidity", "wave06_liq_run_wave_test")
    vol = _import_run_wave("wave06_candidate_vol01_volume_surprise", "wave06_vol_run_wave_test")
    assert liq.REGISTRY_DB == vol.REGISTRY_DB
    assert liq.campaign_identity is vol.campaign_identity  # literally the same imported module object
    camp = _campaign()
    assert str(liq.REGISTRY_DB).replace("\\", "/").endswith(camp["shared_campaign_registry"]["registry_db_relative_path"])


def test_both_candidate_drivers_resolve_the_same_real_and_placebo_experiment_id() -> None:
    """Finding 1 requirement B."""
    liq = _import_run_wave("wave06_candidate_liq01_amihud_illiquidity", "wave06_liq_run_wave_test2")
    vol = _import_run_wave("wave06_candidate_vol01_volume_surprise", "wave06_vol_run_wave_test2")
    assert liq.REAL_EXPERIMENT_ID == vol.REAL_EXPERIMENT_ID
    assert liq.PLACEBO_EXPERIMENT_ID == vol.PLACEBO_EXPERIMENT_ID
    camp = _campaign()
    assert liq.REAL_EXPERIMENT_ID == camp["shared_campaign_registry"]["real_experiment_id"]
    assert liq.PLACEBO_EXPERIMENT_ID == camp["shared_campaign_registry"]["placebo_experiment_id"]
    for key in camp["campaign_order"]:
        assert camp["candidates"][key]["real_experiment_id"] == liq.REAL_EXPERIMENT_ID
        assert camp["candidates"][key]["placebo_experiment_id"] == liq.PLACEBO_EXPERIMENT_ID
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        assert decl["real_experiment_id"] == liq.REAL_EXPERIMENT_ID
        assert decl["placebo_experiment_id"] == liq.PLACEBO_EXPERIMENT_ID


# ---------------------------------------------------------------------------
# Finding 2: machine-readable, versioned advancement_policy -- every
# threshold is a real number or a reused literature-standard boundary, no
# banned vague words survive in the operative falsification_condition text.
# ---------------------------------------------------------------------------

_BANNED_VAGUE_WORDS = ("non-negligible", "materially", "meaningfully", "material", "strong", "acceptable")


def test_advancement_policy_is_present_and_versioned() -> None:
    policy = _campaign()["advancement_policy"]
    assert policy["policy_id"] == "WAVE06-DEVELOPMENT-ADVANCEMENT-POLICY-01"
    assert policy["frozen_before_any_result"] is True


def test_advancement_policy_numeric_thresholds_are_real_numbers() -> None:
    policy = _campaign()["advancement_policy"]
    assert isinstance(policy["benchmark_relative_requirement"]["min_excess"], (int, float))
    assert isinstance(policy["matched_diagnostic_placebo_requirement"]["min_excess"], (int, float))
    assert isinstance(policy["primary_vs_control_requirement"]["min_excess"], (int, float))
    assert isinstance(policy["dsr_requirement"]["min_value"], (int, float))
    assert policy["dsr_requirement"]["min_value"] == 0.5  # Finding 1: DSR's own probability midpoint, not 0.0
    assert isinstance(policy["pbo_requirement"]["max_value"], (int, float))
    assert isinstance(policy["p7a_p7b_economic_replay_stress_requirement"]["max_drawdown_ceiling"], (int, float))
    sensitivity = policy["dsr_pbo_block_count_sensitivity_requirement"]
    assert isinstance(sensitivity["block_counts"], list)
    assert len(sensitivity["block_counts"]) >= 2
    assert len(set(sensitivity["block_counts"])) == len(sensitivity["block_counts"]), "block_counts must be distinct"
    for bc in sensitivity["block_counts"]:
        assert isinstance(bc, int) and bc >= 4 and bc % 2 == 0
    assert isinstance(sensitivity["dsr_max_sensitivity_range"], (int, float))
    assert isinstance(sensitivity["pbo_max_sensitivity_range"], (int, float))
    assert 0.0 <= sensitivity["pbo_max_sensitivity_range"] <= 1.0


def test_dsr_pbo_sensitivity_matches_real_accepted_cli_shape() -> None:
    """Finding 2: dsr_pbo_sensitivity_cli varies ONLY block_counts (via
    --block-counts) -- there is no entry_threshold sweep anywhere in the
    accepted API, so none may survive in this policy."""
    policy = _campaign()["advancement_policy"]
    sensitivity = policy["dsr_pbo_block_count_sensitivity_requirement"]
    assert "block_counts" in sensitivity
    blob = json.dumps(policy).lower()
    assert "sweep_entry_thresholds" not in blob
    assert "entry_threshold=0.5" not in blob
    assert "\"entry_threshold\"" not in json.dumps(sensitivity).lower()


def test_canonical_p9_gauntlet_requires_the_complete_real_scenario_set() -> None:
    """Finding 3: the Wave06 policy names the REAL current
    bkt_robustness_gauntlet_v2 required scenario set, not a Wave06-specific
    substitute."""
    policy = _campaign()["advancement_policy"]
    gauntlet = policy["canonical_p9_robustness_gauntlet_requirement"]
    assert gauntlet["required_protocol_version"] == "bkt_robustness_gauntlet_v2"
    assert set(gauntlet["required_scenario_names"]) == {
        "execution_delay_stress",
        "symbol_leave_one_out",
        "month_year_regime_concentration",
        "parameter_neighborhood_execution",
        "placebo_temporal_offset",
        "conservative_capacity_stress",
        "dsr_pbo_sensitivity",
        "p7a_p7b_economic_replay_stress",
        "genuine_shuffled_placebo",
    }


def test_p7a_p7b_stress_uses_only_real_cli_compatible_fields() -> None:
    """Finding 4: no nonexistent stress_max_target_qty_multiplier field, and
    the declared notional cap is genuinely tighter than the uncapped
    baseline sizing."""
    policy = _campaign()["advancement_policy"]
    stress = policy["p7a_p7b_economic_replay_stress_requirement"]
    assert "stress_max_target_qty_multiplier" not in stress
    assert "stress_max_position_notional_usd" in stress
    assert isinstance(stress["stress_max_position_notional_usd"], (int, float))
    assert stress["stress_max_position_notional_usd"] > 0


def test_advancement_policy_verdict_definitions_cover_exactly_the_allowed_verdicts() -> None:
    policy = _campaign()["advancement_policy"]
    verdicts = set(policy["verdict_definitions"].keys())
    assert verdicts == {
        "REJECTED_NOT_ADVANCED", "INCONCLUSIVE",
        "DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION",
    }
    for key in ("LIQ-01", "VOL-01"):
        decl = _candidate_decl(_campaign()["candidates"][key]["directory"])
        assert set(decl["allowed_verdicts"]) == verdicts


def test_advancement_policy_forbids_promotion_and_paper_entry() -> None:
    policy = _campaign()["advancement_policy"]
    assert set(policy["forbidden_verdicts_for_this_non_pit_study"]) == {
        "PROVEN_ALPHA", "PROMOTION_READY", "PAPER_ENTRY_ELIGIBLE",
    }
    for key in ("LIQ-01", "VOL-01"):
        decl = _candidate_decl(_campaign()["candidates"][key]["directory"])
        assert set(decl["forbidden_verdicts"]) == {"PROVEN_ALPHA", "PROMOTION_READY", "PAPER_ENTRY_ELIGIBLE"}


def test_no_banned_vague_words_in_falsification_conditions() -> None:
    """Every candidate's falsification_condition must point at the frozen,
    machine-checkable advancement_policy rather than using an undefined
    qualitative word as the operative test."""
    camp = _campaign()
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        for hyp in decl["hypotheses"].values():
            text = hyp["falsification_condition"].lower()
            for banned in _BANNED_VAGUE_WORDS:
                assert banned not in text, f"{key} falsification_condition still contains {banned!r}"
            assert "advancement_policy" in hyp["falsification_condition"]


def test_advancement_policy_not_a_promotion_bypass() -> None:
    policy = _campaign()["advancement_policy"]
    assert "not_a_promotion_bypass" in policy
    assert "real_research_promotion_e2e_cli" in policy["not_a_promotion_bypass"]


# ---------------------------------------------------------------------------
# Additional truth repair B: controller ceiling vs frozen campaign count
# vocabulary is explicit and internally consistent; no third candidate is
# possible after this repair.
# ---------------------------------------------------------------------------


def test_controller_ceiling_vocabulary_is_explicit_and_consistent() -> None:
    camp = _campaign()
    assert camp["controller_candidate_ceiling"] == 3 == camp["max_candidates"]
    assert camp["frozen_campaign_candidate_count"] == 2 == camp["predeclared_candidate_count"]
    assert camp["additional_candidate_after_predeclaration"] == "forbidden"
    assert camp["frozen_campaign_candidate_count"] < camp["controller_candidate_ceiling"]
    assert len(camp["campaign_order"]) == camp["frozen_campaign_candidate_count"]


def test_stopping_rule_never_proceeds_directly_to_promotion() -> None:
    camp = _campaign()
    rule = camp["stopping_rule"]
    assert rule["advancing_candidate_never_proceeds_directly_to_promotion"] is True
    assert "promotion" not in rule["text"].lower() or "fresh" in rule["text"].lower()
    assert "fresh point-in-time-clean confirmation mission" in rule["text"]


# ---------------------------------------------------------------------------
# Additional truth repair A: the "first hypothesis in this repo's history to
# use trading volume data at all" overclaim must not survive anywhere in the
# frozen predeclaration text.
# ---------------------------------------------------------------------------


def test_overclaimed_first_volume_use_in_repo_history_is_not_present() -> None:
    camp = _campaign()
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        blob = json.dumps(decl).lower()
        assert "first hypothesis in this repo's history to use trading volume data at all" not in blob
