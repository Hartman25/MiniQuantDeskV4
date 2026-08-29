from __future__ import annotations

from typing import Any, Dict, Optional, Sequence, Tuple

from mqk_research.ml.util_hash import sha256_json

# BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01
# (repaired by BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01-REPAIR-01)
#
# A narrow, explicit, auditable registry of individually-reviewed
# corporate-action resolutions for events that
# BKT-RESEARCH-CA-ROLE-AWARE-RESOLUTION-01's automated role/identity rules
# (alpaca_historical.classify_corporate_action_resolution) cannot honestly
# resolve on their own -- e.g. a holding-company reorganization where the
# SAME ticker continues trading uninterrupted but the legal entity (and
# therefore CUSIP) changes, so the name-change same-CUSIP continuity check
# correctly refuses to auto-clear it, yet primary-source evidence (an SEC
# filing) proves the economic security is genuinely continuous.
#
# This module deliberately contains NO symbol-specific Python branching
# (no `if symbol == "DKNG"`): every reviewed resolution is a plain data
# record in REVIEWED_CA_RESOLUTIONS below, matched generically by
# find_reviewed_resolution against a leg's own identity evidence. A record's
# own event_fingerprint is independently RECOMPUTED from that evidence and
# required to match before the record is ever trusted (see
# require_verified_reviewed_resolution) -- mirrors bars_provenance's evidence_id /
# attestation_id pattern (content-derived, never a caller/author-asserted
# label): a hand-edited record whose event_fingerprint no longer matches
# its own bound fields is refused, never silently accepted.
#
# REPAIR-01: the original patch deliberately bound only five fields
# (provider, action_type, requested_symbol, requested_role, process_date)
# and rejected binding the provider's own opaque event id, reasoning that an
# SEC filing does not report Alpaca's internal event id. That is still true,
# but it conflated "SEC evidence can't corroborate provider_event_id" with
# "provider_event_id shouldn't be part of the fingerprint at all" -- the two
# authorities are complementary, not substitutes: SEC evidence establishes
# ECONOMIC CONTINUITY; the provider's own event id (plus action_type, the
# matched symbol FIELD -- not just the resulting role string -- and the
# security-identity CUSIPs actually observed) identifies WHICH provider
# record a review applies to. Binding only the weaker five fields meant a
# genuinely DIFFERENT live provider event (different id, different declared
# action_type, different matched field, different CUSIPs) could still
# collide with the SAME five-field fingerprint as a reviewed record authored
# for an entirely different event shape. The fingerprint now binds ALL of:
# source_provider_id, provider_event_id, action_type, requested_symbol,
# requested_role, matched_symbol_field, process_date, matched_cusip,
# acquirer_cusip, acquiree_cusip -- so a later provider revision that
# changes any one of these identity-bearing fields fails closed and
# requires fresh review, by design.

RESOLUTION_SCHEMA_VERSION = "reviewed_ca_resolution_v2"

# Intentionally the ONLY resolution semantics this registry supports today.
# Adding a new one is an explicit, auditable code change (CLAUDE.md #6/#20),
# never an implicit broadening of what a record author may assert.
RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY = (
    "verified_one_for_one_successor_security_continuity"
)

_KNOWN_RESOLUTIONS: frozenset = frozenset({RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY})


class ReviewedResolutionUnverifiable(RuntimeError):
    """Fail-closed: raised when a reviewed-resolution record's declared
    event_fingerprint does not match the content independently recomputed
    from its own bound identity fields, its declared resolution_id does not
    match the content independently recomputed from its own full content, its
    resolution is not a known semantics, or its lookup matched more than one
    registry record -- a hand-edited/tampered/malformed/ambiguous record is
    never trusted (see require_verified_reviewed_resolution / find_reviewed_resolution)."""


def _norm_optional_cusip(value: Any) -> Optional[str]:
    """Normalize an optional CUSIP-like identity field: stripped when
    populated, else None -- never an empty string or a fabricated default."""
    if not value:
        return None
    return str(value).strip()


def _canonical_fingerprint_content(
    *,
    source_provider_id: str,
    provider_event_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    matched_symbol_field: str,
    process_date: str,
    matched_cusip: Optional[str] = None,
    acquirer_cusip: Optional[str] = None,
    acquiree_cusip: Optional[str] = None,
) -> Dict[str, Any]:
    """The exact fields a reviewed resolution binds on (REPAIR-01), normalized
    so two logically-identical bindings always hash identically."""
    return {
        "source_provider_id": str(source_provider_id or "").strip().lower(),
        "provider_event_id": str(provider_event_id or "").strip(),
        "action_type": str(action_type or "").strip(),
        "requested_symbol": str(requested_symbol or "").strip().upper(),
        "requested_role": str(requested_role or "").strip(),
        "matched_symbol_field": str(matched_symbol_field or "").strip(),
        "process_date": str(process_date or "").strip(),
        "matched_cusip": _norm_optional_cusip(matched_cusip),
        "acquirer_cusip": _norm_optional_cusip(acquirer_cusip),
        "acquiree_cusip": _norm_optional_cusip(acquiree_cusip),
    }


