from __future__ import annotations

import inspect

import pytest

from mqk_research.data import altdata_provenance as adp
from mqk_research.data.altdata_provenance import (
    PROVIDER_CAPABILITIES,
    AltDataEvent,
    AltDataProvenanceError,
    EventCategory,
    LicenseStatus,
    ProviderCapabilityRecord,
    ProviderUnavailableError,
    deduplicate_events,
    fetch_from_provider,
    require_licensed_for_research,
    require_point_in_time_available,
)


def _event(**over) -> AltDataEvent:
    base = dict(
        provider_id="stocknest",
        event_category=EventCategory.FUNDAMENTAL_STATEMENT,
        instrument_identity="AAPL",
        event_datetime_utc="2026-01-01T00:00:00Z",
        public_disclosure_datetime_utc="2026-01-15T00:00:00Z",
        provider_ingestion_timestamp_utc="2026-01-16T00:00:00Z",
        data_revision_version=1,
        original_source_id="src-001",
        raw_payload={"metric": "pe_ratio", "value": 25.0},
    )
    base.update(over)
    return AltDataEvent(**base)


# ---------------------------------------------------------------------------
# Provider capability registry — fail closed by default
# ---------------------------------------------------------------------------

def test_stocknest_registry_entry_is_fail_closed():
    record = PROVIDER_CAPABILITIES["stocknest"]
    assert record.api_available is False
    assert record.license_status == LicenseStatus.UNKNOWN


def test_every_registered_provider_defaults_to_disabled():
    for record in PROVIDER_CAPABILITIES.values():
        assert record.api_available is False or record.license_status != LicenseStatus.LICENSED_FOR_RESEARCH, (
            "a provider must not be both api_available and licensed without explicit future authorization evidence"
        )


def test_fetch_from_unregistered_provider_is_unsupported_source_category():
    with pytest.raises(AltDataProvenanceError, match="unregistered provider_id"):
        fetch_from_provider("totally_unknown_provider")


def test_fetch_from_stocknest_fails_closed_with_named_reason():
    with pytest.raises(ProviderUnavailableError, match="no verified API"):
        fetch_from_provider("stocknest")


def test_missing_provider_authorization_and_missing_api_capability_are_the_same_fail_closed_path():
    # Both "no API" and "not licensed" cases are exercised; stocknest's
    # current state (api_available=False) exercises the first branch.
    with pytest.raises(ProviderUnavailableError):
        fetch_from_provider("stocknest", symbol="AAPL", fields=["pe_ratio"])


# ---------------------------------------------------------------------------
# Event validation — ambiguous identifiers, missing timestamps, malformed data
# ---------------------------------------------------------------------------

def test_ambiguous_identifier_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="ambiguous"):
        _event(instrument_identity="   ").validate()


def test_unknown_event_category_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="event_category"):
        _event(event_category=EventCategory.UNKNOWN).validate()


def test_missing_public_disclosure_time_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="missing/empty"):
        _event(public_disclosure_datetime_utc=None).validate()


def test_malformed_source_data_fails_closed():
    class NotJsonSerializable:
        pass

    # Rejected at CONSTRUCTION time now (deep-freeze validates eagerly),
    # before .validate() would even be reached.
    with pytest.raises(AltDataProvenanceError, match="unsupported payload value type"):
        _event(raw_payload={"bad": NotJsonSerializable()})


def test_revision_below_one_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="data_revision_version"):
        _event(data_revision_version=0).validate()


def test_event_from_unregistered_provider_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="unregistered provider_id"):
        _event(provider_id="some_random_unregistered_provider").validate()


# ---------------------------------------------------------------------------
# Point-in-time availability — future-publication rejection, event vs
# disclosure date are never confused
# ---------------------------------------------------------------------------

def test_future_publication_is_rejected():
    ev = _event(public_disclosure_datetime_utc="2026-06-01T00:00:00Z")
    with pytest.raises(AltDataProvenanceError, match="not yet knowable"):
        require_point_in_time_available(ev, as_of_utc="2026-01-01T00:00:00Z")


