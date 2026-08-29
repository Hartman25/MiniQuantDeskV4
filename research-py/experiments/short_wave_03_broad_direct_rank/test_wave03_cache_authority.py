"""WAVE03-CACHE-AUTHORITY-REPAIR-01 -- focused, network-free tests for
ensure_bars()/verify_wave03_bars_cache_authority() and the derived-artifact
helpers (ensure_real_targets/ensure_placebo_targets/ensure_full_features).
Uses only synthetic fixture bars/manifests built from the REAL
mqk_research.data.bars_provenance production seams (no hand-invented second
bars hash) -- no Alpaca access, no research-py/src modification.

Covers PREDECLARED_WAVE.json "cache_safety" and the mission's REQUIRED
NEGATIVE CONTROLS (each test's docstring cites which one).
"""
from __future__ import annotations

import json
import sys
import types
from pathlib import Path

import pandas as pd
import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402

sys.path.insert(0, str(EXPERIMENT_ROOT.parents[2] / "src"))
from mqk_research.data.bars_provenance import (  # noqa: E402
    CA_POLICY_ADJUSTED_DATA,
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
    SOURCE_AUTHORITY_OFFICIAL_PROVIDER,
    TRUSTED_CA_DISCOVERY_FLOOR_UTC,
    TRUSTED_CA_DISCOVERY_PROTOCOL_V2,
    TRUSTED_CA_DISCOVERY_TYPES,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    build_source_attestation,
    canonical_semantic_bars_hash,
    corporate_action_evidence_id,
)

_BARS_API_ENDPOINT = "https://data.alpaca.markets/v2/stocks/bars"
_CA_API_ENDPOINT = "https://data.alpaca.markets/v1/corporate-actions"
# Mirrors mqk_research.data.alpaca_historical.EXTRACTOR_ID -- a literal here
# (not an import) deliberately keeps this test file from ever importing the
# network-capable alpaca_historical module, which test_predeclaration.py and
# test_wave03_checkout_local_source_guard.py both assert never appears in
# sys.modules as a side effect of importing/running this experiment's tests.
_EXTRACTOR_ID = "mqk_research.data.alpaca_historical.v1"


# ---------------------------------------------------------------------------
# Fixture helpers -- build a genuinely VALID Wave-03 bars+manifest pair using
# only real production bars_provenance seams (build_source_attestation /
# build_corporate_action_evidence / build_bars_provenance_manifest), so
# verify_wave03_bars_cache_authority's full stack (structural + content-
# binding + corporate-action-integrity + Wave-03 identity binding) can be
# exercised against a real positive case, then selectively mutated.
# ---------------------------------------------------------------------------


def _frozen_symbols() -> list[str]:
    return sorted(run_wave.seed_symbols())


def _make_bars(symbols: list[str], *, date: str = "2020-01-02") -> pd.DataFrame:
    return pd.DataFrame(
        [{"symbol": s, "end_ts": f"{date}T00:00:00+00:00", "close": 100.0 + i} for i, s in enumerate(symbols)]
    )