def event_fingerprint(
    *,
    source_provider_id: str,
    provider_event_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    matched_symbol_field: str,
    process_date: str,
    matched_cusip: Optional[str] = None,
    acquirer_cusip: Optional[str] = None,
    acquiree_cusip: Optional[str] = None,
) -> str:
    """Deterministic, content-DERIVED event fingerprint -- the only
    legitimate way a reviewed resolution record's event_fingerprint may
    arise (REPAIR-01). Binds the provider's own event id, action type,
    requested symbol, requested role, the matched symbol FIELD, process/
    effective date, and every observed security-identity CUSIP -- changing
    any one of these changes the fingerprint, so a mutated query or a later
    provider revision of the SAME nominal event never matches a record
    reviewed for a different event shape."""
    return sha256_json(
        _canonical_fingerprint_content(
            source_provider_id=source_provider_id,
            provider_event_id=provider_event_id,
            action_type=action_type,
            requested_symbol=requested_symbol,
            requested_role=requested_role,
            matched_symbol_field=matched_symbol_field,
            process_date=process_date,
            matched_cusip=matched_cusip,
            acquirer_cusip=acquirer_cusip,
            acquiree_cusip=acquiree_cusip,
        )
    )


def _canonical_resolution_content_for_id(
    *,
    schema_version: str,
    fingerprint_fields: Dict[str, Any],
    fingerprint: str,
    resolution: str,
    evidence_summary: str,
    primary_source_references: Sequence[str],
) -> Dict[str, Any]:
    """The full canonical reviewed-resolution content used to derive
    resolution_id -- everything EXCEPT resolution_id itself (REPAIR-01 E3).
    Shared by build_reviewed_resolution (mint) and
    require_verified_reviewed_resolution (independent recompute) so the two
    can never silently diverge in what they hash."""
    return {
        "schema_version": schema_version,
        **fingerprint_fields,
        "event_fingerprint": fingerprint,
        "resolution": resolution,
        "evidence_summary": str(evidence_summary),
        "primary_source_references": list(primary_source_references),
    }


def build_reviewed_resolution(
    *,
    source_provider_id: str,
    provider_event_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    matched_symbol_field: str,
    process_date: str,
    resolution: str,
    evidence_summary: str,
    primary_source_references: Sequence[str],
    matched_cusip: Optional[str] = None,
    acquirer_cusip: Optional[str] = None,
    acquiree_cusip: Optional[str] = None,
) -> Dict[str, Any]:
    """Pure assembly of one durable, content-addressed reviewed-resolution
    record. `resolution` must be one of _KNOWN_RESOLUTIONS. Requires at
    least one primary_source_reference -- a reference should identify the
    primary-evidence document (e.g. an SEC filing/accession number), never
    reproduce its full text (see CLAUDE.md copyright rule)."""
    if resolution not in _KNOWN_RESOLUTIONS:
        raise ValueError(
            f"Unknown reviewed resolution semantics: {resolution!r} -- must be one of "
            f"{sorted(_KNOWN_RESOLUTIONS)!r}; this registry does not support an arbitrary/generic "
            "'ignore this event' resolution"
        )
    if not primary_source_references:
        raise ValueError("build_reviewed_resolution requires at least one primary_source_reference")

    fingerprint_fields = _canonical_fingerprint_content(
        source_provider_id=source_provider_id,
        provider_event_id=provider_event_id,
        action_type=action_type,
        requested_symbol=requested_symbol,
        requested_role=requested_role,
        matched_symbol_field=matched_symbol_field,
        process_date=process_date,
        matched_cusip=matched_cusip,
        acquirer_cusip=acquirer_cusip,
        acquiree_cusip=acquiree_cusip,
    )
    fingerprint = sha256_json(fingerprint_fields)
    content = _canonical_resolution_content_for_id(
        schema_version=RESOLUTION_SCHEMA_VERSION,
        fingerprint_fields=fingerprint_fields,
        fingerprint=fingerprint,
        resolution=resolution,
        evidence_summary=evidence_summary,
        primary_source_references=primary_source_references,
    )
    record: Dict[str, Any] = dict(content)
    record["resolution_id"] = sha256_json(content)
    return record