def test_available_after_disclosure_is_allowed():
    ev = _event(public_disclosure_datetime_utc="2026-01-15T00:00:00Z")
    require_point_in_time_available(ev, as_of_utc="2026-01-16T00:00:00Z")  # must not raise


def test_available_exactly_at_disclosure_instant_is_allowed():
    ev = _event(public_disclosure_datetime_utc="2026-01-15T00:00:00Z")
    require_point_in_time_available(ev, as_of_utc="2026-01-15T00:00:00Z")  # boundary, must not raise


def test_event_date_long_before_disclosure_date_is_normal_and_gated_by_disclosure_not_event_date():
    # A congressional trade executed months before its disclosure filing.
    ev = _event(event_datetime_utc="2025-06-01T00:00:00Z", public_disclosure_datetime_utc="2026-01-15T00:00:00Z")
    # Not yet available the day after the trade (event date) even though
    # that is long past -- disclosure date is what actually gates it.
    with pytest.raises(AltDataProvenanceError, match="not yet knowable"):
        require_point_in_time_available(ev, as_of_utc="2025-06-02T00:00:00Z")
    # Available once as_of passes the real disclosure date.
    require_point_in_time_available(ev, as_of_utc="2026-01-16T00:00:00Z")


def test_iso8601_timezone_representation_does_not_flip_ordering():
    ev = _event(public_disclosure_datetime_utc="2026-01-15T00:00:00+00:00")
    require_point_in_time_available(ev, as_of_utc="2026-01-15T00:00:00Z")  # same instant, different spelling


# ---------------------------------------------------------------------------
# Licensing gate
# ---------------------------------------------------------------------------

def test_unlicensed_event_fails_the_licensing_gate():
    ev = _event(license_status=LicenseStatus.UNLICENSED_FOR_RESEARCH)
    with pytest.raises(AltDataProvenanceError, match="not verified licensed"):
        require_licensed_for_research(ev)


def test_unknown_license_status_fails_the_licensing_gate_too():
    ev = _event()  # default LicenseStatus.UNKNOWN
    with pytest.raises(AltDataProvenanceError, match="not verified licensed"):
        require_licensed_for_research(ev)


# ---------------------------------------------------------------------------
# Identity / deduplication / revisions
# ---------------------------------------------------------------------------

def test_identical_events_deduplicate_to_one_entry():
    e1 = _event()
    e2 = _event()
    deduped = deduplicate_events([e1, e2])
    assert len(deduped) == 1


def test_conflicting_revisions_are_distinguished_not_merged():
    e_rev1 = _event(data_revision_version=1, raw_payload={"metric": "pe_ratio", "value": 25.0})
    e_rev2 = _event(
        data_revision_version=2, raw_payload={"metric": "pe_ratio", "value": 26.0},  # restated
        revision_publication_datetime_utc="2026-02-01T00:00:00Z",
    )
    deduped = deduplicate_events([e_rev1, e_rev2])
    assert len(deduped) == 2
    assert e_rev1.event_id() != e_rev2.event_id()


def test_re_ingesting_identical_content_later_does_not_manufacture_a_new_event():
    e_first_ingest = _event(provider_ingestion_timestamp_utc="2026-01-16T00:00:00Z")
    e_later_ingest = _event(provider_ingestion_timestamp_utc="2026-02-01T00:00:00Z")  # same content, re-pulled later
    assert e_first_ingest.event_id() == e_later_ingest.event_id()


def test_meaningful_content_change_changes_event_id():
    e1 = _event(raw_payload={"metric": "pe_ratio", "value": 25.0})
    e2 = _event(raw_payload={"metric": "pe_ratio", "value": 25.1})
    assert e1.event_id() != e2.event_id()


# ---------------------------------------------------------------------------
# Negative controls: no network, no fabrication, no result-driven backdating,
# no Research/Promotion authority embedded in this module
# ---------------------------------------------------------------------------

