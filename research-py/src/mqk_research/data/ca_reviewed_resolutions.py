from __future__ import annotations

from typing import Any, Dict, Optional, Sequence, Tuple

from mqk_research.ml.util_hash import sha256_json

# BKT-RESEARCH-CA-REVIEWED-SUCCESSOR-RESOLUTION-01
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
# find_reviewed_resolution against a leg's own (provider, action_type,
# requested_symbol, requested_role, process_date) -- exactly the five
# fields the mission requires binding on, no more. A record's own
# event_fingerprint is independently RECOMPUTED from those five fields and
# required to match before the record is ever trusted (see
# _require_verified_resolution) -- mirrors bars_provenance's evidence_id /
# attestation_id pattern (content-derived, never a caller/author-asserted
# label): a hand-edited record whose event_fingerprint no longer matches
# its own five source fields is refused, never silently accepted.
#
# Binding on the provider's own opaque event id was deliberately rejected:
# that id is not independently verifiable from public primary-source
# evidence (an SEC filing does not report Alpaca's internal event id), so
# trusting it would mean trusting Alpaca's unverified say-so precisely
# where this mechanism exists BECAUSE the provider's own classification
# needs an external, reviewed check. The five bound fields plus the
# resolution's own narrow vocabulary (see _KNOWN_RESOLUTIONS) are the
# actual security boundary: a record can only ever apply to an EXACT
# provider+type+symbol+role+date match, and can only ever assert one of a
# small, explicitly-named set of resolutions -- never a generic "ignore
# this event" escape hatch.