def require_verified_reviewed_resolution(record: Dict[str, Any]) -> None:
    """Fail-closed content-integrity check (REPAIR-01 E3) -- mirrors
    bars_provenance's _require_verified_source_attestation /
    _require_verified_ca_evidence pattern: independently recompute the
    record's own event_fingerprint from its own bound identity fields AND
    independently recompute its own resolution_id from its own full
    canonical content, and require exact equality for BOTH. A hand-edited
    record (e.g. someone changed process_date but not event_fingerprint, or
    mutated evidence_summary/primary_source_references but left
    resolution_id stale, or wrote an unrecognized resolution string) is
    never trusted -- this raises ReviewedResolutionUnverifiable, it does not
    return None; a malformed record found by fingerprint lookup is refused
    with a raised exception, never silently treated as absent."""
    if record.get("schema_version") != RESOLUTION_SCHEMA_VERSION:
        raise ReviewedResolutionUnverifiable(
            f"Fail-closed: reviewed resolution schema_version={record.get('schema_version')!r} != "
            f"{RESOLUTION_SCHEMA_VERSION!r}"
        )
    fingerprint_fields = _canonical_fingerprint_content(
        source_provider_id=record.get("source_provider_id", ""),
        provider_event_id=record.get("provider_event_id", ""),
        action_type=record.get("action_type", ""),
        requested_symbol=record.get("requested_symbol", ""),
        requested_role=record.get("requested_role", ""),
        matched_symbol_field=record.get("matched_symbol_field", ""),
        process_date=record.get("process_date", ""),
        matched_cusip=record.get("matched_cusip"),
        acquirer_cusip=record.get("acquirer_cusip"),
        acquiree_cusip=record.get("acquiree_cusip"),
    )
    recomputed_fingerprint = sha256_json(fingerprint_fields)
    if recomputed_fingerprint != record.get("event_fingerprint"):
        raise ReviewedResolutionUnverifiable(
            "Fail-closed: reviewed resolution event_fingerprint does not match the recomputed hash of "
            f"its own bound identity fields (declared={record.get('event_fingerprint')!r}, "
            f"recomputed={recomputed_fingerprint!r}) -- refusing a tampered/hand-edited record"
        )
    if record.get("resolution") not in _KNOWN_RESOLUTIONS:
        raise ReviewedResolutionUnverifiable(
            f"Fail-closed: reviewed resolution={record.get('resolution')!r} is not a known resolution "
            f"semantics ({sorted(_KNOWN_RESOLUTIONS)!r})"
        )
    content = _canonical_resolution_content_for_id(
        schema_version=record.get("schema_version"),
        fingerprint_fields=fingerprint_fields,
        fingerprint=recomputed_fingerprint,
        resolution=record.get("resolution"),
        evidence_summary=record.get("evidence_summary", ""),
        primary_source_references=record.get("primary_source_references") or [],
    )
    recomputed_resolution_id = sha256_json(content)
    if recomputed_resolution_id != record.get("resolution_id"):
        raise ReviewedResolutionUnverifiable(
            "Fail-closed: reviewed resolution resolution_id does not match the recomputed hash of its "
            f"own full canonical content (declared={record.get('resolution_id')!r}, "
            f"recomputed={recomputed_resolution_id!r}) -- refusing a record whose evidence_summary, "
            "primary_source_references, or identity fields were edited without re-minting resolution_id"
        )


def find_reviewed_resolution(
    *,
    source_provider_id: str,
    provider_event_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    matched_symbol_field: str,
    process_date: str,
    registry: Sequence[Dict[str, Any]],
    matched_cusip: Optional[str] = None,
    acquirer_cusip: Optional[str] = None,
    acquiree_cusip: Optional[str] = None,
) -> Optional[Dict[str, Any]]:
    """Look up a verified reviewed resolution matching EXACTLY the given
    fields' canonical fingerprint. Returns None (the fail-closed default) if
    no record matches. Raises ReviewedResolutionUnverifiable -- never
    returns None -- if the matching record itself fails content
    verification (see require_verified_reviewed_resolution), or if more than one
    registry record shares the same canonical event fingerprint (an
    ambiguous match is refused outright, never resolved by silently
    choosing the first record)."""
    target = event_fingerprint(
        source_provider_id=source_provider_id,
        provider_event_id=provider_event_id,
        action_type=action_type,
        requested_symbol=requested_symbol,
        requested_role=requested_role,
        matched_symbol_field=matched_symbol_field,
        process_date=process_date,
        matched_cusip=matched_cusip,
        acquirer_cusip=acquirer_cusip,
        acquiree_cusip=acquiree_cusip,
    )
    matches = [record for record in registry if record.get("event_fingerprint") == target]
    if not matches:
        return None
    if len(matches) > 1:
        raise ReviewedResolutionUnverifiable(
            f"Fail-closed: {len(matches)} reviewed-resolution registry records share the same canonical "
            f"event fingerprint ({target!r}) -- ambiguous match, refusing to silently choose the first"
        )
    record = matches[0]
    require_verified_reviewed_resolution(record)
    return record


