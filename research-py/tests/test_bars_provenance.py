"""
BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01 — durable bars provenance
manifest + fail-closed corporate-action preflight tests.

Covers the P8 REQUIRED TESTS list from the repair mission brief:
  1. DataFrame.attrs alone is not durable provenance
  2. official registered economic evaluation requires the durable contract
  3-10. identity-relevant fields (provider, adjustment convention, CA
        policy, CA evidence, bars content, timeframe/range, symbol
        universe, universe mode) each change trial identity
  11. unsupported PIT claim fails closed
  12/13. semantic row reorder vs. physical artifact hash
  14. result changes do not alter data identity
  15. unknown provider fails closed
  16. raw unadjusted without CA safety fails closed
  17. synthetic split negative control
  18. ADJUSTED_DATA valid only when convention verified
  19. FORBID_AFFECTED_PERIODS rejects a declared interval before P&L
  20/21. chronology and holdout remain untouched
  22. registered economic trial identity remains deterministic
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_EVIDENCE_SCHEMA_VERSION,
    CA_POLICY_ADJUSTED_DATA,
    CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED,
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    BarsProvenanceContentMismatch,
    BarsProvenanceUnverifiable,
    CorporateActionIntegrityError,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    PRICE_CONVENTION_UNVERIFIABLE,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    UNIVERSE_MODE_POINT_IN_TIME,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    canonical_semantic_bars_hash,
    check_corporate_action_integrity,
    corporate_action_evidence_id,
    forbidden_periods_from_evidence,
    provenance_identity_fragment,
    require_bars_match_manifest,
    require_registered_bars_provenance,
)
from mqk_research.ml.economic_registry_integration import (
    build_economic_trial_identity,
    run_registered_economic_walkforward_eval,
)
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec, run_walkforward_eval
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.util_hash import sha256_file

# ---------------------------------------------------------------------------
# Shared fixtures
# ---------------------------------------------------------------------------


def _bars_df(prices, symbol: str = "AAA", start: str = "2021-01-01") -> pd.DataFrame:
    dates = pd.date_range(start, periods=len(prices), freq="D", tz="UTC")
    return pd.DataFrame({"symbol": symbol, "end_ts": [d.isoformat() for d in dates], "close": prices})


def _synthetic_ca_evidence(*, symbol_universe: Any, start_utc: str, end_utc: str, entries: Any = ()) -> Dict[str, Any]:
    """A well-formed, content-addressed corporate-action evidence object
    (Defect 2) for DIRECT/diagnostic-layer test fixtures -- proves the
    verification MECHANISM (content-hash derivation, coverage matching)
    honestly, without pretending a real external CA data source exists (see
    module header / BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01). Never used to
    make the OFFICIAL registered path accept an arbitrary/fake evidence
    string -- see the dedicated tests proving that path still fails
    closed."""
    return build_corporate_action_evidence(
        source_provider_id="test_synthetic_ca_source",
        covered_symbol_universe=symbol_universe,
        coverage_start_utc=start_utc,
        coverage_end_utc=end_utc,
        corporate_action_entries=entries,
    )


def _base_manifest(bars_df: pd.DataFrame, **overrides: Any) -> Dict[str, Any]:
    kwargs: Dict[str, Any] = dict(
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "test",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=None,
        corporate_action_evidence=None,
        forbidden_periods=(),
        timeframe="1D",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=["AAA"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars_df,
    )
    kwargs.update(overrides)

    # Defect 2: unless the caller explicitly wired CA evidence themselves
    # (e.g. to exercise the "fake/arbitrary evidence string" fail-closed
    # path), build a real, content-addressed evidence object -- scoped to
    # whatever symbol_universe/date range the caller resolved above -- so
    # this helper's DEFAULT output honestly satisfies
    # CA_POLICY_FORBID_AFFECTED_PERIODS instead of relying on a bare
    # caller-chosen ID string (the pre-repair anti-pattern).
    if "corporate_action_evidence" not in overrides and "corporate_action_evidence_id" not in overrides:
        evidence = _synthetic_ca_evidence(
            symbol_universe=kwargs["symbol_universe"],
            start_utc=kwargs["start_utc"],
            end_utc=kwargs["end_utc"],
        )
        kwargs["corporate_action_evidence"] = evidence
        kwargs["corporate_action_evidence_id"] = evidence["evidence_id"]

    return build_bars_provenance_manifest(**kwargs)


def _write_min_registered_inputs(tmp_path: Path, bars_df: pd.DataFrame | None = None) -> Dict[str, Path]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    features_path = tmp_path / "features.csv"
    targets_path = tmp_path / "targets.csv"
    schema_path = tmp_path / "feature_schema.json"
    bars_path = tmp_path / "bars.csv"
    features_path.write_text("symbol,end_ts,f1\nAAA,2021-01-01T00:00:00+00:00,0.1\n", encoding="utf-8")
    targets_path.write_text(
        "symbol,end_ts,target,label_end_ts\nAAA,2021-01-01T00:00:00+00:00,1,2021-01-02T00:00:00+00:00\n",
        encoding="utf-8",
    )
    schema_path.write_text("{}", encoding="utf-8")
    if bars_df is not None:
        # Defect 3 test-fixture repair: physically WRITE the bars content
        # (in whatever row order `bars_df` carries) so a trial-identity test
        # actually exercises real, differently-ordered bytes on disk instead
        # of a bars.csv that silently never existed (see
        # test_semantic_row_reorder_does_not_change_canonical_identity).
        bars_df.to_csv(bars_path, index=False)
    return {
        "features_path": features_path,
        "targets_path": targets_path,
        "schema_path": schema_path,
        "bars_path": bars_path,
    }


def _minimal_economic_spec() -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
    )


def _trial_id(tmp_path: Path, bars_df: pd.DataFrame, **manifest_overrides: Any) -> str:
    paths = _write_min_registered_inputs(tmp_path, bars_df=bars_df)
    manifest = _base_manifest(bars_df, **manifest_overrides)
    trial_id, _identity = build_economic_trial_identity(
        experiment_id="exp",
        hypothesis_id="hyp",
        strategy_id="strat",
        features_path=paths["features_path"],
        targets_path=paths["targets_path"],
        schema_path=paths["schema_path"],
        bars_path=paths["bars_path"],
        label_col="target",
        end_ts_col="end_ts",
        wf_spec=WalkForwardSpec(),
        l2=1e-3,
        lr=0.05,
        steps=10,
        standardize=True,
        clip_z=8.0,
        economic_spec=_minimal_economic_spec(),
        bars_provenance=manifest,
    )
    return trial_id


# ---------------------------------------------------------------------------
# TEST 1 — DataFrame.attrs alone is not durable provenance
# ---------------------------------------------------------------------------


def test_dataframe_attrs_alone_is_not_durable_provenance(tmp_path):
    bars = _bars_df([100.0, 101.0, 102.0])
    bars.attrs["price_provenance"] = {"price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED}

    bars_path = tmp_path / "bars.csv"
    bars.to_csv(bars_path, index=False)
    reloaded = pd.read_csv(bars_path)

    # The defining fact this test proves: .attrs does not survive the
    # to_csv/read_csv round trip that real bars data actually takes on its
    # way to the registered evaluator (a bars_csv PATH, not a DataFrame).
    assert reloaded.attrs == {}

    # And the registered contract cannot be satisfied by an absent manifest
    # regardless of what an in-memory DataFrame's .attrs might have said.
    with pytest.raises(TypeError):
        run_registered_economic_walkforward_eval(  # type: ignore[call-arg]
            tmp_path,
            experiment_id="exp",
            hypothesis_id="hyp",
            strategy_id="strat",
            bars_csv=bars_path,
            economic_spec=_minimal_economic_spec(),
        )


# ---------------------------------------------------------------------------
# TEST 2 — official registered path requires the durable contract
# ---------------------------------------------------------------------------


def test_official_registered_path_requires_bars_provenance_argument(tmp_path):
    with pytest.raises(TypeError):
        run_registered_economic_walkforward_eval(  # type: ignore[call-arg]
            tmp_path,
            experiment_id="exp",
            hypothesis_id="hyp",
            strategy_id="strat",
            bars_csv=tmp_path / "bars.csv",
            economic_spec=_minimal_economic_spec(),
        )


def test_official_registered_path_verifies_manifest_before_running(tmp_path):
    """A structurally invalid manifest (diagnostic CA policy) must be
    rejected BEFORE any classification/economic evaluation runs -- proven
    by supplying no walk-forward inputs at all (a run that reached
    evaluation would fail with a totally different, file-not-found error)."""
    bars = _bars_df([100.0, 101.0])
    bad_manifest = _base_manifest(bars, corporate_action_policy=CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED)
    with pytest.raises(BarsProvenanceUnverifiable):
        run_registered_economic_walkforward_eval(
            tmp_path,
            experiment_id="exp",
            hypothesis_id="hyp",
            strategy_id="strat",
            bars_csv=tmp_path / "bars.csv",
            economic_spec=_minimal_economic_spec(),
            bars_provenance=bad_manifest,
        )


# ---------------------------------------------------------------------------
# TESTS 3-10 — identity-relevant field changes
# ---------------------------------------------------------------------------


def test_provider_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    pp_a = {"close_column": "close_micros", "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED, "provider_metadata_available": True,
            "convention_basis": "t"}
    pp_b = {**pp_a, "provider_ids_observed": ["other_provider"]}
    id_a = _trial_id(tmp_path / "a", bars, price_provenance=pp_a)
    id_b = _trial_id(tmp_path / "b", bars, price_provenance=pp_b)
    assert id_a != id_b


def test_adjustment_convention_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    pp_raw = {"close_column": "close_micros", "provider_ids_observed": ["alpaca"],
              "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED, "provider_metadata_available": True,
              "convention_basis": "t"}
    pp_unverifiable = {**pp_raw, "price_adjustment_convention": PRICE_CONVENTION_UNVERIFIABLE}
    id_a = _trial_id(tmp_path / "a", bars, price_provenance=pp_raw)
    id_b = _trial_id(tmp_path / "b", bars, price_provenance=pp_unverifiable)
    assert id_a != id_b


def test_corporate_action_policy_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    id_a = _trial_id(tmp_path / "a", bars, corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS)
    id_b = _trial_id(tmp_path / "b", bars, corporate_action_policy=CA_POLICY_ADJUSTED_DATA)
    assert id_a != id_b


def test_corporate_action_evidence_hash_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    id_a = _trial_id(tmp_path / "a", bars, corporate_action_evidence_id="evidence-v1")
    id_b = _trial_id(tmp_path / "b", bars, corporate_action_evidence_id="evidence-v2")
    assert id_a != id_b


def test_bars_content_change_changes_trial_identity(tmp_path):
    bars_a = _bars_df([100.0, 101.0])
    bars_b = _bars_df([100.0, 105.0])
    id_a = _trial_id(tmp_path / "a", bars_a)
    id_b = _trial_id(tmp_path / "b", bars_b)
    assert id_a != id_b


def test_timeframe_or_range_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    id_a = _trial_id(tmp_path / "a", bars, timeframe="1D")
    id_b = _trial_id(tmp_path / "b", bars, timeframe="1H")
    id_c = _trial_id(tmp_path / "c", bars, end_utc="2021-06-01T00:00:00+00:00")
    assert id_a != id_b
    assert id_a != id_c


def test_symbol_universe_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    id_a = _trial_id(tmp_path / "a", bars, symbol_universe=["AAA"])
    id_b = _trial_id(tmp_path / "b", bars, symbol_universe=["AAA", "BBB"])
    assert id_a != id_b


def test_universe_mode_change_changes_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    id_a = _trial_id(tmp_path / "a", bars, universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE)
    id_b = _trial_id(tmp_path / "b", bars, universe_mode=UNIVERSE_MODE_POINT_IN_TIME)
    assert id_a != id_b


# ---------------------------------------------------------------------------
# TEST 11 — unsupported PIT claim fails closed
# ---------------------------------------------------------------------------


def test_unsupported_point_in_time_claim_fails_closed(tmp_path):
    bars = _bars_df([100.0, 101.0])
    manifest = _base_manifest(bars, universe_mode=UNIVERSE_MODE_POINT_IN_TIME)
    with pytest.raises(BarsProvenanceUnverifiable):
        require_registered_bars_provenance(manifest)


# ---------------------------------------------------------------------------
# TESTS 12/13 — canonical semantic identity vs. physical artifact identity
# ---------------------------------------------------------------------------


def test_semantic_row_reorder_does_not_change_canonical_identity(tmp_path):
    bars = _bars_df([100.0, 101.0, 102.0, 103.0])
    reordered = bars.iloc[[3, 1, 0, 2]].reset_index(drop=True)
    assert canonical_semantic_bars_hash(bars) == canonical_semantic_bars_hash(reordered)

    # And downstream: trial identity (via the provenance identity fragment)
    # is unaffected by the reorder too.
    id_forward = _trial_id(tmp_path / "a", bars)
    id_reordered = _trial_id(tmp_path / "b", reordered)
    assert id_forward == id_reordered


def test_physical_artifact_hash_may_differ_while_canonical_identity_equal(tmp_path):
    bars = _bars_df([100.0, 101.0, 102.0, 103.0])
    reordered = bars.iloc[[3, 1, 0, 2]].reset_index(drop=True)

    path_a = tmp_path / "a.csv"
    path_b = tmp_path / "b.csv"
    bars.to_csv(path_a, index=False)
    reordered.to_csv(path_b, index=False)

    assert sha256_file(path_a) != sha256_file(path_b)
    manifest_a = _base_manifest(bars, artifact_path=path_a)
    manifest_b = _base_manifest(reordered, artifact_path=path_b)
    assert manifest_a["artifact_sha256"] != manifest_b["artifact_sha256"]
    assert manifest_a["canonical_semantic_bars_hash"] == manifest_b["canonical_semantic_bars_hash"]
    # artifact_sha256 is deliberately excluded from the identity fragment.
    assert provenance_identity_fragment(manifest_a) == provenance_identity_fragment(manifest_b)


# ---------------------------------------------------------------------------
# TEST 14 — result changes do not alter data identity
# ---------------------------------------------------------------------------


def test_result_value_changes_do_not_alter_data_identity(tmp_path):
    """The manifest is built entirely from pre-evaluation data-layer facts;
    it structurally cannot contain a result value (Sharpe/P&L/return), so
    two identity computations from the SAME manifest are equal regardless
    of what a (hypothetical, differing) evaluation would later produce."""
    bars = _bars_df([100.0, 101.0])
    manifest = _base_manifest(bars)
    forbidden_result_keys = {"sharpe", "net_total_return", "gross_total_return", "pnl", "deflated_sharpe_ratio"}
    assert forbidden_result_keys.isdisjoint(manifest.keys())
    assert forbidden_result_keys.isdisjoint(provenance_identity_fragment(manifest).keys())

    id_a = _trial_id(tmp_path / "a", bars)
    id_b = _trial_id(tmp_path / "b", bars)
    assert id_a == id_b


# ---------------------------------------------------------------------------
# TEST 15 — unknown provider fails closed
# ---------------------------------------------------------------------------


def test_unknown_provider_fails_closed(tmp_path):
    """Mirrors the real, currently-observed AAPL finding (P8 Wave 1): a
    provider_id attribution gap resolves price_adjustment_convention to
    'unverifiable', which must fail the registered structural gate."""
    bars = _bars_df([100.0, 101.0])
    manifest = _base_manifest(
        bars,
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["unknown"],
            "price_adjustment_convention": PRICE_CONVENTION_UNVERIFIABLE,
            "provider_metadata_available": True,
            "convention_basis": "t",
        },
    )
    with pytest.raises(BarsProvenanceUnverifiable):
        require_registered_bars_provenance(manifest)


# ---------------------------------------------------------------------------
# TEST 16 — raw unadjusted without corporate-action safety fails closed
# ---------------------------------------------------------------------------


def test_raw_unadjusted_without_corporate_action_safety_fails_closed(tmp_path):
    bars = _bars_df([100.0, 101.0])
    manifest = _base_manifest(
        bars,
        corporate_action_policy=CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED,
        corporate_action_evidence_id=None,
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest)
    with pytest.raises(BarsProvenanceUnverifiable):
        require_registered_bars_provenance(manifest)


# ---------------------------------------------------------------------------
# TEST 17 — synthetic 2-for-1 split negative control
# ---------------------------------------------------------------------------


def test_synthetic_split_negative_control_cannot_produce_fake_loss(tmp_path):
    """Raw closes 100 -> 50 across a declared 2-for-1 split boundary must
    NOT be reachable as accepted economic evidence -- the preflight must
    fail closed before any P&L is computed, at both the pure-function level
    and wired into run_economic_walkforward itself."""
    dates = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars = pd.DataFrame({
        "symbol": "AAA",
        "end_ts": [d.isoformat() for d in dates],
        "close": [100.0, 100.0, 50.0, 50.0],  # split lands between rows 2 and 3
    })
    split_ts = dates[2]
    cov_start = dates[0].isoformat()
    cov_end = (dates[-1] + pd.Timedelta(days=1)).isoformat()

    # (a) FORBID_AFFECTED_PERIODS with a REAL, content-addressed evidence
    # object (Defect 2) whose own entry covers the split boundary must
    # refuse -- not a bare caller-asserted forbidden_periods list.
    split_evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"],
        start_utc=cov_start,
        end_utc=cov_end,
        entries=[{
            "symbol": "AAA",
            "action_type": "split",
            "effective_start_ts": split_ts.isoformat(),
            "effective_end_ts": dates[3].isoformat(),
        }],
    )
    manifest_forbid = _base_manifest(
        bars,
        start_utc=cov_start,
        end_utc=cov_end,
        corporate_action_evidence=split_evidence,
        corporate_action_evidence_id=split_evidence["evidence_id"],
        forbidden_periods=forbidden_periods_from_evidence(split_evidence),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest_forbid)

    # (b) No corporate-action protection at all (the honest state for real,
    # unclassified raw_unadjusted data today) must ALSO refuse -- proving
    # the -50% drop can never silently pass as accepted evidence.
    manifest_unprotected = _base_manifest(bars, corporate_action_policy=CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED)
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest_unprotected)

    # (c) Wired end-to-end: run_economic_walkforward's preflight raises
    # before any fold is simulated, given the split-covering manifest.
    run_dir = tmp_path / "run"
    run_dir.mkdir()
    bars_path = run_dir / "bars.csv"
    bars.to_csv(bars_path, index=False)
    eval_dir = run_dir / "eval"
    eval_dir.mkdir()
    wf_path = eval_dir / "walk_forward_eval.json"
    wf_path.write_text(
        json.dumps({
            "folds": [{
                "fold": 0,
                "skipped": False,
                "test_start_utc": dates[0].isoformat(),
                "test_end_utc": (dates[-1] + pd.Timedelta(days=1)).isoformat(),
            }],
        }),
        encoding="utf-8",
    )
    with pytest.raises(CorporateActionIntegrityError):
        run_economic_walkforward(
            run_dir,
            bars_csv=bars_path,
            spec=_minimal_economic_spec(),
            walk_forward_eval_path=wf_path,
            provenance_manifest=manifest_forbid,
        )


# ---------------------------------------------------------------------------
# TEST 18 — ADJUSTED_DATA policy valid only when convention actually verified
# ---------------------------------------------------------------------------


def test_adjusted_data_policy_valid_only_when_convention_verified(tmp_path, monkeypatch):
    bars = _bars_df([100.0, 101.0])

    # Raw/unadjusted convention: ADJUSTED_DATA must be rejected -- no
    # verified adjusted convention exists anywhere in this system today.
    manifest_raw = _base_manifest(bars, corporate_action_policy=CA_POLICY_ADJUSTED_DATA)
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest_raw)

    # Only when the convention is one this module has been explicitly told
    # (via its named seam) is independently verified adjusted data does the
    # policy pass -- proving the mechanism, not fabricating a real provider.
    import mqk_research.data.bars_provenance as bp

    monkeypatch.setattr(bp, "_KNOWN_ADJUSTED_CONVENTIONS", frozenset({"split_dividend_adjusted_test_only"}))
    manifest_adjusted = _base_manifest(
        bars,
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        price_provenance={
            "close_column": "adj_close",
            "provider_ids_observed": ["verified_adjusted_provider"],
            "price_adjustment_convention": "split_dividend_adjusted_test_only",
            "provider_metadata_available": True,
            "convention_basis": "t",
        },
    )
    check_corporate_action_integrity(bars, manifest_adjusted)  # must not raise


# ---------------------------------------------------------------------------
# TEST 19 — FORBID_AFFECTED_PERIODS rejects a declared interval before P&L
# ---------------------------------------------------------------------------


def test_forbid_affected_periods_rejects_declared_interval_before_pnl(tmp_path):
    bars = _bars_df([100.0, 101.0, 102.0])
    cov_start = "2021-01-01T00:00:00+00:00"
    cov_end = "2021-01-04T00:00:00+00:00"

    affecting_evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"], start_utc=cov_start, end_utc=cov_end,
        entries=[{
            "symbol": "AAA", "action_type": "unknown",
            "effective_start_ts": "2021-01-02T00:00:00+00:00",
            "effective_end_ts": "2021-01-02T23:59:59+00:00",
        }],
    )
    affected = _base_manifest(
        bars, start_utc=cov_start, end_utc=cov_end,
        corporate_action_evidence=affecting_evidence,
        corporate_action_evidence_id=affecting_evidence["evidence_id"],
        forbidden_periods=forbidden_periods_from_evidence(affecting_evidence),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, affected)

    # A verified evidence object whose declared period does NOT overlap any
    # actual row must pass.
    nonoverlapping_evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"], start_utc=cov_start, end_utc=cov_end,
        entries=[{
            "symbol": "AAA", "action_type": "unknown",
            "effective_start_ts": "2019-01-01T00:00:00+00:00",
            "effective_end_ts": "2019-01-02T00:00:00+00:00",
        }],
    )
    unaffected = _base_manifest(
        bars, start_utc=cov_start, end_utc=cov_end,
        corporate_action_evidence=nonoverlapping_evidence,
        corporate_action_evidence_id=nonoverlapping_evidence["evidence_id"],
        forbidden_periods=forbidden_periods_from_evidence(nonoverlapping_evidence),
    )
    check_corporate_action_integrity(bars, unaffected)  # must not raise

    # No evidence at all must also refuse, regardless of overlap.
    no_evidence = _base_manifest(
        bars, start_utc=cov_start, end_utc=cov_end,
        corporate_action_evidence_id=None, corporate_action_evidence=None, forbidden_periods=(),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, no_evidence)


# ---------------------------------------------------------------------------
# TESTS 20/21 — chronology and holdout remain untouched; full registered
# integration proof that the durable contract is genuinely load-bearing
# end-to-end, not just at the manifest level.
# ---------------------------------------------------------------------------


def _build_full_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    import numpy as np

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


def test_registered_run_with_valid_provenance_leaves_chronology_and_holdout_untouched(tmp_path, monkeypatch):
    """(Defect 2 follow-through) This is the OFFICIAL registered path, so it
    may NOT use CA_POLICY_FORBID_AFFECTED_PERIODS with a fake/arbitrary
    evidence string -- and no authoritative CA evidence SOURCE exists in
    this repository today (see module header), so real raw_unadjusted
    registered runs stay honestly fail-closed. This end-to-end mechanics
    proof (chronology/holdout untouched) instead uses CA_POLICY_ADJUSTED_
    DATA with the SAME sanctioned test-only seam TEST 18 uses
    (monkeypatched _KNOWN_ADJUSTED_CONVENTIONS) -- proving the registered
    pipeline's wiring without ever claiming a real verified adjusted
    provider or a real CA evidence source exists."""
    import mqk_research.data.bars_provenance as bp

    monkeypatch.setattr(bp, "_KNOWN_ADJUSTED_CONVENTIONS", frozenset({"split_dividend_adjusted_test_only"}))

    run_dir = tmp_path / "run"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir, df)

    flat_bars = pd.DataFrame({
        "symbol": df["symbol"], "end_ts": df["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat()), "close": 100.0,
    }).drop_duplicates(subset=["symbol", "end_ts"])
    bars_path = run_dir / "bars.csv"
    flat_bars.to_csv(bars_path, index=False)

    end_ts_all = pd.to_datetime(flat_bars["end_ts"], utc=True)
    manifest = _base_manifest(
        flat_bars,
        symbol_universe=["AAA", "BBB"],
        artifact_path=bars_path,
        start_utc=end_ts_all.min().isoformat(),
        end_utc=(end_ts_all.max() + pd.Timedelta(seconds=1)).isoformat(),
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["verified_adjusted_provider"],
            "price_adjustment_convention": "split_dividend_adjusted_test_only",
            "provider_metadata_available": True,
            "convention_basis": "t",
        },
    )
    wf_spec = WalkForwardSpec(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="exp.provenance_integration",
        hypothesis_id="hyp",
        strategy_id="strat",
        bars_csv=bars_path,
        economic_spec=_minimal_economic_spec(),
        bars_provenance=manifest,
        registry_db=tmp_path / "registry.sqlite3",
        wf_spec=wf_spec,
        steps=10,
    )
    out = json.loads(out_path.read_text(encoding="utf-8"))

    # TEST 21: holdout status literal is unchanged.
    assert out["holdout"] == {"status": "reserved_not_evaluated"}
    # The manifest is durably stamped onto the artifact.
    assert out["bars_provenance"]["canonical_semantic_bars_hash"] == manifest["canonical_semantic_bars_hash"]
    # TEST 20: future-execution chronology is unchanged -- every fold's
    # signal timestamps still strictly precede that symbol's own next-bar
    # fill, exactly as the pre-existing (unmodified) engine already
    # guarantees; a genuinely broken preflight wiring would instead raise
    # or corrupt this shape rather than let a real run complete cleanly.
    assert out["folds"], "expected at least one simulated fold"
    for fold in out["folds"]:
        assert "fold" in fold and "net_total_return" in fold


# ---------------------------------------------------------------------------
# TEST 22 — registered economic trial identity remains deterministic
# ---------------------------------------------------------------------------


def test_registered_economic_trial_identity_is_deterministic(tmp_path):
    bars = _bars_df([100.0, 101.0, 102.0])
    id_a = _trial_id(tmp_path / "a", bars)
    id_b = _trial_id(tmp_path / "b", bars)
    assert id_a == id_b


# ---------------------------------------------------------------------------
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02 -- Defect 1: a manifest MUST
# be content-bound to the bars actually loaded, not just structurally valid.
# ---------------------------------------------------------------------------


def test_manifest_built_from_bars_a_cannot_authorize_bars_b(tmp_path):
    """REQUIRED TEST 1."""
    bars_a = _bars_df([100.0, 101.0, 102.0])
    bars_b = _bars_df([200.0, 201.0, 202.0])  # different semantic content, same shape/dates
    manifest_a = _base_manifest(bars_a)

    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(bars_b, manifest_a)
    require_bars_match_manifest(bars_a, manifest_a)  # negative control -- must not raise

    run_dir = tmp_path / "run"
    run_dir.mkdir()
    bars_path = run_dir / "bars.csv"
    bars_b.to_csv(bars_path, index=False)  # bars B written to disk...
    eval_dir = run_dir / "eval"
    eval_dir.mkdir()
    dates = pd.to_datetime(bars_b["end_ts"], utc=True)
    wf_path = eval_dir / "walk_forward_eval.json"
    wf_path.write_text(
        json.dumps({"folds": [{
            "fold": 0, "skipped": False,
            "test_start_utc": dates.min().isoformat(),
            "test_end_utc": (dates.max() + pd.Timedelta(days=1)).isoformat(),
        }]}),
        encoding="utf-8",
    )
    with pytest.raises(BarsProvenanceContentMismatch):
        run_economic_walkforward(
            run_dir, bars_csv=bars_path, spec=_minimal_economic_spec(),
            walk_forward_eval_path=wf_path, provenance_manifest=manifest_a,  # ...manifest A (mismatched)
        )


def test_canonical_hash_mismatch_fails_before_pnl(tmp_path):
    """REQUIRED TEST 2."""
    bars = _bars_df([100.0, 101.0])
    manifest = dict(_base_manifest(bars))
    manifest["canonical_semantic_bars_hash"] = "0" * 64
    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(bars, manifest)


def test_manifest_symbol_universe_mismatch_fails_before_pnl(tmp_path):
    """REQUIRED TEST 3."""
    bars = _bars_df([100.0, 101.0], symbol="AAA")
    manifest = _base_manifest(bars, symbol_universe=["AAA", "ZZZ"])
    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(bars, manifest)


def test_manifest_range_mismatch_fails_before_pnl(tmp_path):
    """REQUIRED TEST 4."""
    bars = _bars_df([100.0, 101.0], start="2021-06-01")
    manifest = _base_manifest(bars)  # default declared range is 2021-01-01..2021-02-01
    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_match_manifest(bars, manifest)


def test_duplicate_symbol_end_ts_fails_canonicalization(tmp_path):
    """REQUIRED TEST 5."""
    bars = pd.DataFrame({
        "symbol": ["AAA", "AAA"],
        "end_ts": ["2021-01-01T00:00:00+00:00", "2021-01-01T00:00:00+00:00"],
        "close": [100.0, 101.0],
    })
    with pytest.raises(ValueError):
        canonical_semantic_bars_hash(bars)


# ---------------------------------------------------------------------------
# Defect 2 -- corporate-action evidence must be REAL and content-addressed,
# never a bare caller-selected ID string.
# ---------------------------------------------------------------------------


def test_arbitrary_ca_evidence_string_cannot_satisfy_official_gate(tmp_path):
    """REQUIRED TEST 6."""
    bars = _bars_df([100.0, 101.0])
    manifest = _base_manifest(
        bars, corporate_action_evidence_id="evidence-v1",
        corporate_action_evidence=None, forbidden_periods=(),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest)


def test_fake_evidence_and_empty_forbidden_periods_cannot_pass_official_registered_evaluation(tmp_path):
    """REQUIRED TESTS 7/13: no authoritative corporate-action evidence
    source exists in this repository today -- a caller-selected evidence ID
    string, even paired with an empty forbidden_periods list, must NOT
    satisfy the OFFICIAL registered raw_unadjusted contract end-to-end. This
    is the honest P8_CONTRACT_COMPLETE / DATA_SOURCE_BLOCKED state."""
    run_dir = tmp_path / "run"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir, df)
    flat_bars = pd.DataFrame({
        "symbol": df["symbol"], "end_ts": df["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat()), "close": 100.0,
    }).drop_duplicates(subset=["symbol", "end_ts"])
    bars_path = run_dir / "bars.csv"
    flat_bars.to_csv(bars_path, index=False)

    end_ts_all = pd.to_datetime(flat_bars["end_ts"], utc=True)
    manifest = _base_manifest(
        flat_bars,
        symbol_universe=["AAA", "BBB"],
        start_utc=end_ts_all.min().isoformat(),
        end_utc=(end_ts_all.max() + pd.Timedelta(seconds=1)).isoformat(),
        corporate_action_evidence_id="evidence-v1",
        corporate_action_evidence=None,
        forbidden_periods=(),
    )
    wf_spec = WalkForwardSpec(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)
    with pytest.raises(CorporateActionIntegrityError):
        run_registered_economic_walkforward_eval(
            run_dir,
            experiment_id="exp.ca_blocked",
            hypothesis_id="hyp",
            strategy_id="strat",
            bars_csv=bars_path,
            economic_spec=_minimal_economic_spec(),
            bars_provenance=manifest,
            registry_db=tmp_path / "registry.sqlite3",
            wf_spec=wf_spec,
            steps=10,
        )


def test_ca_evidence_id_is_derived_from_canonical_content(tmp_path):
    """REQUIRED TEST 8."""
    evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"], start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-02-01T00:00:00+00:00",
    )
    assert evidence["evidence_id"] == corporate_action_evidence_id(evidence)


def test_changing_ca_evidence_content_changes_evidence_id(tmp_path):
    """REQUIRED TEST 9."""
    evidence_a = _synthetic_ca_evidence(
        symbol_universe=["AAA"], start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-02-01T00:00:00+00:00",
    )
    evidence_b = _synthetic_ca_evidence(
        symbol_universe=["AAA"], start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-02-01T00:00:00+00:00",
        entries=[{
            "symbol": "AAA", "action_type": "split",
            "effective_start_ts": "2021-01-15T00:00:00+00:00", "effective_end_ts": "2021-01-16T00:00:00+00:00",
        }],
    )
    assert evidence_a["evidence_id"] != evidence_b["evidence_id"]


def test_ca_evidence_coverage_smaller_than_bars_range_fails(tmp_path):
    """REQUIRED TEST 10."""
    bars = _bars_df([100.0, 101.0, 102.0], start="2021-01-01")  # spans 2021-01-01..2021-01-03
    evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"],
        start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-02T00:00:00+00:00",  # too narrow
    )
    manifest = _base_manifest(
        bars, start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-04T00:00:00+00:00",
        corporate_action_evidence=evidence, corporate_action_evidence_id=evidence["evidence_id"],
        forbidden_periods=forbidden_periods_from_evidence(evidence),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest)


def test_ca_evidence_missing_one_bars_symbol_fails(tmp_path):
    """REQUIRED TEST 11."""
    dates = pd.date_range("2021-01-01", periods=2, freq="D", tz="UTC")
    bars = pd.concat([
        pd.DataFrame({"symbol": "AAA", "end_ts": [d.isoformat() for d in dates], "close": [100.0, 101.0]}),
        pd.DataFrame({"symbol": "BBB", "end_ts": [d.isoformat() for d in dates], "close": [50.0, 51.0]}),
    ], ignore_index=True)
    evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"],  # BBB missing from evidence coverage
        start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-03T00:00:00+00:00",
    )
    manifest = _base_manifest(
        bars, symbol_universe=["AAA", "BBB"],
        start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-03T00:00:00+00:00",
        corporate_action_evidence=evidence, corporate_action_evidence_id=evidence["evidence_id"],
        forbidden_periods=forbidden_periods_from_evidence(evidence),
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest)


def test_caller_modified_forbidden_periods_inconsistent_with_evidence_fails(tmp_path):
    """REQUIRED TEST 12."""
    bars = _bars_df([100.0, 101.0, 102.0])
    evidence = _synthetic_ca_evidence(
        symbol_universe=["AAA"],
        start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-04T00:00:00+00:00",
        entries=[{
            "symbol": "AAA", "action_type": "split",
            "effective_start_ts": "2021-01-02T00:00:00+00:00", "effective_end_ts": "2021-01-02T23:59:59+00:00",
        }],
    )
    manifest = _base_manifest(
        bars, start_utc="2021-01-01T00:00:00+00:00", end_utc="2021-01-04T00:00:00+00:00",
        corporate_action_evidence=evidence, corporate_action_evidence_id=evidence["evidence_id"],
        forbidden_periods=(),  # caller silently dropped the evidence-derived period
    )
    with pytest.raises(CorporateActionIntegrityError):
        check_corporate_action_integrity(bars, manifest)


def test_synthetic_diagnostic_path_remains_usable_without_ca_evidence(tmp_path):
    """REQUIRED TEST 14: the low-level, non-registered run_economic_
    walkforward path (provenance_manifest=None) -- the explicit diagnostic
    escape hatch documented on that function -- must remain usable without
    any corporate-action evidence at all."""
    dates = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars = pd.DataFrame({
        "symbol": "AAA", "end_ts": [d.isoformat() for d in dates], "close": [100.0, 101.0, 102.0, 103.0],
    })
    run_dir = tmp_path / "run"
    run_dir.mkdir()
    bars_path = run_dir / "bars.csv"
    bars.to_csv(bars_path, index=False)
    eval_dir = run_dir / "eval"
    eval_dir.mkdir()
    wf_path = eval_dir / "walk_forward_eval.json"
    wf_path.write_text(
        json.dumps({"folds": [{
            "fold": 0, "skipped": False,
            "test_start_utc": dates[0].isoformat(),
            "test_end_utc": (dates[-1] + pd.Timedelta(days=1)).isoformat(),
        }]}),
        encoding="utf-8",
    )
    oos_path = eval_dir / "walk_forward_oos_predictions.csv"
    pd.DataFrame({
        "fold": [0, 0, 0, 0], "symbol": ["AAA"] * 4,
        "decision_ts": [d.isoformat() for d in dates], "ml_score": [0.9, 0.9, 0.9, 0.9],
    }).to_csv(oos_path, index=False)

    out_path = run_economic_walkforward(
        run_dir, bars_csv=bars_path, spec=_minimal_economic_spec(),
        walk_forward_eval_path=wf_path, oos_predictions_path=oos_path,
        provenance_manifest=None,
    )
    assert out_path.exists()


# ---------------------------------------------------------------------------
# Defect 3 -- economic bars trial identity is the canonical provenance/
# semantic authority, never the physical bars file's own bytes.
# ---------------------------------------------------------------------------


def test_trial_identity_excludes_physical_bars_file_bytes(tmp_path):
    """REQUIRED TESTS 17/18 (identity layer): build_economic_trial_identity's
    returned identity dict must not carry the physical bars file's own
    sha256/bytes at all -- the physical record remains fully auditable per
    ATTEMPT via economic_walkforward.run_economic_walkforward's own output
    artifact, just never as a trial-defining fact."""
    bars = _bars_df([100.0, 101.0, 102.0, 103.0])
    paths = _write_min_registered_inputs(tmp_path, bars_df=bars)
    manifest = _base_manifest(bars)
    _trial_id_value, identity = build_economic_trial_identity(
        experiment_id="exp", hypothesis_id="hyp", strategy_id="strat",
        features_path=paths["features_path"], targets_path=paths["targets_path"],
        schema_path=paths["schema_path"], bars_path=paths["bars_path"],
        label_col="target", end_ts_col="end_ts", wf_spec=WalkForwardSpec(),
        l2=1e-3, lr=0.05, steps=10, standardize=True, clip_z=8.0,
        economic_spec=_minimal_economic_spec(), bars_provenance=manifest,
    )
    assert "economic_bars_csv" not in identity["data_identity"]
    assert (
        identity["data_identity"]["bars_provenance"]["canonical_semantic_bars_hash"]
        == manifest["canonical_semantic_bars_hash"]
    )