def _build_valid_manifest(
    bars: pd.DataFrame, symbols: list[str], *, feed: str | None = None, asof: str | None = None
) -> dict:
    feed = run_wave.FEED if feed is None else feed
    asof = run_wave.ASOF if asof is None else asof
    start_utc = run_wave.START_UTC.tz_convert("UTC").isoformat()
    end_utc = run_wave.END_UTC.tz_convert("UTC").isoformat()

    evidence = build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=symbols,
        coverage_start_utc=start_utc,
        coverage_end_utc=end_utc,
        corporate_action_entries=(),
    )
    ca_coverage = {
        "discovery_protocol": TRUSTED_CA_DISCOVERY_PROTOCOL_V2,
        "ca_discovery_start_utc": TRUSTED_CA_DISCOVERY_FLOOR_UTC,
        "ca_discovery_end_utc": end_utc,
        "research_window_start_utc": start_utc,
        "research_window_end_utc": end_utc,
        "requested_types": sorted(TRUSTED_CA_DISCOVERY_TYPES),
        "symbols_requested": symbols,
    }
    attestation = build_source_attestation(
        source_provider_id="alpaca",
        extractor_id=_EXTRACTOR_ID,
        source_authority=SOURCE_AUTHORITY_OFFICIAL_PROVIDER,
        api_endpoint_bars=_BARS_API_ENDPOINT,
        api_endpoint_corporate_actions=_CA_API_ENDPOINT,
        symbols=symbols,
        requested_start_utc=start_utc,
        requested_end_utc=end_utc,
        returned_coverage_start_utc=start_utc,
        returned_coverage_end_utc=end_utc,
        adjustment_mode="all",
        feed=feed,
        asof=asof,
        pagination_complete_bars=True,
        pagination_complete_corporate_actions=True,
        corporate_action_query_coverage=ca_coverage,
        category_b_events_found=(),
        raw_response_content_hashes={},
        canonical_semantic_bars_hash=canonical_semantic_bars_hash(bars),
        canonical_corporate_action_evidence_hash=corporate_action_evidence_id(evidence),
        retrieval_timestamp_utc=end_utc,
    )
    price_provenance = {
        "close_column": "close",
        "provider_metadata_available": True,
        "provider_ids_observed": ["alpaca"],
        "price_adjustment_convention": PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
        "convention_basis": "test fixture",
    }
    return build_bars_provenance_manifest(
        price_provenance=price_provenance,
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe=run_wave.TIMEFRAME,
        start_utc=start_utc,
        end_utc=end_utc,
        symbol_universe=symbols,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        source_attestation=attestation,
    )


def _valid_bars_and_manifest() -> tuple[pd.DataFrame, dict]:
    symbols = _frozen_symbols()
    bars = _make_bars(symbols)
    manifest = _build_valid_manifest(bars, symbols)
    return bars, manifest


# ---------------------------------------------------------------------------
# POSITIVE CASE: a correctly-built manifest passes full verification.
# ---------------------------------------------------------------------------


def test_valid_wave03_manifest_passes_full_verification() -> None:
    bars, manifest = _valid_bars_and_manifest()
    run_wave.verify_wave03_bars_cache_authority(bars, manifest)  # must not raise


# ---------------------------------------------------------------------------
# REQUIRED NEGATIVE CONTROL: mutate raw_bars.csv content after manifest
# creation -> cache reuse fails.
# ---------------------------------------------------------------------------


def test_mutated_bars_content_after_manifest_creation_fails_verification() -> None:
    bars, manifest = _valid_bars_and_manifest()
    mutated_bars = bars.copy()
    mutated_bars.loc[0, "close"] = mutated_bars.loc[0, "close"] + 999.0
    with pytest.raises(Exception):
        run_wave.verify_wave03_bars_cache_authority(mutated_bars, manifest)


# ---------------------------------------------------------------------------
# REQUIRED NEGATIVE CONTROL: mutate feed/universe/asof identity in manifest
# -> fails.
# ---------------------------------------------------------------------------


def test_manifest_wrong_feed_fails_verification() -> None:
    symbols = _frozen_symbols()
    bars = _make_bars(symbols)
    manifest = _build_valid_manifest(bars, symbols, feed="iex")
    with pytest.raises(RuntimeError, match="feed"):
        run_wave.verify_wave03_bars_cache_authority(bars, manifest)


def test_manifest_wrong_asof_fails_verification() -> None:
    symbols = _frozen_symbols()
    bars = _make_bars(symbols)
    manifest = _build_valid_manifest(bars, symbols, asof="2023-01-01")
    with pytest.raises(RuntimeError, match="asof"):
        run_wave.verify_wave03_bars_cache_authority(bars, manifest)


def test_manifest_wrong_universe_fails_verification() -> None:
    symbols = _frozen_symbols()
    bars = _make_bars(symbols)
    narrower = symbols[:-1]  # drop one frozen seed symbol
    narrower_bars = bars[bars["symbol"].isin(narrower)].reset_index(drop=True)
    manifest = _build_valid_manifest(narrower_bars, narrower)
    with pytest.raises(RuntimeError, match="seed universe"):
        run_wave.verify_wave03_bars_cache_authority(narrower_bars, manifest)


