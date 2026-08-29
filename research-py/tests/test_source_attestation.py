"""
BKT-RESEARCH-MARKET-DATA-AUTHORITY-01 -- tests for the source-attestation
contract added to mqk_research.data.bars_provenance
(build_source_attestation / source_attestation_id /
_require_verified_source_attestation), which closes the mission's explicit
defect: "A caller manually constructing
price_adjustment_convention='alpaca_all_adjusted_v1' must NOT be sufficient"
to authorize an OFFICIAL registered adjusted-data economic run.

Covers REQUIRED TESTS 11-16 (trusted-vs-synthetic evidence boundary) as they
apply to the NEW adjusted_data + source_attestation path, distinct from the
pre-existing forbid_affected_periods + corporate_action_evidence tests
already in test_bars_provenance.py.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict

import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_POLICY_ADJUSTED_DATA,
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    TRUSTED_CA_DISCOVERY_FLOOR_UTC,
    TRUSTED_CA_DISCOVERY_PROTOCOL_V2,
    TRUSTED_CA_DISCOVERY_TYPES,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    BarsProvenanceUnverifiable,
    SourceAttestationUnverifiable,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    build_source_attestation,
    canonical_semantic_bars_hash,
    check_corporate_action_integrity,
    corporate_action_evidence_id,
    require_registered_bars_provenance,
    source_attestation_id,
)
from mqk_research.ml.economic_registry_integration import build_economic_trial_identity
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec


TRUSTED_EXTRACTOR_ID = "mqk_research.data.alpaca_historical.v1"


def _bars_df(prices, symbol: str = "AAA", start: str = "2021-01-01") -> pd.DataFrame:
    dates = pd.date_range(start, periods=len(prices), freq="D", tz="UTC")
    return pd.DataFrame({"symbol": symbol, "end_ts": [d.isoformat() for d in dates], "close": prices})


def _clean_ca_evidence(bars: pd.DataFrame) -> Dict[str, Any]:
    symbols = sorted(bars["symbol"].unique().tolist())
    return build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=symbols,
        coverage_start_utc="2021-01-01T00:00:00+00:00",
        coverage_end_utc="2021-02-01T00:00:00+00:00",
        corporate_action_entries=[],
    )


def _valid_attestation(bars: pd.DataFrame, evidence: Dict[str, Any], **overrides: Any) -> Dict[str, Any]:
    """Builds an internally-consistent attestation: `corporate_action_query_
    coverage`'s research-window/symbols/snapshot-cutoff fields are DERIVED
    from whatever `symbols`/`requested_start_utc`/`requested_end_utc`/
    `retrieval_timestamp_utc` overrides are supplied, so a test overriding
    one of those fields alone (e.g. symbols=["BBB"]) still produces a
    coverage object that agrees with it -- exercising the intended
    downstream check rather than tripping the REPAIR-03 internal-consistency
    check as an unrelated side effect. Pass `corporate_action_query_coverage`
    explicitly to test that consistency check itself."""
    symbols = overrides.get("symbols", sorted(bars["symbol"].unique().tolist()))
    requested_start_utc = overrides.get("requested_start_utc", "2021-01-01T00:00:00+00:00")
    requested_end_utc = overrides.get("requested_end_utc", "2021-02-01T00:00:00+00:00")
    retrieval_timestamp_utc = overrides.get("retrieval_timestamp_utc", "2026-08-15T00:00:00+00:00")
    default_coverage = {
        "discovery_protocol": TRUSTED_CA_DISCOVERY_PROTOCOL_V2,
        "ca_discovery_start_utc": TRUSTED_CA_DISCOVERY_FLOOR_UTC,
        "ca_discovery_end_utc": retrieval_timestamp_utc,
        "research_window_start_utc": requested_start_utc,
        "research_window_end_utc": requested_end_utc,
        # REPAIR-04: the happy-path fixture must use the COMPLETE trusted CA
        # type vocabulary -- it is the actual official query contract, not an
        # arbitrary sample. Tests that want an incomplete/narrow contract
        # build that invalid state explicitly (see REPAIR-04 section below).
        "requested_types": sorted(TRUSTED_CA_DISCOVERY_TYPES),
        "symbols_requested": symbols,
    }
    kwargs: Dict[str, Any] = dict(
        source_provider_id="alpaca",
        extractor_id=TRUSTED_EXTRACTOR_ID,
        source_authority="official_provider",
        api_endpoint_bars="https://data.alpaca.markets/v2/stocks/bars",
        api_endpoint_corporate_actions="https://data.alpaca.markets/v1/corporate-actions",
        symbols=symbols,
        requested_start_utc=requested_start_utc,
        requested_end_utc=requested_end_utc,
        returned_coverage_start_utc="2021-01-01T00:00:00+00:00",
        returned_coverage_end_utc="2021-02-01T00:00:00+00:00",
        adjustment_mode="all",
        feed="iex",
        asof="2021-02-15",
        pagination_complete_bars=True,
        pagination_complete_corporate_actions=True,
        corporate_action_query_coverage=default_coverage,
        category_b_events_found=[],
        raw_response_content_hashes={"bars_pages": ["h1"], "corporate_action_pages": ["h2"]},
        canonical_semantic_bars_hash=canonical_semantic_bars_hash(bars),
        canonical_corporate_action_evidence_hash=corporate_action_evidence_id(evidence),
        retrieval_timestamp_utc=retrieval_timestamp_utc,
    )
    kwargs.update(overrides)
    return build_source_attestation(**kwargs)


def _valid_manifest(bars: pd.DataFrame, **manifest_overrides: Any) -> Dict[str, Any]:
    evidence = manifest_overrides.pop("corporate_action_evidence", None) or _clean_ca_evidence(bars)
    attestation = manifest_overrides.pop("source_attestation", None)
    if attestation is None:
        attestation = _valid_attestation(bars, evidence)
    symbols = sorted(bars["symbol"].unique().tolist())
    kwargs: Dict[str, Any] = dict(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "test",
        },
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1Day",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=symbols,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        source_attestation=attestation,
    )
    kwargs.update(manifest_overrides)
    return build_bars_provenance_manifest(**kwargs)


# ---------------------------------------------------------------------------
# Happy path
# ---------------------------------------------------------------------------


def test_valid_source_attestation_authorizes_adjusted_data_policy():
    bars = _bars_df([100.0, 101.0, 102.0])
    manifest = _valid_manifest(bars)
    require_registered_bars_provenance(manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_source_attestation_id_is_content_derived_and_deterministic():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(bars, evidence)
    att_b = _valid_attestation(bars, evidence)
    assert att_a["attestation_id"] == att_b["attestation_id"]
    assert att_a["attestation_id"] == source_attestation_id(att_a)


def test_source_attestation_id_excludes_retrieval_timestamp():
    """The SAME extraction re-run at a different wall-clock moment must
    still produce the same attestation identity -- retrieval_timestamp_utc
    is audit-only metadata, not part of content identity."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(bars, evidence, retrieval_timestamp_utc="2026-01-01T00:00:00+00:00")
    att_b = _valid_attestation(bars, evidence, retrieval_timestamp_utc="2026-06-01T00:00:00+00:00")
    assert att_a["attestation_id"] == att_b["attestation_id"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 12/13 -- manually-fabricated / caller-asserted convention
# cannot authorize an official run
# ---------------------------------------------------------------------------


def test_bare_convention_string_without_attestation_cannot_authorize_run():
    """The exact defect this patch closes: a hand-built manifest asserting
    price_adjustment_convention=alpaca_all_adjusted_v1 with NO
    source_attestation object must fail closed."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    manifest = build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "hand-typed, not extracted",
        },
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        timeframe="1Day",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=["AAA"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        # no source_attestation supplied at all
    )
    with pytest.raises(BarsProvenanceUnverifiable):
        require_registered_bars_provenance(manifest)
    with pytest.raises(SourceAttestationUnverifiable):
        check_corporate_action_integrity(bars, manifest)


def test_declared_source_attestation_id_must_match_recomputed_hash():
    """A caller who declares a source_attestation_id that does not match the
    recomputed canonical content hash of the attestation object it sits
    beside must fail -- the id is never trusted at face value."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    tampered_manifest = dict(manifest)
    tampered_manifest["source_attestation_id"] = "not-the-real-hash"
    with pytest.raises(SourceAttestationUnverifiable, match="does not match"):
        check_corporate_action_integrity(bars, tampered_manifest)


def test_extractor_id_not_trusted_cannot_authorize_run():
    """REQUIRED TEST 13 analogue for this path: a 'fake' extractor identity
    (the source_attestation equivalent of a fake source_provider_id) must
    not authorize an official run, even with an otherwise well-formed,
    internally-consistent attestation object."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, extractor_id="totally_made_up_extractor")
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="not a trusted research extractor"):
        check_corporate_action_integrity(bars, manifest)


def test_wrong_schema_version_cannot_authorize_run():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = dict(_valid_attestation(bars, evidence))
    attestation["schema_version"] = "some_other_version"
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="schema_version"):
        check_corporate_action_integrity(bars, manifest)


def test_mismatched_adjustment_mode_cannot_authorize_run():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, adjustment_mode="split")
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="adjustment_mode"):
        check_corporate_action_integrity(bars, manifest)


def test_incomplete_pagination_cannot_authorize_run():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, pagination_complete_bars=False)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="incomplete pagination"):
        check_corporate_action_integrity(bars, manifest)


def test_unresolved_category_b_events_cannot_authorize_run():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars,
        evidence,
        category_b_events_found=[{"symbol": "AAA", "action_type": "cash_merger"}],
    )
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="REQUIRES_FAIL_CLOSED_REVIEW"):
        check_corporate_action_integrity(bars, manifest)


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 Defect 3 -- trusted source
# profile: a fake provider id / endpoint / diagnostic authority must fail
# the official gate exactly like a fake extractor_id already does.
# ---------------------------------------------------------------------------


def test_fake_source_provider_id_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, source_provider_id="made-up-provider")
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="source_provider_id"):
        check_corporate_action_integrity(bars, manifest)


def test_fake_bars_endpoint_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, api_endpoint_bars="https://evil.example.com/v2/stocks/bars")
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="api_endpoint_bars"):
        check_corporate_action_integrity(bars, manifest)


def test_fake_corporate_actions_endpoint_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, api_endpoint_corporate_actions="https://evil.example.com/v1/corporate-actions"
    )
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="api_endpoint_corporate_actions"):
        check_corporate_action_integrity(bars, manifest)


def test_diagnostic_source_authority_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, source_authority="diagnostic_synthetic")
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="source_authority"):
        check_corporate_action_integrity(bars, manifest)


def test_official_trusted_profile_passes():
    """REQUIRED TEST 14: a fully well-formed, OFFICIAL-authority attestation
    -- correct provider, extractor, endpoints, source_authority, explicit
    asof, complete pagination, no unresolved review-required events, and
    matching bars/CA hashes -- passes the registered gate."""
    bars = _bars_df([100.0, 101.0])
    manifest = _valid_manifest(bars)
    require_registered_bars_provenance(manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 Defect 2 -- explicit ASOF
# ---------------------------------------------------------------------------


def test_missing_asof_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, asof=None)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="asof"):
        check_corporate_action_integrity(bars, manifest)


def test_changing_asof_changes_source_attestation_id():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(bars, evidence, asof="2021-02-15")
    att_b = _valid_attestation(bars, evidence, asof="2021-06-01")
    assert att_a["attestation_id"] != att_b["attestation_id"]


def test_same_asof_and_semantic_data_is_deterministic():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(bars, evidence, asof="2021-02-15")
    att_b = _valid_attestation(bars, evidence, asof="2021-02-15")
    assert att_a["attestation_id"] == att_b["attestation_id"]


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 Defect 4 -- semantic
# identity excludes transport/pagination artifacts (page hashes)
# ---------------------------------------------------------------------------


def test_different_page_segmentation_preserves_semantic_source_identity():
    """REQUIRED TEST 15: same semantic bars + same CA evidence + same
    request contract but different page boundaries => same semantic
    source_attestation_id."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(
        bars, evidence, raw_response_content_hashes={"bars_pages": ["p1"], "corporate_action_pages": ["c1"]}
    )
    att_b = _valid_attestation(
        bars,
        evidence,
        raw_response_content_hashes={"bars_pages": ["p1a", "p1b"], "corporate_action_pages": ["c1a", "c1b", "c1c"]},
    )
    assert att_a["attestation_id"] == att_b["attestation_id"]


