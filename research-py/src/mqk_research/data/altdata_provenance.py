"""
Provider-neutral provenance boundary for discrete alternative/fundamental
data events (Component D — StockNest research-data intake).

Deliberately NOT built on `bars_provenance.py`: that module's manifest shape
is OHLC-bars-and-corporate-action-specific (RESEARCH-MARKET-DATA-AUTHORITY
lineage). A 13F snapshot, an insider Form-4, or a congressional trade
disclosure is a discrete point-in-time EVENT, not a bar series — forcing one
into the other's schema would misrepresent its semantics. This module
mirrors that module's PATTERN instead (content-addressed ids, fail-closed
preflight before any economic use) for this distinct event shape.

See docs/devtools/STOCKNEST_INTAKE_ASSESSMENT_01.md for the verified
capability assessment this module's registry entries are evidence for.
Zero network access, zero economic/Promotion authority: this module reads
no provider, writes no registry, and calls no Research/Promotion seam.
"""

from __future__ import annotations

import math
from collections.abc import Mapping as ABCMapping
from dataclasses import dataclass, field
from enum import Enum
from types import MappingProxyType
from typing import Any, Dict, Mapping, Optional, Sequence

import pandas as pd

from mqk_research.ml.util_hash import sha256_json


class EventCategory(str, Enum):
    FUNDAMENTAL_STATEMENT = "fundamental_statement"
    SCREENER_SNAPSHOT = "screener_snapshot"
    INSTITUTIONAL_13F_HOLDING = "institutional_13f_holding"
    INSIDER_TRANSACTION = "insider_transaction"
    CONGRESSIONAL_DISCLOSURE = "congressional_disclosure"
    EARNINGS_EVENT = "earnings_event"
    UNKNOWN = "unknown"


class LicenseStatus(str, Enum):
    UNKNOWN = "unknown"
    UNLICENSED_FOR_RESEARCH = "unlicensed_for_research"
    LICENSED_FOR_RESEARCH = "licensed_for_research"


class AltDataProvenanceError(ValueError):
    """Fail-closed refusal: malformed event, ambiguous identity, or a
    not-yet-knowable (future) disclosure treated as available."""


class ProviderUnavailableError(RuntimeError):
    """Fail-closed refusal to fetch from a provider that is not verified
    `api_available=True` AND `license_status=LICENSED_FOR_RESEARCH` in
    `PROVIDER_CAPABILITIES`. Never silently returns synthetic data."""


@dataclass(frozen=True)
class ProviderCapabilityRecord:
    """Durable record of a verification finding, not a live capability
    check. See docs/devtools/STOCKNEST_INTAKE_ASSESSMENT_01.md for the
    evidence behind each field for the `stocknest` entry."""

    provider_id: str
    api_available: bool
    license_status: LicenseStatus
    verified_at_utc: str
    verification_method: str
    notes: str


# Fail-closed by default: every entry must be added explicitly with its
# evidence; there is no implicit "assume available" path anywhere in this
# module. Flipping `api_available`/`license_status` to enabled requires the
# future-authorization evidence described in the assessment doc, not a code
# change alone.
PROVIDER_CAPABILITIES: Dict[str, ProviderCapabilityRecord] = {
    "stocknest": ProviderCapabilityRecord(
        provider_id="stocknest",
        api_available=False,
        license_status=LicenseStatus.UNKNOWN,
        verified_at_utc="2026-10-09T00:00:00Z",
        verification_method="direct_fetch_403_plus_web_search_no_docs_found",
        notes=(
            "stocknest.app is a real consumer fundamentals/comparison/screener "
            "web app (verified via search). Direct automated fetch returned "
            "HTTP 403. No public API, developer docs, or licensing terms were "
            "located. 13F/insider/congressional-disclosure capabilities are "
            "NOT documented for this product (those belong to unrelated "
            "services that appeared only as search noise)."
        ),
    ),
}


