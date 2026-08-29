from __future__ import annotations

import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, FrozenSet, List, Optional, Sequence, Tuple

import numpy as np
import pandas as pd

from mqk_research.data.ca_reviewed_resolutions import (
    REVIEWED_CA_RESOLUTIONS,
    find_reviewed_resolution,
    require_verified_reviewed_resolution,
)
from mqk_research.data.bars_provenance import (
    CA_POLICY_ADJUSTED_DATA,
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
    SOURCE_AUTHORITY_DIAGNOSTIC_SYNTHETIC,
    SOURCE_AUTHORITY_OFFICIAL_PROVIDER,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    build_source_attestation,
    canonical_semantic_bars_hash,
    corporate_action_evidence_id,
)
from mqk_research.ml.util_hash import sha256_file, sha256_json

# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01
#
# The dedicated RESEARCH historical-data extraction authority for official
# US equity/ETF economic research. Distinct from, and does not modify:
#   - core-rs/crates/mqk-md/src/alpaca_provider.rs: the PAPER/RUNTIME
#     historical provider, which explicitly requests adjustment=raw and
#     feeds md_bars via the daemon's ingestion path. That path is
#     unchanged by this module -- production execution never consumes
#     adjusted or synthetic prices.
#   - mqk_research.data.adapters.bars_postgres: reads md_bars (the
#     raw_unadjusted Postgres research mirror of the runtime data).
#
# This module fetches DIRECTLY from Alpaca's Market Data API (not through
# md_bars) so that official research can use adjustment=all -- verified
# against Alpaca's official documentation (docs.alpaca.markets/reference/
# stockbars and docs.alpaca.markets/reference/corporateactions-1, fetched
# 2026-08-15 via Firecrawl) to apply forward/reverse SPLIT price+volume
# adjustment, cash DIVIDEND price adjustment, and SPIN-OFF price adjustment
# in a single request. Every OTHER corporate-action type Alpaca's
# /v1/corporate-actions endpoint reports (mergers, redemptions, name
# changes, rights distributions, worthless removals, partial calls,
# reorganizations, unit splits, stock dividends) is NOT documented as being
# covered by any adjustment mode -- see _COVERED_BY_ADJUSTMENT_ALL below.
# This module deliberately does NOT guess that an undocumented type is
# safe: any REQUIRES_FAIL_CLOSED_REVIEW event intersecting the requested
# symbol/range fails the whole extraction closed (see
# CorporateActionReviewRequired) rather than silently dropping bars or
# symbols.
#
# A successful, clean extraction produces a bars provenance manifest with
# corporate_action_policy=adjusted_data and
# price_adjustment_convention=alpaca_all_adjusted_v1, bound to a real,
# content-addressed source_attestation (see bars_provenance.
# build_source_attestation) that mqk_research.data.bars_provenance.
# check_corporate_action_integrity independently verifies before any
# economic P&L is computed -- a hand-built manifest asserting the same
# convention string is NOT sufficient (see
# bars_provenance.SourceAttestationUnverifiable).
#
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 (independent-review
# repair) closed four further defects:
#   - Defect 1 (CA discovery completeness): Alpaca's /v1/corporate-actions
#     `start`/`end` filter by `process_date`, which Alpaca's own developer
#     relations team confirms "can be several days (or more) after the
#     ex_date" with no documented bound (forum.alpaca.markets/t/
#     querying-corporate-actions-by-ex-date-rather-than-process-date/17724,
#     verified 2026-08-15). A query bounded by the research window can
#     therefore miss an event whose ex_date falls inside the window but
#     whose process_date lands after it. See CA_DISCOVERY_PROCESS_DATE_FLOOR
#     / _ca_discovery_process_date_bounds below: the fix queries the full
#     provider-supported process_date history for the requested symbols
#     through the explicit ASOF, then filters locally by effective-date
#     (ex_date-based) intersection with the research window --
#     _filter_entries_intersecting_range.
#   - Defect 2 (implicit ASOF): fetch_historical_bars now requires an
#     explicit, caller-resolved `asof` (never the provider's implicit
#     current-day default) and records it verbatim in the source
#     attestation, which already participates in attestation identity (see
#     bars_provenance.canonical_source_attestation_content).
#   - Defect 3 (official vs diagnostic authority): extraction is now split
#     into extract_research_bars_with_provenance (OFFICIAL: fixed base URL,
#     real HTTP transport, no injectable transport/base_url, mints
#     SOURCE_AUTHORITY_OFFICIAL_PROVIDER) and
#     extract_research_bars_with_provenance_diagnostic (INTERNAL: injectable
#     transport for tests, always mints SOURCE_AUTHORITY_DIAGNOSTIC_SYNTHETIC
#     + DIAGNOSTIC_EXTRACTOR_ID, which bars_provenance never trusts).
#   - Defect 4 (transport artifacts in semantic identity): per-page raw
#     response hashes remain on the attestation object as audit evidence but
#     no longer participate in source_attestation_id (see bars_provenance.
#     canonical_source_attestation_content).
#   - Provider end-inclusive/internal end-exclusive mismatch: Alpaca's bars
#     `end` is documented inclusive; this repo's internal contract is the
#     half-open [start_utc, end_utc). fetch_historical_bars now normalizes
#     locally so a bar at or past end_utc, or before start_utc, never enters
#     the canonical research dataset.
#
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-02 (independent-review repair
# of REPAIR-01's Defect 1 fix) closed a further defect: REPAIR-01's CA
# discovery ceiling was max(asof, research_end) -- wrongly treating the bars
# `asof` (Alpaca's documented entity/symbol-mapping-resolution parameter for
# /v2/stocks/bars) as if it also bounded corporate-action evidence vintage.
# Nothing in Alpaca's documentation ties `asof` to /v1/corporate-actions
# `process_date` coverage; a corporate action can have process_date after
# BOTH research_end and asof (e.g. ex_date=Jun28 inside the research window,
# process_date=Jul02, with research_end=asof=Jun30) and REPAIR-01's ceiling
# would miss it. The three concepts are kept explicitly separate:
#   - bars_asof: entity/symbol-mapping identity, sent verbatim to
#     /v2/stocks/bars `asof` (unchanged, still required -- Defect 2).
#   - ca_discovery_cutoff_utc: the UTC extraction/retrieval instant through
#     which /v1/corporate-actions process_date evidence was actually queried
#     -- resolved ONCE, before either provider call, from the caller-supplied
#     `retrieval_timestamp_utc` (or real wall-clock UTC now for a live
#     extraction). See _resolve_ca_discovery_cutoff_utc /
#     _ca_discovery_process_date_bounds.
#   - the research interval [start_utc, end_utc) itself, unaffected.
# Corporate-action evidence is a provider snapshot, not timeless truth (Alpaca
# documents unbounded process_date delay -- see Defect 1 above) -- re-running
# the SAME bars request/asof/research window at a LATER ca_discovery_cutoff_utc
# can legitimately discover additional backfilled corporate actions; when it
# does, the resulting corporate_action_evidence content (and therefore
# corporate_action_evidence_id / source_attestation_id / trial identity) is
# meant to change -- this is desired revision sensitivity, not a bug, and is
# never suppressed by this module.
#
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-03 (independent-review repair)
# closed three further defects:
#   - Defect 1 (shared helper could mint official authority): the former
#     _extract_research_bars_with_provenance_impl accepted caller-supplied
#     http_get/extractor_id/source_authority together, so a test (or any
#     caller) could inject a fake transport and still claim
#     SOURCE_AUTHORITY_OFFICIAL_PROVIDER. The shared core is now
#     _neutral_extract: it performs retrieval/normalization/CA-filtering/
#     evidence construction but structurally cannot accept an extractor_id or
#     source_authority. Only the two public wrappers below mint an
#     attestation, each with its own hard-coded identity (_mint_manifest).
#   - Defect 2 (caller-selected snapshot clock): extract_research_bars_with_
#     provenance no longer accepts retrieval_timestamp_utc. It resolves the
#     CA discovery cutoff exactly once via _utc_now() (a monkeypatchable
#     seam), same as the fixed _default_http_get transport seam.
#     extract_research_bars_with_provenance_diagnostic keeps an explicit,
#     honestly-named ca_discovery_cutoff_utc override for deterministic
#     tests.
#   - Defect 3 (snapshot clock leaking into semantic identity): bars_
#     provenance.canonical_source_attestation_content now hashes only the
#     SEMANTIC subset of corporate_action_query_coverage (protocol, discovery
#     floor, research window, requested symbols/types) -- the wall-clock
#     ca_discovery_end_utc stays on the attestation object for audit but no
#     longer participates in source_attestation_id / trial identity.
#
# BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01: the prior normalized
# corporate-action entry shape was exactly
# {symbol, action_type, effective_start_ts, effective_end_ts} -- discarding,
# after _effective_windows_for_entry ran, which raw symbol FIELD matched
# (and therefore which ROLE the matched symbol plays, e.g. acquirer vs.
# acquiree) and every SECURITY-IDENTITY field (CUSIPs, counterparty
# symbols) the provider actually supplied. A future admission policy cannot
# honestly distinguish "requested symbol is the acquirer" from "requested
# symbol is the terminal acquiree" of the SAME merger, or prove two
# same-ticker name-change events are unrelated securities, from that shape
# alone. _effective_windows_for_entry now preserves this evidence on every
# leg (see _SYMBOL_FIELD_ROLE / _ROLE_CUSIP_FIELD); this patch does NOT
# change which events are admitted -- see classify_corporate_action_type /
# find_requires_review_events, unchanged here.
#
# BKT-RESEARCH-CA-ROLE-AWARE-RESOLUTION-01: find_requires_review_events now
# runs classify_corporate_action_resolution over the role/identity evidence
# BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01 preserved, so a merger ACQUIRER
# leg (never an acquiree leg) and a name_change with old_cusip==new_cusip
# (real, provider-supplied security continuity, never a bare type-based
# guess) no longer block the whole extraction -- see
# CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG / CATEGORY_VERIFIED_SAME_
# SECURITY_CONTINUITY. Every other REQUIRES_FAIL_CLOSED_REVIEW-by-type event
# is unaffected and still fails the whole extraction closed.
#
# BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01: classify_corporate_
# action_resolution now falls back, as a LAST resort (never a bypass for
# CATEGORY_COVERED_BY_ADJUSTMENT or an already role-aware-resolved leg), to
# mqk_research.data.ca_reviewed_resolutions.find_reviewed_resolution -- a
# narrow, explicit, content-addressed registry of individually-reviewed
# exceptions (e.g. a holding-company reorganization whose legal-entity
# CUSIP changes but whose ticker and economic ownership do not) for events
# no automated rule here can honestly resolve. Matched on an EXACT
# (provider, action_type, requested_symbol, requested_role, process_date)
# tuple -- see that module's header for why the provider's own opaque
# event id is deliberately NOT part of the binding. This module contains
# no symbol-specific branching for any such exception.
#
# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01: the three CA admission/
# resolution patches above are a SEMANTIC data-authority change -- this
# extractor now treats some events as non-blocking that V1 always refused.
# EXTRACTOR_ID / DIAGNOSTIC_EXTRACTOR_ID are bumped from
# EXTRACTOR_ID_V1_LEGACY to v2/diagnostic_v2 (never minted as v1 again),
# and resolution_policy_fingerprint binds the actual resolution policy
# (role-aware rules + reviewed-resolution registry content) into
# source_attestation_id via a new ca_resolution_policy_id field -- see that
# function and bars_provenance.canonical_source_attestation_content /
# _V2_PLUS_EXTRACTOR_IDS. A V1 attestation never carried this field and its
# canonical content/identity computation is untouched, so an
# already-persisted V1 artifact remains independently verifiable under its
# own historical contract (bars_provenance._TRUSTED_EXTRACTOR_IDS still
# trusts EXTRACTOR_ID_V1_LEGACY) -- never reinterpreted as V2.

ALPACA_DATA_BASE_URL = "https://data.alpaca.markets"
BARS_PATH = "/v2/stocks/bars"
CORPORATE_ACTIONS_PATH = "/v1/corporate-actions"

ADJUSTMENT_ALL = "all"
DEFAULT_FEED = "iex"
DEFAULT_TIMEFRAME = "1Day"

ENV_ALPACA_KEY = "ALPACA_API_KEY_PAPER"
ENV_ALPACA_SECRET = "ALPACA_API_SECRET_PAPER"

# Historical only (BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01): the extractor
# identity every OFFICIAL attestation minted BEFORE this repair carried.
# This module no longer mints this id anywhere -- it is retained only so
# bars_provenance._TRUSTED_EXTRACTOR_IDS can still independently verify an
# already-persisted, pre-repair V1 artifact under its own historical
# contract. Never reinterpret a V1 attestation as V2.
EXTRACTOR_ID_V1_LEGACY = "mqk_research.data.alpaca_historical.v1"

# Trusted, OFFICIAL extraction identity -- the only extractor_id
# bars_provenance._TRUSTED_EXTRACTOR_IDS accepts for a FRESH extraction.
# Only extract_research_bars_with_provenance (fixed base URL, real HTTP
# transport) may mint an attestation carrying this id.
#
# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01: bumped from
# EXTRACTOR_ID_V1_LEGACY because BKT-RESEARCH-CA-ROLE-AWARE-RESOLUTION-01 /
# BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01 changed which corporate
# actions this extractor treats as non-blocking -- a SEMANTIC data-authority
# change (CLAUDE.md #6) that must change official research source identity
# even when the underlying OHLC bars are byte-identical to a V1 extraction
# of the same window. See resolution_policy_fingerprint / bars_provenance.
# canonical_source_attestation_content's ca_resolution_policy_id handling.
EXTRACTOR_ID = "mqk_research.data.alpaca_historical.v2"

# Explicitly UNtrusted diagnostic identity (Defect 3), bumped alongside
# EXTRACTOR_ID (V2-01) for the same reason -- the diagnostic path runs the
# SAME role-aware/reviewed-resolution logic and should honestly report
# which policy generation it ran under, even though it can never satisfy
# bars_provenance._TRUSTED_EXTRACTOR_IDS or SOURCE_AUTHORITY_OFFICIAL_PROVIDER
# regardless of what transport/base_url a caller injects. Minted only by
# extract_research_bars_with_provenance_diagnostic.
DIAGNOSTIC_EXTRACTOR_ID = "mqk_research.data.alpaca_historical.diagnostic_v2"

_ASOF_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")

# Corporate-action type vocabulary, verified 2026-08-15 against Alpaca's
# official /v1/corporate-actions OpenAPI schema
# (docs.alpaca.markets/reference/corporateactions-1). Do not add a type here
# without re-verifying against the live schema -- an unmapped type must fail
# closed (see _effective_windows_for_entry), never be silently ignored.
KNOWN_CORPORATE_ACTION_TYPES: FrozenSet[str] = frozenset(
    {
        "reverse_split",
        "forward_split",
        "unit_split",
        "cash_dividend",
        "stock_dividend",
        "spin_off",
        "cash_merger",
        "stock_merger",
        "stock_and_cash_merger",
        "redemption",
        "name_change",
        "worthless_removal",
        "rights_distribution",
        "partial_call",
        "reorganization",
    }
)

# Maps the /v1/corporate-actions response's pluralized array keys
# (corporate_actions.<key>) to the singular `types` filter vocabulary above
# -- verified against the same OpenAPI schema.
_RESPONSE_KEY_TO_TYPE: Dict[str, str] = {
    "reverse_splits": "reverse_split",
    "forward_splits": "forward_split",
    "unit_splits": "unit_split",
    "cash_dividends": "cash_dividend",
    "stock_dividends": "stock_dividend",
    "spin_offs": "spin_off",
    "cash_mergers": "cash_merger",
    "stock_mergers": "stock_merger",
    "stock_and_cash_mergers": "stock_and_cash_merger",
    "redemptions": "redemption",
    "name_changes": "name_change",
    "worthless_removals": "worthless_removal",
    "rights_distributions": "rights_distribution",
    "partial_calls": "partial_call",
    "reorganizations": "reorganization",
}

# Which symbol field(s) on each raw corporate-action entry identify the
# ticker(s) affected -- verified against the same OpenAPI schema (multi-leg
# events like mergers/spin-offs/name-changes touch two-plus symbols; every
# affected symbol gets its own flattened entry so universe-intersection
# checks below cannot miss one leg of the event).
_SYMBOL_FIELDS_BY_TYPE: Dict[str, Tuple[str, ...]] = {
    "cash_dividend": ("symbol",),
    "cash_merger": ("acquirer_symbol", "acquiree_symbol"),
    "forward_split": ("symbol",),
    "name_change": ("old_symbol", "new_symbol"),
    "partial_call": ("symbol",),
    "redemption": ("symbol",),
    "reorganization": ("symbol",),
    "reverse_split": ("symbol",),
    "rights_distribution": ("source_symbol", "new_symbol"),
    "spin_off": ("source_symbol", "new_symbol"),
    "stock_and_cash_merger": ("acquirer_symbol", "acquiree_symbol"),
    "stock_dividend": ("symbol",),
    "stock_merger": ("acquirer_symbol", "acquiree_symbol"),
    "unit_split": ("old_symbol", "new_symbol", "alternate_symbol"),
    "worthless_removal": ("symbol",),
}

# BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01: the semantic ROLE a matched
# symbol field plays in its event -- a field name means the same role
# regardless of which action_type it appears under (verified against the
# same OpenAPI schema _SYMBOL_FIELDS_BY_TYPE cites). This is what lets a
# downstream policy distinguish "requested symbol is the acquirer" from
# "requested symbol is the acquiree" for the SAME cash_merger/stock_merger
# event, instead of collapsing both legs to an undifferentiated "symbol".
_SYMBOL_FIELD_ROLE: Dict[str, str] = {
    "symbol": "primary",
    "acquirer_symbol": "acquirer",
    "acquiree_symbol": "acquiree",
    "old_symbol": "old_symbol",
    "new_symbol": "new_symbol",
    "source_symbol": "source_symbol",
    "alternate_symbol": "alternate_symbol",
}

# The raw CUSIP field (if any) the verified schema associates with a given
# role -- used to attach real security-identity evidence to the leg that
# role produced. A role absent here has no known CUSIP field; its identity
# evidence is left None, never guessed.
_ROLE_CUSIP_FIELD: Dict[str, str] = {
    "primary": "cusip",
    "acquirer": "acquirer_cusip",
    "acquiree": "acquiree_cusip",
    "old_symbol": "old_cusip",
    "new_symbol": "new_cusip",
}


def _norm_symbol_or_none(value: Any) -> Optional[str]:
    """Normalize an optional raw symbol field: uppercased/stripped when
    populated, else None -- never an empty string or a fabricated default."""
    if not value:
        return None
    return str(value).strip().upper()


# Types whose schema includes an ex_date (verified against the OpenAPI
# schema's "required" lists): cash_dividend, forward_split, reverse_split,
# rights_distribution, spin_off, stock_dividend. Every type has a required
# process_date. effective_start uses ex_date when available (conservative:
# ex_date <= process_date), else falls back to process_date; effective_end
# is always process_date (inclusive, closed interval -- matches this repo's
# ForbidEntry/forbidden_periods convention).
_EX_DATE_TYPES: FrozenSet[str] = frozenset(
    {"cash_dividend", "forward_split", "reverse_split", "rights_distribution", "spin_off", "stock_dividend"}
)

CATEGORY_COVERED_BY_ADJUSTMENT = "covered_by_adjustment"
CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW = "requires_fail_closed_review"

# BKT-RESEARCH-CA-ROLE-AWARE-RESOLUTION-01: two additional, narrow,
# provably-safe resolution outcomes a type-level CATEGORY_REQUIRES_FAIL_
# CLOSED_REVIEW event can still resolve into, once the requested symbol's
# ROLE and SECURITY IDENTITY (see BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01)
# are examined -- see classify_corporate_action_resolution. Neither is
# "covered by adjustment=all"; both are distinct, honestly-named claims:
#   - a merger ACQUIRER leg's own shares are not converted/cancelled by the
#     event, so no missing price-adjustment/return-transformation is
#     required for THAT leg specifically (the acquirer's real traded
#     returns are used as-is, never synthesized);
#   - a name_change whose old_cusip/new_cusip are populated and equal is
#     the SAME underlying security continuing under a new ticker (matching
#     Alpaca's documented `asof` entity-mapping semantics), not a
#     transformation requiring adjustment.
# Every other REQUIRES_FAIL_CLOSED_REVIEW-by-type event (acquiree/terminal
# merger legs, unresolved name changes, stock_dividend, unit_split,
# redemption, worthless_removal, rights_distribution, partial_call,
# reorganization) remains CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW -- this patch
# deliberately does not attempt those; see PATCH C for the narrow,
# explicit, auditable exception mechanism for an individually-reviewed
# event no automated rule here can honestly resolve.
CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG = "no_adjustment_required_for_requested_leg"
CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY = "verified_same_security_continuity"

# BKT-RESEARCH-CA-POLICY-SINGLE-SOURCE-OF-TRUTH-01: the SOLE authoritative
# representation of the merger-acquirer automated resolution rule. Patch F
# (BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01-REPAIR-01) hashed a SEPARATE,
# manually-typed description of this rule's values into
# _canonical_ca_resolution_policy_spec while classify_corporate_action_
# resolution/_event_group_key independently hard-coded their own literals
# ("acquirer", "acquiree", ("provider_event_id", "action_type")) -- an edit
# to one copy (e.g. flipping which role is required) could silently change
# production CA admission behavior while leaving the hashed spec, the policy
# fingerprint, and the trusted V2 policy id completely unchanged. Both
# classify_corporate_action_resolution/_event_group_key AND
# _canonical_ca_resolution_policy_spec now read every decision-bearing value
# from THIS ONE object -- referenced as a bare module global (never a
# function-default parameter, which Python binds once at def-time) so a test
# monkeypatching MERGER_ACQUIRER_RULE is observed by both consumers.
#
# This does NOT detect an arbitrary rewrite of classify_corporate_action_
# resolution's control flow (e.g. an edit that stops reading this object
# altogether) -- no purely data-driven fingerprint can prove that. What it
# DOES guarantee: as long as the classifier consumes these fields (which the
# required negative controls below prove for every field), changing any one
# of them changes BOTH the classifier's actual admission behavior AND
# resolution_policy_fingerprint identically, because there is only one place
# the value is written down.
@dataclass(frozen=True)
class MergerAcquirerRule:
    action_types: FrozenSet[str]
    required_role: str
    terminal_conflict_role: str
    same_event_grouping_identity: Tuple[str, ...]
    reject_same_symbol_terminal_conflict: bool


# BKT-RESEARCH-CA-POLICY-SINGLE-SOURCE-OF-TRUTH-01: the SOLE authoritative
# representation of the name-change automated resolution rule -- same
# rationale as MergerAcquirerRule above.
@dataclass(frozen=True)
class NameChangeRule:
    action_type: str
    allowed_roles: FrozenSet[str]
    require_old_cusip_populated: bool
    require_new_cusip_populated: bool
    require_cusip_equality: bool


MERGER_ACQUIRER_RULE = MergerAcquirerRule(
    action_types=frozenset({"cash_merger", "stock_merger", "stock_and_cash_merger"}),
    required_role="acquirer",
    terminal_conflict_role="acquiree",
    same_event_grouping_identity=("provider_event_id", "action_type"),
    reject_same_symbol_terminal_conflict=True,
)

NAME_CHANGE_RULE = NameChangeRule(
    action_type="name_change",
    allowed_roles=frozenset({"old_symbol", "new_symbol"}),
    require_old_cusip_populated=True,
    require_new_cusip_populated=True,
    require_cusip_equality=True,
)

# BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01, bumped by -REPAIR-01 F2 and again
# by -SINGLE-SOURCE-OF-TRUTH-01: bumped whenever classify_corporate_action_
# resolution's actual decision logic changes (which raw fields resolve which
# category) -- the discipline mirrors CA_DISCOVERY_PROTOCOL_V2's existing
# versioning pattern in this module. Part of resolution_policy_fingerprint
# (as policy_spec_version, see _canonical_ca_resolution_policy_spec), so a
# future logic change is reflected in official research source identity even
# if this constant is NOT bumped in the same patch (the reviewed-resolution
# registry's own resolution_ids still change identity independently, and the
# spec below now single-sources the automated rules' actual field values --
# not just this version label -- so a rule-value change that forgets to bump
# this constant still changes the fingerprint) -- but bumping it IS the
# correct thing to do whenever the merger/name-change contracts themselves
# change, exactly like EXTRACTOR_ID. SINGLE-SOURCE-OF-TRUTH-01: bumped v2 ->
# v3 because this patch changed the canonical policy spec's own shape
# (added terminal_conflict_role, renamed require_old_cusip_equals_new_cusip
# -> require_cusip_equality, renamed reject_when_same_matched_symbol_also_
# acquiree_sibling -> reject_same_symbol_terminal_conflict).
CA_RESOLUTION_POLICY_VERSION = "role_aware_reviewed_registry_v3"


def _canonical_ca_resolution_policy_spec(
    *, reviewed_resolutions: Sequence[Dict[str, Any]] = REVIEWED_CA_RESOLUTIONS
) -> Dict[str, Any]:
    """The full canonical CA RESOLUTION POLICY SPEC (BKT-RESEARCH-CA-
    AUTHORITY-IDENTITY-V2-01-REPAIR-01 F2, repaired by -SINGLE-SOURCE-OF-
    TRUTH-01): serializes the SAME MergerAcquirerRule/NameChangeRule objects
    classify_corporate_action_resolution/_event_group_key actually consume
    (see the module comment above MergerAcquirerRule) -- not a second,
    independently-typed description of their values -- plus the exact
    provider-adjusted type set and the reviewed registry actually in force.
    Every reviewed-resolution record is independently re-verified (require_
    verified_reviewed_resolution) before its resolution_id is trusted to
    contribute to this spec: a stale/tampered reviewed-resolution record
    must fail closed here too, not just at lookup time, so a corrupted
    registry entry can never silently mint a policy identity that includes
    it."""
    for record in reviewed_resolutions:
        require_verified_reviewed_resolution(record)
    return {
        "policy_spec_version": CA_RESOLUTION_POLICY_VERSION,
        "merger_acquirer_rule": {
            "action_types": sorted(MERGER_ACQUIRER_RULE.action_types),
            "required_role": MERGER_ACQUIRER_RULE.required_role,
            "terminal_conflict_role": MERGER_ACQUIRER_RULE.terminal_conflict_role,
            "same_event_grouping_identity": list(MERGER_ACQUIRER_RULE.same_event_grouping_identity),
            "reject_same_symbol_terminal_conflict": MERGER_ACQUIRER_RULE.reject_same_symbol_terminal_conflict,
        },
        "name_change_rule": {
            "action_type": NAME_CHANGE_RULE.action_type,
            "allowed_roles": sorted(NAME_CHANGE_RULE.allowed_roles),
            "require_old_cusip_populated": NAME_CHANGE_RULE.require_old_cusip_populated,
            "require_new_cusip_populated": NAME_CHANGE_RULE.require_new_cusip_populated,
            "require_cusip_equality": NAME_CHANGE_RULE.require_cusip_equality,
        },
        "provider_adjusted_covered_types": sorted(_COVERED_BY_ADJUSTMENT_ALL),
        "reviewed_registry_resolution_ids": sorted(r["resolution_id"] for r in reviewed_resolutions),
    }


def resolution_policy_fingerprint(
    *, reviewed_resolutions: Sequence[Dict[str, Any]] = REVIEWED_CA_RESOLUTIONS
) -> str:
    """Content-derived fingerprint of the CORPORATE-ACTION RESOLUTION POLICY
    as a whole (BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01, repaired by
    -REPAIR-01 F2 and -SINGLE-SOURCE-OF-TRUTH-01): hashes the full canonical
    policy spec (see _canonical_ca_resolution_policy_spec) -- the policy
    VERSION, the automated merger-acquirer and name-change rules' ACTUAL
    field values (the same MergerAcquirerRule/NameChangeRule objects the
    classifier itself consumes -- see the module comment above
    MergerAcquirerRule for exactly what guarantee this does and does not
    provide), the exact provider-adjusted type set, and every reviewed-
    resolution record actually in force identified by ITS OWN content-
    addressed resolution_id (so adding, removing, or editing a reviewed-
    resolution record changes this fingerprint even if no Python logic
    changes at all). Threaded into build_source_attestation as ca_resolution_
    policy_id -- see that function and bars_provenance.canonical_source_
    attestation_content -- so two extractions with byte-identical bars and
    byte-identical raw corporate-action evidence, but governed by a
    DIFFERENT resolution policy, never share a source_attestation_id. Fails
    closed (via require_verified_reviewed_resolution) if any reviewed-
    resolution record is itself stale/tampered."""
    return sha256_json(_canonical_ca_resolution_policy_spec(reviewed_resolutions=reviewed_resolutions))


# Conservative, docs-verified classification for adjustment="all": Alpaca's
# official documentation states adjustment=all "applies all" of the
# individually-documented raw|split|dividend|spin-off adjustments, i.e.
# forward/reverse SPLIT price+volume adjustment, cash DIVIDEND price
# adjustment, and SPIN-OFF price adjustment. No other corporate-action type
# is documented as covered by any adjustment mode -- this explicitly
# includes stock_dividend and unit_split, which are easy to mistake for
# "the same as cash_dividend/split" but are never named in Alpaca's
# adjustment documentation. Per the mission's "do not guess" instruction,
# anything not in this set is REQUIRES_FAIL_CLOSED_REVIEW.
_COVERED_BY_ADJUSTMENT_ALL: FrozenSet[str] = frozenset(
    {"forward_split", "reverse_split", "cash_dividend", "spin_off"}
)

# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 (Defect 1): Alpaca
# documents no bound on how far a corporate action's process_date can lag
# its ex_date/effective_date ("can be several days (or more)" -- Alpaca
# Developer Relations, forum.alpaca.markets, 2025-08-28). With no documented
# bound, only querying the full provider-supported process_date history for
# the requested symbols is a PROVEN superset -- a short arbitrary buffer
# (30/60/90 days) would be an unproven guess. This is an explicit "no lower
# bound" sentinel, not a padding heuristic: well before any US-listed
# equity/ETF's corporate-action history.
CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC = pd.Timestamp("1900-01-01T00:00:00Z")

# REPAIR-02: protocol name bumped from REPAIR-01's *_through_asof_v1 -- the
# ceiling is no longer derived from bars_asof (see module header REPAIR-02
# section); it is the resolved CA discovery snapshot cutoff.
CA_DISCOVERY_PROTOCOL_V2 = "process_date_full_history_through_retrieval_snapshot_v2"


class AlpacaCredentialsMissing(RuntimeError):
    """Fail-closed: raised when ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER
    are not present in the environment (this repo's existing Alpaca
    credential convention -- see
    core-rs/crates/mqk-md/src/alpaca_provider.rs::load_alpaca_paper_credentials).
    Never logs, prints, or serializes the values themselves."""


class AlpacaHistoricalExtractionError(RuntimeError):
    """Fail-closed: raised for any provider HTTP error, incomplete
    pagination, malformed/unmapped response content, duplicate bar,
    non-finite price, or unsupported request shape encountered while
    extracting official research bars/corporate-action data from Alpaca."""


class CorporateActionReviewRequired(RuntimeError):
    """Fail-closed: raised when a REQUIRES_FAIL_CLOSED_REVIEW corporate
    action intersects the requested symbol/range -- see module header and
    find_requires_review_events. Narrow the symbol universe or date range
    to exclude the affected symbol/period, or wait for an explicit handling
    policy for that action type; bars are never silently dropped."""


@dataclass(frozen=True)
class AlpacaCredentials:
    api_key: str
    api_secret: str


def load_alpaca_credentials(env: Optional[Dict[str, str]] = None) -> AlpacaCredentials:
    """Load Alpaca research-data credentials from the environment using this
    repo's existing convention (ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER
    -- the same paper credentials core-rs/crates/mqk-md/src/alpaca_provider.rs
    uses for historical data; market-data access is tied to the account, not
    to paper-vs-live order routing). Fails closed if either is absent. Never
    logs or returns the values in any exception message."""
    e = env if env is not None else os.environ
    key = e.get(ENV_ALPACA_KEY)
    secret = e.get(ENV_ALPACA_SECRET)
    if not key or not secret:
        raise AlpacaCredentialsMissing(
            f"Missing Alpaca research credentials: set {ENV_ALPACA_KEY} and {ENV_ALPACA_SECRET} "
            "in the environment (repo convention; values are never printed/logged)."
        )
    return AlpacaCredentials(api_key=key, api_secret=secret)


def _require_resolved_asof(asof: str) -> str:
    """Fail-closed format check for a caller-RESOLVED asof date (Defect 2):
    must be an explicit YYYY-MM-DD, never None/empty -- this module never
    falls back to Alpaca's implicit current-day default."""
    if not asof or not _ASOF_RE.match(asof):
        raise ValueError(f"asof must be an explicit 'YYYY-MM-DD' date string, got {asof!r}")
    return asof


# (url, query_params, headers) -> (status_code, response_body_bytes). Tests
# inject a fake implementation -- no real network call is ever required to
# exercise this module.
HttpGet = Callable[[str, Dict[str, str], Dict[str, str]], Tuple[int, bytes]]


def _default_http_get(url: str, params: Dict[str, str], headers: Dict[str, str]) -> Tuple[int, bytes]:
    qs = urllib.parse.urlencode(params)
    full_url = f"{url}?{qs}" if qs else url
    req = urllib.request.Request(full_url, headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (fixed https host)
            return int(resp.status), resp.read()
    except urllib.error.HTTPError as exc:
        return int(exc.code), exc.read()


def _fetch_all_pages(
    *,
    endpoint_url: str,
    base_params: Dict[str, str],
    credentials: AlpacaCredentials,
    http_get: HttpGet,
    max_pages: int = 500,
) -> Tuple[List[Dict[str, Any]], bool, List[str]]:
    """Paginate an Alpaca Market Data API GET endpoint to completion.
    Returns (raw_json_pages, pagination_complete, per_page_content_hashes).
    Fails closed (AlpacaHistoricalExtractionError) on any non-200 response,
    undecodable JSON, or pagination that does not terminate within
    max_pages -- never returns a partial result silently."""
    headers = {
        "APCA-API-KEY-ID": credentials.api_key,
        "APCA-API-SECRET-KEY": credentials.api_secret,
        "accept": "application/json",
    }
    pages: List[Dict[str, Any]] = []
    hashes: List[str] = []
    params = dict(base_params)
    page_token: Optional[str] = None
    for _ in range(max_pages):
        if page_token:
            params["page_token"] = page_token
        else:
            params.pop("page_token", None)
        status, body = http_get(endpoint_url, params, headers)
        if status != 200:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca API error: endpoint={endpoint_url} status={status} body={body[:500]!r}"
            )
        try:
            payload = json.loads(body.decode("utf-8"))
        except Exception as exc:  # noqa: BLE001 - re-raised as a typed, fail-closed error
            raise AlpacaHistoricalExtractionError(
                f"Alpaca API response JSON decode failed: endpoint={endpoint_url}: {exc}"
            ) from exc
        pages.append(payload)
        hashes.append(sha256_json(payload))
        page_token = payload.get("next_page_token")
        if not page_token:
            return pages, True, hashes
    raise AlpacaHistoricalExtractionError(
        f"Alpaca API pagination did not terminate within max_pages={max_pages}: endpoint={endpoint_url} "
        "-- refusing to return a possibly-incomplete result"
    )


def fetch_historical_bars(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    asof: str,
    timeframe: str = DEFAULT_TIMEFRAME,
    adjustment: str = ADJUSTMENT_ALL,
    feed: str = DEFAULT_FEED,
    credentials: Optional[AlpacaCredentials] = None,
    http_get: Optional[HttpGet] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
) -> Tuple[pd.DataFrame, Dict[str, Any]]:
    """Fetch complete, deterministically-normalized historical bars from
    Alpaca's official /v2/stocks/bars endpoint. Returns (bars_df, fetch_meta).
    bars_df has columns [symbol, end_ts (ISO-8601 UTC string), open, high,
    low, close, volume], sorted (symbol, end_ts) ascending, containing only
    rows satisfying start_utc <= end_ts < end_utc (Defect: Alpaca's `end` is
    documented INCLUSIVE; this repo's internal contract is the half-open
    [start_utc, end_utc), so a bar at or past end_utc -- or, defensively,
    before start_utc -- is normalized out locally, never entering the
    canonical dataset). `asof` (Defect 2) must be an explicit caller-resolved
    'YYYY-MM-DD' string -- this function never relies on Alpaca's implicit
    current-day asof default. Fails closed on: provider HTTP error,
    incomplete pagination, a missing 'bars' key, duplicate (symbol,end_ts)
    rows, non-finite OHLC prices, zero bars inside the canonical window, or
    any requested symbol returning zero such bars."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    if not symbols_norm:
        raise ValueError("symbols must be non-empty")
    if start_utc.tzinfo is None or end_utc.tzinfo is None:
        raise ValueError("start_utc/end_utc must be timezone-aware (UTC)")
    start_utc = start_utc.tz_convert("UTC")
    end_utc = end_utc.tz_convert("UTC")
    if end_utc <= start_utc:
        raise ValueError("end_utc must be > start_utc")
    asof = _require_resolved_asof(asof)

    creds = credentials or load_alpaca_credentials()
    getter = http_get or _default_http_get

    base_params = {
        "symbols": ",".join(symbols_norm),
        "timeframe": timeframe,
        "start": start_utc.isoformat(),
        "end": end_utc.isoformat(),
        "adjustment": adjustment,
        "asof": asof,
        "feed": feed,
        "limit": "10000",
        "sort": "asc",
    }
    endpoint_url = f"{base_url}{BARS_PATH}"
    pages, complete, page_hashes = _fetch_all_pages(
        endpoint_url=endpoint_url, base_params=base_params, credentials=creds, http_get=getter
    )

    rows: List[Dict[str, Any]] = []
    for page in pages:
        bars_by_symbol = page.get("bars")
        if bars_by_symbol is None:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca bars response missing required 'bars' key: endpoint={endpoint_url}"
            )
        for sym, bar_list in bars_by_symbol.items():
            for b in bar_list or []:
                ts = pd.Timestamp(b["t"])
                ts = ts.tz_localize("UTC") if ts.tzinfo is None else ts.tz_convert("UTC")
                rows.append(
                    {
                        "symbol": str(sym).strip().upper(),
                        "end_ts": ts,
                        "open": float(b["o"]),
                        "high": float(b["h"]),
                        "low": float(b["l"]),
                        "close": float(b["c"]),
                        "volume": float(b.get("v", 0.0)),
                    }
                )

    if not rows:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca returned zero bars for symbols={symbols_norm} window=[{start_utc.isoformat()}, "
            f"{end_utc.isoformat()})"
        )

    df = pd.DataFrame(rows)

    # Alpaca's bars `end` is a documented INCLUSIVE upper bound
    # (docs.alpaca.markets/reference/stockbars, verified 2026-08-15): a bar
    # timestamped exactly at end_utc can be returned by the provider. This
    # repo's internal research contract is the half-open [start_utc,
    # end_utc) -- normalize locally so such a bar (or, defensively, one
    # before start_utc) never enters the canonical dataset, semantic hash,
    # or economic evaluation.
    in_window = (df["end_ts"] >= start_utc) & (df["end_ts"] < end_utc)
    df = df.loc[in_window].reset_index(drop=True)
    if df.empty:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca returned bars for symbols={symbols_norm} but none fall inside the internal "
            f"half-open window [{start_utc.isoformat()}, {end_utc.isoformat()})"
        )

    for col in ("open", "high", "low", "close"):
        values = df[col].to_numpy(dtype=float)
        if not np.isfinite(values).all():
            raise AlpacaHistoricalExtractionError(f"Alpaca bars contain non-finite {col!r} values")

    dup_mask = df.duplicated(subset=["symbol", "end_ts"], keep=False)
    if bool(dup_mask.any()):
        dup_rows = df.loc[dup_mask, ["symbol", "end_ts"]].drop_duplicates().to_dict("records")
        raise AlpacaHistoricalExtractionError(f"Alpaca bars contain duplicate (symbol,end_ts) rows: {dup_rows}")

    df = df.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    df["end_ts"] = df["end_ts"].map(lambda t: t.isoformat())

    present = set(df["symbol"].unique().tolist())
    missing = sorted(set(symbols_norm) - present)
    if missing:
        raise AlpacaHistoricalExtractionError(f"Alpaca returned no bars at all for symbol(s): {missing}")

    meta: Dict[str, Any] = {
        "pagination_complete": complete,
        "raw_response_content_hashes": page_hashes,
        "resolved_feed": feed,
        "resolved_adjustment": adjustment,
        "resolved_timeframe": timeframe,
        "resolved_asof": asof,
        "requested_start_utc": start_utc.isoformat(),
        "requested_end_utc": end_utc.isoformat(),
        "returned_coverage_start_utc": df["end_ts"].min(),
        "returned_coverage_end_utc": df["end_ts"].max(),
        "symbols_requested": symbols_norm,
        "symbols_returned": sorted(present),
        "endpoint": endpoint_url,
    }
    return df, meta


def _effective_windows_for_entry(action_type: str, raw: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Flatten one raw /v1/corporate-actions entry into one normalized
    evidence dict per affected symbol leg (BKT-RESEARCH-CA-ROLE-IDENTITY-
    EVIDENCE-01). Each leg preserves enough provider-supplied information to
    distinguish EVENT TYPE from the ROLE the matched symbol plays in that
    event, and from SECURITY/ENTITY IDENTITY (CUSIPs) -- see
    _SYMBOL_FIELD_ROLE / _ROLE_CUSIP_FIELD. Fails closed on an unmapped
    action_type or an entry with no populated symbol field -- never silently
    skips an entry. Does NOT decide which events are admitted -- that
    remains classify_corporate_action_type / find_requires_review_events."""
    symbol_fields = _SYMBOL_FIELDS_BY_TYPE.get(action_type)
    if symbol_fields is None:
        raise AlpacaHistoricalExtractionError(
            f"Unmapped corporate action type from Alpaca response: {action_type!r} -- refusing to "
            "silently ignore an unrecognized event; verify KNOWN_CORPORATE_ACTION_TYPES against "
            "current Alpaca documentation"
        )
    process_date = raw.get("process_date")
    if not process_date:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca corporate action entry missing required process_date: type={action_type} raw={raw}"
        )
    ex_date = raw.get("ex_date")
    start_date = ex_date if action_type in _EX_DATE_TYPES else None
    start_date = start_date or process_date

    # Raw identity context shared verbatim across every leg of this SAME
    # event -- so a single leg's evidence (e.g. the acquirer leg) also
    # records who its counterparty was, without a downstream consumer
    # needing to re-join sibling legs by provider_event_id.
    shared_identity: Dict[str, Any] = {
        "old_symbol": _norm_symbol_or_none(raw.get("old_symbol")),
        "new_symbol": _norm_symbol_or_none(raw.get("new_symbol")),
        "old_cusip": raw.get("old_cusip"),
        "new_cusip": raw.get("new_cusip"),
        "acquirer_symbol": _norm_symbol_or_none(raw.get("acquirer_symbol")),
        "acquiree_symbol": _norm_symbol_or_none(raw.get("acquiree_symbol")),
        "acquirer_cusip": raw.get("acquirer_cusip"),
        "acquiree_cusip": raw.get("acquiree_cusip"),
        "source_symbol": _norm_symbol_or_none(raw.get("source_symbol")),
        "alternate_symbol": _norm_symbol_or_none(raw.get("alternate_symbol")),
    }

    out: List[Dict[str, Any]] = []
    for field in symbol_fields:
        sym = raw.get(field)
        if not sym:
            continue
        role = _SYMBOL_FIELD_ROLE.get(field, field)
        cusip_field = _ROLE_CUSIP_FIELD.get(role)
        matched_symbol = str(sym).strip().upper()
        leg: Dict[str, Any] = {
            "symbol": matched_symbol,
            "action_type": action_type,
            "effective_start_ts": str(start_date),
            "effective_end_ts": str(process_date),
            "provider_event_id": raw.get("id"),
            "process_date": str(process_date),
            "ex_date": str(ex_date) if ex_date else None,
            "matched_symbol": matched_symbol,
            "matched_role": role,
            "matched_symbol_field": field,
            "matched_cusip": raw.get(cusip_field) if cusip_field else None,
        }
        leg.update(shared_identity)
        out.append(leg)
    if not out:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca corporate action entry has no populated symbol field(s): type={action_type} "
            f"expected_fields={symbol_fields} raw={raw}"
        )
    return out