def test_raw_response_hashes_remain_audit_visible_despite_exclusion_from_identity():
    """REQUIRED TEST 16: page/raw-response hashes stay on the attestation
    object (audit evidence) even though they do not participate in
    source_attestation_id."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, raw_response_content_hashes={"bars_pages": ["p1", "p2"], "corporate_action_pages": ["c1"]}
    )
    assert attestation["raw_response_content_hashes"] == {"bars_pages": ["p1", "p2"], "corporate_action_pages": ["c1"]}


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-03 Defect 3 -- CA discovery
# snapshot clock excluded from semantic identity; internal consistency
# enforced on what remains.
# ---------------------------------------------------------------------------


def test_ca_discovery_end_utc_excluded_from_source_attestation_id():
    """RED/GREEN proof: same semantic bars + same semantic CA evidence + same
    asof + a DIFFERENT CA discovery snapshot cutoff must produce the SAME
    source_attestation_id (REQUIRED TESTS 9/10). Pre-repair, ca_discovery_
    end_utc sat inside the hashed corporate_action_query_coverage dict, so
    this would have failed (a different cutoff meant a different hash);
    post-repair, canonical_ca_query_semantic_content excludes it."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    att_a = _valid_attestation(bars, evidence, retrieval_timestamp_utc="2022-01-01T00:00:00+00:00")
    att_b = _valid_attestation(bars, evidence, retrieval_timestamp_utc="2022-09-20T00:00:00+00:00")
    assert (
        att_a["corporate_action_query_coverage"]["ca_discovery_end_utc"]
        != att_b["corporate_action_query_coverage"]["ca_discovery_end_utc"]
    )
    assert att_a["attestation_id"] == att_b["attestation_id"]


