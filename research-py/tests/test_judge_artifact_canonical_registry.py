"""
P7C-REPAIR-04 — cross-language durable judge-authority canonicalization.

Python's `json.dumps` and Rust's `serde_json` are not guaranteed to format
every float identically (e.g. "1e-06" vs "1e-6" for the SAME value), so the
Rust verifier must never recompute a hash of a supplied judge artifact with
its own serializer and compare it against a hash Python produced. Instead
the registry durably persists BOTH the exact canonical JSON TEXT
`build_multiple_testing_judge` produced and its SHA-256, both derived from
the SAME `canonical_json` call (see `multiple_testing_judge.py`). These
tests cover the Python-side half of that contract:
  * `build_multiple_testing_judge` persists canonical text whose own hash
    matches the registered `judge_artifact_sha256` (REQUIRED TEST 12).
  * the additive schema upgrade for `canonical_judge_json` is idempotent
    and never destroys/backfills a legacy row (REQUIRED TEST 7).
  * `register_judge_artifact` fails closed on conflicting re-registration
    under the same immutable `judge_artifact_sha256` key (REQUIRED TEST 13)
    and is idempotent on identical re-registration (REQUIRED TEST 14).

The Rust-side half (semantic same-language comparison, per-row integrity
checks, exponent-format interoperability) lives in
`core-rs/crates/mqk-promotion/tests/scenario_promotion_oos_evidence_gate_p7c_repair_01.rs`.
"""
from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any, Dict, List, Sequence

import pandas as pd
import pytest

from mqk_research.exp_distributed.hashing import sha256_bytes
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID
from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
from mqk_research.ml.util_hash import file_record


def _write_economic_artifact(eval_dir: Path, *, dates: Sequence[str], net_returns: Sequence[float]) -> Path:
    eval_dir.mkdir(parents=True, exist_ok=True)
    daily_df = pd.DataFrame({"date": list(dates), "net_daily_return": [float(x) for x in net_returns]})
    daily_path = eval_dir / "economic_daily_returns.csv"
    daily_df.to_csv(daily_path, index=False)
    out: Dict[str, Any] = {
        "schema_version": "economic_walk_forward_v1",
        "protocol": {"protocol_id": ECONOMIC_PROTOCOL_ID},
        "holdout": {"status": "reserved_not_evaluated"},
        "outputs": {"economic_daily_returns_csv": file_record(daily_path)},
    }
    out["ids"] = {"economic_eval_id": f"econ_eval_{eval_dir}"}
    out_path = eval_dir / "economic_walk_forward.json"
    out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path


def _register_trial_with_result(
    store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str, strategy_id: str,
    run_dir: Path, dates: Sequence[str], net_returns: Sequence[float],
) -> str:
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    identity: Dict[str, Any] = {
        "experiment_id": experiment_id, "hypothesis_id": hypothesis_id, "strategy_id": strategy_id,
        "protocol_id": ECONOMIC_PROTOCOL_ID,
        "data_identity": {
            "features_csv": {"sha256": f"feat-{strategy_id}", "bytes": 1},
            "targets_csv": {"sha256": f"targ-{strategy_id}", "bytes": 1},
            "feature_schema": {"sha256": f"schema-{strategy_id}", "bytes": 1},
            "bars_provenance": {
                "schema_version": "bars_provenance_manifest_v1", "provider_ids_observed": ["alpaca"],
                "resolved_close_column": "close_micros", "price_adjustment_convention": "raw_unadjusted",
                "corporate_action_policy": "forbid_affected_periods",
                "corporate_action_evidence_id": "evidence-fixture", "forbidden_periods": [],
                "timeframe": "1D", "start_utc": "2021-01-01T00:00:00+00:00",
                "end_utc": "2021-02-01T00:00:00+00:00", "symbol_universe": ["AAA"],
                "universe_mode": "fixed_ex_ante", "canonical_semantic_bars_hash": "bars-abc",
            },
        },
        "evaluation_spec": {
            "label_col": "target", "end_ts_col": "end_ts", "train_years": 1, "test_months": 1,
            "step_months": 1, "min_rows_per_fold": 200, "purge_enabled": True,
            "label_end_ts_col": "label_end_ts", "embargo_seconds": 0, "holdout_months": 1,
        },
        "model_spec": {"l2": 1e-3, "lr": 0.05, "steps": 10, "standardize": True, "clip_z": 8.0},
        "economic_protocol": {
            "protocol_id": ECONOMIC_PROTOCOL_ID,
            "signal_policy": {
                "entry_threshold": 0.5, "long_only": True, "sizing": "equal_weight_active",
                "max_gross_exposure": 1.0, "fold_end_policy": "force_flat_last_bar",
                "capacity_policy": "reduce_first_defer_increase_batch_v1",
            },
            "cost_model": {"commission_bps_per_side": 10.0, "slippage_bps_per_side": 5.0, "diagnostic_zero_cost": False},
            "annualization": {"annualization_days": 252, "risk_free_rate_annual": 0.0},
        },
    }
    from mqk_research.exp_distributed.hashing import short_hash
    trial_id = short_hash(identity, length=32)
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id, origin="test")
    econ_path = _write_economic_artifact(run_dir / "eval", dates=dates, net_returns=net_returns)
    econ_out = json.loads(econ_path.read_text(encoding="utf-8"))
    store.finalize_attempt(
        attempt_id, status="succeeded", result_id=econ_out["ids"]["economic_eval_id"],
        artifact_paths={"economic_walk_forward": str(econ_path)},
        result_summary={"net_total_return": float(sum(net_returns))},
    )
    return trial_id


def _dates(n: int) -> List[str]:
    return [d.strftime("%Y-%m-%d") for d in pd.date_range("2021-01-01", periods=n, freq="D")]


