from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, FrozenSet, List, Optional, Sequence

import pandas as pd

from mqk_research.ml.util_hash import sha256_file, sha256_json

# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01
#
# Durable, content-addressed provenance manifest for OFFICIAL research bars
# data, and the fail-closed corporate-action preflight that must run before
# any economic P&L is computed from those bars.
#
# Distinct from two OTHER, already-existing "provenance" concepts in this
# codebase that this module does not replace:
#   - bars_postgres.py's price_provenance: query-time metadata attached to a
#     DataFrame's `.attrs["price_provenance"]`. Useful in-memory, but NOT
#     durable -- `.attrs` does not survive a `DataFrame.to_csv()` /
#     `pd.read_csv()` round trip (see
#     test_dataframe_attrs_alone_is_not_durable_provenance), which is
#     exactly how bars data actually reaches the registered economic
#     evaluator (economic_registry_integration.
#     run_registered_economic_walkforward_eval takes a bars_csv PATH, not a
#     DataFrame). This module's manifest is the durable, content-addressed
#     record that survives that trip.
#   - economic_walkforward.verify_bars_provenance: an unrelated existing
#     check that bars_csv's file bytes match what shadow_label_meta.json
#     recorded when targets.csv's labels were built. That check answers
#     "is this the SAME bars file the labels came from?" -- it says nothing
#     about the price data's own adjustment convention or corporate-action
#     safety, which is what THIS module answers.
#
# Root Defect 2 (raw prices + corporate actions): the only verified price
# convention anywhere in this system today is raw_unadjusted (see
# bars_postgres.py's BKT-DATA-PROVENANCE-POINT-IN-TIME-01 header). Raw,
# unadjusted closes make a stock split look like a real ~50% loss and omit
# dividend cash flows entirely (core-rs/crates/mqk-backtest/src/
# corporate_actions.rs documents the same fact for the Rust engine). This
# module mirrors that Rust module's conservative two-policy design
# (Allow / ForbidPeriods) rather than building a real adjustment-tables
# subsystem: an explicit ADJUSTED_DATA claim (valid only when actually
# verified) or an explicit FORBID_AFFECTED_PERIODS exclusion (valid only
# with real, content-addressed exclusion evidence). No authoritative
# corporate-action exclusion source exists anywhere in this repository today
# (no DB table, no migration, no fixture) -- so for real RAW_UNADJUSTED
# registered data, this module is DESIGNED to fail closed until that source
# exists. See docs/research/Research_Backtest_V1_Closeout_Audit.md for the
# P8 PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED status this implies.
#
# BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02 (independent-review repair):
#   - Defect 1: a manifest's STRUCTURAL validity (require_registered_
#     bars_provenance) said nothing about whether it actually describes the
#     bars being evaluated -- a manifest built for bars A could be paired
#     with bars B. require_bars_match_manifest below is a new, separate
#     CONTENT-BINDING preflight (canonical hash / symbol universe /
#     timestamp range) that must run before check_corporate_action_integrity
#     and before any economic P&L.
#   - Defect 2: a bare corporate_action_evidence_id string (e.g.
#     "evidence-v1") was trusted merely for being non-empty, and
#     forbidden_periods embedded in the SAME manifest were trusted merely
#     for sitting next to it -- a caller could assert anything. CA_POLICY_
#     FORBID_AFFECTED_PERIODS now requires a real, content-addressed
#     corporate_action_evidence object (build_corporate_action_evidence /
#     corporate_action_evidence_id) whose declared coverage provably
#     includes the full bars universe/range and whose forbidden_periods are
#     independently re-derivable from it. No authoritative corporate-action
#     evidence SOURCE exists in this repository today -- this is the P8
#     CONTRACT (verifiable, testable via synthetic evidence fixtures), not
#     the external DATA SOURCE (a separate, future patch:
#     BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01). Real registered
#     raw_unadjusted research stays honestly fail-closed until that source
#     exists.

SCHEMA_VERSION = "bars_provenance_manifest_v1"

# --- price adjustment convention (mirrors bars_postgres.py's vocabulary) ---
PRICE_CONVENTION_RAW_UNADJUSTED = "raw_unadjusted"
PRICE_CONVENTION_UNVERIFIABLE = "unverifiable"

# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01: the first verified adjusted-data
# provider in this system -- Alpaca's /v2/stocks/bars with adjustment=all,
# confirmed against Alpaca's official documentation
# (docs.alpaca.markets/reference/stockbars, fetched 2026-08-15) to apply
# split + cash-dividend + spin-off price/volume adjustment. This is an
# auditable, explicit code addition (the named seam the module header
# above anticipated), NOT a silent broadening of trust -- see
# _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION below: satisfying
# corporate_action_policy=adjusted_data for THIS convention additionally
# requires a verified, content-addressed source_attestation object proving
# the data was actually produced by the trusted extractor
# (mqk_research.data.alpaca_historical) with adjustment=all -- a caller
# manually constructing price_adjustment_convention="alpaca_all_adjusted_v1"
# on a hand-built manifest is NOT sufficient (see
# _require_verified_source_attestation / SourceAttestationUnverifiable).
PRICE_CONVENTION_ALPACA_ALL_ADJUSTED = "alpaca_all_adjusted_v1"

# No price-adjustment convention other than raw_unadjusted and (as of
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01) alpaca_all_adjusted_v1 has ever
# been independently verified in this system. This set exists as a named
# seam so a FUTURE verified adjusted-data provider can be added here
# explicitly (an auditable code change) rather than the adjusted_data
# policy being silently accepted for an unverified source.
_KNOWN_ADJUSTED_CONVENTIONS: FrozenSet[str] = frozenset({PRICE_CONVENTION_ALPACA_ALL_ADJUSTED})

# Adjusted conventions whose adjusted_data policy claim additionally requires
# a verified source_attestation (see _require_verified_source_attestation),
# mapped to the adjustment mode the attestation must declare. Conventions NOT
# in this dict (e.g. a test-only convention a test monkeypatches into
# _KNOWN_ADJUSTED_CONVENTIONS) are accepted on the structural convention-name
# check alone, unchanged from the pre-existing contract.
_CONVENTION_REQUIRED_ADJUSTMENT_MODE: Dict[str, str] = {
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED: "all",
}
_CONVENTIONS_REQUIRING_SOURCE_ATTESTATION: FrozenSet[str] = frozenset(_CONVENTION_REQUIRED_ADJUSTMENT_MODE.keys())