def test_ca_discovery_coverage_still_audit_visible_despite_exclusion_from_identity():
    """REQUIRED TEST 12: attestation objects still visibly record the
    different audit snapshot cutoffs even though identity is unaffected."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, retrieval_timestamp_utc="2022-09-20T00:00:00+00:00")
    assert attestation["corporate_action_query_coverage"]["ca_discovery_end_utc"] == "2022-09-20T00:00:00+00:00"
    assert attestation["retrieval_timestamp_utc"] == "2022-09-20T00:00:00+00:00"


def test_ca_discovery_coverage_missing_fields_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, corporate_action_query_coverage={"discovery_protocol": "v2"}
    )
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="missing required field"):
        check_corporate_action_integrity(bars, manifest)


def test_ca_discovery_coverage_retrieval_timestamp_mismatch_fails_official_gate():
    """The recorded snapshot cutoff inside corporate_action_query_coverage
    must agree with the attestation's own retrieval_timestamp_utc -- a
    contradictory pair (both structurally present) must still fail closed."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    contradictory_coverage = {
        "discovery_protocol": "process_date_full_history_through_retrieval_snapshot_v2",
        "ca_discovery_start_utc": "1900-01-01T00:00:00+00:00",
        "ca_discovery_end_utc": "2019-01-01T00:00:00+00:00",  # does not match retrieval_timestamp_utc below
        "research_window_start_utc": "2021-01-01T00:00:00+00:00",
        "research_window_end_utc": "2021-02-01T00:00:00+00:00",
        "requested_types": ["cash_dividend"],
        "symbols_requested": ["AAA"],
    }
    attestation = _valid_attestation(
        bars,
        evidence,
        corporate_action_query_coverage=contradictory_coverage,
        retrieval_timestamp_utc="2026-08-15T00:00:00+00:00",
    )
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="does not agree with the attestation's own retrieval_timestamp_utc"):
        check_corporate_action_integrity(bars, manifest)