# ---------------------------------------------------------------------------
# REQUIRED TEST 12 — a real judge build persists canonical text whose own
# hash matches the registered judge_artifact_sha256, and that text
# round-trips to the exact artifact returned.
# ---------------------------------------------------------------------------

def test_build_multiple_testing_judge_persists_canonical_text_matching_hash(tmp_path):
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    _register_trial_with_result(
        store, experiment_id="exp.canon", hypothesis_id="hyp", strategy_id="a",
        run_dir=tmp_path / "a", dates=_dates(20), net_returns=[0.001] * 20,
    )

    artifact = build_multiple_testing_judge(experiment_id="exp.canon", registry_db=db)
    judge_id = artifact["ids"]["judge_id"]

    rows = store.list_judge_artifacts_for_judge_id(judge_id)
    assert len(rows) == 1
    row = rows[0]

    assert row["canonical_judge_json"] is not None
    # SAME relationship the Rust verifier independently re-checks: the
    # stored text's own hash must equal the row's primary key.
    assert sha256_bytes(row["canonical_judge_json"].encode("utf-8")) == row["judge_artifact_sha256"]
    # the stored text is exactly the artifact this call returned.
    assert json.loads(row["canonical_judge_json"]) == artifact


# ---------------------------------------------------------------------------
# REQUIRED TEST 7 (Python side) — additive schema upgrade for
# canonical_judge_json is idempotent, never destructive, and never
# backfills/fabricates canonical text for a pre-existing row.
# ---------------------------------------------------------------------------

def test_legacy_db_missing_canonical_column_upgrades_additively_without_backfill(tmp_path):
    db = tmp_path / "legacy_registry.sqlite3"
    # Simulate a pre-P7C-REPAIR-04 database: the OLD schema, no
    # canonical_judge_json column at all, with one genuine historical row.
    conn = sqlite3.connect(db)
    conn.executescript(
        """
        create table research_judge_artifacts (
            judge_artifact_sha256 text primary key,
            judge_id text not null,
            experiment_id text not null,
            hypothesis_id text,
            artifact_path text,
            schema_version text not null,
            protocol_id text not null
        );
        """
    )
    conn.execute(
        "insert into research_judge_artifacts "
        "(judge_artifact_sha256, judge_id, experiment_id, hypothesis_id, artifact_path, schema_version, protocol_id) "
        "values (?, ?, ?, ?, ?, ?, ?)",
        ("legacy_sha", "legacy_judge", "exp.legacy", None, None, "multiple_testing_judge_v1", "research_multiple_testing_judge_v1"),
    )
    conn.commit()
    conn.close()

    # Reopening through the current code must not destroy the legacy row,
    # must additively gain the new column, and must NOT fabricate canonical
    # text for a row that never had any.
    store = ResearchResultStore(db)
    row = store.get_judge_artifact("legacy_sha")
    assert row["judge_id"] == "legacy_judge"
    assert row["canonical_judge_json"] is None

    # Idempotent: reopening again does not error or duplicate the column.
    store2 = ResearchResultStore(db)
    row2 = store2.get_judge_artifact("legacy_sha")
    assert row2["canonical_judge_json"] is None


# ---------------------------------------------------------------------------
# REQUIRED TEST 13 — conflicting re-registration under the same immutable
# authority key (judge_artifact_sha256) fails closed.
# ---------------------------------------------------------------------------

def test_register_judge_artifact_conflicting_canonical_text_fails_closed(tmp_path):
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    store.register_judge_artifact(
        judge_id="judge.a", experiment_id="exp.a", hypothesis_id=None, artifact_path=None,
        judge_artifact_sha256="shared_sha", canonical_judge_json='{"a":1}',
        schema_version="multiple_testing_judge_v1", protocol_id="research_multiple_testing_judge_v1",
    )
    with pytest.raises(RuntimeError, match="conflicting canonical identity"):
        store.register_judge_artifact(
            judge_id="judge.a", experiment_id="exp.a", hypothesis_id=None, artifact_path=None,
            judge_artifact_sha256="shared_sha", canonical_judge_json='{"a":2}',
            schema_version="multiple_testing_judge_v1", protocol_id="research_multiple_testing_judge_v1",
        )
    # the original row is untouched by the rejected write.
    row = store.get_judge_artifact("shared_sha")
    assert row["canonical_judge_json"] == '{"a":1}'


# ---------------------------------------------------------------------------
# REQUIRED TEST 14 — identical re-registration is a safe no-op.
# ---------------------------------------------------------------------------

def test_register_judge_artifact_identical_reregistration_is_idempotent(tmp_path):
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    kwargs = dict(
        judge_id="judge.b", experiment_id="exp.b", hypothesis_id="hyp.b", artifact_path=None,
        judge_artifact_sha256="idempotent_sha", canonical_judge_json='{"b":1}',
        schema_version="multiple_testing_judge_v1", protocol_id="research_multiple_testing_judge_v1",
    )
    store.register_judge_artifact(**kwargs)
    store.register_judge_artifact(**kwargs)  # must not raise

    rows = store.list_judge_artifacts_for_judge_id("judge.b")
    assert len(rows) == 1


def test_register_judge_artifact_requires_nonempty_canonical_text(tmp_path):
    db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(db)
    with pytest.raises(ValueError, match="canonical_judge_json"):
        store.register_judge_artifact(
            judge_id="judge.c", experiment_id="exp.c", hypothesis_id=None, artifact_path=None,
            judge_artifact_sha256="sha_c", canonical_judge_json="",
            schema_version="multiple_testing_judge_v1", protocol_id="research_multiple_testing_judge_v1",
        )
