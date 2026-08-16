"""
RESEARCH-LEGACY-TRAINING-BOUNDARY-01 — fail-closed evidence-class boundary
tests.

Proves, against REAL artifacts produced by the real evaluators (not just
hand-crafted fixtures), that:
  * the legacy single-shot path (train.py/model_logreg.py) is classified
    DIAGNOSTIC_OR_FIT_ONLY and rejected by the promotion-grade OOS gate;
  * the registered purged walk-forward path (eval_walkforward.py) and the
    causal economic walk-forward evaluator (economic_walkforward.py) are
    both classified PROMOTION_GRADE_OOS and accepted;
  * merely flipping a label string on a single-shot artifact is NOT enough
    to pass the gate (the required negative control) -- the boundary checks
    real structural shape, not a self-declared field.
"""
from __future__ import annotations

import json
from pathlib import Path

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
from mqk_research.ml.contracts import MLTrainConfig
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economic_walkforward import AnnualizationSpec, CostModelSpec, EconomicWalkForwardSpec, SignalPolicySpec
from mqk_research.ml.eval_walkforward import WalkForwardSpec, run_walkforward_eval
from mqk_research.ml.evidence_boundary import (
    DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS,
    PROMOTION_GRADE_OOS_EVIDENCE_CLASS,
    NotPromotionGradeOosEvidence,
    classify_research_artifact,
    require_promotion_grade_oos_evidence,
)
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.train import train_model


# ---------------------------------------------------------------------------
# Shared fixture builders
# ---------------------------------------------------------------------------


def _build_full_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for d in dates:
            f1 = float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append({
                "symbol": sym, "end_ts": d, "f1": f1, "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
            })
    return pd.DataFrame(rows)


def _write_full_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_flat_bars(df: pd.DataFrame) -> pd.DataFrame:
    rows = []
    for (sym, ts), _ in df.groupby(["symbol", "end_ts"]):
        rows.append({"symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(), "close": 100.0})
    return pd.DataFrame(rows)


def _synthetic_bars_provenance(bars_path: Path) -> dict:
    """BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01: the official
    registered path now requires a durable bars provenance manifest. See
    test_economic_walkforward.py's identical helper for the full rationale.

    REPAIR-02 (Defect 2): a bare evidence-ID string no longer satisfies
    CA_POLICY_FORBID_AFFECTED_PERIODS -- a real, content-addressed evidence
    object covering the full symbol universe/date range is required (see
    test_economic_walkforward.py's identically-updated helper)."""
    bars = pd.read_csv(bars_path)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    symbol_universe = sorted(bars["symbol"].astype(str).unique().tolist())
    coverage_start = end_ts.min().isoformat()
    coverage_end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="test_fixture_no_known_corporate_actions",
        covered_symbol_universe=symbol_universe,
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
            "convention_basis": "synthetic test fixture — no real provider involved",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc=coverage_start,
        end_utc=coverage_end,
        symbol_universe=symbol_universe,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        artifact_path=bars_path,
    )


def _real_legacy_train_artifact(tmp_path: Path) -> dict:
    """Real, unmutated output of the legacy single-shot path."""
    run_dir = tmp_path / "legacy_run"
    df = _build_full_dataset(periods_days=250)
    _write_full_run_dir(run_dir, df)
    cfg = MLTrainConfig(min_rows=50)
    model_path = train_model(run_dir, cfg)
    meta_path = model_path.parent / "ml_train_meta.json"
    return json.loads(meta_path.read_text(encoding="utf-8"))


def _real_walkforward_eval_artifact(tmp_path: Path) -> dict:
    """Real, unmutated output of the registered purged walk-forward path."""
    run_dir = tmp_path / "wf_run"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir, df)
    wf_spec = WalkForwardSpec(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)
    out_path = run_walkforward_eval(run_dir, spec=wf_spec, steps=10)
    return json.loads(out_path.read_text(encoding="utf-8"))


