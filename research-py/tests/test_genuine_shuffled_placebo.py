"""FINAL-P9-ROBUSTNESS-SEMANTICS-01 -- genuine shuffled placebo negative
controls for `genuine_shuffled_placebo_cli._run_shuffled_placebo`.

Unlike `test_p7a_p7b_economic_replay_stress.py`'s fixtures (which
deliberately use FLAT bars decoupled from the label -- fine for replay-
mechanics tests, but useless for a placebo test that must prove a REAL
economic edge is destroyed by shuffling), this file builds bars whose
actual forward price movement is deterministically tied to each row's
`target` label, so the trained classifier's `ml_score` has a genuine,
exploitable economic edge that a shuffle can meaningfully destroy.
"""
from __future__ import annotations

import json
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
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import ExecutionPricingSpec
from mqk_research.ml.genuine_shuffled_placebo_cli import ReplayAuthorityError, _run_shuffled_placebo
from mqk_research.ml.schema import generate_feature_schema

BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)

# Low commission, diagnostic (close-only) pricing, continuous weights -- this
# test is about SIGNAL quality, not P7A/P7B parity, so the simplest economic
# protocol that produces a genuine, measurable P&L is used.
EDGE_SPEC = EconomicWalkForwardSpec(
    signal_policy=SignalPolicySpec(entry_threshold=0.5),
    cost_model=CostModelSpec(commission_bps_per_side=1.0, slippage_bps_per_side=0.0),
    execution_pricing=ExecutionPricingSpec(),
    weight_to_share=None,
    annualization=AnnualizationSpec(),
)


def _build_full_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0, block_len=20) -> pd.DataFrame:
    """Unlike a per-row IID label, `target` here is REGIME-PERSISTENT (runs
    of `block_len` consecutive days sharing the same target) -- a
    classifier learns to recover the regime from noisy `f1`, and (paired
    with `_build_edge_bars` below) the resulting `ml_score` produces a
    genuine, low-turnover directional bet that survives the causal
    signal-to-execution delay (a 1-2 day entry lag loses only a small
    fraction of a `block_len`-day trend, unlike a daily-flipping label)."""
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym_idx, sym in enumerate(symbols):
        regime = 1
        for i, d in enumerate(dates):
            if i % block_len == 0:
                regime = 1 - regime
            f1 = (2 * regime - 1) * 1.0 + float(rng.normal()) * 0.3
            target = regime
            rows.append({
                "symbol": sym, "end_ts": d, "f1": f1, "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
                "fwd_ret": 999.0 if target == 1 else -999.0,
            })
    return pd.DataFrame(rows)


def _write_full_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts", "fwd_ret"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_edge_bars(df: pd.DataFrame, *, delta: float = 0.50, invert: bool = False) -> pd.DataFrame:
    """A REAL, exploitable economic edge: `close[t+1] - close[t]` is `+delta`
    exactly when `target[t] == 1` (or `-delta` if `invert=True`), so a
    position entered when the classifier's `ml_score` is high captures that
    move -- a genuine signal, not a diagnostic fixture. High/low carry a
    small, genuine spread around each day's close."""
    rows = []
    for sym, g in df.groupby("symbol"):
        g = g.sort_values("end_ts").reset_index(drop=True)
        close = 100.0
        closes = [close]
        for t in range(len(g) - 1):
            up = g.loc[t, "target"] == 1
            if invert:
                up = not up
            close = close + (delta if up else -delta)
            closes.append(close)
        for i, ts in enumerate(g["end_ts"]):
            c = closes[i]
            rows.append({
                "symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(),
                "close": c, "high": c + 0.05, "low": c - 0.05,
            })
    return pd.DataFrame(rows)


def _synthetic_bars_provenance(bars_path: Path) -> Dict[str, Any]:
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


def _registered_run(tmp_path: Path, name: str, *, registry_db: Path, invert: bool = False, seed: int = 0):
    """Returns `(trial_id, economic_eval_id, out_path)`."""
    run_dir = tmp_path / name
    df = _build_full_dataset(periods_days=560, seed=seed)
    _write_full_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_edge_bars(df, invert=invert).to_csv(bars_path, index=False)
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id=f"genuine_placebo.test.{name}",
        hypothesis_id=f"genuine_placebo.hyp.{name}",
        strategy_id=f"research.placebo_{name}",
        bars_csv=bars_path,
        economic_spec=EDGE_SPEC,
        bars_provenance=_synthetic_bars_provenance(bars_path),
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**BASE_SPEC_KW),
        steps=200,
    )
    out = json.loads(out_path.read_text(encoding="utf-8"))
    return out["registry"]["trial_id"], out["ids"]["economic_eval_id"], out_path


# ---------------------------------------------------------------------------
# Determinism
# ---------------------------------------------------------------------------

