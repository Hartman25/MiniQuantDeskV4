"""W06-A-CAMPAIGN-PREDECLARATION-01 (LIQ-01) -- proves the predeclaration is
internally consistent, network-free in check mode, and hard-guarded against
accidental real execution. Adapted from
discovery_01_low_volatility_anomaly/test_predeclaration.py (same
required-test numbering convention), trimmed to this experiment's single
LIQ-01 family.

None of these tests import mqk_research.data.alpaca_historical or any
network-capable module at collection time -- run_wave.py itself only
imports that module lazily, inside ensure_bars(), which these tests never
call.
"""
from __future__ import annotations

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
# REQUIRED TESTS 1-6: seed universe truth (identical universe to SHORT-WAVE-03)
# ---------------------------------------------------------------------------


def test_seed_universe_derives_from_committed_registry_snapshot() -> None:
    """REQUIRED TEST 1: rebuilding the snapshot from the currently-committed
    config/instruments/equities.json reproduces the frozen SEED_UNIVERSE.json's
    universe_id exactly -- same universe as SHORT-WAVE-03, reused unchanged."""
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
    assert seed["symbol_count"] == 88


def test_universe_id_recorded() -> None:
    """REQUIRED TEST 4."""
    seed = _seed()
    decl = _decl()
    assert isinstance(seed["universe_id"], str) and len(seed["universe_id"]) == 32
    assert decl["seed_universe"]["universe_id"] == seed["universe_id"]
    # Byte-for-byte the SAME universe_id as SHORT-WAVE-03 (not regenerated).
    assert seed["universe_id"] == "f25e8ec952c1429af7ac3bb58169408e"


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


def test_exactly_two_real_candidates() -> None:
    """REQUIRED TEST 8: this experiment predeclares exactly ONE hypothesis
    family (LIQ-01) -> exactly 2 real candidates (long_only, long_short)."""
    decl = _decl()
    assert len(decl["real_candidate_population"]) == 2
    assert len(set(decl["real_candidate_population"])) == 2


def test_exactly_one_placebo_candidate() -> None:
    """REQUIRED TEST 9."""
    decl = _decl()
    assert len(decl["diagnostic_placebo_population"]) == 1
    assert len(set(decl["diagnostic_placebo_population"])) == 1


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


def test_feature_is_new_and_distinct_from_every_prior_family() -> None:
    """REQUIRED TEST 12: the single predeclared feature
    (illiquidity_amihud_rank_20) is not any of the six feature columns
    already tested by ALPHA-01/SHORT-01/SHORT-WAVE-02/03/DISCOVERY-01
    (momentum_score, slope_60, ret_rank_20, ret_5, gap_pct_1, vol_rank_20)
    -- guards against silently relabeling a retuned variant of a
    REJECTED_NOT_ADVANCED / INCONCLUSIVE mechanism as a new discovery."""
    decl = _decl()
    feature_columns = decl["hypotheses"]["LIQ-01"]["feature_columns"]
    assert feature_columns == ["illiquidity_amihud_rank_20"]
    already_tested = {
        "momentum_score", "slope_60", "ret_rank_20", "ret_5", "gap_pct_1", "vol_rank_20",
    }
    assert set(feature_columns).isdisjoint(already_tested)


def test_source_production_column_is_existing_unmodified_feature() -> None:
    """REQUIRED TEST 12b: the predeclared feature is a LOCAL cross-sectional
    rank of an EXISTING, unmodified feature_set_v1 output column (never a
    newly invented raw statistic) -- guards against silently smuggling new,
    unreviewed statistical methodology into a predeclaration under the guise
    of "reusing existing seams"."""
    decl = _decl()
    source_column = decl["hypotheses"]["LIQ-01"]["source_production_column"]
    assert source_column == "illiquidity_amihud"
    assert run_wave.SOURCE_FEATURE_COLUMN == source_column
    assert run_wave.FEATURE_COLUMN == "illiquidity_amihud_rank_20"

    import numpy as np
    import pandas as pd
    from mqk_research.features.feature_set_v1 import build_feature_set_v1

    rng = np.random.default_rng(0)
    dates = pd.date_range("2020-01-01", periods=100, freq="D", tz="UTC")
    rows = []
    for sym in ("AAA", "BBB", "CCC"):
        price = 100.0
        for d in dates:
            price *= float(np.exp(rng.normal(0, 0.01)))
            vol = float(rng.integers(1000, 5000))
            rows.append(
                {
                    "symbol": sym, "end_ts": d.isoformat(),
                    "open": price, "high": price * 1.01, "low": price * 0.99,
                    "close": price, "volume": vol,
                }
            )
    bars = pd.DataFrame(rows)
    feats = build_feature_set_v1(bars)
    assert source_column in feats.columns  # already computed by unmodified production code
    assert "illiquidity_amihud_rank_20" not in feats.columns  # NOT ranked by production code

    ranked = run_wave.add_cross_sectional_rank(
        feats, source_column=source_column, rank_column="illiquidity_amihud_rank_20"
    )
    valid = ranked["illiquidity_amihud_rank_20"].dropna()
    assert len(valid) > 0
    assert float(valid.min()) >= 0.0
    assert float(valid.max()) <= 1.0