def test_manifest_wrong_window_fails_verification() -> None:
    bars, manifest = _valid_bars_and_manifest()
    manifest = json.loads(json.dumps(manifest))
    manifest["start_utc"] = "2017-01-01T00:00:00+00:00"
    with pytest.raises(RuntimeError, match="window"):
        run_wave.verify_wave03_bars_cache_authority(bars, manifest)


# ---------------------------------------------------------------------------
# REQUIRED NEGATIVE CONTROL: orphan bars without manifest / orphan manifest
# without bars -> ensure_bars() fails closed.
# ---------------------------------------------------------------------------


def test_ensure_bars_fails_closed_on_orphan_bars_without_manifest(monkeypatch, tmp_path: Path) -> None:
    run_root = tmp_path / "runs" / "run_01"
    run_root.mkdir(parents=True)
    (run_root / "raw_bars.csv").write_text("symbol,end_ts,close\nAAPL,2020-01-02T00:00:00+00:00,100.0\n", encoding="utf-8")
    monkeypatch.setattr(run_wave, "RUN_ROOT", run_root)
    with pytest.raises(RuntimeError, match="orphan"):
        run_wave.ensure_bars()


def test_ensure_bars_fails_closed_on_orphan_manifest_without_bars(monkeypatch, tmp_path: Path) -> None:
    run_root = tmp_path / "runs" / "run_01"
    run_root.mkdir(parents=True)
    (run_root / "bars_provenance_manifest.json").write_text("{}", encoding="utf-8")
    monkeypatch.setattr(run_wave, "RUN_ROOT", run_root)
    with pytest.raises(RuntimeError, match="orphan"):
        run_wave.ensure_bars()


# ---------------------------------------------------------------------------
# REQUIRED NEGATIVE CONTROL: a verified cache IS reused (no re-fetch) --
# and rank01/rank02/rank03 (i.e. every ensure_bars() caller within one run)
# resolve the identical canonical bars hash from that one cache.
# ---------------------------------------------------------------------------


def test_ensure_bars_reuses_verified_cache_without_refetching(monkeypatch, tmp_path: Path) -> None:
    run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", run_root)

    bars, manifest = _valid_bars_and_manifest()
    fetch_calls = {"n": 0}

    def fake_extract(**kwargs):
        fetch_calls["n"] += 1
        return {"bars": bars, "manifest": manifest}

    monkeypatch.setattr(run_wave, "_load_paper_credentials_into_env", lambda: None)
    # ensure_bars() lazily does `from mqk_research.data.alpaca_historical
    # import extract_research_bars_with_provenance` inside the function body
    # -- inject a fake module into sys.modules (monkeypatch restores/removes
    # it afterward) rather than importing the real network-capable module,
    # which test_predeclaration.py and test_wave03_checkout_local_source_
    # guard.py both assert never appears in sys.modules as a side effect of
    # this experiment's own test suite.
    fake_module = types.ModuleType("mqk_research.data.alpaca_historical")
    fake_module.extract_research_bars_with_provenance = fake_extract
    monkeypatch.setitem(sys.modules, "mqk_research.data.alpaca_historical", fake_module)

    bars_1, manifest_1 = run_wave.ensure_bars()  # first call: fetches fresh, writes cache
    bars_2, manifest_2 = run_wave.ensure_bars()  # rank02's own call: must reuse the verified cache
    bars_3, manifest_3 = run_wave.ensure_bars()  # rank03's own call: must reuse the verified cache

    assert fetch_calls["n"] == 1
    hash_1 = manifest_1["canonical_semantic_bars_hash"]
    hash_2 = manifest_2["canonical_semantic_bars_hash"]
    hash_3 = manifest_3["canonical_semantic_bars_hash"]
    assert hash_1 == hash_2 == hash_3 == manifest["canonical_semantic_bars_hash"]