# The narrow allowlist of extractor identities trusted to produce a
# source_attestation that can satisfy _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION.
# Bumping the extractor's contract/logic should mint a new id here (an
# explicit, auditable change), not silently keep trusting the old one.
_TRUSTED_EXTRACTOR_IDS: FrozenSet[str] = frozenset({"mqk_research.data.alpaca_historical.v1"})

# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 (Defect 3): a caller-typed
# extractor_id string alone was found to be an insufficiently strong
# authority boundary -- see mqk_research.data.alpaca_historical's OFFICIAL
# vs DIAGNOSTIC extraction split (extract_research_bars_with_provenance vs
# extract_research_bars_with_provenance_diagnostic). Every source_attestation
# now also declares an explicit, typed source_authority; only the official
# authority can satisfy _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION.
SOURCE_AUTHORITY_OFFICIAL_PROVIDER = "official_provider"
SOURCE_AUTHORITY_DIAGNOSTIC_SYNTHETIC = "diagnostic_synthetic"

# Per-convention expected trusted profile (Defect 3): the source_provider_id
# and exact official API endpoints a source_attestation must declare to
# satisfy this convention -- a hand-typed or diagnostic-transport attestation
# claiming a fake provider id or endpoint must fail the same way a fake
# extractor_id already does. Conventions not in these maps are accepted on
# the pre-existing (adjustment-mode-only) checks alone.
_CONVENTION_REQUIRED_SOURCE_PROVIDER: Dict[str, str] = {
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED: "alpaca",
}
_CONVENTION_REQUIRED_ENDPOINTS: Dict[str, "tuple[str, str]"] = {
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED: (
        "https://data.alpaca.markets/v2/stocks/bars",
        "https://data.alpaca.markets/v1/corporate-actions",
    ),
}

SOURCE_ATTESTATION_SCHEMA_VERSION = "research_source_attestation_v1"

# --- corporate-action policy (mirrors mqk-backtest::CorporateActionPolicy) ---
CA_POLICY_ADJUSTED_DATA = "adjusted_data"
CA_POLICY_FORBID_AFFECTED_PERIODS = "forbid_affected_periods"
CA_POLICY_DIAGNOSTIC_SYNTHETIC_UNPROTECTED = "diagnostic_synthetic_unprotected"

# The only two policies that constitute an honest corporate-action safety
# guarantee for OFFICIAL registered research. The diagnostic policy exists
# solely for synthetic/unit-test fixtures reached through the low-level,
# non-registered economic_walkforward.run_economic_walkforward path -- see
# that function's provenance_manifest=None default, which is the actual
# diagnostic escape hatch; this constant is never treated as satisfying the
# registered contract.
_REAL_CA_POLICIES: FrozenSet[str] = frozenset({CA_POLICY_ADJUSTED_DATA, CA_POLICY_FORBID_AFFECTED_PERIODS})

# --- corporate-action EVIDENCE contract (Defect 2) ---
# The narrow interface a future BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01 data
# patch must satisfy for CA_POLICY_FORBID_AFFECTED_PERIODS to be usable by
# OFFICIAL registered raw_unadjusted research. See build_corporate_action_
# evidence / corporate_action_evidence_id / _require_verified_ca_evidence.
CA_EVIDENCE_SCHEMA_VERSION = "corporate_action_evidence_v1"

# --- universe mode ---
# Confirmed by direct code reading (Research_Backtest_V1_Closeout_Audit.md
# P8 findings): this system's universe construction is unambiguously
# "fixed, explicit ex-ante" (the operator supplies --symbols; no code path
# anywhere queries historical index/constituent membership). Point-in-time
# dynamic universe semantics are NOT implemented -- claiming that mode here
# would be false, so it is named but deliberately unsupported.
UNIVERSE_MODE_FIXED_EX_ANTE = "fixed_ex_ante"
UNIVERSE_MODE_POINT_IN_TIME = "point_in_time"
_SUPPORTED_UNIVERSE_MODES: FrozenSet[str] = frozenset({UNIVERSE_MODE_FIXED_EX_ANTE})

_CANONICAL_HASH_COLUMNS = ("symbol", "end_ts", "close")


class BarsProvenanceUnverifiable(RuntimeError):
    """Fail-closed: raised when a bars provenance manifest does not satisfy
    the durable, structural contract required for OFFICIAL registered
    economic evaluation (see require_registered_bars_provenance)."""


class CorporateActionIntegrityError(RuntimeError):
    """Fail-closed: raised BEFORE any economic P&L is computed when the
    manifest's declared corporate-action policy cannot be honestly
    satisfied for the bars actually being evaluated (see
    check_corporate_action_integrity)."""


class SourceAttestationUnverifiable(RuntimeError):
    """Fail-closed (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01): raised when a
    manifest's corporate_action_policy=adjusted_data claim, for a price
    convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION, cannot be
    verified against a real, content-addressed source_attestation object
    proving the data was produced by a trusted research extractor. A
    manually-asserted convention string alone is never sufficient -- see
    _require_verified_source_attestation."""


class BarsProvenanceContentMismatch(RuntimeError):
    """Fail-closed (Defect 1): raised BEFORE any economic P&L is computed
    when the bars actually loaded for evaluation do not match the content a
    supplied provenance manifest claims to describe (canonical semantic
    hash, declared symbol universe, or declared extraction range) -- catches
    a stale or wrong manifest being paired with different bars data. See
    require_bars_match_manifest."""


def canonical_semantic_bars_hash(bars: pd.DataFrame) -> str:
    """Content hash of the bars' SEMANTIC content (symbol, end_ts, close),
    normalized by sorting rows chronologically per-symbol and selecting
    columns in a FIXED order -- invariant to the physical file's row order
    or column order, sensitive to any actual change in symbol/timestamp/
    price content. Distinct from a physical artifact's raw file-bytes hash
    (sha256_file), which two byte-identical-content-but-differently-ordered
    CSVs will NOT share even though this function's output is identical for
    both (see build_bars_provenance_manifest's artifact_sha256 field)."""
    missing = [c for c in _CANONICAL_HASH_COLUMNS if c not in bars.columns]
    if missing:
        raise ValueError(f"canonical_semantic_bars_hash: bars missing required columns: {missing}")
    normalized = bars[list(_CANONICAL_HASH_COLUMNS)].copy()
    normalized["symbol"] = normalized["symbol"].astype(str)
    normalized["end_ts"] = pd.to_datetime(normalized["end_ts"], utc=True).map(lambda t: t.isoformat())
    normalized["close"] = normalized["close"].astype(float)
    normalized = normalized.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    dup_mask = normalized.duplicated(subset=["symbol", "end_ts"], keep=False)
    if bool(dup_mask.any()):
        # Fail closed rather than silently sort/hash an ambiguous semantic
        # observation -- agrees with economic_walkforward.load_bars's own
        # duplicate (symbol,end_ts) contract.
        dup_rows = normalized.loc[dup_mask, ["symbol", "end_ts"]].drop_duplicates().to_dict("records")
        raise ValueError(
            f"Fail-closed: canonical_semantic_bars_hash: duplicate (symbol,end_ts) rows: {dup_rows}"
        )
    rows = [
        {"symbol": r.symbol, "end_ts": r.end_ts, "close": r.close} for r in normalized.itertuples(index=False)
    ]
    return sha256_json(rows)


