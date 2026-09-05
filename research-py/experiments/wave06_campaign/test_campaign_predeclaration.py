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
    camp = dict(_campaign())
    blob = json.dumps(camp).lower()
    for forbidden in ("sharpe", "net_total_return", "gross_total_return", "trial_id", "economic_eval_id"):
        assert forbidden not in blob
    assert camp["no_result_or_pnl_field_in_this_document"] is True


def test_universe_reused_identically_across_both_candidates() -> None:
    camp = _campaign()
    universe_ids = set()
    for key in camp["campaign_order"]:
        decl = _candidate_decl(camp["candidates"][key]["directory"])
        universe_ids.add(decl["seed_universe"]["universe_id"])
    assert universe_ids == {"f25e8ec952c1429af7ac3bb58169408e"}
