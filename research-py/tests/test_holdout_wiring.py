"""
RESEARCH-HOLDOUT-RESERVATION-WIRING-01 — proof that the durable holdout
ledger (RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01, see test_holdout_ledger.py)
is actually wired into the two REAL registered evaluation entry points
(mqk_research.ml.registry_integration.run_registered_walkforward_eval and
mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval),
not just callable in isolation.

Every test here runs a real (small, synthetic) end-to-end evaluation through
the actual production entry point -- no mocking of reserve_holdout/the
ledger. consume_holdout is never called anywhere in this file: nothing in
the current research pipeline scores a holdout region, so wiring
reservation is this patch's full scope (see holdout_ledger.py's own module
docstring and test_holdout_ledger.py's
test_consume_holdout_has_no_caller_outside_this_module_and_tests, which
this patch leaves passing unchanged). This file does NOT close
RESEARCH-HOLDOUT-CONSUMPTION-WIRING-01 (or any consumption-wiring ledger
item) -- there is no real final-holdout evaluation authority in this repo
yet, so no evaluation call site can ever be honest about consuming one.
REAL_HOLDOUT_CONSUMED remains NO.

Holdout identity uses each entry point's canonical EVALUATION SEMANTICS
identifier (registry_integration.PROTOCOL_ID /
economic_registry_integration.ECONOMIC_PROTOCOL_ID), never the evaluated
artifact's own "schema_version" field -- the two happen to share the same
string value today (see test_protocol_version_uses_semantic_identity_not_
artifact_schema_version below for the negative control that would catch a
future divergence), but they identify different things: schema_version is
artifact layout, PROTOCOL_ID is evaluation semantics. Conflating them was
the confirmed defect in the original wiring.
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
from mqk_research.ml.economic_registry_integration import (
    ECONOMIC_PROTOCOL_ID,
    run_registered_economic_walkforward_eval,
)
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.holdout_ledger import compute_holdout_id, get_holdout
from mqk_research.ml.registry_integration import PROTOCOL_ID, run_registered_walkforward_eval
from mqk_research.ml.schema import generate_feature_schema

BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)


def _build_wf_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for d in dates:
            f1 = float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append({
                "symbol": sym,
                "end_ts": d,
                "f1": f1,
                "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
            })
    return pd.DataFrame(rows)


def _write_wf_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _registered_eval(run_dir, df, *, registry_db, spec=None, steps=5, **overrides):
    _write_wf_run_dir(run_dir, df)
    kwargs = dict(
        experiment_id="holdout.wiring.test",
        hypothesis_id="holdout.wiring.hyp",
        strategy_id="research.holdout_wiring_v1",
        registry_db=registry_db,
        spec=spec or WalkForwardSpec(**BASE_SPEC_KW),
        steps=steps,
    )
    kwargs.update(overrides)
    return run_registered_walkforward_eval(run_dir, **kwargs)


def _ledger_row_count(registry_db: Path) -> int:
    import sqlite3

    with sqlite3.connect(registry_db) as conn:
        (n,) = conn.execute("select count(*) from research_holdout_ledger").fetchone()
    return int(n)


# ---------------------------------------------------------------------------
# TEST 1 — a real successful registered evaluation durably reserves its
# exact holdout region, before/independent of any holdout scoring.
# ---------------------------------------------------------------------------


def test_registered_walkforward_eval_reserves_holdout(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    out_path = _registered_eval(run_dir, df, registry_db=registry_db)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    # Matches registry_integration._content_identity: sha256+bytes only,
    # never the "path" field file_record also carries.
    def _content_identity(rec: Dict[str, Any]) -> Dict[str, Any]:
        return {"sha256": rec["sha256"], "bytes": rec["bytes"]}

    holdout_id = out["registry"]["holdout_id"]
    expected_id = compute_holdout_id(
        dataset_identity={
            "features_csv": _content_identity(out["inputs"]["features_csv"]),
            "targets_csv": _content_identity(out["inputs"]["targets_csv"]),
            "feature_schema": _content_identity(out["inputs"]["feature_schema"]),
        },
        holdout_start_utc=out["holdout"]["start_utc"],
        holdout_end_utc=out["holdout"]["end_utc"],
        protocol_version=PROTOCOL_ID,
    )
    assert holdout_id == expected_id

    record = get_holdout(registry_db, holdout_id)
    assert record["status"] == "reserved"
    assert record["consumed_at"] is None
    assert record["consumer_identity"] is None
    assert record["holdout_start_utc"] == out["holdout"]["start_utc"]
    assert record["holdout_end_utc"] == out["holdout"]["end_utc"]
    assert record["protocol_version"] == PROTOCOL_ID

    # The artifact's own reported holdout status is untouched by this patch
    # -- reservation is a durable side record, never a claim of evaluation.
    assert out["holdout"]["status"] == "reserved_not_evaluated"


# ---------------------------------------------------------------------------
# TEST 2 — same-trial retry reserves the SAME holdout_id, never a duplicate
# or independent reservation, and never errors.
# ---------------------------------------------------------------------------


def test_rerun_same_candidate_reserves_same_holdout_no_duplicate(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()

    out_1 = json.loads(_registered_eval(run_dir, df, registry_db=registry_db).read_text(encoding="utf-8"))
    out_2 = json.loads(_registered_eval(run_dir, df, registry_db=registry_db).read_text(encoding="utf-8"))

    assert out_1["registry"]["trial_id"] == out_2["registry"]["trial_id"]
    assert out_1["registry"]["attempt_id"] != out_2["registry"]["attempt_id"]
    assert out_1["registry"]["holdout_id"] == out_2["registry"]["holdout_id"]

    # Exactly one ledger row exists, still reserved (never consumed by a
    # mere reservation retry).
    assert _ledger_row_count(registry_db) == 1
    record = get_holdout(registry_db, out_1["registry"]["holdout_id"])
    assert record["status"] == "reserved"


# ---------------------------------------------------------------------------
# TEST 3 — a failed evaluation attempt reserves NO holdout at all. The
# attempt itself remains registered/failed (pre-existing contract, unchanged
# by this patch); only the ledger side must show zero rows.
# ---------------------------------------------------------------------------


def test_failed_evaluation_reserves_no_holdout(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset(symbols=("AAA",), periods_days=40)  # too short -> fail-closed

    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        _registered_eval(run_dir, df, registry_db=registry_db)

    store = ResearchResultStore(registry_db)
    summary = store.registry_summary(experiment_id="holdout.wiring.test", hypothesis_id="holdout.wiring.hyp")
    assert summary["unique_trials"] == 1
    assert summary["failed_attempts"] == 1
    assert summary["succeeded_attempts"] == 0

    assert _ledger_row_count(registry_db) == 0


# ---------------------------------------------------------------------------
# TEST 4 — two structurally DIFFERENT trials (different model
# hyperparameters -> different trial_id/result) over the IDENTICAL dataset
# and holdout boundary converge on the SAME holdout_id. Proves, at the wired
# integration level, that result/param values never influence holdout
# identity -- only the dataset-scoped facts do.
# ---------------------------------------------------------------------------


def test_different_trials_same_dataset_reserve_same_holdout(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_wf_dataset()

    run_a = tmp_path / "a"
    out_a = json.loads(
        _registered_eval(run_a, df, registry_db=registry_db, l2=1e-3).read_text(encoding="utf-8")
    )
    run_b = tmp_path / "b"
    out_b = json.loads(
        _registered_eval(run_b, df, registry_db=registry_db, l2=5e-2).read_text(encoding="utf-8")
    )

    assert out_a["registry"]["trial_id"] != out_b["registry"]["trial_id"]
    assert out_a["registry"]["holdout_id"] == out_b["registry"]["holdout_id"]
    assert _ledger_row_count(registry_db) == 1


# ---------------------------------------------------------------------------
# TEST 5 — the economic registered path also reserves a holdout, sharing the
# classification protocol's exact time boundary but under its OWN protocol
# identity (economic_walk_forward_v1) -- so it reserves a DIFFERENT
# holdout_id from the classification protocol's, per compute_holdout_id's
# own contract (a different protocol_version always yields a different id).
# ---------------------------------------------------------------------------


_DEFAULT_COST = CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0)
_DEFAULT_SIGNAL = SignalPolicySpec(entry_threshold=0.5)
_DEFAULT_ANN = AnnualizationSpec()


def _spec(**overrides: Any) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=overrides.get("signal_policy", _DEFAULT_SIGNAL),
        cost_model=overrides.get("cost_model", _DEFAULT_COST),
        annualization=overrides.get("annualization", _DEFAULT_ANN),
    )


def _build_flat_bars(df: pd.DataFrame) -> pd.DataFrame:
    rows = []
    for (sym, ts), _ in df.groupby(["symbol", "end_ts"]):
        rows.append({"symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(), "close": 100.0})
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


def test_registered_economic_eval_reserves_distinct_holdout_id(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "run"
    df = _build_wf_dataset()
    _write_wf_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_flat_bars(df).to_csv(bars_path, index=False)

    # Classification protocol's own reservation, for comparison.
    class_run_dir = tmp_path / "class_run"
    class_out = json.loads(
        _registered_eval(class_run_dir, df, registry_db=registry_db).read_text(encoding="utf-8")
    )

    economic_out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="holdout.wiring.econ.test",
        hypothesis_id="holdout.wiring.econ.hyp",
        strategy_id="research.holdout_wiring_econ_v1",
        bars_csv=bars_path,
        economic_spec=_spec(),
        bars_provenance=_synthetic_bars_provenance(bars_path),
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**BASE_SPEC_KW),
        steps=5,
    )
    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))

    econ_holdout_id = economic_out["registry"]["holdout_id"]
    assert econ_holdout_id != class_out["registry"]["holdout_id"]

    record = get_holdout(registry_db, econ_holdout_id)
    assert record["status"] == "reserved"
    assert record["protocol_version"] == ECONOMIC_PROTOCOL_ID

    # Both protocols' reservations coexist in the same ledger.
    assert _ledger_row_count(registry_db) == 2


# ---------------------------------------------------------------------------
# TEST 6 (negative control 1) — an artifact-schema-only change (the
# evaluated artifact's own "schema_version" field bumps) must NOT change
# holdout identity when the semantic protocol identity is unchanged. This is
# the confirmed defect this patch closes: the original wiring used
# out["schema_version"] as compute_holdout_id's protocol_version.
#
# NOTE: PROTOCOL_ID and the real artifact's schema_version currently share
# the identical string value ("walk_forward_eval_v2"), so a test comparing
# against the REAL artifact's schema_version cannot distinguish "production
# used PROTOCOL_ID" from "production used schema_version" -- both give the
# same answer today. This test instead monkeypatches the real production
# entry point's own run_walkforward_eval call to rewrite the ARTIFACT's
# schema_version to a hypothetical bumped value after real evaluation, so
# the two code paths (schema_version-keyed vs PROTOCOL_ID-keyed) are
# actually forced to diverge and this test can tell them apart.
# ---------------------------------------------------------------------------


def test_artifact_schema_only_change_does_not_change_holdout_identity(tmp_path, monkeypatch):
    import mqk_research.ml.registry_integration as ri

    registry_db = tmp_path / "registry.sqlite3"
    df = _build_wf_dataset()

    run_a = tmp_path / "a"
    out_a = json.loads(_registered_eval(run_a, df, registry_db=registry_db).read_text(encoding="utf-8"))

    original_run_walkforward_eval = ri.run_walkforward_eval

    def _bumped_schema_run_walkforward_eval(*args, **kwargs):
        out_path = original_run_walkforward_eval(*args, **kwargs)
        data = json.loads(out_path.read_text(encoding="utf-8"))
        data["schema_version"] = data["schema_version"] + "_v99_hypothetical_future_bump"
        out_path.write_text(json.dumps(data, sort_keys=True, separators=(",", ":")), encoding="utf-8")
        return out_path

    monkeypatch.setattr(ri, "run_walkforward_eval", _bumped_schema_run_walkforward_eval)
    run_b = tmp_path / "b"
    out_b = json.loads(_registered_eval(run_b, df, registry_db=registry_db).read_text(encoding="utf-8"))

    assert out_b["schema_version"] != out_a["schema_version"], (
        "sanity: the artifact's own schema_version must actually differ "
        "between the two runs, or this test proves nothing"
    )
    assert out_a["registry"]["holdout_id"] == out_b["registry"]["holdout_id"], (
        "an artifact-schema-only change must never manufacture a new "
        "holdout region -- holdout identity must be keyed on PROTOCOL_ID, "
        "not on the artifact's own schema_version"
    )


# ---------------------------------------------------------------------------
# TEST 7 (negative control 2) — a semantic protocol identity change MUST
# change holdout identity, even when nothing about the artifact schema
# changes. Proves the fix does not merely stop tracking schema_version, but
# genuinely tracks PROTOCOL_ID. Monkeypatches PROTOCOL_ID itself (rather
# than comparing against a hand-computed id) for the same reason TEST 6
# monkeypatches the artifact schema -- the two identifiers coincide today.
# ---------------------------------------------------------------------------


def test_semantic_protocol_change_does_change_holdout_identity(tmp_path, monkeypatch):
    import mqk_research.ml.registry_integration as ri

    registry_db = tmp_path / "registry.sqlite3"
    df = _build_wf_dataset()

    run_a = tmp_path / "a"
    out_a = json.loads(_registered_eval(run_a, df, registry_db=registry_db).read_text(encoding="utf-8"))

    monkeypatch.setattr(ri, "PROTOCOL_ID", "walk_forward_eval_v3_hypothetical")
    run_b = tmp_path / "b"
    out_b = json.loads(_registered_eval(run_b, df, registry_db=registry_db).read_text(encoding="utf-8"))

    assert out_a["schema_version"] == out_b["schema_version"], (
        "sanity: the artifact's own schema_version must be unchanged "
        "between the two runs, or this test cannot isolate the semantic "
        "protocol identity's effect"
    )
    assert out_a["registry"]["holdout_id"] != out_b["registry"]["holdout_id"], (
        "a semantic protocol identity change must produce a different "
        "holdout_id even when the artifact schema and dataset/boundary "
        "facts are unchanged"
    )


# ---------------------------------------------------------------------------
# TEST 8 (negative control 3) — classification and economic protocols
# reserve genuinely distinct identity, keyed on their own semantic
# PROTOCOL_ID constants (not by coincidence of a shared schema_version
# string). Restates TEST 5's distinctness claim explicitly against the
# named constants, independent of any string literal.
# ---------------------------------------------------------------------------


def test_classification_and_economic_protocol_ids_are_distinct_constants():
    assert PROTOCOL_ID != ECONOMIC_PROTOCOL_ID