def canonical_source_attestation_content(attestation: Dict[str, Any]) -> Dict[str, Any]:
    """The canonical, hashable subset of a source attestation object
    (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01) -- normalized field selection so
    two logically-identical attestations always hash identically, mirroring
    canonical_ca_evidence_content's normalization discipline. Deliberately
    excludes `retrieval_timestamp_utc` (audit-only wall-clock metadata --
    the SAME extraction re-run at a different moment must still produce the
    same attestation identity) and any self-reported `attestation_id`
    (never trusted -- see source_attestation_id).

    BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 (Defect 4): also
    deliberately excludes `raw_response_content_hashes` -- per-page
    provider-response hashes are a TRANSPORT/pagination-boundary fact, not a
    semantic research fact. Two extractions with byte-identical semantic
    bars/corporate-action content that merely paginated differently (e.g. a
    different `limit`, or the provider splitting pages differently across
    reruns) must share one semantic source_attestation_id; the raw hashes
    remain on the attestation object itself as audit-only evidence (see
    build_source_attestation), just outside canonical identity -- the same
    treatment `retrieval_timestamp_utc` and `attestation_id` already get."""
    return {
        "schema_version": attestation.get("schema_version"),
        "source_provider_id": attestation.get("source_provider_id"),
        "extractor_id": attestation.get("extractor_id"),
        "source_authority": attestation.get("source_authority"),
        "api_endpoint_bars": attestation.get("api_endpoint_bars"),
        "api_endpoint_corporate_actions": attestation.get("api_endpoint_corporate_actions"),
        "symbols": sorted({str(s).strip().upper() for s in attestation.get("symbols") or []}),
        "requested_start_utc": attestation.get("requested_start_utc"),
        "requested_end_utc": attestation.get("requested_end_utc"),
        "returned_coverage_start_utc": attestation.get("returned_coverage_start_utc"),
        "returned_coverage_end_utc": attestation.get("returned_coverage_end_utc"),
        "adjustment_mode": attestation.get("adjustment_mode"),
        "feed": attestation.get("feed"),
        "asof": attestation.get("asof"),
        "pagination_complete_bars": attestation.get("pagination_complete_bars"),
        "pagination_complete_corporate_actions": attestation.get("pagination_complete_corporate_actions"),
        "corporate_action_query_coverage": attestation.get("corporate_action_query_coverage"),
        "category_b_events_found": attestation.get("category_b_events_found"),
        "canonical_semantic_bars_hash": attestation.get("canonical_semantic_bars_hash"),
        "canonical_corporate_action_evidence_hash": attestation.get("canonical_corporate_action_evidence_hash"),
        "protocol_version": attestation.get("protocol_version"),
    }


def source_attestation_id(attestation: Dict[str, Any]) -> str:
    """Deterministic, content-DERIVED source attestation id -- the only
    legitimate value for a manifest's source_attestation_id. A caller can
    never simply assert an arbitrary string here:
    _require_verified_source_attestation independently recomputes this from
    the supplied attestation object and requires exact equality."""
    return sha256_json(canonical_source_attestation_content(attestation))


def build_source_attestation(
    *,
    source_provider_id: str,
    extractor_id: str,
    source_authority: str,
    api_endpoint_bars: str,
    api_endpoint_corporate_actions: str,
    symbols: Sequence[str],
    requested_start_utc: str,
    requested_end_utc: str,
    returned_coverage_start_utc: Optional[str],
    returned_coverage_end_utc: Optional[str],
    adjustment_mode: str,
    feed: Optional[str],
    asof: Optional[str],
    pagination_complete_bars: bool,
    pagination_complete_corporate_actions: bool,
    corporate_action_query_coverage: Dict[str, Any],
    category_b_events_found: Sequence[Dict[str, Any]],
    raw_response_content_hashes: Dict[str, Any],
    canonical_semantic_bars_hash: str,
    canonical_corporate_action_evidence_hash: str,
    retrieval_timestamp_utc: str,
    protocol_version: str = "1",
) -> Dict[str, Any]:
    """Pure assembly of one durable, content-addressed source attestation
    (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01) -- the evidence a trusted
    research extractor (e.g. mqk_research.data.alpaca_historical) attaches
    to prove a bars provenance manifest's adjusted-data claim was actually
    produced by that extractor with the declared adjustment mode, not
    merely asserted. `category_b_events_found` must be an empty sequence for
    an attestation to ever satisfy _require_verified_source_attestation --
    a non-empty value records (for audit purposes only) that the extractor
    itself refused to produce clean adjusted data; such an attestation
    should never accompany a manifest claiming a usable adjusted_data
    policy in the first place (see
    mqk_research.data.alpaca_historical.CorporateActionReviewRequired)."""
    attestation: Dict[str, Any] = {
        "schema_version": SOURCE_ATTESTATION_SCHEMA_VERSION,
        "source_provider_id": source_provider_id,
        "extractor_id": extractor_id,
        "source_authority": source_authority,
        "api_endpoint_bars": api_endpoint_bars,
        "api_endpoint_corporate_actions": api_endpoint_corporate_actions,
        "symbols": sorted({str(s).strip().upper() for s in symbols}),
        "requested_start_utc": requested_start_utc,
        "requested_end_utc": requested_end_utc,
        "returned_coverage_start_utc": returned_coverage_start_utc,
        "returned_coverage_end_utc": returned_coverage_end_utc,
        "adjustment_mode": adjustment_mode,
        "feed": feed,
        "asof": asof,
        "pagination_complete_bars": bool(pagination_complete_bars),
        "pagination_complete_corporate_actions": bool(pagination_complete_corporate_actions),
        "corporate_action_query_coverage": corporate_action_query_coverage,
        "category_b_events_found": list(category_b_events_found),
        "raw_response_content_hashes": raw_response_content_hashes,
        "canonical_semantic_bars_hash": canonical_semantic_bars_hash,
        "canonical_corporate_action_evidence_hash": canonical_corporate_action_evidence_hash,
        "protocol_version": protocol_version,
        "retrieval_timestamp_utc": retrieval_timestamp_utc,
    }
    attestation["attestation_id"] = source_attestation_id(attestation)
    return attestation