def _freeze_json_native(obj: Any, *, label: str) -> Any:
    """Recursively validates and DEEPLY FREEZES a value for inclusion in an
    event's content identity (FW-D-R6): only finite JSON-native types are
    accepted, and the result is immutable at every level (tuple/
    MappingProxyType), not just the outer container -- a caller mutating
    `raw_payload={"values": [10, 20]}` by appending to that same list
    object after construction must not change `event_id()`. Mirrors
    strategy_mining/grammar.py's `_canonical_value` discipline but is kept
    local to this module (not cross-imported) so Component D's provenance
    boundary stays self-contained, per its own module docstring."""
    if obj is None or isinstance(obj, str):
        return obj
    if isinstance(obj, bool):
        return obj
    if isinstance(obj, int):
        return obj
    if isinstance(obj, float):
        if not math.isfinite(obj):
            raise AltDataProvenanceError(f"{label}: non-finite float is not an allowed payload value: {obj!r}")
        return obj
    if isinstance(obj, (list, tuple)):
        return tuple(_freeze_json_native(v, label=label) for v in obj)
    if isinstance(obj, ABCMapping):
        canon: Dict[str, Any] = {}
        for k, v in obj.items():
            if not isinstance(k, str):
                raise AltDataProvenanceError(f"{label}: only string keys are allowed, got {type(k).__name__}: {k!r}")
            canon[k] = _freeze_json_native(v, label=label)
        return MappingProxyType(canon)
    raise AltDataProvenanceError(f"{label}: unsupported payload value type {type(obj).__name__}: {obj!r}")


def _thaw_json_native(obj: Any) -> Any:
    """Inverse of `_freeze_json_native`: produces a plain JSON-native
    dict/list, used only when building `content_fields()`'s output."""
    if isinstance(obj, MappingProxyType):
        return {k: _thaw_json_native(v) for k, v in obj.items()}
    if isinstance(obj, tuple):
        return [_thaw_json_native(v) for v in obj]
    return obj


def _require_known_provider(provider_id: str) -> ProviderCapabilityRecord:
    record = PROVIDER_CAPABILITIES.get(provider_id)
    if record is None:
        raise AltDataProvenanceError(f"unregistered provider_id {provider_id!r} — unsupported source category")
    return record


def _require_utc_instant(label: str, value: Optional[str]) -> pd.Timestamp:
    """Strict UTC-instant parsing: rejects None/empty, unparseable strings,
    NaT, and naive (non-timezone-aware) timestamps -- never silently lets a
    malformed `as_of`/event timestamp slip through a comparison where NaT
    would otherwise just evaluate False and let an event through."""
    if value is None or not isinstance(value, str) or not value.strip():
        raise AltDataProvenanceError(f"{label} is missing/empty — a UTC instant is required")
    try:
        ts = pd.Timestamp(value)
    except (ValueError, TypeError) as exc:
        raise AltDataProvenanceError(f"{label} is not a parseable timestamp: {value!r}: {exc}") from exc
    if pd.isna(ts):
        raise AltDataProvenanceError(f"{label} parsed to NaT (not-a-time): {value!r}")
    if ts.tzinfo is None:
        raise AltDataProvenanceError(f"{label} must be timezone-aware/UTC, got a naive timestamp: {value!r}")
    return ts


def fetch_from_provider(provider_id: str, **_kwargs: Any) -> None:
    """Always refuses. There is no provider in `PROVIDER_CAPABILITIES` that
    is currently both `api_available` and `license_status ==
    LICENSED_FOR_RESEARCH`, so this function has no success path to fall
    back on by construction — it cannot silently return synthetic or
    partially-authorized data."""
    record = _require_known_provider(provider_id)
    if not record.api_available:
        raise ProviderUnavailableError(
            f"provider {provider_id!r} has no verified API (see PROVIDER_CAPABILITIES: {record.notes})"
        )
    if record.license_status != LicenseStatus.LICENSED_FOR_RESEARCH:
        raise ProviderUnavailableError(
            f"provider {provider_id!r} is not verified licensed for research use "
            f"(license_status={record.license_status.value})"
        )
    raise ProviderUnavailableError(  # pragma: no cover — unreachable while the registry above stays all-disabled
        f"provider {provider_id!r} is marked available but no ingestion implementation exists yet"
    )


