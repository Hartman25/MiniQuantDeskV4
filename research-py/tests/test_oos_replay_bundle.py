"""
W06-P9-REPLAY-AUTHORITY-01 (Patch A) -- focused tests for
mqk_research.ml.oos_replay_bundle. Covers the mission's Patch A REQUIRED
TESTS list (12 items, referenced by number in each test's docstring).

No network. Every registry/registered-trial fixture is real (constructed
through the actual production entry points: ResearchResultStore,
run_registered_economic_walkforward_eval), not mocked.
"""
from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any, Dict

import numpy as np
import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economic_walkforward import (
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.oos_replay_bundle import (
    ReplayBundleError,
    _assert_no_holdout_rows,
    assert_no_duplicate_schedule_rows,
    build_replay_bundle,
    build_schedule_rows,
    recompute_loo_feature_frame,
    resolve_registered_economic_attempt,
)
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.weight_to_share import WeightToShareSpec

FEATURE_COL = "test_xs_rank"
SYMBOLS = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"]
WF_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)
RANK_SIDE_COUNT = 2

# ---------------------------------------------------------------------------
# Shared end-to-end registered-trial fixture (mirrors test_holdout_wiring.py
# / test_direct_rank_registered_identity.py's own local helper conventions --
# no shared conftest exists in this test package today).
# ---------------------------------------------------------------------------


def _build_dataset(symbols=SYMBOLS, periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    label_rng = np.random.default_rng(seed + 1)
    rows = []
    for d in dates:
        for i, sym in enumerate(symbols):
            raw = float(i)  # deterministic, distinct per-symbol level -> stable, tie-free cross-sectional order
            rows.append({"symbol": sym, "end_ts": d, "raw": raw})
    df = pd.DataFrame(rows)
    df[FEATURE_COL] = df.groupby("end_ts")["raw"].rank(pct=True, method="average")
    df["target"] = (label_rng.random(len(df)) > 0.5).astype(int)
    df["label_end_ts"] = df["end_ts"] + pd.Timedelta(days=horizon_days)
    return df


def _write_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", FEATURE_COL]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_bars(df: pd.DataFrame, symbols=SYMBOLS) -> pd.DataFrame:
    rows = []
    base = {sym: 100.0 + 10.0 * i for i, sym in enumerate(symbols)}
    for ts in sorted(df["end_ts"].unique()):
        for sym in symbols:
            rows.append({"symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(), "close": base[sym]})
    return pd.DataFrame(rows)


def _bars_provenance(bars_path: Path, symbols=SYMBOLS) -> Dict[str, Any]:
    bars = pd.read_csv(bars_path)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    coverage_start = end_ts.min().isoformat()
    coverage_end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="test_fixture_no_known_corporate_actions",
        covered_symbol_universe=sorted(symbols),
        coverage_start_utc=coverage_start,
        coverage_end_utc=coverage_end,
        corporate_action_entries=(),
    )
    return build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["test_fixture"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "synthetic test fixture -- no real provider involved",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc=coverage_start,
        end_utc=coverage_end,
        symbol_universe=sorted(symbols),
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        artifact_path=bars_path,
    )


def _register_trial(tmp_path: Path, *, trial_label: str, seed: int = 0, l2: float = 1e-3) -> Dict[str, Any]:
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / f"run_{trial_label}"
    df = _build_dataset(seed=seed)
    _write_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_bars(df).to_csv(bars_path, index=False)
    manifest = _bars_provenance(bars_path)

    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=RANK_SIDE_COUNT, max_gross_exposure=1.0,
        ),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
    )
    economic_out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="w06.p9.replay.test",
        hypothesis_id=f"w06.p9.replay.hyp.{trial_label}",
        strategy_id="test_single_feature_xs_rank_direct_rank_v1",
        bars_csv=bars_path,
        economic_spec=spec,
        bars_provenance=manifest,
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**WF_SPEC_KW),
        l2=l2,
        steps=5,
    )
    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))
    return {
        "registry_db": registry_db,
        "run_dir": run_dir,
        "trial_id": economic_out["registry"]["trial_id"],
        "economic_eval_id": economic_out["ids"]["economic_eval_id"],
        "economic_out": economic_out,
    }


# ---------------------------------------------------------------------------
# REQUIRED TESTS 1-4: resolution / re-verification refusals
# ---------------------------------------------------------------------------