def _require_verified_source_attestation(bars: pd.DataFrame, manifest: Dict[str, Any]) -> None:
    """Defect closure (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01): for a price
    convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION,
    corporate_action_policy=adjusted_data is satisfied ONLY when a real
    source_attestation object is supplied, its canonical content hash
    recomputes to the manifest's declared source_attestation_id, it comes
    from a trusted extractor identity, its declared adjustment_mode matches
    what the convention requires, both retrievals report complete
    pagination, it records zero unresolved REQUIRES_FAIL_CLOSED_REVIEW
    corporate actions, and its coverage (symbols + returned date range)
    includes everything actually present in `bars`. A bare convention
    string with no verifiable attestation -- the pre-patch status quo -- is
    never sufficient."""
    convention = manifest.get("price_adjustment_convention")
    declared_id = manifest.get("source_attestation_id")
    attestation = manifest.get("source_attestation")
    if not declared_id or not isinstance(attestation, dict):
        raise SourceAttestationUnverifiable(
            f"Fail-closed: corporate_action_policy={CA_POLICY_ADJUSTED_DATA!r} with "
            f"price_adjustment_convention={convention!r} requires a real, content-addressed "
            "source_attestation object (see build_source_attestation) proving the data was "
            "produced by a trusted research extractor -- a manually-asserted convention string "
            "alone does not satisfy this contract"
        )
    if attestation.get("schema_version") != SOURCE_ATTESTATION_SCHEMA_VERSION:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation.schema_version={attestation.get('schema_version')!r} "
            f"!= {SOURCE_ATTESTATION_SCHEMA_VERSION!r}"
        )
    recomputed_id = source_attestation_id(attestation)
    if recomputed_id != declared_id:
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation_id does not match the recomputed canonical content "
            f"hash of the supplied attestation object (declared={declared_id!r}, "
            f"recomputed={recomputed_id!r}) -- refusing to trust a caller-selected attestation id"
        )
    if attestation.get("extractor_id") not in _TRUSTED_EXTRACTOR_IDS:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation.extractor_id={attestation.get('extractor_id')!r} is "
            f"not a trusted research extractor ({sorted(_TRUSTED_EXTRACTOR_IDS)!r})"
        )
    if attestation.get("source_authority") != SOURCE_AUTHORITY_OFFICIAL_PROVIDER:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation.source_authority={attestation.get('source_authority')!r} "
            f"is not {SOURCE_AUTHORITY_OFFICIAL_PROVIDER!r} -- a diagnostic/synthetic-authority "
            "attestation (see mqk_research.data.alpaca_historical.extract_research_bars_with_"
            "provenance_diagnostic) can never authorize official registered research, regardless of "
            "whether it is otherwise internally consistent"
        )
    expected_provider = _CONVENTION_REQUIRED_SOURCE_PROVIDER.get(convention)
    if expected_provider is not None and attestation.get("source_provider_id") != expected_provider:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation.source_provider_id={attestation.get('source_provider_id')!r} "
            f"does not match the provider required for price_adjustment_convention={convention!r} "
            f"({expected_provider!r})"
        )
    expected_endpoints = _CONVENTION_REQUIRED_ENDPOINTS.get(convention)
    if expected_endpoints is not None:
        expected_bars_endpoint, expected_ca_endpoint = expected_endpoints
        if attestation.get("api_endpoint_bars") != expected_bars_endpoint:
            raise SourceAttestationUnverifiable(
                f"Fail-closed: source_attestation.api_endpoint_bars={attestation.get('api_endpoint_bars')!r} "
                f"does not match the official endpoint required for price_adjustment_convention="
                f"{convention!r} ({expected_bars_endpoint!r})"
            )
        if attestation.get("api_endpoint_corporate_actions") != expected_ca_endpoint:
            raise SourceAttestationUnverifiable(
                "Fail-closed: source_attestation.api_endpoint_corporate_actions="
                f"{attestation.get('api_endpoint_corporate_actions')!r} does not match the official "
                f"endpoint required for price_adjustment_convention={convention!r} "
                f"({expected_ca_endpoint!r})"
            )
    if not attestation.get("asof"):
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation is missing an explicit asof -- an official extraction "
            "must never rely on the provider's implicit current-day default (see "
            "mqk_research.data.alpaca_historical.fetch_historical_bars's required asof parameter)"
        )
    expected_mode = _CONVENTION_REQUIRED_ADJUSTMENT_MODE.get(convention)
    if attestation.get("adjustment_mode") != expected_mode:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation.adjustment_mode={attestation.get('adjustment_mode')!r} "
            f"does not match the adjustment mode required for price_adjustment_convention="
            f"{convention!r} ({expected_mode!r})"
        )
    if not attestation.get("pagination_complete_bars") or not attestation.get(
        "pagination_complete_corporate_actions"
    ):
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation declares incomplete pagination for bars and/or "
            "corporate-action retrieval"
        )
    if attestation.get("category_b_events_found"):
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation records unresolved REQUIRES_FAIL_CLOSED_REVIEW "
            f"corporate action(s) -- refusing to trust adjusted data over them: "
            f"{attestation.get('category_b_events_found')}"
        )
    if attestation.get("canonical_semantic_bars_hash") != manifest.get("canonical_semantic_bars_hash"):
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation.canonical_semantic_bars_hash does not match the "
            "manifest's own canonical_semantic_bars_hash"
        )

    manifest_ca_evidence = manifest.get("corporate_action_evidence")
    if not isinstance(manifest_ca_evidence, dict):
        raise SourceAttestationUnverifiable(
            "Fail-closed: manifest is missing a corporate_action_evidence object required alongside "
            "an attested adjusted_data claim"
        )
    recomputed_ca_hash = corporate_action_evidence_id(manifest_ca_evidence)
    if recomputed_ca_hash != manifest.get("corporate_action_evidence_id"):
        raise SourceAttestationUnverifiable(
            "Fail-closed: manifest corporate_action_evidence does not match manifest "
            f"corporate_action_evidence_id (declared={manifest.get('corporate_action_evidence_id')!r}, "
            f"recomputed={recomputed_ca_hash!r}) -- refusing to trust tampered corporate-action evidence"
        )
    if attestation.get("canonical_corporate_action_evidence_hash") != recomputed_ca_hash:
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation.canonical_corporate_action_evidence_hash does not match "
            "the manifest's own (recomputed) corporate-action evidence hash"
        )

    if bars.empty:
        return

    actual_symbols = {str(s).strip().upper() for s in bars["symbol"].unique()}
    attested_symbols = set(attestation.get("symbols") or [])
    missing_symbols = actual_symbols - attested_symbols
    if missing_symbols:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation symbol coverage does not include bars symbol(s) "
            f"{sorted(missing_symbols)!r}"
        )

    cov_start = attestation.get("returned_coverage_start_utc")
    cov_end = attestation.get("returned_coverage_end_utc")
    if not cov_start or not cov_end:
        raise SourceAttestationUnverifiable(
            "Fail-closed: source_attestation missing returned_coverage_start_utc/"
            "returned_coverage_end_utc"
        )
    actual_ts = pd.to_datetime(bars["end_ts"], utc=True)
    cov_start_ts = pd.Timestamp(cov_start)
    cov_end_ts = pd.Timestamp(cov_end)
    if actual_ts.min() < cov_start_ts or actual_ts.max() > cov_end_ts:
        raise SourceAttestationUnverifiable(
            f"Fail-closed: source_attestation returned-coverage range [{cov_start_ts.isoformat()}, "
            f"{cov_end_ts.isoformat()}] does not include the full observed bars date range "
            f"[{actual_ts.min().isoformat()}, {actual_ts.max().isoformat()}]"
        )