# ---------------------------------------------------------------------------
# REQUIRED NEGATIVE CONTROL: targets/features from bars A cannot be reused
# when current bars are B -- ensure_real_targets/ensure_placebo_targets/
# ensure_full_features never read a stale on-disk file, always recompute
# from the in-memory bars they were given.
# ---------------------------------------------------------------------------


def test_ensure_real_targets_never_reuses_stale_file_from_a_different_bars_snapshot(monkeypatch, tmp_path: Path) -> None:
    run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", run_root)

    bars_a = pd.DataFrame(
        [{"symbol": "A", "end_ts": f"2020-01-{d:02d} 00:00:00", "close": 100.0 + d} for d in range(1, 15)]
    )
    bars_b = pd.DataFrame(
        [{"symbol": "A", "end_ts": f"2020-01-{d:02d} 00:00:00", "close": 500.0 + d} for d in range(1, 15)]
    )

    targets_a = run_wave.ensure_real_targets(bars_a)
    on_disk_after_a = pd.read_csv(run_root / "real_targets.csv")
    assert on_disk_after_a["fwd_ret"].iloc[0] == pytest.approx(targets_a["fwd_ret"].iloc[0])

    targets_b = run_wave.ensure_real_targets(bars_b)
    on_disk_after_b = pd.read_csv(run_root / "real_targets.csv")

    # bars_b's targets must NOT equal bars_a's stale targets (different price
    # levels/paths -> different fwd_ret), and the on-disk file must reflect
    # the FRESH (bars_b) computation, never the earlier (bars_a) one.
    assert targets_b["fwd_ret"].iloc[0] != pytest.approx(targets_a["fwd_ret"].iloc[0])
    assert on_disk_after_b["fwd_ret"].iloc[0] == pytest.approx(targets_b["fwd_ret"].iloc[0])


def test_ensure_full_features_never_reuses_stale_file_from_a_different_bars_snapshot(monkeypatch, tmp_path: Path) -> None:
    run_root = tmp_path / "runs" / "run_01"
    monkeypatch.setattr(run_wave, "RUN_ROOT", run_root)

    dates = pd.date_range("2020-01-01", periods=100, freq="D")

    def _ohlcv(close_fn):
        return pd.DataFrame(
            [
                {
                    "symbol": "A", "end_ts": ts.strftime("%Y-%m-%d 00:00:00"),
                    "open": close_fn(i), "high": close_fn(i) * 1.01, "low": close_fn(i) * 0.99,
                    "close": close_fn(i), "volume": 1000,
                }
                for i, ts in enumerate(dates)
            ]
        )

    bars_a = _ohlcv(lambda d: 100.0 + d)
    bars_b = _ohlcv(lambda d: 500.0 - d)

    feats_a = run_wave.ensure_full_features(bars_a)
    feats_b = run_wave.ensure_full_features(bars_b)
    on_disk_after_b = pd.read_csv(run_root / "full_features.csv")

    assert not feats_a["gap_pct_1"].fillna(0).equals(feats_b["gap_pct_1"].fillna(0))
    pd.testing.assert_frame_equal(
        on_disk_after_b.reset_index(drop=True), feats_b.reset_index(drop=True), check_dtype=False
    )


# ---------------------------------------------------------------------------
# Static source proof, updated for R2 (replaces the old, no-longer-valid
# "no `.exists()` anywhere in ensure_bars" assertion -- a verified cache
# reuse path legitimately needs an existence check as part of orphan
# detection; what must never happen is trusting existence ALONE).
# ---------------------------------------------------------------------------


def test_ensure_bars_never_returns_cached_data_without_calling_the_verifier() -> None:
    source = (EXPERIMENT_ROOT / "run_wave.py").read_text(encoding="utf-8")
    start = source.index("def ensure_bars(")
    end = source.index("\ndef ", start + 1)
    body = source[start:end]
    assert "verify_wave03_bars_cache_authority" in body