def test_placebo_shuffle_is_deterministic_across_runs(tmp_path):
    """The SAME trial_id must always produce the SAME permutation -- no
    wall-clock or process-state dependency."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db)

    result_1 = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo_1",
    )
    result_2 = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo_2",
    )
    assert result_1["shuffle_seed"] == result_2["shuffle_seed"]
    assert result_1["placebo_net_total_return"] == result_2["placebo_net_total_return"]
    # NOTE: `placebo_economic_eval_id` is NOT asserted equal here -- it is a
    # content hash that (correctly) includes the output artifact's own file
    # paths (`placebo_1/` vs `placebo_2/`), so it legitimately differs
    # across different `placebo_out_dir` values even though the economic
    # CONTENT (returns, shuffle assignment) is identical, already proven by
    # the two assertions above.
    shuffled_1 = pd.read_csv(tmp_path / "placebo_1" / "shuffled_oos_predictions.csv")
    shuffled_2 = pd.read_csv(tmp_path / "placebo_2" / "shuffled_oos_predictions.csv")
    pd.testing.assert_frame_equal(shuffled_1, shuffled_2)


def test_placebo_destroys_time_association_preserves_marginal_distribution(tmp_path):
    """The shuffled ml_score column is a genuine permutation of the
    original within each fold: same multiset of values, different row
    assignment."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    original_oos = pd.read_csv(econ["inputs"]["oos_predictions_csv"]["path"])

    result = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )
    shuffled_oos = pd.read_csv(tmp_path / "placebo" / "shuffled_oos_predictions.csv")

    for fold in sorted(original_oos["fold"].unique()):
        orig_scores = sorted(original_oos.loc[original_oos["fold"] == fold, "ml_score"].tolist())
        shuf_scores = sorted(shuffled_oos.loc[shuffled_oos["fold"] == fold, "ml_score"].tolist())
        assert orig_scores == pytest.approx(shuf_scores), f"fold {fold}: marginal distribution changed"

    # The (symbol, decision_ts) <-> score association must genuinely differ
    # somewhere -- a shuffle that happens to be the identity permutation
    # would defeat the whole point.
    merged = original_oos.merge(
        shuffled_oos, on=["fold", "symbol", "decision_ts"], suffixes=("_orig", "_shuf")
    )
    assert (merged["ml_score_orig"] != merged["ml_score_shuf"]).any()
    assert result["shuffle_rows"] == len(original_oos)


# ---------------------------------------------------------------------------
# Never creates another trial
# ---------------------------------------------------------------------------

def test_placebo_never_registers_a_new_trial(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db)

    store = ResearchResultStore(registry_db)
    trials_before = store.list_trials()

    _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )

    trials_after = store.list_trials()
    assert [t["trial_id"] for t in trials_after] == [t["trial_id"] for t in trials_before]
    assert len(trials_after) == 1


def test_placebo_leaves_holdout_reserved(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db)

    result = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )

    placebo_artifact = json.loads(Path(result["placebo_artifact_path"]).read_text(encoding="utf-8"))
    assert placebo_artifact["holdout"] == {"status": "reserved_not_evaluated"}


# ---------------------------------------------------------------------------
# Genuine signal vs. genuine null result
# ---------------------------------------------------------------------------

def test_real_edge_beats_shuffled_placebo(tmp_path):
    """A candidate with a REAL, exploitable economic edge (price moves
    exactly as the trained classifier predicts) must beat its own shuffled
    placebo -- passed: true."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "edge", registry_db=registry_db, invert=False)

    result = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )

    assert result["status"] == "evaluated"
    assert result["baseline_net_total_return"] > result["placebo_net_total_return"]
    assert result["passed"] is True


def test_inverted_signal_placebo_performs_as_well_or_better_fails(tmp_path):
    """Mission hard stop: when the candidate's own signal is actively
    HARMFUL (systematically wrong), a shuffled placebo performs as well or
    better on average -- this must be reported honestly as `passed: false`,
    never tuned away."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "inverted", registry_db=registry_db, invert=True)

    result = _run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )

    assert result["status"] == "evaluated"
    assert result["placebo_net_total_return"] >= result["baseline_net_total_return"]
    assert result["passed"] is False


# ---------------------------------------------------------------------------
# Tamper / binding controls (mirrors p7a_p7b's own Section B/C proofs)
# ---------------------------------------------------------------------------

def test_economic_eval_id_mismatch_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db)
    assert eval_id != "some_other_eval_id"

    with pytest.raises(ReplayAuthorityError, match="no succeeded attempt"):
        _run_shuffled_placebo(
            registry_db=registry_db, trial_id=trial_id, economic_eval_id="some_other_eval_id",
            placebo_out_dir=tmp_path / "placebo",
        )


def test_tampered_economic_artifact_fails_closed(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, eval_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))

    original_text = econ_path.read_text(encoding="utf-8")
    tampered = dict(econ)
    tampered["aggregate"] = dict(econ["aggregate"])
    tampered["aggregate"]["folds_used"] = 999999
    try:
        econ_path.write_text(json.dumps(tampered), encoding="utf-8")
        with pytest.raises(ReplayAuthorityError, match="content hash disagrees"):
            _run_shuffled_placebo(
                registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
                placebo_out_dir=tmp_path / "placebo",
            )
    finally:
        econ_path.write_text(original_text, encoding="utf-8")
