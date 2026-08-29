"""SHORT-WAVE-03-BROAD-DIRECT-RANK-PREDECLARATION-01 -- proves the
predeclaration is internally consistent, network-free in check mode, and
hard-guarded against accidental real execution. Mirrors the mission's
Patch E "WAVE-03 PREDECLARATION TESTS" list (20 items, referenced by
number in each test's docstring).

None of these tests import mqk_research.data.alpaca_historical or any
network-capable module at collection time -- run_wave.py itself only
imports that module lazily, inside ensure_bars(), which these tests never
call.
"""
from __future__ import annotations

import importlib
import json
import sys
from pathlib import Path

import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402  (local experiment module, path inserted above)


def _decl() -> dict:
    return json.loads((EXPERIMENT_ROOT / "PREDECLARED_WAVE.json").read_text(encoding="utf-8"))


def _seed() -> dict:
    return json.loads((EXPERIMENT_ROOT / "SEED_UNIVERSE.json").read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# REQUIRED TESTS 1-6: seed universe truth
# ---------------------------------------------------------------------------


def test_seed_universe_derives_from_committed_registry_snapshot() -> None:
    """REQUIRED TEST 1: rebuilding the snapshot from the currently-committed
    config/instruments/equities.json via the Patch D module reproduces the
    frozen SEED_UNIVERSE.json's universe_id exactly (no drift between the
    frozen artifact and its stated source, at the time this was frozen)."""
    from mqk_research.universe.snapshot import build_current_enabled_equity_registry_snapshot

    registry_path = EXPERIMENT_ROOT.parents[2] / "config" / "instruments" / "equities.json"
    rebuilt = build_current_enabled_equity_registry_snapshot(registry_path)
    seed = _seed()
    assert rebuilt.universe_id == seed["universe_id"]
    assert list(rebuilt.symbols) == seed["symbols"]


def test_seed_symbols_unique_and_sorted() -> None:
    """REQUIRED TEST 2."""
    symbols = run_wave.seed_symbols()
    assert symbols == sorted(symbols)
    assert len(symbols) == len(set(symbols))


def test_actual_seed_count_recorded() -> None:
    """REQUIRED TEST 3."""
    seed = _seed()
    decl = _decl()
    assert seed["symbol_count"] == len(seed["symbols"])
    assert decl["seed_universe"]["symbol_count"] == seed["symbol_count"]
    assert seed["symbol_count"] > 12  # broader than the old fixed 12-ETF universe


def test_universe_id_recorded() -> None:
    """REQUIRED TEST 4."""
    seed = _seed()
    decl = _decl()
    assert isinstance(seed["universe_id"], str) and len(seed["universe_id"]) == 32
    assert decl["seed_universe"]["universe_id"] == seed["universe_id"]


def test_point_in_time_membership_false() -> None:
    """REQUIRED TEST 5."""
    seed = _seed()
    assert seed["point_in_time_membership"] is False
    assert _decl()["seed_universe"]["point_in_time_membership"] is False


def test_survivorship_caveat_recorded() -> None:
    """REQUIRED TEST 6."""
    seed = _seed()
    assert seed["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"
    assert _decl()["seed_universe"]["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"


# ---------------------------------------------------------------------------
# REQUIRED TESTS 7-11: candidate population / policy structure
# ---------------------------------------------------------------------------


def test_rank_side_count_frozen_at_5() -> None:
    """REQUIRED TEST 7."""
    decl = _decl()
    assert decl["rank_side_count"] == 5
    assert decl["signal_policies"]["rank_long_only"]["rank_side_count"] == 5
    assert decl["signal_policies"]["rank_long_short"]["rank_side_count"] == 5
    assert run_wave.RANK_SIDE_COUNT == 5


def test_exactly_six_real_candidates() -> None:
    """REQUIRED TEST 8."""
    decl = _decl()
    assert len(decl["real_candidate_population"]) == 6
    assert len(set(decl["real_candidate_population"])) == 6


def test_exactly_three_placebo_candidates() -> None:
    """REQUIRED TEST 9."""
    decl = _decl()
    assert len(decl["diagnostic_placebo_population"]) == 3
    assert len(set(decl["diagnostic_placebo_population"])) == 3


def test_real_and_placebo_experiment_ids_differ() -> None:
    """REQUIRED TEST 10."""
    decl = _decl()
    assert decl["real_experiment_id"] != decl["placebo_experiment_id"]
    assert set(decl["real_candidate_population"]).isdisjoint(set(decl["diagnostic_placebo_population"]))


def test_only_new_direct_rank_policies_used() -> None:
    """REQUIRED TEST 11."""
    decl = _decl()
    sp = decl["signal_policies"]
    assert sp["rank_long_only"]["direction_policy"] == "cross_sectional_rank_long_only_v1"
    assert sp["rank_long_short"]["direction_policy"] == "cross_sectional_rank_long_short_v1"


# ---------------------------------------------------------------------------
# REQUIRED TESTS 12-16: frozen data/model constants
# ---------------------------------------------------------------------------


def test_same_three_frozen_features() -> None:
    """REQUIRED TEST 12: same three single-feature families as SHORT-WAVE-02."""
    decl = _decl()
    feature_columns = sorted(
        decl["hypotheses"][k]["feature_columns"][0] for k in ("RANK-01", "RANK-02", "RANK-03")
    )
    assert feature_columns == sorted(["ret_rank_20", "ret_5", "gap_pct_1"])
    for k in ("RANK-01", "RANK-02", "RANK-03"):
        assert len(decl["hypotheses"][k]["feature_columns"]) == 1  # single-feature schema


def test_data_feed_frozen_to_sip() -> None:
    """REQUIRED TEST 13."""
    assert _decl()["data"]["feed"] == "sip"
    assert run_wave.FEED == "sip"


def test_date_range_frozen() -> None:
    """REQUIRED TEST 14."""
    decl = _decl()
    assert decl["data"]["start_utc"] == "2016-01-01T00:00:00Z"
    assert decl["data"]["end_utc"] == "2024-01-01T00:00:00Z"
    assert decl["data"]["asof"] == "2024-01-01"


def test_holdout_months_is_6() -> None:
    """REQUIRED TEST 15."""
    assert _decl()["walk_forward"]["holdout_months"] == 6


def test_placebo_seed_is_1234() -> None:
    """REQUIRED TEST 16."""
    assert _decl()["placebo_seed"] == 1234
    assert run_wave.PLACEBO_SEED == 1234


# ---------------------------------------------------------------------------
# REQUIRED TESTS 17-18: no threshold policy, no result field
# ---------------------------------------------------------------------------


def test_no_threshold_long_short_candidate_exists() -> None:
    """REQUIRED TEST 17."""
    decl = _decl()
    for policy in decl["signal_policies"].values():
        assert policy["direction_policy"] != "long_short_threshold_v1"
    for hyp_id in decl["real_candidate_population"] + decl["diagnostic_placebo_population"]:
        assert "threshold" not in hyp_id


def test_no_result_or_pnl_field_in_predeclaration() -> None:
    """REQUIRED TEST 18: no ACTUAL result/P&L VALUE is embedded anywhere in
    the predeclaration. `required_future_run_recording_fields` is the one
    legitimate exception -- it names, as pure documentation, which result
    field LABELS the future RUN mission must capture (mission "WAVE-03
    FUTURE ACCEPTANCE QUESTIONS"/"FUTURE RUN MUST RECORD"); it contains no
    populated numeric result itself, so it is excluded from this scan."""
    decl = dict(_decl())
    decl.pop("required_future_run_recording_fields", None)
    blob = json.dumps(decl).lower()
    for forbidden in ("sharpe", "net_total_return", "gross_total_return", "trial_id", "economic_eval_id"):
        assert forbidden not in blob
    seed_blob = json.dumps(_seed()).lower()
    for forbidden in ("sharpe", "net_total_return", "gross_total_return"):
        assert forbidden not in seed_blob


# ---------------------------------------------------------------------------
# REQUIRED TESTS 19-20: no network in check mode / execution guard
# ---------------------------------------------------------------------------


def test_check_mode_never_contacts_alpaca(capsys: pytest.CaptureFixture[str]) -> None:
    """REQUIRED TEST 19: run_wave.py's own source never imports the Alpaca
    historical-data module at top level (only lazily inside ensure_bars(),
    which `check` never calls); running `check` succeeds with no network
    module even present in sys.modules as a result of this import."""
    source = (EXPERIMENT_ROOT / "run_wave.py").read_text(encoding="utf-8")
    top_level_lines = [
        line for line in source.splitlines()
        if line.startswith("from mqk_research.data.alpaca_historical") or line.startswith("import mqk_research.data.alpaca_historical")
    ]
    assert top_level_lines == []  # only ensure_bars()'s lazy import may reference it

    run_wave.main(["check"])
    captured = capsys.readouterr()
    assert "PREDECLARATION_AGREEMENT=PASS" in captured.out
    assert "mqk_research.data.alpaca_historical" not in sys.modules


def test_execution_requires_explicit_authorization() -> None:
    """REQUIRED TEST 20: every EXECUTE_REQUIRED_STAGES stage refuses
    (SystemExit(3)) unless the literal --execute flag is present; this
    predeclaration controller never passes it. The guard lives ENTIRELY in
    main()'s dispatch (mirroring SHORT-WAVE-02's accepted design) --
    run_family/run_family_judge are not self-gating stubs once implemented,
    so this test only ever calls them THROUGH main(), never directly (a
    direct run_family() call is real, network-touching execution -- see
    test_wave03_family_harness.py for run_family's own network-free,
    monkeypatched structural tests)."""
    for stage in sorted(run_wave.EXECUTE_REQUIRED_STAGES):
        with pytest.raises(SystemExit) as exc_info:
            run_wave.main([stage])
        assert exc_info.value.code == 3

    with pytest.raises(NotImplementedError):
        run_wave.run_family_judge()


# ---------------------------------------------------------------------------
# Driver/predeclaration agreement (backs every test above)
# ---------------------------------------------------------------------------


def test_driver_agrees_with_predeclaration() -> None:
    run_wave.assert_driver_agrees_with_predeclaration()