def test_data_feed_frozen_to_sip() -> None:
    """REQUIRED TEST 13."""
    assert _decl()["data"]["feed"] == "sip"
    assert run_wave.FEED == "sip"


def test_date_range_frozen_and_extends_past_every_prior_experiment() -> None:
    """REQUIRED TEST 14: start_utc unchanged from SHORT-WAVE-03 (2016-01-01),
    but end_utc/asof extend past every prior experiment's 2024-01-01 cutoff
    -- this is the fresh-data predeclaration, frozen before any strategy
    result was observed (narrowed once, from an initially-predeclared
    2026-09-05, purely in reaction to a DATA provenance boundary --see
    PREDECLARED_WAVE.json "data".freshness_note -- never in reaction to an
    economic/strategy result)."""
    decl = _decl()
    assert decl["data"]["start_utc"] == "2016-01-01T00:00:00Z"
    assert decl["data"]["end_utc"] == "2025-05-01T00:00:00Z"
    assert decl["data"]["asof"] == "2025-05-01"
    end_ts = __import__("pandas").Timestamp(decl["data"]["end_utc"])
    prior_experiments_cutoff = __import__("pandas").Timestamp("2024-01-01T00:00:00Z")
    assert end_ts > prior_experiments_cutoff


def test_holdout_months_is_6() -> None:
    """REQUIRED TEST 15."""
    assert _decl()["walk_forward"]["holdout_months"] == 6


def test_placebo_seed_is_frozen() -> None:
    """REQUIRED TEST 16."""
    assert _decl()["placebo_seed"] == 60601
    assert run_wave.PLACEBO_SEED == 60601


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
    legitimate exception -- pure documentation of which result field LABELS
    the run must capture, containing no populated numeric result itself."""
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
    (SystemExit(3)) unless the literal --execute flag is present."""
    for stage in sorted(run_wave.EXECUTE_REQUIRED_STAGES):
        with pytest.raises(SystemExit) as exc_info:
            run_wave.main([stage])
        assert exc_info.value.code == 3


# ---------------------------------------------------------------------------
# Driver/predeclaration agreement (backs every test above)
# ---------------------------------------------------------------------------


def test_driver_agrees_with_predeclaration() -> None:
    run_wave.assert_driver_agrees_with_predeclaration()


def test_causal_placebo_actually_changes_targets() -> None:
    """Negative control (mission Section 10): the causal placebo helper must
    actually permute at least one (fwd_ret, target) pair -- reused verbatim
    from the already-accepted implementation, but re-proven here against a
    small synthetic fixture specific to this experiment's own seed/wiring."""
    import pandas as pd

    targets = pd.DataFrame(
        {
            "symbol": ["AAA", "BBB", "CCC", "DDD"] * 3,
            "end_ts": sorted(["2020-01-0" + str(i) for i in range(1, 4)]) * 4,
            "label_end_ts": sorted(["2020-01-1" + str(i) for i in range(1, 4)]) * 4,
            "fwd_ret": [0.01, -0.02, 0.03, -0.04] * 3,
            "target": [1, 0, 1, 0] * 3,
        }
    )
    placebo = run_wave.build_causal_placebo_targets(targets, seed=run_wave.PLACEBO_SEED)
    assert not placebo["target"].equals(targets["target"]) or not placebo["fwd_ret"].equals(targets["fwd_ret"])
    # Grouping key (end_ts, label_end_ts) membership must be preserved exactly
    # -- only within-group permutation, never cross-group leakage.
    assert sorted(placebo["end_ts"]) == sorted(targets["end_ts"])
    assert sorted(placebo["label_end_ts"]) == sorted(targets["label_end_ts"])