def test_ca_discovery_coverage_research_window_mismatch_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    contradictory_coverage = {
        "discovery_protocol": "process_date_full_history_through_retrieval_snapshot_v2",
        "ca_discovery_start_utc": "1900-01-01T00:00:00+00:00",
        "ca_discovery_end_utc": "2026-08-15T00:00:00+00:00",
        "research_window_start_utc": "1999-01-01T00:00:00+00:00",  # does not match requested_start_utc
        "research_window_end_utc": "2021-02-01T00:00:00+00:00",
        "requested_types": ["cash_dividend"],
        "symbols_requested": ["AAA"],
    }
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=contradictory_coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="research window"):
        check_corporate_action_integrity(bars, manifest)


def test_ca_discovery_coverage_symbols_mismatch_fails_official_gate():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    contradictory_coverage = {
        "discovery_protocol": "process_date_full_history_through_retrieval_snapshot_v2",
        "ca_discovery_start_utc": "1900-01-01T00:00:00+00:00",
        "ca_discovery_end_utc": "2026-08-15T00:00:00+00:00",
        "research_window_start_utc": "2021-01-01T00:00:00+00:00",
        "research_window_end_utc": "2021-02-01T00:00:00+00:00",
        "requested_types": ["cash_dividend"],
        "symbols_requested": ["ZZZ"],  # does not match attestation's own symbols=["AAA"]
    }
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=contradictory_coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="symbols_requested"):
        check_corporate_action_integrity(bars, manifest)


# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-04 -- official attestations
# must claim the TRUSTED CA discovery query contract (protocol, full-history
# floor, complete type vocabulary, snapshot covering the full research
# window), not merely an internally-consistent one.
# ---------------------------------------------------------------------------


def _coverage(**overrides: Any) -> Dict[str, Any]:
    base = {
        "discovery_protocol": TRUSTED_CA_DISCOVERY_PROTOCOL_V2,
        "ca_discovery_start_utc": TRUSTED_CA_DISCOVERY_FLOOR_UTC,
        "ca_discovery_end_utc": "2026-08-15T00:00:00+00:00",
        "research_window_start_utc": "2021-01-01T00:00:00+00:00",
        "research_window_end_utc": "2021-02-01T00:00:00+00:00",
        "requested_types": sorted(TRUSTED_CA_DISCOVERY_TYPES),
        "symbols_requested": ["AAA"],
    }
    base.update(overrides)
    return base


def test_narrow_requested_types_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 2 + RED proof 9: an official-looking
    attestation claiming only requested_types=["cash_dividend"] -- exactly
    the pre-repair happy-path fixture's value -- must now be rejected. RED:
    REPAIR-03's _require_consistent_ca_discovery_coverage ALONE (the
    pre-REPAIR-04 gate) does not catch this -- it only checks field
    presence/internal agreement, never completeness against the trusted
    vocabulary -- proving the pre-repair gap described in the mission was
    real. GREEN: the full official gate (adding REPAIR-04's
    _require_trusted_ca_discovery_contract) rejects it."""
    import mqk_research.data.bars_provenance as bp

    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    narrow_coverage = _coverage(requested_types=["cash_dividend"])
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=narrow_coverage)

    bp._require_consistent_ca_discovery_coverage(attestation)  # RED: must not raise

    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="requested_types"):
        check_corporate_action_integrity(bars, manifest)  # GREEN: must raise


def test_requested_types_missing_one_category_b_type_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 3: omitting a single Category-B type
    (cash_merger) from an otherwise-complete requested_types set must still
    fail closed."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    incomplete_types = sorted(TRUSTED_CA_DISCOVERY_TYPES - {"cash_merger"})
    coverage = _coverage(requested_types=incomplete_types)
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="cash_merger"):
        check_corporate_action_integrity(bars, manifest)


def test_requested_types_unknown_extra_type_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 4: a superset containing an unverified type
    is equally untrusted as a subset -- must fail closed."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    extra_types = sorted(TRUSTED_CA_DISCOVERY_TYPES | {"totally_made_up_type"})
    coverage = _coverage(requested_types=extra_types)
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="totally_made_up_type"):
        check_corporate_action_integrity(bars, manifest)


def test_wrong_discovery_protocol_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 5."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    coverage = _coverage(discovery_protocol="process_date_full_history_through_retrieval_snapshot_v1")
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="discovery_protocol"):
        check_corporate_action_integrity(bars, manifest)


def test_narrower_discovery_floor_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 6: a later/narrower discovery floor than the
    trusted 1900-01-01 floor cannot prove complete CA discovery coverage."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    coverage = _coverage(ca_discovery_start_utc="2015-01-01T00:00:00+00:00")
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="ca_discovery_start_utc"):
        check_corporate_action_integrity(bars, manifest)


def test_ca_discovery_cutoff_earlier_than_research_window_end_fails_official_gate():
    """REQUIRED NEGATIVE CONTROL 7: the resolved CA discovery snapshot cutoff
    must cover the full claimed research interval -- a snapshot that
    predates research_window_end cannot honestly claim CA coverage for it."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    early_cutoff = "2020-06-01T00:00:00+00:00"  # earlier than the default research_window_end (2021-02-01)
    attestation = _valid_attestation(bars, evidence, retrieval_timestamp_utc=early_cutoff)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="ca_discovery_end_utc"):
        check_corporate_action_integrity(bars, manifest)