def build_bars_provenance_manifest(
    *,
    price_provenance: Dict[str, Any],
    corporate_action_policy: str,
    corporate_action_evidence_id: Optional[str] = None,
    corporate_action_evidence: Optional[Dict[str, Any]] = None,
    forbidden_periods: Sequence[Dict[str, str]] = (),
    timeframe: str,
    start_utc: str,
    end_utc: str,
    symbol_universe: Sequence[str],
    universe_mode: str,
    bars: pd.DataFrame,
    artifact_path: Optional[Path] = None,
    source_attestation: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Pure assembly of one durable, versioned bars provenance manifest. No
    IO except (optionally) hashing `artifact_path`'s bytes if supplied.

    `price_provenance` is the dict produced by
    mqk_research.data.adapters.bars_postgres.resolve_price_provenance (or
    the equivalent fields for a non-Postgres source) -- this function does
    not re-derive it, it durably records what was already resolved.
    `forbidden_periods` are plain {"symbol","start_ts","end_ts"} dicts
    (ISO-8601 UTC, inclusive), matching core-rs's ForbidEntry shape.
    `corporate_action_evidence` (Defect 2 / P8 REPAIR-02) is the full,
    content-addressed evidence object (see build_corporate_action_evidence)
    that `corporate_action_evidence_id` must be independently re-derivable
    from -- required for CA_POLICY_FORBID_AFFECTED_PERIODS to satisfy
    check_corporate_action_integrity for OFFICIAL registered research. Not
    part of provenance_identity_fragment (its content-derived ID already is)
    -- an audit-only field, like artifact_sha256.
    `source_attestation` (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01) is the full,
    content-addressed source-attestation object (see build_source_attestation)
    that a manifest claiming corporate_action_policy=adjusted_data with a
    convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION must supply --
    only its derived id enters provenance_identity_fragment/trial identity;
    the full object here is audit-only, like corporate_action_evidence.
    """
    symbols_sorted = sorted({str(s).strip().upper() for s in symbol_universe if str(s).strip()})
    forbidden_sorted = sorted(
        (dict(e) for e in forbidden_periods), key=lambda e: (e["symbol"], e["start_ts"], e["end_ts"])
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "provider_ids_observed": sorted(price_provenance.get("provider_ids_observed") or []),
        "resolved_close_column": price_provenance.get("close_column"),
        "price_adjustment_convention": price_provenance.get("price_adjustment_convention"),
        "corporate_action_policy": corporate_action_policy,
        "corporate_action_evidence_id": corporate_action_evidence_id,
        "corporate_action_evidence": corporate_action_evidence,
        "forbidden_periods": forbidden_sorted,
        "source_query_identity": {
            "provider_metadata_available": price_provenance.get("provider_metadata_available"),
            "convention_basis": price_provenance.get("convention_basis"),
        },
        "timeframe": timeframe,
        "start_utc": start_utc,
        "end_utc": end_utc,
        "symbol_universe": symbols_sorted,
        "symbol_universe_id": sha256_json(symbols_sorted),
        "universe_mode": universe_mode,
        "canonical_semantic_bars_hash": canonical_semantic_bars_hash(bars),
        "row_count": int(len(bars)),
        "artifact_sha256": sha256_file(Path(artifact_path)) if artifact_path is not None else None,
        "source_attestation": source_attestation,
        "source_attestation_id": source_attestation_id(source_attestation) if source_attestation is not None else None,
    }


def provenance_identity_fragment(manifest: Dict[str, Any]) -> Dict[str, Any]:
    """The SUBSET of a bars provenance manifest that belongs in trial
    identity -- must change trial_id whenever it changes, and must NOT
    change trial_id when it doesn't. Deliberately excludes artifact_sha256
    and row_count: two semantically-identical bars files that differ only
    in physical row order share the same canonical_semantic_bars_hash (and
    therefore the same identity) even though their raw file bytes, and
    hence artifact_sha256, differ (see canonical_semantic_bars_hash). Also
    excludes source_query_identity's informational-only sub-fields
    (provider_metadata_available / convention_basis are audit context, not
    an independent economic fact once provider_ids_observed and
    price_adjustment_convention are already included). Includes
    `source_attestation_id` (BKT-RESEARCH-MARKET-DATA-AUTHORITY-01) -- its
    derived id only, not the full attestation object -- so which trusted
    extraction produced an adjusted-data candidate is part of its identity,
    the same way corporate_action_evidence_id already is."""
    return {
        "schema_version": manifest["schema_version"],
        "provider_ids_observed": manifest["provider_ids_observed"],
        "resolved_close_column": manifest["resolved_close_column"],
        "price_adjustment_convention": manifest["price_adjustment_convention"],
        "corporate_action_policy": manifest["corporate_action_policy"],
        "corporate_action_evidence_id": manifest["corporate_action_evidence_id"],
        "forbidden_periods": manifest["forbidden_periods"],
        "timeframe": manifest["timeframe"],
        "start_utc": manifest["start_utc"],
        "end_utc": manifest["end_utc"],
        "symbol_universe": manifest["symbol_universe"],
        "universe_mode": manifest["universe_mode"],
        "canonical_semantic_bars_hash": manifest["canonical_semantic_bars_hash"],
        "source_attestation_id": manifest.get("source_attestation_id"),
    }


def require_registered_bars_provenance(manifest: Dict[str, Any]) -> None:
    """Fail-closed STRUCTURAL gate for the OFFICIAL registered economic
    evaluation entry point (economic_registry_integration.
    run_registered_economic_walkforward_eval). Raises
    BarsProvenanceUnverifiable unless the manifest is well-formed and
    declares: a known/verified price-adjustment convention, a real
    (non-diagnostic) corporate-action policy, and a supported universe
    mode. This is a manifest-SHAPE check; check_corporate_action_integrity
    (below) does the bars-CONTENT check separately -- both must pass before
    any economic P&L is computed."""
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: bars provenance manifest schema_version={manifest.get('schema_version')!r} "
            f"!= {SCHEMA_VERSION!r}"
        )

    convention = manifest.get("price_adjustment_convention")
    known_conventions = {PRICE_CONVENTION_RAW_UNADJUSTED} | _KNOWN_ADJUSTED_CONVENTIONS
    if convention not in known_conventions:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: price_adjustment_convention={convention!r} is not a verified/known "
            "convention -- refusing official registered economic evaluation on bars data whose "
            "price-adjustment convention cannot be confirmed (e.g. a provider-attribution gap "
            "reporting 'unverifiable')"
        )

    if convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION and not manifest.get("source_attestation_id"):
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: price_adjustment_convention={convention!r} requires a source_attestation_id "
            "on the manifest (see build_source_attestation) -- a caller-asserted convention string "
            "alone is not sufficient to authorize official registered economic evaluation"
        )

    policy = manifest.get("corporate_action_policy")
    if policy not in _REAL_CA_POLICIES:
        raise BarsProvenanceUnverifiable(
            "Fail-closed: official registered economic evaluation requires corporate_action_policy "
            f"in {sorted(_REAL_CA_POLICIES)!r}, got {policy!r}"
        )

    universe_mode = manifest.get("universe_mode")
    if universe_mode not in _SUPPORTED_UNIVERSE_MODES:
        raise BarsProvenanceUnverifiable(
            f"Fail-closed: universe_mode={universe_mode!r} is not a supported universe mode "
            f"({sorted(_SUPPORTED_UNIVERSE_MODES)!r}) -- point-in-time/dynamic universe membership "
            "is not implemented anywhere in this system; claiming it here would be false"
        )

    if not manifest.get("canonical_semantic_bars_hash"):
        raise BarsProvenanceUnverifiable("Fail-closed: bars provenance manifest missing canonical_semantic_bars_hash")
    if not manifest.get("symbol_universe"):
        raise BarsProvenanceUnverifiable("Fail-closed: bars provenance manifest missing symbol_universe")


def require_bars_match_manifest(bars: pd.DataFrame, manifest: Dict[str, Any]) -> None:
    """Fail-closed CONTENT-BINDING preflight (Defect 1 / P8 REPAIR-02). MUST
    run BEFORE check_corporate_action_integrity and BEFORE any economic
    fold/P&L: check_corporate_action_integrity trusts the manifest's own
    claims (policy, evidence, forbidden_periods) about `bars` -- those claims
    are only meaningful if the manifest actually DESCRIBES these bars. This
    function proves that binding by recomputing the canonical semantic hash
    from the bars actually loaded and requiring exact equality with the
    manifest's declared hash, then cross-checking the actually-observed
    symbol universe and timestamp range against the manifest's declared
    extraction contract. Never regenerates or repairs the manifest -- the
    whole point is to catch a stale/wrong manifest, so any mismatch fails
    closed with BarsProvenanceContentMismatch."""
    actual_hash = canonical_semantic_bars_hash(bars)
    declared_hash = manifest.get("canonical_semantic_bars_hash")
    if actual_hash != declared_hash:
        raise BarsProvenanceContentMismatch(
            f"Fail-closed: actual bars canonical_semantic_bars_hash={actual_hash!r} does not "
            f"match manifest-declared canonical_semantic_bars_hash={declared_hash!r} -- refusing "
            "to evaluate bars data under a manifest that does not describe it"
        )

    actual_symbols = sorted({str(s).strip().upper() for s in bars["symbol"].unique()})
    declared_symbols = list(manifest.get("symbol_universe") or [])
    if actual_symbols != declared_symbols:
        raise BarsProvenanceContentMismatch(
            f"Fail-closed: actual bars symbol universe {actual_symbols!r} does not match "
            f"manifest-declared symbol_universe {declared_symbols!r}"
        )

    if bars.empty:
        return

    start_utc = manifest.get("start_utc")
    end_utc = manifest.get("end_utc")
    if not start_utc or not end_utc:
        raise BarsProvenanceContentMismatch(
            "Fail-closed: bars provenance manifest missing start_utc/end_utc extraction range"
        )
    start_bound = pd.Timestamp(start_utc)
    end_bound = pd.Timestamp(end_utc)
    actual_ts = pd.to_datetime(bars["end_ts"], utc=True)
    actual_min = actual_ts.min()
    actual_max = actual_ts.max()
    if actual_min < start_bound or actual_max >= end_bound:
        raise BarsProvenanceContentMismatch(
            f"Fail-closed: actual bars timestamp range [{actual_min.isoformat()}, "
            f"{actual_max.isoformat()}] falls outside the manifest-declared extraction range "
            f"[{start_bound.isoformat()}, {end_bound.isoformat()})"
        )

    # Timeframe/extraction semantics, as far as they can be independently
    # established from the bars content alone: a manifest declaring whole-
    # day granularity ("1D") is falsifiable by observing any sub-day
    # timestamp in the actual data. Unrecognized timeframe strings are not
    # rejected here -- this system does not carry an independent granularity
    # authority beyond what the bars themselves can prove.
    if manifest.get("timeframe") == "1D":
        sub_day = (
            (actual_ts.dt.hour != 0) | (actual_ts.dt.minute != 0)
            | (actual_ts.dt.second != 0) | (actual_ts.dt.microsecond != 0)
        )
        if bool(sub_day.any()):
            raise BarsProvenanceContentMismatch(
                "Fail-closed: manifest declares timeframe='1D' but actual bars contain sub-day "
                "timestamps -- declared timeframe does not match observed bar granularity"
            )


def canonical_ca_evidence_content(evidence: Dict[str, Any]) -> Dict[str, Any]:
    """The canonical, hashable subset of a corporate-action evidence object
    (Defect 2) -- normalized field selection/ordering so two logically-
    identical evidence objects always hash identically, mirroring
    canonical_semantic_bars_hash's normalization discipline. Deliberately
    excludes `artifact_sha256` (a physical file fact, not part of the
    evidence's own logical content) and any self-reported `evidence_id`
    (never trusted -- see corporate_action_evidence_id)."""
    entries_sorted = sorted(
        (dict(e) for e in evidence.get("corporate_action_entries") or []),
        key=lambda e: (
            str(e.get("symbol")), str(e.get("action_type")),
            str(e.get("effective_start_ts")), str(e.get("effective_end_ts")),
        ),
    )
    return {
        "schema_version": evidence.get("schema_version"),
        "source_provider_id": evidence.get("source_provider_id"),
        "covered_symbol_universe": sorted(
            {str(s).strip().upper() for s in evidence.get("covered_symbol_universe") or []}
        ),
        "coverage_start_utc": evidence.get("coverage_start_utc"),
        "coverage_end_utc": evidence.get("coverage_end_utc"),
        "corporate_action_entries": entries_sorted,
    }


def corporate_action_evidence_id(evidence: Dict[str, Any]) -> str:
    """Deterministic, content-DERIVED evidence ID (Defect 2) -- the only
    legitimate value for a manifest's corporate_action_evidence_id under
    CA_POLICY_FORBID_AFFECTED_PERIODS. A caller can never simply assert an
    arbitrary string here: check_corporate_action_integrity independently
    recomputes this from the supplied evidence object and requires exact
    equality with the manifest's declared ID."""
    return sha256_json(canonical_ca_evidence_content(evidence))


def build_corporate_action_evidence(
    *,
    source_provider_id: str,
    covered_symbol_universe: Sequence[str],
    coverage_start_utc: str,
    coverage_end_utc: str,
    corporate_action_entries: Sequence[Dict[str, Any]] = (),
    artifact_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """Pure assembly of one durable, content-addressed corporate-action
    evidence object (Defect 2 / P8 REPAIR-02) -- the narrow interface a
    future BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01 data patch must satisfy
    to make CA_POLICY_FORBID_AFFECTED_PERIODS usable for OFFICIAL registered
    raw_unadjusted research. No authoritative source calls this with real
    data anywhere in this repository today -- see module header.

    `corporate_action_entries` are plain {"symbol","action_type",
    "effective_start_ts","effective_end_ts"} dicts ("action_type" may be
    None/"unknown" when not established). The returned object's own
    `evidence_id` is DERIVED from its canonical content (corporate_action_
    evidence_id) -- never a caller-chosen label."""
    covered_sorted = sorted({str(s).strip().upper() for s in covered_symbol_universe if str(s).strip()})
    entries_sorted = sorted(
        (dict(e) for e in corporate_action_entries),
        key=lambda e: (
            str(e.get("symbol")), str(e.get("action_type")),
            str(e.get("effective_start_ts")), str(e.get("effective_end_ts")),
        ),
    )
    evidence: Dict[str, Any] = {
        "schema_version": CA_EVIDENCE_SCHEMA_VERSION,
        "source_provider_id": source_provider_id,
        "covered_symbol_universe": covered_sorted,
        "coverage_start_utc": coverage_start_utc,
        "coverage_end_utc": coverage_end_utc,
        "corporate_action_entries": entries_sorted,
        "artifact_sha256": sha256_file(Path(artifact_path)) if artifact_path is not None else None,
    }
    evidence["evidence_id"] = corporate_action_evidence_id(evidence)
    return evidence


def forbidden_periods_from_evidence(evidence: Dict[str, Any]) -> List[Dict[str, str]]:
    """Deterministically DERIVE the forbidden-periods exclusion list from a
    verified corporate-action evidence object's own entries (Defect 2) --
    the only two ways a manifest's forbidden_periods may legitimately arise
    under CA_POLICY_FORBID_AFFECTED_PERIODS: this direct derivation, or an
    independently-recomputed list PROVEN identical to it (see
    _require_verified_ca_evidence)."""
    return sorted(
        (
            {
                "symbol": str(e["symbol"]).strip().upper(),
                "start_ts": e["effective_start_ts"],
                "end_ts": e["effective_end_ts"],
            }
            for e in evidence.get("corporate_action_entries") or []
        ),
        key=lambda e: (e["symbol"], e["start_ts"], e["end_ts"]),
    )


def _require_verified_ca_evidence(bars: pd.DataFrame, manifest: Dict[str, Any]) -> None:
    """Defect 2 core check: CA_POLICY_FORBID_AFFECTED_PERIODS is satisfied
    ONLY when a real evidence object is supplied, its canonical content hash
    recomputes to the manifest's declared corporate_action_evidence_id, its
    coverage includes the COMPLETE observed bars universe/date range, and
    the manifest's forbidden_periods are exactly the periods independently
    derivable from that verified evidence. A bare evidence-ID string with no
    verifiable evidence object -- the pre-repair status quo -- is never
    sufficient."""
    declared_id = manifest.get("corporate_action_evidence_id")
    evidence = manifest.get("corporate_action_evidence")
    if not declared_id or not isinstance(evidence, dict):
        raise CorporateActionIntegrityError(
            "Fail-closed: corporate_action_policy=forbid_affected_periods requires a real, "
            "content-addressed corporate-action evidence object (schema_version, "
            "source_provider_id, covered_symbol_universe, coverage_start_utc/coverage_end_utc, "
            "corporate_action_entries) -- a bare corporate_action_evidence_id string with no "
            "verifiable evidence object does not satisfy OFFICIAL registered raw_unadjusted "
            "research (no authoritative corporate-action evidence source exists in this "
            "repository today; see BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01)"
        )
    if evidence.get("schema_version") != CA_EVIDENCE_SCHEMA_VERSION:
        raise CorporateActionIntegrityError(
            f"Fail-closed: corporate_action_evidence.schema_version={evidence.get('schema_version')!r} "
            f"!= {CA_EVIDENCE_SCHEMA_VERSION!r}"
        )
    recomputed_id = corporate_action_evidence_id(evidence)
    if recomputed_id != declared_id:
        raise CorporateActionIntegrityError(
            "Fail-closed: corporate_action_evidence_id does not match the recomputed canonical "
            f"content hash of the supplied evidence object (declared={declared_id!r}, "
            f"recomputed={recomputed_id!r}) -- refusing to trust a caller-selected evidence ID"
        )

    if bars.empty:
        return

    actual_symbols = {str(s).strip().upper() for s in bars["symbol"].unique()}
    covered_symbols = set(evidence.get("covered_symbol_universe") or [])
    missing_symbols = actual_symbols - covered_symbols
    if missing_symbols:
        raise CorporateActionIntegrityError(
            "Fail-closed: corporate-action evidence coverage does not include bars symbol(s) "
            f"{sorted(missing_symbols)!r} -- refusing to trust exclusion evidence with incomplete "
            "symbol coverage"
        )

    coverage_start = evidence.get("coverage_start_utc")
    coverage_end = evidence.get("coverage_end_utc")
    if not coverage_start or not coverage_end:
        raise CorporateActionIntegrityError(
            "Fail-closed: corporate-action evidence missing coverage_start_utc/coverage_end_utc"
        )
    actual_ts = pd.to_datetime(bars["end_ts"], utc=True)
    coverage_start_ts = pd.Timestamp(coverage_start)
    coverage_end_ts = pd.Timestamp(coverage_end)
    if actual_ts.min() < coverage_start_ts or actual_ts.max() >= coverage_end_ts:
        raise CorporateActionIntegrityError(
            "Fail-closed: corporate-action evidence coverage window "
            f"[{coverage_start_ts.isoformat()}, {coverage_end_ts.isoformat()}) does not include "
            f"the full observed bars date range [{actual_ts.min().isoformat()}, "
            f"{actual_ts.max().isoformat()}]"
        )

    expected_forbidden = forbidden_periods_from_evidence(evidence)
    declared_forbidden = sorted(
        (dict(e) for e in manifest.get("forbidden_periods") or []),
        key=lambda e: (e["symbol"], e["start_ts"], e["end_ts"]),
    )
    if expected_forbidden != declared_forbidden:
        raise CorporateActionIntegrityError(
            "Fail-closed: manifest forbidden_periods do not match the periods independently "
            "derivable from the verified corporate-action evidence -- refusing a caller-modified "
            "exclusion list inconsistent with the verified evidence"
        )


def check_corporate_action_integrity(bars: pd.DataFrame, manifest: Dict[str, Any]) -> None:
    """Fail-closed preflight. MUST be called BEFORE any economic P&L is
    computed from `bars` (Root Defect 2 / the repair mission's pre-flight
    requirement) -- this is an integrity preflight, not a change to the
    already-accepted future-execution chronology.

    - CA_POLICY_ADJUSTED_DATA is satisfied only when
      price_adjustment_convention is one of the independently-verified
      adjusted conventions this system currently recognizes
      (_KNOWN_ADJUSTED_CONVENTIONS). Refuses to trust an unproven "adjusted"
      claim. For a convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION
      (e.g. alpaca_all_adjusted_v1), the convention name alone is NOT
      enough -- see _require_verified_source_attestation.
    - CA_POLICY_FORBID_AFFECTED_PERIODS is satisfied only when a real,
      content-addressed corporate_action_evidence object verifies against
      the manifest's declared corporate_action_evidence_id (Defect 2 --
      see _require_verified_ca_evidence), its coverage includes the full
      observed bars universe/range, its forbidden_periods are exactly what
      the evidence independently derives, AND every row in `bars` falls
      OUTSIDE every declared forbidden (symbol, period) window.
    - Anything else (including the diagnostic policy, or a missing/unknown
      policy) fails closed.
    """
    policy = manifest.get("corporate_action_policy")
    convention = manifest.get("price_adjustment_convention")

    if policy == CA_POLICY_ADJUSTED_DATA:
        if convention not in _KNOWN_ADJUSTED_CONVENTIONS:
            raise CorporateActionIntegrityError(
                f"Fail-closed: corporate_action_policy={CA_POLICY_ADJUSTED_DATA!r} requires a "
                "verified adjusted price convention, but price_adjustment_convention="
                f"{convention!r} is not one of the independently-verified adjusted conventions "
                "this system currently recognizes -- refusing to trust an unproven 'adjusted' claim"
            )
        if convention in _CONVENTIONS_REQUIRING_SOURCE_ATTESTATION:
            _require_verified_source_attestation(bars, manifest)
        return

    if policy == CA_POLICY_FORBID_AFFECTED_PERIODS:
        _require_verified_ca_evidence(bars, manifest)
        violations = _find_forbidden_period_violations(bars, manifest.get("forbidden_periods") or [])
        if violations:
            raise CorporateActionIntegrityError(
                "Fail-closed: bars contain rows inside a declared corporate-action exclusion "
                f"period -- refusing to score the contaminated interval: {violations}"
            )
        return

    raise CorporateActionIntegrityError(
        f"Fail-closed: corporate_action_policy={policy!r} does not provide a valid, honest "
        f"corporate-action safety guarantee for this bars data (price_adjustment_convention="
        f"{convention!r}) -- refusing to compute economic P&L over data that may contain "
        "unadjusted split/dividend contamination"
    )


def _find_forbidden_period_violations(bars: pd.DataFrame, entries: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    violations: List[Dict[str, Any]] = []
    if not entries or bars.empty:
        return violations
    symbols = bars["symbol"].astype(str)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    for entry in entries:
        symbol = str(entry["symbol"])
        start = pd.to_datetime(entry["start_ts"], utc=True)
        end = pd.to_datetime(entry["end_ts"], utc=True)
        mask = (symbols == symbol) & (end_ts >= start) & (end_ts <= end)
        if bool(mask.any()):
            violations.append(
                {
                    "symbol": symbol,
                    "start_ts": entry["start_ts"],
                    "end_ts": entry["end_ts"],
                    "violating_row_count": int(mask.sum()),
                }
            )
    return violations