def test_module_touches_no_network_registry_promotion_or_broker():
    import_lines = [
        line.strip()
        for line in inspect.getsource(adp).splitlines()
        if line.strip().startswith(("import ", "from "))
    ]
    for forbidden in (
        "requests", "httpx", "socket", "subprocess",
        "exp_distributed.storage", "ResearchResultStore",
        "promotion", "broker", "daemon",
    ):
        assert not any(forbidden in line for line in import_lines), (
            f"altdata_provenance unexpectedly imports {forbidden!r}: {import_lines}"
        )


def test_no_fabricated_fundamental_observations_module_returns_no_hardcoded_numeric_value():
    # The module must never hand back a "default" fundamental value; it
    # only ever validates/rejects the caller's own declared event.
    source = inspect.getsource(adp)
    assert "return 0.0" not in source
    assert "return 25" not in source  # the test fixture's illustrative PE ratio must never leak into prod code


def test_require_point_in_time_available_does_not_mutate_the_event():
    ev = _event()
    before = ev.event_id()
    require_point_in_time_available(ev, as_of_utc="2026-02-01T00:00:00Z")
    # Calling the gate with a different as_of does not recompute/backdate
    # the event's own recorded timestamps.
    assert ev.event_id() == before
    assert ev.event_datetime_utc == "2026-01-01T00:00:00Z"
    assert ev.public_disclosure_datetime_utc == "2026-01-15T00:00:00Z"


# ---------------------------------------------------------------------------
# D1: economic use requires BOTH verified provider-level authority AND
# event-level rights -- a caller cannot confer license authority a
# provider does not have by self-declaring it on an event object.
# ---------------------------------------------------------------------------

def test_caller_declared_license_on_a_disabled_provider_still_refuses():
    # stocknest is disabled in PROVIDER_CAPABILITIES; a caller-constructed
    # event claiming LICENSED_FOR_RESEARCH must not bypass that.
    ev = _event(license_status=LicenseStatus.LICENSED_FOR_RESEARCH)
    assert PROVIDER_CAPABILITIES["stocknest"].license_status != LicenseStatus.LICENSED_FOR_RESEARCH
    with pytest.raises(AltDataProvenanceError, match="provider .* is not verified licensed"):
        require_licensed_for_research(ev)


def test_licensed_provider_but_unlicensed_event_still_refuses(monkeypatch):
    # Independently exercise the OTHER half of the AND: even if the
    # provider itself were licensed, an event that doesn't itself declare
    # LICENSED_FOR_RESEARCH must still refuse.
    licensed_providers = dict(PROVIDER_CAPABILITIES)
    licensed_providers["test_licensed_provider"] = ProviderCapabilityRecord(
        provider_id="test_licensed_provider", api_available=True,
        license_status=LicenseStatus.LICENSED_FOR_RESEARCH,
        verified_at_utc="2026-01-01T00:00:00Z", verification_method="test_fixture", notes="test only",
    )
    monkeypatch.setattr(adp, "PROVIDER_CAPABILITIES", licensed_providers)
    ev = _event(provider_id="test_licensed_provider", license_status=LicenseStatus.UNLICENSED_FOR_RESEARCH)
    with pytest.raises(AltDataProvenanceError, match="event from provider .* is not itself verified licensed"):
        require_licensed_for_research(ev)


def test_both_provider_and_event_licensed_succeeds(monkeypatch):
    licensed_providers = dict(PROVIDER_CAPABILITIES)
    licensed_providers["test_licensed_provider"] = ProviderCapabilityRecord(
        provider_id="test_licensed_provider", api_available=True,
        license_status=LicenseStatus.LICENSED_FOR_RESEARCH,
        verified_at_utc="2026-01-01T00:00:00Z", verification_method="test_fixture", notes="test only",
    )
    monkeypatch.setattr(adp, "PROVIDER_CAPABILITIES", licensed_providers)
    ev = _event(provider_id="test_licensed_provider", license_status=LicenseStatus.LICENSED_FOR_RESEARCH)
    require_licensed_for_research(ev)  # must not raise


# ---------------------------------------------------------------------------
# D2: a restated/revised payload must prove ITS OWN publication time; the
# original disclosure date alone cannot gate a later revision.
# ---------------------------------------------------------------------------