def fetch_corporate_actions(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    types: Optional[Sequence[str]] = None,
    credentials: Optional[AlpacaCredentials] = None,
    http_get: Optional[HttpGet] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    """Fetch complete corporate-action evidence from Alpaca's official
    /v1/corporate-actions endpoint for the given symbol universe and
    [start_utc, end_utc] PROCESS-DATE query window (Alpaca filters/sorts
    this endpoint's start/end by process_date, not ex_date/effective_date --
    see module header Defect 1; callers wanting complete effective-date
    coverage must pass a process-date window proven broad enough, then
    filter locally with _filter_entries_intersecting_range -- this function
    itself is a pure, literal pass-through of whatever window it is given).
    Returns (entries, fetch_meta): entries is a flat, normalized list of
    {"symbol","action_type","effective_start_ts","effective_end_ts"} dicts
    merged across every corporate_actions.<type> response bucket and every
    page (see _effective_windows_for_entry). Fails closed on: provider HTTP
    error, incomplete pagination, a missing 'corporate_actions' key, or an
    unknown requested type."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    if not symbols_norm:
        raise ValueError("symbols must be non-empty")
    if start_utc.tzinfo is None or end_utc.tzinfo is None:
        raise ValueError("start_utc/end_utc must be timezone-aware (UTC)")
    start_utc = start_utc.tz_convert("UTC")
    end_utc = end_utc.tz_convert("UTC")

    creds = credentials or load_alpaca_credentials()
    getter = http_get or _default_http_get

    requested_types = sorted({t.strip() for t in (types or KNOWN_CORPORATE_ACTION_TYPES) if t.strip()})
    unknown_types = sorted(set(requested_types) - KNOWN_CORPORATE_ACTION_TYPES)
    if unknown_types:
        raise ValueError(f"Unknown corporate action type(s) requested: {unknown_types}")

    base_params = {
        "symbols": ",".join(symbols_norm),
        "types": ",".join(requested_types),
        "start": start_utc.date().isoformat(),
        "end": end_utc.date().isoformat(),
        "region": "us",
        "limit": "1000",
        "data_quality": "complete",
        "sort": "asc",
    }
    endpoint_url = f"{base_url}{CORPORATE_ACTIONS_PATH}"
    pages, complete, page_hashes = _fetch_all_pages(
        endpoint_url=endpoint_url, base_params=base_params, credentials=creds, http_get=getter
    )

    entries: List[Dict[str, Any]] = []
    for page in pages:
        ca = page.get("corporate_actions")
        if ca is None:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca corporate-actions response missing required 'corporate_actions' key: "
                f"endpoint={endpoint_url}"
            )
        unmapped_keys = sorted(set(ca.keys()) - set(_RESPONSE_KEY_TO_TYPE.keys()))
        if unmapped_keys:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca corporate-actions response contains unmapped bucket(s) not in this module's "
                f"verified vocabulary: {unmapped_keys} -- refusing to silently ignore a possibly-new "
                "corporate-action type; update KNOWN_CORPORATE_ACTION_TYPES/_RESPONSE_KEY_TO_TYPE/"
                "_SYMBOL_FIELDS_BY_TYPE against current Alpaca documentation first"
            )
        for response_key, action_type in _RESPONSE_KEY_TO_TYPE.items():
            for raw in ca.get(response_key) or []:
                entries.extend(_effective_windows_for_entry(action_type, raw))

    meta: Dict[str, Any] = {
        "pagination_complete": complete,
        "raw_response_content_hashes": page_hashes,
        "requested_start_utc": start_utc.isoformat(),
        "requested_end_utc": end_utc.isoformat(),
        "requested_types": requested_types,
        "symbols_requested": symbols_norm,
        "endpoint": endpoint_url,
    }
    return entries, meta


def classify_corporate_action_type(action_type: str, *, adjustment_mode: str = ADJUSTMENT_ALL) -> str:
    """Pure classification, no IO. Returns CATEGORY_COVERED_BY_ADJUSTMENT
    only for the docs-verified subset of types covered by the given
    adjustment mode; everything else (including every type this module does
    not have a verified mapping for) is CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW."""
    if adjustment_mode != ADJUSTMENT_ALL:
        raise ValueError(
            f"classify_corporate_action_type only has a verified mapping for adjustment_mode="
            f"{ADJUSTMENT_ALL!r}, got {adjustment_mode!r}"
        )
    if action_type not in KNOWN_CORPORATE_ACTION_TYPES:
        raise ValueError(f"Unknown corporate action type: {action_type!r}")
    return CATEGORY_COVERED_BY_ADJUSTMENT if action_type in _COVERED_BY_ADJUSTMENT_ALL else CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def _filter_entries_intersecting_range(
    entries: Sequence[Dict[str, Any]],
    *,
    symbol_universe: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
) -> List[Dict[str, Any]]:
    """Return the subset of `entries` that affect a symbol in
    `symbol_universe` and whose [effective_start_ts, effective_end_ts]
    window intersects [start_utc, end_utc) -- deterministic, order-
    independent (sorted by symbol/action_type/effective_start_ts). Operates
    on EFFECTIVE (ex_date-based) windows regardless of what process-date
    query window the entries were originally discovered under (see Defect 1
    / CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC)."""
    universe = {s.strip().upper() for s in symbol_universe}
    start_bound = start_utc.tz_convert("UTC")
    end_bound = end_utc.tz_convert("UTC")
    hits: List[Dict[str, Any]] = []
    for e in entries:
        if e["symbol"] not in universe:
            continue
        e_start = pd.Timestamp(e["effective_start_ts"])
        e_start = e_start.tz_localize("UTC") if e_start.tzinfo is None else e_start.tz_convert("UTC")
        e_end = pd.Timestamp(e["effective_end_ts"])
        e_end = e_end.tz_localize("UTC") if e_end.tzinfo is None else e_end.tz_convert("UTC")
        if e_end >= start_bound and e_start < end_bound:
            hits.append(e)
    return sorted(hits, key=lambda e: (e["symbol"], e["action_type"], e["effective_start_ts"]))


def _event_group_key(leg: Dict[str, Any]) -> Tuple[str, ...]:
    """Group key identifying legs that came from the SAME raw provider
    event -- reads MERGER_ACQUIRER_RULE.same_event_grouping_identity (a bare
    module global, see the comment above MergerAcquirerRule) rather than a
    second hard-coded copy of the field list, so a test mutating the
    authoritative grouping fields observes a changed grouping result here,
    not just a changed policy fingerprint. provider_event_id alone is not
    guaranteed globally unique across different action types, so the
    default grouping identity also includes action_type."""
    return tuple(str(leg.get(field)) for field in MERGER_ACQUIRER_RULE.same_event_grouping_identity)


def classify_corporate_action_resolution(
    leg: Dict[str, Any],
    *,
    sibling_legs: Sequence[Dict[str, Any]] = (),
    adjustment_mode: str = ADJUSTMENT_ALL,
    reviewed_resolutions: Sequence[Dict[str, Any]] = REVIEWED_CA_RESOLUTIONS,
) -> str:
    """Pure, per-leg role-aware resolution (BKT-RESEARCH-CA-ROLE-AWARE-
    RESOLUTION-01, extended by BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-
    RESOLUTION-01). `leg` is one normalized evidence dict as produced by
    _effective_windows_for_entry (must carry action_type/matched_role/
    old_cusip/new_cusip/process_date). `sibling_legs` are the OTHER legs of
    the SAME raw event (same provider_event_id + action_type, see
    _event_group_key) -- used only for the merger acquirer contract's
    same-event terminal-role self-consistency check (a requested symbol
    must not be simultaneously represented by BOTH an acquirer and an
    acquiree leg of the SAME event).

    Resolution order:
      1. a type already CATEGORY_COVERED_BY_ADJUSTMENT stays that;
      2. the two automated, narrow role/identity contracts (merger
         acquirer / name-change CUSIP continuity) apply where they hold;
      3. otherwise, ca_reviewed_resolutions.find_reviewed_resolution is
         consulted against `reviewed_resolutions` (default:
         REVIEWED_CA_RESOLUTIONS) for an EXACT match on the provider's own
         event id, action_type, requested_symbol, requested_role, the
         matched symbol FIELD, process_date, and every observed CUSIP (see
         ca_reviewed_resolutions.event_fingerprint) -- an individually-
         reviewed exception (see PATCH C / REPAIR-01 module header), never a
         type-wide bypass;
      4. anything still unresolved (including a blank/unmapped
         matched_role, an acquiree leg, an under-evidenced or mismatched
         name-change, or any other type) is
         CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW."""
    action_type = leg.get("action_type")
    base = classify_corporate_action_type(action_type, adjustment_mode=adjustment_mode)
    if base == CATEGORY_COVERED_BY_ADJUSTMENT:
        return CATEGORY_COVERED_BY_ADJUSTMENT

    role = leg.get("matched_role")
    matched_symbol = leg.get("matched_symbol")
    resolution = CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW

    # BKT-RESEARCH-CA-POLICY-SINGLE-SOURCE-OF-TRUTH-01: every decision-
    # bearing literal below is read from MERGER_ACQUIRER_RULE/NAME_CHANGE_
    # RULE (bare module globals, see the comment above MergerAcquirerRule) --
    # never a locally hard-coded copy -- so these are the SAME values
    # _canonical_ca_resolution_policy_spec hashes into resolution_policy_
    # fingerprint.
    if action_type in MERGER_ACQUIRER_RULE.action_types and role == MERGER_ACQUIRER_RULE.required_role:
        same_symbol_terminal_conflict = MERGER_ACQUIRER_RULE.reject_same_symbol_terminal_conflict and any(
            sib.get("matched_role") == MERGER_ACQUIRER_RULE.terminal_conflict_role
            and sib.get("matched_symbol") == matched_symbol
            for sib in sibling_legs
        )
        if not same_symbol_terminal_conflict:
            resolution = CATEGORY_NO_ADJUSTMENT_REQUIRED_FOR_LEG

    elif action_type == NAME_CHANGE_RULE.action_type and role in NAME_CHANGE_RULE.allowed_roles:
        old_cusip = leg.get("old_cusip")
        new_cusip = leg.get("new_cusip")
        old_ok = (not NAME_CHANGE_RULE.require_old_cusip_populated) or bool(old_cusip)
        new_ok = (not NAME_CHANGE_RULE.require_new_cusip_populated) or bool(new_cusip)
        equality_ok = (not NAME_CHANGE_RULE.require_cusip_equality) or (old_cusip == new_cusip)
        if old_ok and new_ok and equality_ok:
            resolution = CATEGORY_VERIFIED_SAME_SECURITY_CONTINUITY

    if resolution != CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW:
        return resolution

    reviewed = find_reviewed_resolution(
        source_provider_id="alpaca",
        provider_event_id=str(leg.get("provider_event_id")) if leg.get("provider_event_id") else "",
        action_type=str(action_type) if action_type else "",
        requested_symbol=str(matched_symbol) if matched_symbol else "",
        requested_role=str(role) if role else "",
        matched_symbol_field=str(leg.get("matched_symbol_field")) if leg.get("matched_symbol_field") else "",
        process_date=str(leg.get("process_date")) if leg.get("process_date") else "",
        matched_cusip=leg.get("matched_cusip"),
        acquirer_cusip=leg.get("acquirer_cusip"),
        acquiree_cusip=leg.get("acquiree_cusip"),
        registry=reviewed_resolutions,
    )
    if reviewed is not None:
        return reviewed["resolution"]

    return CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def find_requires_review_events(
    entries: Sequence[Dict[str, Any]],
    *,
    symbol_universe: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    adjustment_mode: str = ADJUSTMENT_ALL,
) -> List[Dict[str, Any]]:
    """Return the subset of `entries` that remain
    CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW after role-aware resolution (see
    classify_corporate_action_resolution) and intersect [start_utc, end_utc)
    for a symbol in `symbol_universe` (see _filter_entries_intersecting_range)
    -- deterministic, order-independent."""
    intersecting = _filter_entries_intersecting_range(
        entries, symbol_universe=symbol_universe, start_utc=start_utc, end_utc=end_utc
    )
    groups: Dict[Tuple[Any, str], List[Dict[str, Any]]] = {}
    for leg in intersecting:
        groups.setdefault(_event_group_key(leg), []).append(leg)

    hits: List[Dict[str, Any]] = []
    for leg in intersecting:
        siblings = [s for s in groups[_event_group_key(leg)] if s is not leg]
        resolution = classify_corporate_action_resolution(
            leg, sibling_legs=siblings, adjustment_mode=adjustment_mode
        )
        if resolution == CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW:
            hits.append(leg)
    return sorted(hits, key=lambda e: (e["symbol"], e["action_type"], e["effective_start_ts"]))


def _utc_now() -> pd.Timestamp:
    """Live wall-clock UTC seam, isolated to a single module-level function
    (REPAIR-03 Defect 2) so a test can monkeypatch this one symbol to make
    the OFFICIAL extraction path's snapshot clock deterministic, without the
    public official signature accepting a caller-selectable override."""
    return pd.Timestamp.now(tz="UTC")


def _resolve_ca_discovery_cutoff_utc(ca_discovery_cutoff_utc: Optional[str]) -> pd.Timestamp:
    """Resolve the CA discovery snapshot cutoff -- the UTC extraction/
    retrieval instant through which /v1/corporate-actions process_date
    evidence is queried (REPAIR-02). Deliberately NOT derived from bars
    symbol-mapping `asof` or the research window's end_utc -- see module
    header REPAIR-02 section. Resolved from the caller-supplied
    `ca_discovery_cutoff_utc` when given (the determinism seam: passing the
    same value across two extractions of identical underlying provider
    content must yield an identical attestation) -- REPAIR-03: only the
    DIAGNOSTIC wrapper ever supplies this; else real wall-clock UTC now (see
    _utc_now) for a live extraction."""
    ts = pd.Timestamp(ca_discovery_cutoff_utc) if ca_discovery_cutoff_utc else _utc_now()
    return ts.tz_localize("UTC") if ts.tzinfo is None else ts.tz_convert("UTC")


def _ca_discovery_process_date_bounds(
    *, ca_discovery_cutoff_utc: pd.Timestamp
) -> Tuple[pd.Timestamp, pd.Timestamp]:
    """The PROCESS-DATE query window (Defect 1, corrected by REPAIR-02)
    proven broad enough to discover every corporate action whose EFFECTIVE
    (ex_date-based) window could intersect the research interval: floor is
    CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC (no documented process_date/ex_date
    bound exists -- see module header), ceiling is the resolved CA discovery
    snapshot cutoff (see _resolve_ca_discovery_cutoff_utc) -- NOT max(asof,
    research_end); REPAIR-01's ceiling formula wrongly coupled corporate-
    action evidence vintage to the bars entity/symbol-mapping `asof`, which
    could miss a real event whose process_date lands after both asof and
    research_end (see module header REPAIR-02 section)."""
    return CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC, ca_discovery_cutoff_utc


def _neutral_extract(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    asof: str,
    timeframe: str,
    feed: str,
    credentials: Optional[AlpacaCredentials],
    http_get: HttpGet,
    base_url: str,
    ca_discovery_cutoff_utc: pd.Timestamp,
) -> Dict[str, Any]:
    """AUTHORITY-NEUTRAL shared extraction core (REPAIR-03 Defect 1): bars
    retrieval, CA retrieval/filtering, fail-closed review-required check, and
    evidence/hash/coverage construction. Deliberately accepts NO
    extractor_id/source_authority parameter -- those are official-vs-
    diagnostic identity, minted only by _mint_manifest under a hard-coded
    value chosen by whichever public wrapper below calls it, never threaded
    through here. `ca_discovery_cutoff_utc` must already be resolved (REPAIR-
    02: resolved exactly once by the caller, before this runs) -- this
    function never touches the clock itself. Not part of this module's
    public surface."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    asof = _require_resolved_asof(asof)
    retrieval_ts = ca_discovery_cutoff_utc.isoformat()

    bars_df, bars_meta = fetch_historical_bars(
        symbols=symbols_norm,
        start_utc=start_utc,
        end_utc=end_utc,
        asof=asof,
        timeframe=timeframe,
        adjustment=ADJUSTMENT_ALL,
        feed=feed,
        credentials=credentials,
        http_get=http_get,
        base_url=base_url,
    )

    ca_discovery_start_utc, ca_discovery_end_utc = _ca_discovery_process_date_bounds(
        ca_discovery_cutoff_utc=ca_discovery_cutoff_utc
    )
    ca_entries_discovered, ca_meta = fetch_corporate_actions(
        symbols=symbols_norm,
        start_utc=ca_discovery_start_utc,
        end_utc=ca_discovery_end_utc,
        credentials=credentials,
        http_get=http_get,
        base_url=base_url,
    )
    ca_entries = _filter_entries_intersecting_range(
        ca_entries_discovered, symbol_universe=symbols_norm, start_utc=start_utc, end_utc=end_utc
    )

    review_required = find_requires_review_events(
        ca_entries,
        symbol_universe=symbols_norm,
        start_utc=start_utc,
        end_utc=end_utc,
        adjustment_mode=ADJUSTMENT_ALL,
    )
    if review_required:
        raise CorporateActionReviewRequired(
            "Fail-closed: the following corporate action(s) intersect the requested symbol/range, are "
            f"NOT covered by Alpaca's adjustment={ADJUSTMENT_ALL!r} semantics, and could not be "
            "role-aware-resolved (see classify_corporate_action_resolution) -- refusing to produce "
            "official research bars provenance until an explicit handling policy or reviewed "
            "resolution exists for them. Narrow the symbol universe or date range to exclude the "
            f"affected symbol/period, or wait for a future patch that adds explicit handling: "
            f"{review_required}"
        )

    evidence = build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=symbols_norm,
        coverage_start_utc=start_utc.tz_convert("UTC").isoformat(),
        coverage_end_utc=end_utc.tz_convert("UTC").isoformat(),
        # BKT-RESEARCH-CA-ROLE-IDENTITY-EVIDENCE-01: pass every leg's full
        # role/identity evidence through verbatim -- previously narrowed to
        # {symbol, action_type, effective_start_ts, effective_end_ts} here,
        # which discarded exactly the role/CUSIP evidence
        # _effective_windows_for_entry now preserves.
        corporate_action_entries=[dict(e) for e in ca_entries],
    )

    corporate_action_query_coverage = {
        # REPAIR-02: these are the resolved CA discovery snapshot bounds (see
        # _ca_discovery_process_date_bounds / _resolve_ca_discovery_cutoff_utc),
        # never derived from `asof`. REPAIR-03: ca_discovery_end_utc is the
        # exact wall-clock snapshot cutoff -- audit-only, deliberately
        # excluded from bars_provenance.canonical_source_attestation_content's
        # semantic hash (see that function's corporate_action_query_coverage
        # handling) so it cannot manufacture a new trial identity by itself.
        "discovery_protocol": CA_DISCOVERY_PROTOCOL_V2,
        "ca_discovery_start_utc": ca_meta["requested_start_utc"],
        "ca_discovery_end_utc": ca_meta["requested_end_utc"],
        "research_window_start_utc": start_utc.tz_convert("UTC").isoformat(),
        "research_window_end_utc": end_utc.tz_convert("UTC").isoformat(),
        "requested_types": ca_meta["requested_types"],
        "symbols_requested": ca_meta["symbols_requested"],
    }

    return {
        "bars_df": bars_df,
        "bars_meta": bars_meta,
        "ca_meta": ca_meta,
        "ca_entries": ca_entries,
        "evidence": evidence,
        "bars_hash": canonical_semantic_bars_hash(bars_df),
        "retrieval_ts": retrieval_ts,
        "corporate_action_query_coverage": corporate_action_query_coverage,
        "symbols_norm": symbols_norm,
        "timeframe": timeframe,
        "feed": feed,
        "start_utc": start_utc,
        "end_utc": end_utc,
    }