def test_requested_types_order_independent_passes_official_gate():
    """REQUIRED NEGATIVE CONTROL 8: requested_types comparison is an exact
    SET comparison -- a reordered (but complete) type list must still pass."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    reordered_types = sorted(TRUSTED_CA_DISCOVERY_TYPES, reverse=True)
    coverage = _coverage(requested_types=reordered_types)
    attestation = _valid_attestation(bars, evidence, corporate_action_query_coverage=coverage)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    require_registered_bars_provenance(manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_different_semantic_bars_hash_changes_source_attestation_id():
    """REQUIRED TEST 17: different semantic bars still changes identity."""
    bars_a = _bars_df([100.0, 101.0])
    bars_b = _bars_df([200.0, 201.0])
    evidence = _clean_ca_evidence(bars_a)
    att_a = _valid_attestation(bars_a, evidence)
    att_b = _valid_attestation(bars_b, evidence)
    assert att_a["attestation_id"] != att_b["attestation_id"]


def test_different_ca_evidence_hash_changes_source_attestation_id():
    """REQUIRED TEST 18: different semantic CA evidence still changes
    identity."""
    bars = _bars_df([100.0, 101.0])
    evidence_a = _clean_ca_evidence(bars)
    evidence_b = build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=["AAA"],
        coverage_start_utc="2021-01-01T00:00:00+00:00",
        coverage_end_utc="2021-02-01T00:00:00+00:00",
        corporate_action_entries=[
            {"symbol": "AAA", "action_type": "cash_dividend", "effective_start_ts": "2021-01-10", "effective_end_ts": "2021-01-10"}
        ],
    )
    att_a = _valid_attestation(bars, evidence_a)
    att_b = _valid_attestation(bars, evidence_b)
    assert att_a["attestation_id"] != att_b["attestation_id"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 14 -- altered bars after provenance creation fail before P&L
# ---------------------------------------------------------------------------


def test_bars_hash_mismatch_between_attestation_and_manifest_fails():
    bars = _bars_df([100.0, 101.0])
    other_bars = _bars_df([999.0, 998.0])
    evidence = _clean_ca_evidence(bars)
    # Attestation was built for DIFFERENT bars than the manifest declares.
    attestation = _valid_attestation(other_bars, evidence)
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="canonical_semantic_bars_hash"):
        check_corporate_action_integrity(bars, manifest)


# ---------------------------------------------------------------------------
# REQUIRED TEST 15 -- altered CA evidence fails
# ---------------------------------------------------------------------------


def test_tampered_corporate_action_evidence_fails():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    manifest = _valid_manifest(bars, corporate_action_evidence=evidence)
    tampered_manifest = dict(manifest)
    tampered_evidence = dict(manifest["corporate_action_evidence"])
    tampered_evidence["corporate_action_entries"] = [
        {"symbol": "AAA", "action_type": "cash_merger", "effective_start_ts": "x", "effective_end_ts": "y"}
    ]
    tampered_manifest["corporate_action_evidence"] = tampered_evidence
    # corporate_action_evidence_id is left UNCHANGED (the tamper doesn't
    # update it) -- this is exactly the "altered evidence" scenario.
    with pytest.raises(SourceAttestationUnverifiable, match="tampered corporate-action evidence"):
        check_corporate_action_integrity(bars, tampered_manifest)


def test_attestation_ca_evidence_hash_mismatch_fails():
    bars = _bars_df([100.0, 101.0])
    evidence_a = _clean_ca_evidence(bars)
    evidence_b = build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=["AAA"],
        coverage_start_utc="2021-01-01T00:00:00+00:00",
        coverage_end_utc="2021-02-01T00:00:00+00:00",
        corporate_action_entries=[
            {"symbol": "AAA", "action_type": "cash_dividend", "effective_start_ts": "2021-01-10", "effective_end_ts": "2021-01-10"}
        ],
    )
    # Attestation attests to evidence_b's hash, but the manifest carries evidence_a.
    attestation = _valid_attestation(bars, evidence_b)
    manifest = _valid_manifest(bars, corporate_action_evidence=evidence_a, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="canonical_corporate_action_evidence_hash"):
        check_corporate_action_integrity(bars, manifest)


# ---------------------------------------------------------------------------
# Coverage checks
# ---------------------------------------------------------------------------


def test_attestation_missing_symbol_coverage_fails():
    bars = _bars_df([100.0, 101.0], symbol="AAA")
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence, symbols=["BBB"])
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="symbol coverage"):
        check_corporate_action_integrity(bars, manifest)


def test_attestation_coverage_range_too_narrow_fails():
    bars = _bars_df([100.0, 101.0, 102.0], start="2021-01-25")  # Jan25, Jan26, Jan27
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, returned_coverage_start_utc="2021-01-01T00:00:00+00:00", returned_coverage_end_utc="2021-01-26T00:00:00+00:00"
    )
    manifest = _valid_manifest(bars, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable, match="coverage range"):
        check_corporate_action_integrity(bars, manifest)


# ---------------------------------------------------------------------------
# Structural (require_registered_bars_provenance) gate
# ---------------------------------------------------------------------------


def test_structural_gate_requires_source_attestation_id_for_alpaca_convention():
    bars = _bars_df([100.0, 101.0])
    manifest = _valid_manifest(bars)
    stripped = dict(manifest)
    stripped["source_attestation_id"] = None
    with pytest.raises(BarsProvenanceUnverifiable, match="source_attestation_id"):
        require_registered_bars_provenance(stripped)


def test_structural_gate_unaffected_for_raw_unadjusted_convention():
    """raw_unadjusted never required a source_attestation before this patch
    and must not require one now -- _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION
    is scoped to alpaca_all_adjusted_v1 only."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    manifest = build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "t",
        },
        corporate_action_policy="forbid_affected_periods",
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        timeframe="1Day",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=["AAA"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
    )
    require_registered_bars_provenance(manifest)  # must not raise