def test_revision_without_its_own_publication_time_fails_closed():
    ev = _event(data_revision_version=2, revision_publication_datetime_utc=None)
    with pytest.raises(AltDataProvenanceError, match="revision_publication_datetime_utc"):
        ev.validate()


def test_revision_gated_by_its_own_later_publication_time_not_the_original_disclosure():
    ev = _event(
        public_disclosure_datetime_utc="2026-01-15T00:00:00Z",
        data_revision_version=2,
        revision_publication_datetime_utc="2026-03-01T00:00:00Z",  # restated much later
    )
    # Available right after the ORIGINAL disclosure date would be wrong for
    # this revision -- it wasn't knowable yet.
    with pytest.raises(AltDataProvenanceError, match="not yet knowable"):
        require_point_in_time_available(ev, as_of_utc="2026-01-16T00:00:00Z")
    # Only becomes available once as_of passes the REVISION's own publication date.
    require_point_in_time_available(ev, as_of_utc="2026-03-02T00:00:00Z")


def test_revision_one_without_an_explicit_revision_publication_time_is_fine():
    # Revision 1 has no separate restatement -- falls back to the original
    # disclosure date, preserving pre-existing behavior.
    ev = _event(data_revision_version=1, revision_publication_datetime_utc=None)
    ev.validate()  # must not raise
    assert ev.effective_publication_datetime_utc == ev.public_disclosure_datetime_utc


def test_revision_publication_before_original_disclosure_fails_closed():
    ev = _event(
        public_disclosure_datetime_utc="2026-01-15T00:00:00Z",
        data_revision_version=2,
        revision_publication_datetime_utc="2026-01-01T00:00:00Z",  # before the original disclosure
    )
    with pytest.raises(AltDataProvenanceError, match="before"):
        ev.validate()


# ---------------------------------------------------------------------------
# D3: strict UTC timestamp parsing -- malformed/naive/NaT values must
# never silently pass a comparison.
# ---------------------------------------------------------------------------

def test_as_of_utc_none_fails_closed():
    ev = _event()
    with pytest.raises(AltDataProvenanceError, match="missing/empty"):
        require_point_in_time_available(ev, as_of_utc=None)  # type: ignore[arg-type]


def test_as_of_utc_empty_string_fails_closed():
    ev = _event()
    with pytest.raises(AltDataProvenanceError, match="missing/empty"):
        require_point_in_time_available(ev, as_of_utc="")


def test_as_of_utc_malformed_fails_closed():
    ev = _event()
    with pytest.raises(AltDataProvenanceError, match="not a parseable timestamp"):
        require_point_in_time_available(ev, as_of_utc="not-a-timestamp")


def test_as_of_utc_naive_timestamp_fails_closed():
    ev = _event()
    with pytest.raises(AltDataProvenanceError, match="timezone-aware"):
        require_point_in_time_available(ev, as_of_utc="2026-01-20T00:00:00")  # no tz


def test_event_datetime_after_disclosure_fails_closed():
    ev = _event(event_datetime_utc="2026-02-01T00:00:00Z", public_disclosure_datetime_utc="2026-01-15T00:00:00Z")
    with pytest.raises(AltDataProvenanceError, match="cannot be disclosed before it happens"):
        ev.validate()


def test_malformed_event_datetime_fails_closed():
    ev = _event(event_datetime_utc="not-a-timestamp")
    with pytest.raises(AltDataProvenanceError, match="not a parseable timestamp"):
        ev.validate()


def test_naive_public_disclosure_timestamp_fails_closed():
    ev = _event(public_disclosure_datetime_utc="2026-01-15T00:00:00")  # no tz
    with pytest.raises(AltDataProvenanceError, match="timezone-aware"):
        ev.validate()


# ---------------------------------------------------------------------------
# D4: re-verify fetch_from_provider stays fully disabled after D1-D3.
# ---------------------------------------------------------------------------

def test_fetch_from_provider_still_has_no_success_path_after_corrections():
    with pytest.raises(ProviderUnavailableError):
        fetch_from_provider("stocknest")