def test_real_synthetic_registry_trial_replay_bundle_succeeds(tmp_path: Path) -> None:
    """REQUIRED TEST 1."""
    reg = _register_trial(tmp_path, trial_label="a")
    manifest_path = build_replay_bundle(
        reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
        out_dir=tmp_path / "bundle", excluded_symbols=["AAA", "FFF"],
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["lineage"]["trial_id"] == reg["trial_id"]
    assert manifest["lineage"]["economic_eval_id"] == reg["economic_eval_id"]
    assert (tmp_path / "bundle" / manifest["baseline_schedule"]["file"]).exists()
    for sym in ("AAA", "FFF"):
        assert (tmp_path / "bundle" / manifest["symbol_loo_schedules"][sym]["file"]).exists()


def test_wrong_economic_eval_id_refusal(tmp_path: Path) -> None:
    """REQUIRED TEST 2."""
    reg = _register_trial(tmp_path, trial_label="a")
    with pytest.raises(ReplayBundleError, match="no succeeded attempt"):
        resolve_registered_economic_attempt(
            reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id="not-the-real-id"
        )


def test_mutated_recorded_source_file_refusal(tmp_path: Path) -> None:
    """REQUIRED TEST 3."""
    reg = _register_trial(tmp_path, trial_label="a")
    features_path = reg["run_dir"] / "features.csv"
    text = features_path.read_text(encoding="utf-8")
    features_path.write_text(text + "\n# mutated after registration\n", encoding="utf-8")
    with pytest.raises(ReplayBundleError, match="no longer matches"):
        build_replay_bundle(
            reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
            out_dir=tmp_path / "bundle", excluded_symbols=[],
        )


def test_missing_recorded_source_file_refusal(tmp_path: Path) -> None:
    """REQUIRED TEST 4."""
    reg = _register_trial(tmp_path, trial_label="a")
    (reg["run_dir"] / "targets.csv").unlink()
    with pytest.raises(ReplayBundleError, match="missing required registry input"):
        build_replay_bundle(
            reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
            out_dir=tmp_path / "bundle", excluded_symbols=[],
        )


# ---------------------------------------------------------------------------
# REQUIRED TEST 5: duplicate (symbol, decision_ts) refusal
# ---------------------------------------------------------------------------


def test_duplicate_symbol_decision_ts_refusal() -> None:
    """REQUIRED TEST 5."""
    rows = [
        {"decision_ts": "2020-01-01T00:00:00+00:00", "symbol": "AAA", "target_qty": 1},
        {"decision_ts": "2020-01-01T00:00:00+00:00", "symbol": "AAA", "target_qty": -1},
    ]
    with pytest.raises(ReplayBundleError, match="duplicate"):
        assert_no_duplicate_schedule_rows(rows)


# ---------------------------------------------------------------------------
# REQUIRED TESTS 6-7: baseline reconstruction self-consistency gate
# ---------------------------------------------------------------------------


def test_baseline_deterministic_refit_reproduction_succeeds(tmp_path: Path) -> None:
    """REQUIRED TEST 6: build_replay_bundle only succeeds because the
    mandatory verify_baseline_oos_reproduction gate passed -- this exercises
    it as an integration proof (see oos_replay_bundle.build_replay_bundle,
    which calls it unconditionally before any schedule is trusted)."""
    reg = _register_trial(tmp_path, trial_label="a")
    manifest_path = build_replay_bundle(
        reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
        out_dir=tmp_path / "bundle", excluded_symbols=[],
    )
    assert manifest_path.exists()


def test_mutated_training_row_reproduction_refuses(tmp_path: Path) -> None:
    """REQUIRED TEST 7: unit-tests the mandatory self-consistency gate
    directly (mission A3) -- mutating an in-memory training-region feature
    value before refitting must diverge from the recorded OOS predictions
    and fail closed, independent of any file-hash mechanics."""
    from mqk_research.ml.oos_replay_bundle import reconstruct_fold_models, verify_baseline_oos_reproduction
    from mqk_research.ml.economic_walkforward import load_oos_predictions

    reg = _register_trial(tmp_path, trial_label="a")
    run_dir = reg["run_dir"]
    features_df = pd.read_csv(run_dir / "features.csv")
    targets_df = pd.read_csv(run_dir / "targets.csv")
    schema = json.loads((run_dir / "feature_schema.json").read_text(encoding="utf-8"))
    identity = json.loads(
        sqlite3.connect(reg["registry_db"]).execute(
            "select identity_json from research_trials where trial_id=?", (reg["trial_id"],)
        ).fetchone()[0]
    )
    wf_spec = WalkForwardSpec(**{
        k: identity["evaluation_spec"][k]
        for k in ("train_years", "test_months", "step_months", "min_rows_per_fold", "purge_enabled",
                  "label_end_ts_col", "embargo_seconds", "holdout_months")
    }).normalized()

    recorded_oos = load_oos_predictions(run_dir / "eval" / "walk_forward_oos_predictions.csv")

    # Mutate a training-region row's feature value (leave test-fold rows
    # untouched so this isn't merely re-deriving a hash mismatch).
    mutated = features_df.copy()
    mutated.loc[0, FEATURE_COL] = 999.0

    _fold_models, reconstructed = reconstruct_fold_models(
        features_df=mutated, targets_df=targets_df, schema=schema,
        end_ts_col=identity["evaluation_spec"]["end_ts_col"], label_col=identity["evaluation_spec"]["label_col"],
        label_end_ts_col=identity["evaluation_spec"]["label_end_ts_col"], wf_spec=wf_spec,
        l2=identity["model_spec"]["l2"], lr=identity["model_spec"]["lr"], steps=identity["model_spec"]["steps"],
        standardize=identity["model_spec"]["standardize"], clip_z=identity["model_spec"]["clip_z"],
    )
    with pytest.raises(ReplayBundleError, match="baseline reproduction failed"):
        verify_baseline_oos_reproduction(reconstructed, recorded_oos)


# ---------------------------------------------------------------------------
# REQUIRED TESTS 8-9: LOO cross-sectional feature recomputation mutation /
# no-effect fixtures.
# ---------------------------------------------------------------------------


def _three_symbol_feature_frame() -> pd.DataFrame:
    ts = pd.Timestamp("2021-01-01T00:00:00+00:00")
    raw = {"X1": 1.0, "X2": 2.0, "X3": 3.0}
    df = pd.DataFrame({"symbol": list(raw.keys()), "end_ts": [ts] * 3, "raw": list(raw.values())})
    df[FEATURE_COL] = df.groupby("end_ts")["raw"].rank(pct=True, method="average")
    return df


def test_naive_frozen_score_loo_differs_from_correct_recomputation() -> None:
    """REQUIRED TEST 8: excluding the MIDDLE symbol (X2) changes the
    remaining LOWER survivor's (X1) cross-sectional percentile value --
    proving the naive approach (reuse the ORIGINAL full-universe percentile,
    merely dropping X2's row) is NOT the same as properly recomputing the
    percentile rank over the survivor set."""
    df = _three_symbol_feature_frame()
    naive = df[df["symbol"] != "X2"].set_index("symbol")[FEATURE_COL]
    correct = recompute_loo_feature_frame(
        df, feature_col=FEATURE_COL, end_ts_col="end_ts", symbol_col="symbol", excluded_symbol="X2"
    ).set_index("symbol")[FEATURE_COL]
    assert naive.loc["X1"] != pytest.approx(correct.loc["X1"])


def test_loo_recomputation_no_effect_fixture_for_unaffected_extreme() -> None:
    """REQUIRED TEST 9 (negative control for TEST 8): the cross-sectional
    MAXIMUM (X3) is unaffected by excluding a strictly lower symbol (X2) --
    its percentile rank is 1.0 (the group maximum) both before and after
    exclusion, byte-identical. Proves TEST 8's divergence assertion is not
    vacuously true for every symbol/exclusion pair -- only genuinely
    order-position-dependent members move."""
    df = _three_symbol_feature_frame()
    naive = df[df["symbol"] != "X2"].set_index("symbol")[FEATURE_COL]
    correct = recompute_loo_feature_frame(
        df, feature_col=FEATURE_COL, end_ts_col="end_ts", symbol_col="symbol", excluded_symbol="X2"
    ).set_index("symbol")[FEATURE_COL]
    assert naive.loc["X3"] == pytest.approx(correct.loc["X3"])
    assert correct.loc["X3"] == pytest.approx(1.0)


# ---------------------------------------------------------------------------
# REQUIRED TEST 10: signal-time qty freeze -- no future-bar leakage.
# ---------------------------------------------------------------------------


def test_signal_time_qty_freeze_later_price_mutation_has_no_effect() -> None:
    """REQUIRED TEST 10."""
    ts0 = pd.Timestamp("2021-01-01T00:00:00+00:00")
    ts1 = pd.Timestamp("2021-01-02T00:00:00+00:00")
    scores_by_date = {
        ts0: {"AAA": 0.9, "BBB": 0.1},
        ts1: {"AAA": 0.9, "BBB": 0.1},
    }
    fold_symbols_by_date = {ts0: ["AAA", "BBB"], ts1: ["AAA", "BBB"]}
    wts_spec = WeightToShareSpec(equity_usd=100_000.0)

    close_lookup_v1 = {("AAA", ts0): 100.0, ("BBB", ts0): 50.0, ("AAA", ts1): 200.0, ("BBB", ts1): 60.0}
    close_lookup_v2 = dict(close_lookup_v1)
    close_lookup_v2[("AAA", ts1)] = 999_999.0  # mutate ONLY the later date's price

    rows_v1 = build_schedule_rows(
        scores_by_date=scores_by_date, fold_symbols_by_date=fold_symbols_by_date,
        rank_side_count=1, long_only=True, max_gross_exposure=1.0, wts_spec=wts_spec,
        close_lookup=close_lookup_v1,
    )
    rows_v2 = build_schedule_rows(
        scores_by_date=scores_by_date, fold_symbols_by_date=fold_symbols_by_date,
        rank_side_count=1, long_only=True, max_gross_exposure=1.0, wts_spec=wts_spec,
        close_lookup=close_lookup_v2,
    )
    row_ts0_v1 = [r for r in rows_v1 if r["decision_ts"] == ts0.isoformat()]
    row_ts0_v2 = [r for r in rows_v2 if r["decision_ts"] == ts0.isoformat()]
    assert row_ts0_v1 == row_ts0_v2


# ---------------------------------------------------------------------------
# REQUIRED TEST 11: no holdout/future rows enter the schedule.
# ---------------------------------------------------------------------------


def test_no_holdout_rows_enter_schedule() -> None:
    """REQUIRED TEST 11."""
    holdout_start = "2024-11-01T00:00:00+00:00"
    ok_rows = [{"decision_ts": "2024-10-31T00:00:00+00:00", "symbol": "AAA", "target_qty": 1}]
    _assert_no_holdout_rows(ok_rows, holdout_start)  # does not raise

    bad_rows = [{"decision_ts": "2024-11-01T00:00:00+00:00", "symbol": "AAA", "target_qty": 1}]
    with pytest.raises(ReplayBundleError, match="holdout boundary"):
        _assert_no_holdout_rows(bad_rows, holdout_start)


def test_replay_bundle_end_to_end_schedules_never_reach_holdout(tmp_path: Path) -> None:
    """REQUIRED TEST 11 (integration companion)."""
    reg = _register_trial(tmp_path, trial_label="a")
    manifest_path = build_replay_bundle(
        reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
        out_dir=tmp_path / "bundle", excluded_symbols=[],
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    holdout_start = pd.Timestamp(manifest["holdout_start_utc"])
    baseline = pd.read_csv(tmp_path / "bundle" / manifest["baseline_schedule"]["file"])
    assert (pd.to_datetime(baseline["decision_ts"], utc=True) < holdout_start).all()


# ---------------------------------------------------------------------------
# REQUIRED TEST 12: result values do not enter replay semantic identity.
# ---------------------------------------------------------------------------


def test_result_values_absent_from_replay_semantic_identity(tmp_path: Path) -> None:
    """REQUIRED TEST 12."""
    reg = _register_trial(tmp_path, trial_label="a")
    manifest_path = build_replay_bundle(
        reg["registry_db"], trial_id=reg["trial_id"], economic_eval_id=reg["economic_eval_id"],
        out_dir=tmp_path / "bundle", excluded_symbols=[],
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    semantic_blob = json.dumps(manifest["replay_semantic_spec"]).lower()
    for forbidden in ("sharpe", "net_total_return", "economic_eval_id", "ml_score", "attempt_id", "trial_id"):
        assert forbidden not in semantic_blob
    # excluded_symbol / result identity live only in lineage / per-symbol
    # LOO keys, never inside the result-independent semantic spec itself.
    assert "excluded_symbol" not in semantic_blob