RESOLUTION_SCHEMA_VERSION = "reviewed_ca_resolution_v1"

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
    from its own five bound identity fields, or its resolution is not a
    known semantics -- a hand-edited/tampered/malformed record is never
    trusted (see _require_verified_resolution)."""


def _canonical_fingerprint_content(
    *,
    source_provider_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    process_date: str,
) -> Dict[str, str]:
    """The exact five fields a reviewed resolution binds on, normalized so
    two logically-identical bindings always hash identically."""
    return {
        "source_provider_id": str(source_provider_id or "").strip().lower(),
        "action_type": str(action_type or "").strip(),
        "requested_symbol": str(requested_symbol or "").strip().upper(),
        "requested_role": str(requested_role or "").strip(),
        "process_date": str(process_date or "").strip(),
    }


def event_fingerprint(
    *,
    source_provider_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    process_date: str,
) -> str:
    """Deterministic, content-DERIVED event fingerprint -- the only
    legitimate way a reviewed resolution record's event_fingerprint may
    arise. Binds exactly provider, action type, requested symbol, requested
    role, and process/effective date -- changing any one of the five
    changes the fingerprint, so a mutated query never matches a record
    reviewed for a different event."""
    return sha256_json(
        _canonical_fingerprint_content(
            source_provider_id=source_provider_id,
            action_type=action_type,
            requested_symbol=requested_symbol,
            requested_role=requested_role,
            process_date=process_date,
        )
    )


def build_reviewed_resolution(
    *,
    source_provider_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    process_date: str,
    resolution: str,
    evidence_summary: str,
    primary_source_references: Sequence[str],
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

    fingerprint = event_fingerprint(
        source_provider_id=source_provider_id,
        action_type=action_type,
        requested_symbol=requested_symbol,
        requested_role=requested_role,
        process_date=process_date,
    )
    record: Dict[str, Any] = {
        "schema_version": RESOLUTION_SCHEMA_VERSION,
        **_canonical_fingerprint_content(
            source_provider_id=source_provider_id,
            action_type=action_type,
            requested_symbol=requested_symbol,
            requested_role=requested_role,
            process_date=process_date,
        ),
        "event_fingerprint": fingerprint,
        "resolution": resolution,
        "evidence_summary": str(evidence_summary),
        "primary_source_references": list(primary_source_references),
    }
    record["resolution_id"] = sha256_json(record)
    return record


def _require_verified_resolution(record: Dict[str, Any]) -> None:
    """Fail-closed content-integrity check -- mirrors bars_provenance's
    _require_verified_source_attestation / _require_verified_ca_evidence
    pattern: independently recompute the record's own event_fingerprint
    from its own five declared fields and require exact equality, and
    require its resolution to be a known semantics. A hand-edited record
    (e.g. someone changed process_date but not event_fingerprint, or wrote
    an unrecognized resolution string) is never trusted."""
    if record.get("schema_version") != RESOLUTION_SCHEMA_VERSION:
        raise ReviewedResolutionUnverifiable(
            f"Fail-closed: reviewed resolution schema_version={record.get('schema_version')!r} != "
            f"{RESOLUTION_SCHEMA_VERSION!r}"
        )
    recomputed = event_fingerprint(
        source_provider_id=record.get("source_provider_id", ""),
        action_type=record.get("action_type", ""),
        requested_symbol=record.get("requested_symbol", ""),
        requested_role=record.get("requested_role", ""),
        process_date=record.get("process_date", ""),
    )
    if recomputed != record.get("event_fingerprint"):
        raise ReviewedResolutionUnverifiable(
            "Fail-closed: reviewed resolution event_fingerprint does not match the recomputed hash of "
            f"its own five bound fields (declared={record.get('event_fingerprint')!r}, "
            f"recomputed={recomputed!r}) -- refusing a tampered/hand-edited record"
        )
    if record.get("resolution") not in _KNOWN_RESOLUTIONS:
        raise ReviewedResolutionUnverifiable(
            f"Fail-closed: reviewed resolution={record.get('resolution')!r} is not a known resolution "
            f"semantics ({sorted(_KNOWN_RESOLUTIONS)!r})"
        )


def find_reviewed_resolution(
    *,
    source_provider_id: str,
    action_type: str,
    requested_symbol: str,
    requested_role: str,
    process_date: str,
    registry: Sequence[Dict[str, Any]],
) -> Optional[Dict[str, Any]]:
    """Look up a verified reviewed resolution matching EXACTLY the given
    five fields' canonical fingerprint. Returns None (the fail-closed
    default) if no record matches, or if the matching record itself fails
    content verification (see _require_verified_resolution) -- a
    tampered/malformed record is treated as absent, never as a match, and
    never silently skipped past to a different record."""
    target = event_fingerprint(
        source_provider_id=source_provider_id,
        action_type=action_type,
        requested_symbol=requested_symbol,
        requested_role=requested_role,
        process_date=process_date,
    )
    for record in registry:
        if record.get("event_fingerprint") != target:
            continue
        _require_verified_resolution(record)
        return record
    return None


# ---------------------------------------------------------------------------
# The registry itself. Each entry is a narrow, explicit, individually-
# reviewed record; adding one is an auditable code change citing real
# primary-source evidence -- never bar-price smoothness alone (mission
# rule: "Do not treat smooth price observations alone as sufficient
# evidence for security continuity").
#
# NOTE (pending live verification): `action_type="reorganization"` is this
# patch's documented-schema best fit for a holding-company reorganization
# among alpaca_historical.KNOWN_CORPORATE_ACTION_TYPES; `process_date` is
# set to the primary-evidence-supported trading-resumption date
# (2022-05-05). Neither has been confirmed against Alpaca's live
# /v1/corporate-actions feed for this specific event (no live provider
# credentials/query were used to author this record). Until confirmed, this
# record is inert: find_reviewed_resolution only matches an EXACT (provider,
# action_type, symbol, role, process_date) tuple, so a live DKNG event
# reported under a different action_type or process_date will not match it
# and will correctly continue to fail closed (see mission "OPTIONAL
# READ-ONLY PROVIDER PROOF" step for how to confirm/update this record).
# ---------------------------------------------------------------------------

_DKNG_2022_HOLDCO_REORG = build_reviewed_resolution(
    source_provider_id="alpaca",
    action_type="reorganization",
    requested_symbol="DKNG",
    requested_role="primary",
    process_date="2022-05-05",
    resolution=RESOLUTION_VERIFIED_ONE_FOR_ONE_SUCCESSOR_SECURITY_CONTINUITY,
    evidence_summary=(
        "DraftKings Inc. holding-company reorganization: pre-reorganization DraftKings Inc. common "
        "stock was converted one-for-one into common stock of a new successor holding company, which "
        "became the successor SEC registrant; the successor's Class A common stock continued trading "
        "under the same ticker DKNG from market open on 2022-05-05. A verified one-for-one "
        "successor-security continuity -- not a change in economic ownership requiring a return "
        "adjustment -- even though the legal-entity CUSIP change means the automated same-CUSIP "
        "name-change continuity check cannot honestly auto-clear it."
    ),
    primary_source_references=(
        "SEC EDGAR full-text search, company name 'DraftKings Inc.' -- Form 8-K reporting completion "
        "of the holding company reorganization and commencement of trading of the successor's Class A "
        "common stock under ticker DKNG effective on or about 2022-05-05 (locate via SEC EDGAR company "
        "search; exact accession number pending confirmation before this record's action_type/"
        "process_date are treated as live-verified -- see module header NOTE).",
    ),
)

REVIEWED_CA_RESOLUTIONS: Tuple[Dict[str, Any], ...] = (_DKNG_2022_HOLDCO_REORG,)