# ---------------------------------------------------------------------------
# Trial identity integration
# ---------------------------------------------------------------------------


def _write_min_registered_inputs(tmp_path: Path, bars_df: pd.DataFrame) -> Dict[str, Path]:
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
    bars_df.to_csv(bars_path, index=False)
    return {"features_path": features_path, "targets_path": targets_path, "schema_path": schema_path, "bars_path": bars_path}


def _minimal_economic_spec() -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
    )


def test_source_attestation_id_participates_in_trial_identity(tmp_path):
    bars = _bars_df([100.0, 101.0])
    paths = _write_min_registered_inputs(tmp_path, bars)
    evidence = _clean_ca_evidence(bars)

    manifest_a = _valid_manifest(bars, corporate_action_evidence=evidence)
    attestation_b = _valid_attestation(bars, evidence, api_endpoint_bars="https://data.alpaca.markets/v2/stocks/bars?v=2")
    manifest_b = _valid_manifest(bars, corporate_action_evidence=evidence, source_attestation=attestation_b)
    assert manifest_a["source_attestation_id"] != manifest_b["source_attestation_id"]

    common_kwargs = dict(
        experiment_id="exp", hypothesis_id="hyp", strategy_id="strat",
        features_path=paths["features_path"], targets_path=paths["targets_path"],
        schema_path=paths["schema_path"], bars_path=paths["bars_path"],
        label_col="target", end_ts_col="end_ts", wf_spec=WalkForwardSpec(),
        l2=1e-3, lr=0.05, steps=10, standardize=True, clip_z=8.0,
        economic_spec=_minimal_economic_spec(),
    )
    trial_id_a, _ = build_economic_trial_identity(bars_provenance=manifest_a, **common_kwargs)
    trial_id_b, _ = build_economic_trial_identity(bars_provenance=manifest_b, **common_kwargs)
    assert trial_id_a != trial_id_b


def test_no_secrets_in_source_attestation():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(bars, evidence)
    import json

    blob = json.dumps(attestation)
    for forbidden in ("APCA-API-KEY-ID", "APCA-API-SECRET-KEY", "api_key", "api_secret"):
        assert forbidden not in blob


# ---------------------------------------------------------------------------
# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01 -- ca_resolution_policy_id binds
# the CA resolution policy into source_attestation_id for V2+ extractor
# identities, while a V1 attestation's identity computation is untouched.
# ---------------------------------------------------------------------------

V2_EXTRACTOR_ID = "mqk_research.data.alpaca_historical.v2"
DIAGNOSTIC_V2_EXTRACTOR_ID = "mqk_research.data.alpaca_historical.diagnostic_v2"