def _real_economic_walkforward_artifact(tmp_path: Path) -> dict:
    """Real, unmutated output of the causal economic walk-forward evaluator."""
    run_dir = tmp_path / "econ_run"
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    bars_df = _build_flat_bars(df)
    bars_path = run_dir / "bars.csv"
    _write_full_run_dir(run_dir, df)
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_df.to_csv(bars_path, index=False)
    wf_spec = WalkForwardSpec(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="exp.boundary", hypothesis_id="hyp.boundary", strategy_id="research.boundary",
        bars_csv=bars_path,
        economic_spec=EconomicWalkForwardSpec(
            signal_policy=SignalPolicySpec(entry_threshold=0.5),
            cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
            annualization=AnnualizationSpec(),
        ),
        bars_provenance=_synthetic_bars_provenance(bars_path),
        registry_db=registry_db, wf_spec=wf_spec, steps=10,
    )
    return json.loads(out_path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# TEST 1 — real legacy single-shot artifact is DIAGNOSTIC_OR_FIT_ONLY
# ---------------------------------------------------------------------------


def test_real_legacy_train_artifact_is_diagnostic_only(tmp_path):
    artifact = _real_legacy_train_artifact(tmp_path)
    assert artifact["schema_version"] == "ml_train_meta_v1"
    assert artifact["evidence_class"] == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    assert classify_research_artifact(artifact) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    with pytest.raises(NotPromotionGradeOosEvidence):
        require_promotion_grade_oos_evidence(artifact, context="legacy train artifact")


# ---------------------------------------------------------------------------
# TEST 2 — real registered walk-forward artifact is PROMOTION_GRADE_OOS
# ---------------------------------------------------------------------------


def test_real_walkforward_eval_artifact_is_promotion_grade_oos(tmp_path):
    artifact = _real_walkforward_eval_artifact(tmp_path)
    assert artifact["schema_version"] == "walk_forward_eval_v2"
    assert classify_research_artifact(artifact) == PROMOTION_GRADE_OOS_EVIDENCE_CLASS
    require_promotion_grade_oos_evidence(artifact)  # must not raise


# ---------------------------------------------------------------------------
# TEST 3 — real economic walk-forward artifact is PROMOTION_GRADE_OOS
# ---------------------------------------------------------------------------


def test_real_economic_walkforward_artifact_is_promotion_grade_oos(tmp_path):
    artifact = _real_economic_walkforward_artifact(tmp_path)
    assert artifact["schema_version"] == "economic_walk_forward_v1"
    assert classify_research_artifact(artifact) == PROMOTION_GRADE_OOS_EVIDENCE_CLASS
    require_promotion_grade_oos_evidence(artifact)  # must not raise


# ---------------------------------------------------------------------------
# TEST 4 — REQUIRED NEGATIVE CONTROL: relabeling alone must not pass the gate
# ---------------------------------------------------------------------------


def test_negative_control_relabeling_alone_does_not_pass_the_gate(tmp_path):
    """The mission's required negative control: present a single-shot
    artifact to the boundary and prove it is rejected, then prove that
    MERELY changing a string (evidence_class, or even schema_version alone)
    is not sufficient to get it admitted -- if it were, the boundary would
    be no stronger than an easily-spoofed self-declared label."""
    artifact = _real_legacy_train_artifact(tmp_path)
    assert classify_research_artifact(artifact) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    # Attack 1: flip only the self-declared evidence_class label.
    poisoned_label = dict(artifact)
    poisoned_label["evidence_class"] = PROMOTION_GRADE_OOS_EVIDENCE_CLASS
    assert classify_research_artifact(poisoned_label) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    with pytest.raises(NotPromotionGradeOosEvidence):
        require_promotion_grade_oos_evidence(poisoned_label)

    # Attack 2: flip only schema_version to a registered fold-safe protocol
    # name, WITHOUT rebuilding the structural shape (folds/holdout/etc.)
    # that only a genuine run of that evaluator produces.
    poisoned_schema = dict(artifact)
    poisoned_schema["schema_version"] = "walk_forward_eval_v2"
    assert classify_research_artifact(poisoned_schema) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    with pytest.raises(NotPromotionGradeOosEvidence):
        require_promotion_grade_oos_evidence(poisoned_schema)

    poisoned_schema_econ = dict(artifact)
    poisoned_schema_econ["schema_version"] = "economic_walk_forward_v1"
    assert classify_research_artifact(poisoned_schema_econ) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    # Attack 3: flip schema_version AND add a fabricated non-empty "folds"
    # key and a correctly-worded "holdout" block, but still miss the OTHER
    # required structural keys (temporal_contract/spec) -- still rejected.
    poisoned_partial_shape = dict(artifact)
    poisoned_partial_shape["schema_version"] = "walk_forward_eval_v2"
    poisoned_partial_shape["folds"] = [{"fold": 0, "skipped": False}]
    poisoned_partial_shape["holdout"] = {"status": "reserved_not_evaluated"}
    assert classify_research_artifact(poisoned_partial_shape) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS


# ---------------------------------------------------------------------------
# TEST 5 — degenerate / malformed inputs fail closed, never crash
# ---------------------------------------------------------------------------


def test_degenerate_inputs_classify_as_diagnostic_only():
    assert classify_research_artifact({}) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    assert classify_research_artifact({"schema_version": "walk_forward_eval_v2"}) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    assert classify_research_artifact({"schema_version": "unknown_protocol_v9"}) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    assert classify_research_artifact({
        "schema_version": "walk_forward_eval_v2", "holdout": {"status": "reserved_not_evaluated"},
        "folds": [], "temporal_contract": {}, "spec": {},
    }) == DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS  # empty folds list


# ---------------------------------------------------------------------------
# TEST 6 — official walk-forward path and legacy diagnostic workflow both
# remain fully functional (no regression from this patch)
# ---------------------------------------------------------------------------


def test_legacy_train_workflow_still_produces_a_usable_model(tmp_path):
    """Existing diagnostic workflow (train_model) must not be broken by
    adding the evidence_class label -- the model artifact itself, and the
    rest of ml_train_meta.json, are unchanged in shape and content."""
    run_dir = tmp_path / "legacy_run"
    df = _build_full_dataset(periods_days=250)
    _write_full_run_dir(run_dir, df)
    cfg = MLTrainConfig(min_rows=50)
    model_path = train_model(run_dir, cfg)
    assert model_path.exists()
    model_out = json.loads(model_path.read_text(encoding="utf-8"))
    assert model_out["model_type"] == "logreg_v1"
    assert len(model_out["coef"]) > 0