def _mint_manifest(neutral: Dict[str, Any], *, extractor_id: str, source_authority: str) -> Dict[str, Any]:
    """Mint the OFFICIAL or DIAGNOSTIC source attestation + bars provenance
    manifest from a _neutral_extract result (REPAIR-03 Defect 1):
    `extractor_id`/`source_authority` are supplied here, by the caller-facing
    wrapper, and are never derived from whatever transport produced
    `neutral` -- an injected diagnostic transport can never produce an
    OFFICIAL attestation by construction, regardless of what URL it points
    at."""
    bars_meta = neutral["bars_meta"]
    ca_meta = neutral["ca_meta"]
    evidence = neutral["evidence"]

    attestation = build_source_attestation(
        source_provider_id="alpaca",
        extractor_id=extractor_id,
        source_authority=source_authority,
        api_endpoint_bars=bars_meta["endpoint"],
        api_endpoint_corporate_actions=ca_meta["endpoint"],
        symbols=neutral["symbols_norm"],
        requested_start_utc=bars_meta["requested_start_utc"],
        requested_end_utc=bars_meta["requested_end_utc"],
        returned_coverage_start_utc=bars_meta["returned_coverage_start_utc"],
        returned_coverage_end_utc=bars_meta["returned_coverage_end_utc"],
        adjustment_mode=ADJUSTMENT_ALL,
        feed=neutral["feed"],
        asof=bars_meta["resolved_asof"],
        pagination_complete_bars=bars_meta["pagination_complete"],
        pagination_complete_corporate_actions=ca_meta["pagination_complete"],
        corporate_action_query_coverage=neutral["corporate_action_query_coverage"],
        category_b_events_found=[],
        raw_response_content_hashes={
            "bars_pages": bars_meta["raw_response_content_hashes"],
            "corporate_action_pages": ca_meta["raw_response_content_hashes"],
        },
        canonical_semantic_bars_hash=neutral["bars_hash"],
        canonical_corporate_action_evidence_hash=corporate_action_evidence_id(evidence),
        retrieval_timestamp_utc=neutral["retrieval_ts"],
        # BKT-RESEARCH-CA-AUTHORITY-IDENTITY-V2-01: binds the CA resolution
        # policy actually in force (role-aware rules + reviewed-resolution
        # registry) into source identity -- see resolution_policy_fingerprint
        # and bars_provenance.canonical_source_attestation_content. Applies
        # to BOTH the official and diagnostic wrappers (both run through
        # this same _mint_manifest); only the extractor_id gate in
        # canonical_source_attestation_content decides whether it actually
        # participates in a given attestation's identity.
        ca_resolution_policy_id=resolution_policy_fingerprint(),
    )

    price_provenance = {
        "close_column": "close",
        "provider_metadata_available": True,
        "provider_ids_observed": ["alpaca"],
        "price_adjustment_convention": PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
        "convention_basis": (
            "mqk_research.data.alpaca_historical requested adjustment=all directly from Alpaca's "
            "official /v2/stocks/bars endpoint (verified against docs.alpaca.markets to apply "
            "split+dividend+spin-off price/volume adjustment); see the manifest's source_attestation "
            "for full retrieval evidence."
        ),
    }

    manifest = build_bars_provenance_manifest(
        price_provenance=price_provenance,
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe=neutral["timeframe"],
        start_utc=neutral["start_utc"].tz_convert("UTC").isoformat(),
        end_utc=neutral["end_utc"].tz_convert("UTC").isoformat(),
        symbol_universe=neutral["symbols_norm"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=neutral["bars_df"],
        source_attestation=attestation,
    )

    return {
        "bars": neutral["bars_df"],
        "manifest": manifest,
        "corporate_action_evidence": evidence,
        "corporate_action_entries": neutral["ca_entries"],
    }


def extract_research_bars_with_provenance(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    asof: str,
    timeframe: str = DEFAULT_TIMEFRAME,
    feed: str = DEFAULT_FEED,
    credentials: Optional[AlpacaCredentials] = None,
) -> Dict[str, Any]:
    """OFFICIAL, narrow entry point for fixed_ex_ante US equity/ETF research
    extraction (Defect 3, hardened by REPAIR-03 Defect 1): fixed official
    Alpaca base URL, real HTTP transport -- no injectable transport/base_url,
    extractor_id, source_authority, or CA-discovery snapshot clock override
    (REPAIR-03 Defect 2: the snapshot cutoff is resolved exactly once from
    real wall-clock UTC now, via the monkeypatchable _utc_now seam -- an
    ordinary caller can never claim an arbitrary stale/future OFFICIAL CA
    discovery snapshot). Fetches adjustment=all bars AND corporate-actions
    for the SAME symbol/range from Alpaca (CA discovery is proven-superset,
    not window-bounded -- see Defect 1 / _ca_discovery_process_date_bounds),
    requires an explicit `asof` (Defect 2 -- never Alpaca's implicit
    current-day default), fails closed (CorporateActionReviewRequired) if
    any REQUIRES_FAIL_CLOSED_REVIEW event intersects the request, and
    otherwise returns a fully-formed, OFFICIAL-authority source-attested
    bars provenance manifest ready to pass directly as `bars_provenance` to
    mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
    -- no manual manifest fabrication required. Returns {"bars": DataFrame,
    "manifest": {...}, "corporate_action_evidence": {...},
    "corporate_action_entries": [...]}.

    For deterministic, offline unit testing with an injected transport, use
    extract_research_bars_with_provenance_diagnostic instead -- that path
    can never mint an attestation this function's OFFICIAL authority
    satisfies (see bars_provenance.SOURCE_AUTHORITY_OFFICIAL_PROVIDER)."""
    ca_discovery_cutoff_utc = _resolve_ca_discovery_cutoff_utc(None)
    neutral = _neutral_extract(
        symbols=symbols,
        start_utc=start_utc,
        end_utc=end_utc,
        asof=asof,
        timeframe=timeframe,
        feed=feed,
        credentials=credentials,
        http_get=_default_http_get,
        base_url=ALPACA_DATA_BASE_URL,
        ca_discovery_cutoff_utc=ca_discovery_cutoff_utc,
    )
    return _mint_manifest(neutral, extractor_id=EXTRACTOR_ID, source_authority=SOURCE_AUTHORITY_OFFICIAL_PROVIDER)


def extract_research_bars_with_provenance_diagnostic(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    asof: str,
    http_get: HttpGet,
    timeframe: str = DEFAULT_TIMEFRAME,
    feed: str = DEFAULT_FEED,
    credentials: Optional[AlpacaCredentials] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
    ca_discovery_cutoff_utc: Optional[str] = None,
) -> Dict[str, Any]:
    """INTERNAL/DIAGNOSTIC extraction (Defect 3): the injectable-transport
    testability seam for deterministic, offline unit tests (`http_get` is
    REQUIRED -- no default -- forcing the caller to supply an explicit fake;
    normal test suites never touch the network). Same extraction/validation
    logic as extract_research_bars_with_provenance, but ALWAYS mints
    DIAGNOSTIC_EXTRACTOR_ID + bars_provenance.SOURCE_AUTHORITY_DIAGNOSTIC_SYNTHETIC
    -- neither is caller-overridable here, so an attestation produced by this
    function can never satisfy bars_provenance._require_verified_source_attestation
    (see check_corporate_action_integrity's adjusted_data branch) and can
    never authorize OFFICIAL registered economic evaluation, regardless of
    how realistic the injected transport/base_url/response content is.
    `ca_discovery_cutoff_utc` (REPAIR-03: renamed from retrieval_timestamp_utc
    -- it names what it actually is, the CA discovery snapshot cutoff, not a
    generic retrieval timestamp) MAY be pinned explicitly so a test can model
    a historical snapshot deterministically; omitted, it resolves to real
    wall-clock UTC now, same as the official path."""
    resolved_cutoff = _resolve_ca_discovery_cutoff_utc(ca_discovery_cutoff_utc)
    neutral = _neutral_extract(
        symbols=symbols,
        start_utc=start_utc,
        end_utc=end_utc,
        asof=asof,
        timeframe=timeframe,
        feed=feed,
        credentials=credentials,
        http_get=http_get,
        base_url=base_url,
        ca_discovery_cutoff_utc=resolved_cutoff,
    )
    return _mint_manifest(neutral, extractor_id=DIAGNOSTIC_EXTRACTOR_ID, source_authority=SOURCE_AUTHORITY_DIAGNOSTIC_SYNTHETIC)


def write_research_extraction_artifacts(run_dir: Path, result: Dict[str, Any]) -> Dict[str, Path]:
    """Write the deterministic durable artifacts for one extraction result
    (see extract_research_bars_with_provenance) into `run_dir`:
    research_bars.csv, research_bars_provenance.json, corporate_actions.json,
    corporate_actions_provenance.json. The manifest's `artifact_sha256`/
    `row_count` (PHYSICAL file facts) are filled in from the file actually
    written here -- distinct from `canonical_semantic_bars_hash` (the
    CANONICAL SEMANTIC identity, already fixed at extraction time and never
    changed by this function; see bars_provenance.canonical_semantic_bars_hash)."""
    run_dir = Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)

    bars_path = run_dir / "research_bars.csv"
    manifest_path = run_dir / "research_bars_provenance.json"
    ca_entries_path = run_dir / "corporate_actions.json"
    ca_provenance_path = run_dir / "corporate_actions_provenance.json"

    bars_out = result["bars"].sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    bars_out.to_csv(bars_path, index=False, lineterminator="\n")

    manifest = dict(result["manifest"])
    manifest["artifact_sha256"] = sha256_file(bars_path)
    manifest["row_count"] = int(len(bars_out))
    manifest_path.write_text(
        json.dumps(manifest, sort_keys=True, separators=(",", ":")), encoding="utf-8"
    )

    ca_entries_path.write_text(
        json.dumps(
            {"corporate_action_entries": result["corporate_action_entries"]},
            sort_keys=True,
            separators=(",", ":"),
        ),
        encoding="utf-8",
    )
    ca_provenance_path.write_text(
        json.dumps(result["corporate_action_evidence"], sort_keys=True, separators=(",", ":")),
        encoding="utf-8",
    )

    return {
        "bars_csv": bars_path,
        "bars_provenance_json": manifest_path,
        "corporate_actions_json": ca_entries_path,
        "corporate_actions_provenance_json": ca_provenance_path,
    }