@dataclass(frozen=True)
class AltDataEvent:
    """
    One discrete, point-in-time alternative/fundamental-data observation.

    Distinguishes:
      - event_datetime_utc: when the underlying economic event actually
        happened (e.g. the date of a congressional trade, or the fiscal
        period a statement covers).
      - public_disclosure_datetime_utc: when the event was FIRST publicly
        knowable (e.g. the original filing/disclosure date).
      - revision_publication_datetime_utc: when THIS SPECIFIC payload
        revision became public, if different from the first disclosure
        (a later restatement/correction). Required whenever
        data_revision_version > 1 -- fail closed rather than assume a
        restated revision was knowable as of the original disclosure date.
      - provider_ingestion_timestamp_utc: when our own system observed it.
      - data_revision_version: a later correction/restatement of the SAME
        logical event is a new revision, not a silent overwrite.
    """

    provider_id: str
    event_category: EventCategory
    instrument_identity: str  # e.g. a symbol; ambiguous/empty values are refused
    event_datetime_utc: str
    public_disclosure_datetime_utc: Optional[str]
    provider_ingestion_timestamp_utc: str
    data_revision_version: int
    original_source_id: str
    raw_payload: Mapping[str, Any]
    license_status: LicenseStatus = LicenseStatus.UNKNOWN
    revision_publication_datetime_utc: Optional[str] = None

    def __post_init__(self) -> None:
        """Defensively deep-freezes `raw_payload` at construction time
        (FW-D-R6): a frozen dataclass does not stop a caller from mutating
        a mutable value stored BY REFERENCE inside it. Also rejects
        non-finite/unsupported payload content immediately rather than
        only when `event_id()` happens to be called later -- and before it
        could ever reach `util_hash.sha256_json`, which (being shared,
        unmodified, protected infrastructure) allows NaN/Infinity through
        as non-standard JSON tokens with no complaint of its own."""
        object.__setattr__(self, "raw_payload", _freeze_json_native(dict(self.raw_payload), label="raw_payload"))

    def validate(self) -> None:
        if not self.instrument_identity or not self.instrument_identity.strip():
            raise AltDataProvenanceError("instrument_identity is empty/ambiguous")
        if self.event_category == EventCategory.UNKNOWN:
            raise AltDataProvenanceError("event_category must be a known, supported category")
        if not isinstance(self.original_source_id, str) or not self.original_source_id.strip():
            raise AltDataProvenanceError("original_source_id is empty/ambiguous")
        if isinstance(self.data_revision_version, bool) or not isinstance(self.data_revision_version, int):
            raise AltDataProvenanceError(
                f"data_revision_version must be a real int, not {type(self.data_revision_version).__name__}: "
                f"{self.data_revision_version!r} (NaN/bool are not valid revisions)"
            )
        if self.data_revision_version < 1:
            raise AltDataProvenanceError("data_revision_version must be >= 1")
        _require_known_provider(self.provider_id)
        _require_utc_instant("provider_ingestion_timestamp_utc", self.provider_ingestion_timestamp_utc)

        event_ts = _require_utc_instant("event_datetime_utc", self.event_datetime_utc)
        disclosure_ts = _require_utc_instant("public_disclosure_datetime_utc", self.public_disclosure_datetime_utc)
        if event_ts > disclosure_ts:
            raise AltDataProvenanceError(
                f"event_datetime_utc ({self.event_datetime_utc!r}) is after "
                f"public_disclosure_datetime_utc ({self.public_disclosure_datetime_utc!r}) — "
                "an event cannot be disclosed before it happens"
            )

        if self.data_revision_version > 1 and self.revision_publication_datetime_utc is None:
            raise AltDataProvenanceError(
                f"data_revision_version={self.data_revision_version} but revision_publication_datetime_utc "
                "is missing — when a payload has been restated, its OWN publication time must be proven; "
                "fail closed rather than assume it was knowable as of the original disclosure date"
            )
        if self.revision_publication_datetime_utc is not None:
            revision_ts = _require_utc_instant("revision_publication_datetime_utc", self.revision_publication_datetime_utc)
            if revision_ts < disclosure_ts:
                raise AltDataProvenanceError(
                    f"revision_publication_datetime_utc ({self.revision_publication_datetime_utc!r}) is before "
                    f"public_disclosure_datetime_utc ({self.public_disclosure_datetime_utc!r}) — "
                    "a revision cannot be published before the original disclosure"
                )

    @property
    def effective_publication_datetime_utc(self) -> str:
        """The timestamp that actually gates availability of THIS payload
        revision: its own revision publication time if proven, else the
        original disclosure time (valid for revision 1, or any revision
        whose own publication time coincides with the original disclosure)."""
        return self.revision_publication_datetime_utc or self.public_disclosure_datetime_utc

    def content_fields(self) -> Dict[str, Any]:
        """The content that defines this event's identity. Deliberately
        EXCLUDES provider_ingestion_timestamp_utc — re-ingesting the exact
        same disclosed content a day later must not manufacture a new
        logical event; only a real data_revision_version bump does."""
        return {
            "provider_id": self.provider_id,
            "event_category": self.event_category.value,
            "instrument_identity": self.instrument_identity,
            "event_datetime_utc": self.event_datetime_utc,
            "public_disclosure_datetime_utc": self.public_disclosure_datetime_utc,
            "revision_publication_datetime_utc": self.revision_publication_datetime_utc,
            "data_revision_version": self.data_revision_version,
            "original_source_id": self.original_source_id,
            "raw_payload": _thaw_json_native(self.raw_payload),
        }

    def event_id(self) -> str:
        """Deterministic, content-derived id. A later revision of the same
        logical event gets a DIFFERENT id (because data_revision_version is
        part of the content) rather than overwriting the prior one.
        Validates first: an unvalidated/invalid object must never return a
        valid-looking hash."""
        self.validate()
        return sha256_json(self.content_fields())