# ---------------------------------------------------------------------------
# FW-D-R5 (surgical closeout 02): data_revision_version must be a real
# positive int, not NaN/bool; provider_ingestion_timestamp_utc must be a
# valid UTC instant; original_source_id must be non-empty.
# ---------------------------------------------------------------------------

def test_nan_data_revision_version_fails_closed():
    # float('nan') < 1 is False in Python -- the old bare comparison let it through.
    with pytest.raises(AltDataProvenanceError, match="data_revision_version must be a real int"):
        _event(data_revision_version=float("nan")).validate()


def test_bool_true_data_revision_version_fails_closed():
    # True < 1 is also False (True == 1) -- bool-as-int coercion gap.
    with pytest.raises(AltDataProvenanceError, match="data_revision_version must be a real int"):
        _event(data_revision_version=True).validate()


def test_malformed_provider_ingestion_timestamp_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="not a parseable timestamp"):
        _event(provider_ingestion_timestamp_utc="junk").validate()


def test_empty_original_source_id_fails_closed():
    with pytest.raises(AltDataProvenanceError, match="original_source_id is empty"):
        _event(original_source_id="").validate()


def test_ingestion_timestamp_is_not_identity_bearing_despite_now_being_validated():
    # D3/D-R5 added validation of provider_ingestion_timestamp_utc, but it
    # must still be deliberately EXCLUDED from content_fields()/event_id()
    # -- re-ingesting identical disclosed content later must not manufacture
    # a new logical event.
    e1 = _event(provider_ingestion_timestamp_utc="2026-01-16T00:00:00Z")
    e2 = _event(provider_ingestion_timestamp_utc="2026-02-01T00:00:00Z")
    assert e1.event_id() == e2.event_id()
    assert "provider_ingestion_timestamp_utc" not in e1.content_fields()


# ---------------------------------------------------------------------------
# FW-D-R6 (surgical closeout 02): raw_payload must be deep-frozen at
# construction; a caller mutating the original mutable object they passed
# in, or a direct attempt to mutate event.raw_payload itself, must not
# change event_id().
# ---------------------------------------------------------------------------

def test_original_caller_raw_payload_list_mutation_does_not_change_event_id():
    original_payload = {"values": [10, 20]}
    ev = _event(raw_payload=original_payload)
    id_before = ev.event_id()
    original_payload["values"].append(30)  # mutate the caller's own object
    assert ev.event_id() == id_before
    assert ev.content_fields()["raw_payload"] == {"values": [10, 20]}


def test_direct_mutation_of_event_raw_payload_is_ineffective():
    ev = _event(raw_payload={"values": [10, 20]})
    with pytest.raises(AttributeError):
        ev.raw_payload["values"].append(30)  # stored as a frozen tuple, no .append


def test_mutating_the_returned_content_fields_does_not_mutate_the_event():
    ev = _event(raw_payload={"values": [10, 20]})
    id_before = ev.event_id()
    cf = ev.content_fields()
    cf["raw_payload"]["values"].append(30)  # mutate the freshly-thawed, detached copy
    cf["original_source_id"] = "tampered"
    assert ev.event_id() == id_before
    assert ev.original_source_id != "tampered"


def test_non_finite_raw_payload_value_fails_closed_at_construction():
    with pytest.raises(AltDataProvenanceError, match="non-finite float"):
        _event(raw_payload={"value": float("nan")})


def test_same_valid_content_yields_the_same_event_id_sha256_json_compatible():
    e1 = _event(raw_payload={"metric": "pe_ratio", "value": 25.0})
    e2 = _event(raw_payload={"metric": "pe_ratio", "value": 25.0})
    assert e1.event_id() == e2.event_id()
    assert len(e1.event_id()) == 64  # sha256 hex digest


def test_event_id_calls_validate_so_an_invalid_object_never_returns_a_looking_valid_hash():
    ev = _event(instrument_identity="   ")  # invalid, but construction alone doesn't catch this
    with pytest.raises(AltDataProvenanceError, match="ambiguous"):
        ev.event_id()