# ---------------------------------------------------------------------------
# The registry itself. Each entry is a narrow, explicit, individually-
# reviewed record; adding one is an auditable code change citing real
# primary-source evidence -- never bar-price smoothness alone (mission
# rule: "Do not treat smooth price observations alone as sufficient
# evidence for security continuity").
#
# REPAIR-01: the original record bound to a HYPOTHETICAL action_type=
# "reorganization" event that no live Alpaca query had confirmed, and was
# therefore inert. The confirmed live Alpaca /v1/corporate-actions event for
# this filing is a stock_merger with the pre-reorganization "DraftKings
# Inc." reported in the ACQUIREE role (its shares/CUSIP were retired and
# replaced one-for-one by the new holding company's shares/CUSIP; the
# ticker DKNG itself continued uninterrupted under the new holding company).
# This record binds to that EXACT confirmed event -- source_provider_id=
# alpaca, provider_event_id=e21ce7ea-649b-456a-a4f4-b025cbdc1fca, action_
# type=stock_merger, requested_symbol=DKNG, requested_role=acquiree,
# matched_symbol_field=acquiree_symbol, process_date=2022-05-05,
# acquirer_cusip=26142V105 (New Duke Holdco, Inc. / successor DraftKings
# Inc.), acquiree_cusip=26142R104 (pre-reorganization DraftKings Inc.),
# matched_cusip=26142R104 (the acquiree leg's own CUSIP, per
# alpaca_historical._ROLE_CUSIP_FIELD["acquiree"]) -- so it can NEVER match
# any other DKNG event, any other symbol, or a future provider revision of
# this same nominal event that changes one of these identity fields.
# ---------------------------------------------------------------------------

_DKNG_2022_STOCK_MERGER_ACQUIREE = build_reviewed_resolution(
    source_provider_id="alpaca",
    provider_event_id="e21ce7ea-649b-456a-a4f4-b025cbdc1fca",
    action_type="stock_merger",
    requested_symbol="DKNG",
    requested_role="acquiree",
    matched_symbol_field="acquiree_symbol",
    process_date="2022-05-05",
    matched_cusip="26142R104",
    acquirer_cusip="26142V105",
    acquiree_cusip="26142R104",
    resolution=RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
    evidence_summary=(
        "DraftKings Inc. holding-company reorganization, effective 2022-05-05: pre-reorganization "
        "DraftKings Inc. (CUSIP 26142R104, reported by Alpaca as this event's acquiree, acquiree_cusip "
        "26142R104) common stock was converted one-for-one into common stock of New Duke Holdco, Inc. "
        "(CUSIP 26142V105, reported by Alpaca as this event's acquirer_cusip; no acquirer_symbol was "
        "reported because the successor retained the SAME ticker), which became the successor SEC "
        "registrant and continued trading under ticker DKNG from market open on 2022-05-05. A verified "
        "one-for-one successor-security continuity -- not a change in economic ownership requiring a "
        "return adjustment -- even though the acquirer/acquiree CUSIP change means the automated "
        "merger-acquirer and name-change-CUSIP-continuity contracts cannot honestly auto-clear an "
        "acquiree leg."
    ),
    primary_source_references=(
        "SEC accession 0001104659-22-056134, Form 8-K12B, filed 2022-05-05 by New Duke Holdco, Inc. "
        "(successor DraftKings Inc.), document tm2214276d1_8k.htm, reporting completion of the holding "
        "company reorganization and commencement of trading of the successor's Class A common stock "
        "under ticker DKNG on 2022-05-05.",
        "Same SEC accession 0001104659-22-056134, exhibit tm2214276d1_ex4-6.htm, describing the "
        "one-for-one conversion of pre-reorganization DraftKings Inc. common stock into the successor "
        "holding company's common stock.",
    ),
)

REVIEWED_CA_RESOLUTIONS: Tuple[Dict[str, Any], ...] = (_DKNG_2022_STOCK_MERGER_ACQUIREE,)