def require_point_in_time_available(event: AltDataEvent, *, as_of_utc: str) -> None:
    """
    Fail-closed gate: a strategy evaluating "as of" `as_of_utc` must never
    see an event before THIS SPECIFIC revision was actually publicly
    knowable. Compares against `effective_publication_datetime_utc` (the
    revision's own publication time when proven, else the original
    disclosure) — never `event_datetime_utc`, which may legitimately be
    much earlier (e.g. a congressional trade's execution date, long before
    its disclosure filing).
    """
    event.validate()
    # Strict parsed comparison (never a raw string/NaT comparison): a
    # malformed or naive `as_of_utc` raises here rather than silently
    # letting an event through because a NaT comparison evaluates False.
    as_of_ts = _require_utc_instant("as_of_utc", as_of_utc)
    effective_ts = _require_utc_instant(
        "effective_publication_datetime_utc", event.effective_publication_datetime_utc
    )
    if as_of_ts < effective_ts:
        raise AltDataProvenanceError(
            f"event {event.event_id()[:16]} is not yet knowable as of {as_of_utc!r} "
            f"(effective_publication_datetime_utc={event.effective_publication_datetime_utc!r})"
        )


def require_licensed_for_research(event: AltDataEvent) -> None:
    """
    Fail-closed gate requiring BOTH verified provider-level authority AND
    event-level rights. A caller constructing an event with
    license_status=LICENSED_FOR_RESEARCH cannot, by itself, confer license
    authority a provider does not actually have (D1) — e.g. a caller-built
    `stocknest` event claiming licensed status still refuses, because the
    provider's own registry entry (PROVIDER_CAPABILITIES) stays unlicensed
    until real authorization evidence exists.
    """
    event.validate()
    provider_record = _require_known_provider(event.provider_id)
    if provider_record.license_status != LicenseStatus.LICENSED_FOR_RESEARCH:
        raise AltDataProvenanceError(
            f"provider {event.provider_id!r} is not verified licensed for research "
            f"(provider license_status={provider_record.license_status.value}) — "
            "an event cannot confer license authority a provider does not have"
        )
    if event.license_status != LicenseStatus.LICENSED_FOR_RESEARCH:
        raise AltDataProvenanceError(
            f"event from provider {event.provider_id!r} is not itself verified licensed "
            f"(event license_status={event.license_status.value})"
        )


def deduplicate_events(events: Sequence[AltDataEvent]) -> Dict[str, AltDataEvent]:
    """Keyed by event_id: identical content (including revision) collapses
    to one entry; a different revision of the same logical event keeps its
    own distinct entry because its content — and therefore its id — differs."""
    out: Dict[str, AltDataEvent] = {}
    for ev in events:
        ev.validate()
        out[ev.event_id()] = ev
    return out