def test_identical_bars_different_ca_resolution_policy_yields_different_identity():
    """Required test 1: identical bars + evidence, only ca_resolution_
    policy_id differs (simulating two different CA-resolution policy
    generations) -> different source_attestation_id for a V2 extractor."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation_a = _valid_attestation(
        bars, evidence, extractor_id=V2_EXTRACTOR_ID, ca_resolution_policy_id="policy-fingerprint-a"
    )
    attestation_b = _valid_attestation(
        bars, evidence, extractor_id=V2_EXTRACTOR_ID, ca_resolution_policy_id="policy-fingerprint-b"
    )
    assert attestation_a["canonical_semantic_bars_hash"] == attestation_b["canonical_semantic_bars_hash"]
    assert attestation_a["canonical_corporate_action_evidence_hash"] == attestation_b["canonical_corporate_action_evidence_hash"]
    assert attestation_a["attestation_id"] != attestation_b["attestation_id"]


def test_ca_resolution_policy_id_ignored_for_v1_extractor_identity():
    """A V1 attestation's identity must be unaffected by ca_resolution_
    policy_id -- V1 never carried this field, so two V1 attestations that
    differ ONLY in this field must still be the SAME identity (proves
    canonical_source_attestation_content's V1/V2 gate, and that adding this
    field could not silently reinterpret a historical V1 artifact's hash)."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation_a = _valid_attestation(bars, evidence, ca_resolution_policy_id="ignored-a")
    attestation_b = _valid_attestation(bars, evidence, ca_resolution_policy_id="ignored-b")
    assert attestation_a["extractor_id"] == TRUSTED_EXTRACTOR_ID
    assert attestation_a["attestation_id"] == attestation_b["attestation_id"]


def test_old_v1_manifest_remains_verifiable_under_its_historical_contract():
    """Required test 3: a manifest built exactly as this module's PRE-V2
    tests always built it (V1 extractor_id, no ca_resolution_policy_id
    override) still passes the full registered gate, unaffected by V2-01."""
    bars = _bars_df([100.0, 101.0, 102.0])
    manifest = _valid_manifest(bars)
    assert manifest["source_attestation"]["extractor_id"] == TRUSTED_EXTRACTOR_ID
    require_registered_bars_provenance(manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_v2_extraction_manifest_verifies_with_ca_resolution_policy_id():
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, extractor_id=V2_EXTRACTOR_ID, ca_resolution_policy_id="real-policy-fingerprint"
    )
    manifest = _valid_manifest(bars, corporate_action_evidence=evidence, source_attestation=attestation)
    require_registered_bars_provenance(manifest)
    check_corporate_action_integrity(bars, manifest)  # must not raise


def test_hand_built_attestation_cannot_spoof_v2_authority_by_mutating_policy_id():
    """Required test 6: mutating a V2 attestation's ca_resolution_policy_id
    AFTER it was minted (leaving attestation_id/source_attestation_id
    stale) must be refused -- the recomputed identity no longer matches
    the declared one, exactly like any other post-mint tamper this module
    already defends against."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars, evidence, extractor_id=V2_EXTRACTOR_ID, ca_resolution_policy_id="real-policy-fingerprint"
    )
    manifest = _valid_manifest(bars, corporate_action_evidence=evidence, source_attestation=attestation)

    tampered_attestation = dict(attestation)
    tampered_attestation["ca_resolution_policy_id"] = "spoofed-policy-fingerprint"
    # attestation_id/source_attestation_id deliberately left stale.
    tampered_manifest = dict(manifest)
    tampered_manifest["source_attestation"] = tampered_attestation
    with pytest.raises(SourceAttestationUnverifiable, match="does not match"):
        check_corporate_action_integrity(bars, tampered_manifest)


def test_diagnostic_v2_authority_still_cannot_pass_registered_gates():
    """Required test 5, at the bars_provenance level: a diagnostic_v2-
    identified attestation is exactly as untrusted as diagnostic_v1ever was
    -- V2-01 only changed WHICH official identity is trusted, never
    weakened the official-vs-diagnostic authority boundary."""
    bars = _bars_df([100.0, 101.0])
    evidence = _clean_ca_evidence(bars)
    attestation = _valid_attestation(
        bars,
        evidence,
        extractor_id=DIAGNOSTIC_V2_EXTRACTOR_ID,
        source_authority="diagnostic_synthetic",
        ca_resolution_policy_id="real-policy-fingerprint",
    )
    manifest = _valid_manifest(bars, corporate_action_evidence=evidence, source_attestation=attestation)
    with pytest.raises(SourceAttestationUnverifiable):
        check_corporate_action_integrity(bars, manifest)
